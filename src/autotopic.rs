use crate::config::Config;
use crate::context::ErisContext;
use crate::desertbus::DesertBus;
use crate::extract::Extract;
use crate::google::calendar::{Calendar, Event, LRR};
use crate::models::{Game, GameEntry, Show};
use crate::rpc::LRRbot;
use crate::time::HumanReadable;
use crate::twitch::helix::User;
use crate::twitch::Helix;
use crate::typemap_keys::PgPool;
use chrono::{DateTime, FixedOffset, Utc};
use chrono_tz::Tz;
use diesel::OptionalExtension;
use failure::{Error, ResultExt};
use futures::compat::Stream01CompatExt;
use futures::prelude::*;
use separator::FixedPlaceSeparatable;
use slog_scope::error;
use std::fmt;
use std::time::{Duration, Instant};
use tokio::timer::Interval;

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
            self.event
                .start
                .with_timezone(&self.tz)
                .format("%a %e %b at %I:%M %p %Z")
        )?;

        Ok(())
    }
}

pub async fn autotopic(ctx: ErisContext) {
    let mut timer = Interval::new(Instant::now(), Duration::from_secs(60)).compat();

    loop {
        match timer.try_next().await {
            Ok(Some(_)) => {
                if let Err(err) = Autotopic.update_topic(&ctx).await {
                    error!("Failed to update the topic"; "error" => ?err);
                }
            }
            Ok(None) => break,
            Err(err) => {
                error!("Timer error"; "error" => ?err);
            }
        }
    }
}

#[derive(Copy, Clone)]
struct Autotopic;

impl Autotopic {
    async fn update_topic(self, ctx: &ErisContext) -> Result<(), Error> {
        let header = {
            let lrrbot = ctx.data.read().extract::<LRRbot>()?.clone();
            lrrbot
                .get_header_info()
                .await
                .context("failed to fetch header info")?
        };

        let mut messages = vec![];

        if header.is_live {
            let (game, show, game_entry) = {
                let conn = ctx
                    .data
                    .read()
                    .extract::<PgPool>()?
                    .get()
                    .context("failed to get a database connection from the pool")?;

                let game = header
                    .current_game
                    .map(|game| Game::find(game.id, &conn))
                    .transpose()
                    .context("failed to load the game")?;
                let show = header
                    .current_show
                    .map(|show| Show::find(show.id, &conn))
                    .transpose()
                    .context("failed to load the show")?;
                let game_entry =
                    if let (Some(game), Some(show)) = (header.current_game, header.current_show) {
                        GameEntry::find(game.id, show.id, &conn)
                            .optional()
                            .context("failed to load the game entry")?
                    } else {
                        None
                    };

                (game, show, game_entry)
            };

            match (game, show) {
                (Some(game), Some(show)) => {
                    messages.push(format!(
                        "Now live: {} on {}.",
                        game_entry
                            .and_then(|entry| entry.display_name)
                            .unwrap_or(game.name),
                        show.name
                    ));
                }
                (Some(game), None) => {
                    messages.push(format!(
                        "Now live: {}.",
                        game_entry
                            .and_then(|entry| entry.display_name)
                            .unwrap_or(game.name)
                    ));
                }
                (None, Some(show)) => {
                    messages.push(format!("Now live: {}.", show.name));
                }
                (None, None) => messages.push(String::from("Now live: something?")),
            }

            messages.push(self.uptime_msg(ctx, &header.channel).await?);
        } else {
            let now = Utc::now();

            let calendar = ctx.data.read().extract::<Calendar>()?.clone();
            let events = calendar
                .get_upcoming_events(LRR, now)
                .await
                .context("failed to get the next scheduled stream")?;
            let events = Calendar::get_next_event(&events, now, false);

            let desertbus = self.desertbus(ctx, now, &events).await?;
            if !desertbus.is_empty() {
                messages.extend(desertbus);
            } else {
                let tz = ctx.data.read().extract::<Config>()?.timezone;
                messages.extend(
                    events
                        .iter()
                        .map(|event| EventDisplay { event, now, tz }.to_string()),
                );
            }
        }

        if let Some(advice) = header.advice {
            messages.push(advice);
        }

        let general_channel = ctx.data.read().extract::<Config>()?.general_channel;

        // TODO: shorten to a max of 1024 characters, whatever that means.
        crate::blocking::blocking(|| general_channel.edit(ctx, |c| c.topic(&messages.join(" "))))
            .await
            .context("failed to exit the runtime")?
            .context("failed to update the topic")?;

        Ok(())
    }

    async fn uptime_msg<'a>(self, ctx: &'a ErisContext, channel: &'a str) -> Result<String, Error> {
        let helix = ctx.data.read().extract::<Helix>()?.clone();
        Ok(helix
            .get_stream(User::Login(channel))
            .await
            .context("failed to get the stream")?
            .map(|stream| {
                format!(
                    "The stream has been live for {}.",
                    HumanReadable::new(Utc::now() - stream.started_at.with_timezone(&Utc))
                )
            })
            .unwrap_or_else(|| String::from("The stream is not live.")))
    }

    async fn desertbus<'a>(
        self,
        ctx: &'a ErisContext,
        now: DateTime<Utc>,
        events: &'a [Event],
    ) -> Result<Vec<String>, Error> {
        let start = DesertBus::start_time().with_timezone(&Utc);
        let announce_start = start - chrono::Duration::days(2);
        let announce_end = start + chrono::Duration::days(7);
        let mut messages = vec![];

        if announce_start <= now && now <= announce_end {
            if let Some(next_event_start) = events.get(0).map(|event| event.start) {
                if next_event_start.with_timezone(&Utc) < start {
                    return Ok(messages);
                }
            }

            let desertbus = ctx.data.read().extract::<DesertBus>()?.clone();

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
                        // FIXME: .unwrap()
                        tz: ctx.data.read().extract::<Config>()?.timezone,
                    }
                    .to_string(),
                );
                messages.push(format!(
                    "${} raised.",
                    money_raised.separated_string_with_fixed_place(2)
                ));
            } else if now <= start + chrono::Duration::hours(total_hours) {
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
}
