use crate::config::Config;
use crate::context::ErisContext;
use crate::desertbus::DesertBus;
use crate::extract::Extract;
use crate::google::calendar::{Calendar, Event, LRR};
use crate::models::{Game, GameEntry, Show, User};
use crate::rpc::LRRbot;
use crate::time::HumanReadable;
use crate::twitch::helix::UserId;
use crate::twitch::Helix;
use crate::{truncate::truncate, typemap_keys::PgPool};
use anyhow::{Context, Error};
use chrono::{DateTime, FixedOffset, Utc};
use chrono_tz::Tz;
use diesel::OptionalExtension;
use separator::FixedPlaceSeparatable;
use serenity::prelude::TypeMap;
use slog_scope::error;
use std::fmt;
use std::time::Duration;

struct EventDisplay<'a> {
    event: &'a Event,
    now: DateTime<Utc>,
    tz: Tz,
}

impl<'a> fmt::Display for EventDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let start = self.event.start.with_timezone(&Utc);
        if start > self.now {
            write!(f, "In {}: ", HumanReadable::new(start - self.now))?;
        } else {
            write!(f, "{} ago: ", HumanReadable::new(self.now - start))?;
        }

        f.write_str(&self.event.summary)?;

        if let Some(ref location) = self.event.location {
            write!(f, " ({})", location)?;
        }

        if let Some(ref desc) = self.event.description {
            // TODO: shorten to 200 characters.
            write!(f, " ({})", Calendar::format_description(desc))?;
        }
        write!(
            f,
            " on {}.",
            self.event.start.with_timezone(&self.tz).format("%a %e %b at %I:%M %p %Z")
        )?;

        Ok(())
    }
}

pub async fn autotopic(ctx: ErisContext) {
    let mut timer = tokio::time::interval(Duration::from_secs(60));

    loop {
        timer.tick().await;

        if let Err(err) = Autotopic.update_topic(&ctx).await {
            error!("Failed to update the topic"; "error" => ?err);
        }
    }
}

#[derive(Copy, Clone)]
struct Autotopic;

impl Autotopic {
    async fn update_topic(self, ctx: &ErisContext) -> Result<(), Error> {
        let data = ctx.data.read().await;

        let header = data
            .extract::<LRRbot>()?
            .get_header_info()
            .await
            .context("failed to fetch header info")?;

        let mut messages = vec![];

        let user;
        let game;
        let show;
        let game_entry;

        {
            let conn = data
                .extract::<PgPool>()?
                .get()
                .context("failed to get a database connection from the pool")?;

            user = User::by_name(&data.extract::<Config>()?.username, &conn)
                .context("failed to load the bot user")?;

            if header.is_live {
                game = header
                    .current_game
                    .map(|game| Game::find(game.id, &conn))
                    .transpose()
                    .context("failed to load the game")?;
                show = header
                    .current_show
                    .map(|show| Show::find(show.id, &conn))
                    .transpose()
                    .context("failed to load the show")?;
                game_entry =
                    if let (Some(game), Some(show)) = (header.current_game, header.current_show) {
                        GameEntry::find(game.id, show.id, &conn)
                            .optional()
                            .context("failed to load the game entry")?
                    } else {
                        None
                    };
            } else {
                game = None;
                show = None;
                game_entry = None;
            }
        }

        if header.is_live {
            match (game, show) {
                (Some(game), Some(show)) => {
                    messages.push(format!(
                        "Now live: {} on {}.",
                        game_entry.and_then(|entry| entry.display_name).unwrap_or(game.name),
                        show.name
                    ));
                }
                (Some(game), None) => {
                    messages.push(format!(
                        "Now live: {}.",
                        game_entry.and_then(|entry| entry.display_name).unwrap_or(game.name)
                    ));
                }
                (None, Some(show)) => {
                    messages.push(format!("Now live: {}.", show.name));
                }
                (None, None) => messages.push(String::from("Now live: something?")),
            }

            match self.uptime_msg(&data, &user, &header.channel).await {
                Ok(msg) => messages.push(msg),
                Err(err) => error!("failed to generate the uptime message"; "error" => ?err),
            }
        } else {
            let now = Utc::now();

            let events = data
                .extract::<Calendar>()?
                .get_upcoming_events(LRR, now)
                .await
                .context("failed to get the next scheduled stream")?;
            let events = Calendar::get_next_event(&events, now, false);

            let desertbus = self.desertbus(&data, &user, now, &events).await?;
            if !desertbus.is_empty() {
                messages.extend(desertbus);
            } else {
                let tz = data.extract::<Config>()?.timezone;
                messages
                    .extend(events.iter().map(|event| EventDisplay { event, now, tz }.to_string()));
            }
        }

        if let Some(advice) = header.advice {
            messages.push(advice);
        }

        data.extract::<Config>()?
            .general_channel
            .edit(ctx, |c| c.topic(truncate(&messages.join(" "), 1024).0))
            .await
            .context("failed to update the topic")?;

        Ok(())
    }

