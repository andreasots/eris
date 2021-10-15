use crate::config::Config;
use crate::context::ErisContext;
use crate::desertbus::DesertBus;
use crate::extract::Extract;
use crate::google::calendar::{Calendar, Event, LRR};
use crate::models::{Game, GameEntry, Show, User};
use crate::rpc::client::HeaderInfo;
use crate::rpc::LRRbot;
use crate::twitch::helix::UserId;
use crate::twitch::Helix;
use crate::{truncate::truncate, typemap_keys::PgPool};
use anyhow::{Context, Error};
use chrono::{DateTime, FixedOffset, Utc};
use diesel::OptionalExtension;
use separator::FixedPlaceSeparatable;
use serenity::prelude::TypeMap;
use std::fmt;
use std::time::Duration;
use tracing::error;

const TOPIC_MAX_LEN: usize = 1024;

struct EventDisplay<'a> {
    event: &'a Event,
}

impl<'a> fmt::Display for EventDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "<t:{}:R>: {} ", self.event.start.timestamp(), self.event.summary)?;

        if let Some(ref location) = self.event.location {
            write!(f, "({}) ", location)?;
        }

        if let Some(ref desc) = self.event.description {
            // TODO: shorten to 200 characters.
            write!(f, "({}) ", Calendar::format_description(desc))?;
        }
        write!(f, "on <t:{}:F>.", self.event.start.timestamp())?;

        Ok(())
    }
}

pub async fn autotopic(ctx: ErisContext) {
    let mut timer = tokio::time::interval(Duration::from_secs(60));

    loop {
        timer.tick().await;

        if let Err(error) = Autotopic.update_topic(&ctx).await {
            error!(?error, "Failed to update the topic");
        }
    }
}

#[derive(Copy, Clone)]
struct Autotopic;

impl Autotopic {
    async fn update_topic(self, ctx: &ErisContext) -> Result<(), Error> {
        let data = ctx.data.read().await;

        let header = match data.extract::<LRRbot>()?.get_header_info().await {
            Ok(header) => header,
            Err(error) => {
                error!(?error, "failed to fetch header info");

                HeaderInfo {
                    is_live: false,
                    channel: data.extract::<Config>()?.channel.clone(),
                    current_game: None,
                    current_show: None,
                    advice: None,
                }
            }
        };

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
                Err(error) => error!(?error, "failed to generate the uptime message"),
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
                messages.extend(events.iter().map(|event| EventDisplay { event }.to_string()));
            }
        }

        let mut channel = data
            .extract::<Config>()?
            .general_channel
            .to_channel(&ctx)
            .await
            .context("failed to get the #general channel")?
            .guild()
            .context("#general is not a guild channel?")?;

        let mut topic = messages.join(" ");

        if channel
            .topic
            .as_deref()
            .unwrap_or_default()
            .starts_with(truncate(&topic, TOPIC_MAX_LEN).0)
        {
            return Ok(());
        }

        if let Some(advice) = header.advice {
            if !topic.is_empty() {
                topic.push_str(" ");
            }
            topic.push_str(&advice);
        }

        channel
            .edit(ctx, |c| c.topic(truncate(&topic, TOPIC_MAX_LEN).0))
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
            .map(|stream| format!("The stream started <t:{}:R>.", stream.started_at.timestamp()))
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
                Err(error) => {
                    error!(?error, "Failed to fetch the current Desert Bus total");
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