    async fn uptime_msg(self, data: &TypeMap, user: &User, channel: &str) -> Result<String, Error> {
        Ok(data
            .extract::<Helix>()?
            .get_streams(
                &user.twitch_oauth.as_ref().context("token missing")?,
                &[UserId::Login(channel)],
            )
            .await
            .context("failed to get the stream")?
            .first()
            .map(|stream| {
                format!(
                    "The stream has been live for {}.",
                    HumanReadable::new(Utc::now() - stream.started_at.with_timezone(&Utc))
                )
            })
            .unwrap_or_else(|| String::from("The stream is not live.")))
    }

    async fn desertbus(
        self,
        data: &TypeMap,
        user: &User,
        now: DateTime<Utc>,
        events: &[Event],
    ) -> Result<Vec<String>, Error> {
        let start = DesertBus::start_time().with_timezone(&Utc);
        let announce_start = start - chrono::Duration::days(2);
        let announce_end = start + chrono::Duration::days(9);
        let mut messages = vec![];

        if announce_start <= now && now <= announce_end {
            if let Some(next_event_start) = events.get(0).map(|event| event.start) {
                if next_event_start.with_timezone(&Utc) < start {
                    return Ok(messages);
                }
            }
            let desertbus = data.extract::<DesertBus>()?;

            let money_raised = match desertbus.money_raised().await {
                Ok(money_raised) => money_raised,
                Err(err) => {
                    error!("Failed to fetch the current Desert Bus total"; "error" => ?err);
                    messages.push(String::from("DESERT BUS?"));
                    return Ok(messages);
                }
            };
            let total_hours = DesertBus::hours_raised(money_raised) as i64;
            if now < start {
                messages.push(
                    EventDisplay {
                        event: &Event {
                            start: start.with_timezone(&FixedOffset::east(0)),
                            summary: String::from("Desert Bus for Hope"),
                            end: start.with_timezone(&FixedOffset::east(0))
                                + chrono::Duration::hours(total_hours),
                            location: Some(String::from(
                                "https://desertbus.org/ or https://twitch.tv/desertbus",
                            )),
                            description: None,
                        },
                        now,
                        tz: data.extract::<Config>()?.timezone,
                    }
                    .to_string(),
                );
                messages.push(format!(
                    "${} raised.",
                    money_raised.separated_string_with_fixed_place(2)
                ));
            } else if now <= start + chrono::Duration::hours(total_hours)
                || self.is_desertbus_live(data, user).await?
            {
                messages.push(String::from(
                    "DESERT BUS! (https://desertbus.org/ or https://twitch.tv/desertbus)",
                ));
                messages.push(format!(
                    "${} raised.",
                    money_raised.separated_string_with_fixed_place(2)
                ));
                let bussed = now - start;
                messages.push(format!(
                    "{}:{:02} hours of {} so far.",
                    bussed.num_hours(),
                    bussed.num_minutes() % 60,
                    total_hours
                ));
            }
        }

        Ok(messages)
    }

    async fn is_desertbus_live(self, data: &TypeMap, user: &User) -> Result<bool, Error> {
        if let Some(token) = user.twitch_oauth.as_ref() {
            Ok(!data
                .extract::<Helix>()?
                .get_streams(&token, &[UserId::Login("desertbus")])
                .await
                .ok()
                .unwrap_or_else(Vec::new)
                .is_empty())
        } else {
            Ok(false)
        }
    }
}
