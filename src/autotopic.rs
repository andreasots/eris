use crate::config::Config;
use crate::context::ErisContext;
use crate::desertbus::DesertBus;
use crate::extract::Extract;
use crate::google::calendar::{Calendar, Event, LRR};
use crate::models::{Game, GameEntry, Show, User};
use crate::rpc::client::HeaderInfo;
use crate::rpc::LRRbot;
use crate::shorten::shorten;
use crate::twitch::helix::UserId;
use crate::twitch::Helix;
use crate::typemap_keys::PgPool;
use anyhow::{Context, Error};
use chrono::{DateTime, FixedOffset, Utc};
use diesel::OptionalExtension;
use separator::FixedPlaceSeparatable;
use serenity::prelude::TypeMap;
use std::fmt;
use std::time::Duration;
use tracing::error;

const TOPIC_MAX_LEN: usize = 1024;
// Hopefully normal messages don't contain this sequence.
const DYNAMIC_TAIL_SEPARATOR: &str = " \u{2009}\u{200A}\u{200B}";
// Don't update the topic if the old and new topics have a Levenshtein distance below `SIMILARITY_THRESHOLD`.
const SIMILARITY_THRESHOLD: usize = 5;
// But even then update the topic every `SIMILAR_MIN_UPDATE_INTERVAL_MINUTES` minutes.
const SIMILAR_MIN_UPDATE_INTERVAL_MINUTES: i64 = 30;

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
    let mut autotopic = Autotopic { last_updated: None };

    loop {
        timer.tick().await;

        if let Err(error) = autotopic.update_topic(&ctx).await {
            error!(?error, "Failed to update the topic");
        }
    }
}

#[derive(Copy, Clone)]
struct Autotopic {
    last_updated: Option<DateTime<Utc>>,
}

impl Autotopic {
    async fn set_topic(
        &mut self,
        new_topic: &str,
        is_dynamic: bool,
        ctx: &ErisContext,
        data: &TypeMap,
    ) -> Result<(), Error> {
        let new_topic = shorten(new_topic, TOPIC_MAX_LEN);
        let new_topic = new_topic.as_ref();

        let mut channel = data
            .extract::<Config>()?
            .general_channel
            .to_channel(&ctx)
            .await
            .context("failed to get the #general channel")?
            .guild()
            .context("#general is not a guild channel?")?;

        let old_topic = channel.topic.as_deref().unwrap_or_default();

        let new_topic_static_prefix =
            new_topic.rsplit_once(DYNAMIC_TAIL_SEPARATOR).unwrap_or((new_topic, "")).0;
        let old_topic_static_prefix =
            old_topic.rsplit_once(DYNAMIC_TAIL_SEPARATOR).unwrap_or((old_topic, "")).0;

        let now = Utc::now();

        if !is_dynamic {
            if old_topic_static_prefix == new_topic_static_prefix {
                return Ok(());
            }
        } else {
            let distance =
                levenshtein::levenshtein(old_topic_static_prefix, new_topic_static_prefix);
            if distance == 0 {
                return Ok(());
            } else if distance < SIMILARITY_THRESHOLD {
                // Unfortunately `chrono::Duration`'s constructors are not `const`.
                let update_interval =
                    chrono::Duration::minutes(SIMILAR_MIN_UPDATE_INTERVAL_MINUTES);
                if self.last_updated.map(|t| (now - t) < update_interval).unwrap_or(false) {
                    return Ok(());
                }
            }
        }

        channel.edit(ctx, |c| c.topic(new_topic)).await.context("failed to update the topic")?;
        self.last_updated = Some(now);

        Ok(())
    }

    async fn update_topic(&mut self, ctx: &ErisContext) -> Result<(), Error> {
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
        let mut is_dynamic = false;

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

            let (desertbus, desertbus_is_dynamic) =
                self.desertbus(&data, &user, now, &events).await?;
            if !desertbus.is_empty() {
                messages.extend(desertbus);
                is_dynamic |= desertbus_is_dynamic;
            } else {
                messages.extend(events.iter().map(|event| EventDisplay { event }.to_string()));
            }
        }

        let mut topic = messages.join(" ");
        if let Some(advice) = header.advice {
            if !topic.is_empty() {
                topic.push_str(DYNAMIC_TAIL_SEPARATOR);
            }
            topic.push_str(&advice);
        }

        self.set_topic(&topic, is_dynamic, ctx, &data)
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
    ) -> Result<(Vec<String>, bool), Error> {
        let start = DesertBus::start_time().with_timezone(&Utc);
        let announce_start = start - chrono::Duration::days(2);
        let announce_end = start + chrono::Duration::days(9);
        let mut messages = vec![];
        let mut is_dynamic = false;

        if announce_start <= now && now <= announce_end {
            if let Some(next_event_start) = events.get(0).map(|event| event.start) {
                if next_event_start.with_timezone(&Utc) < start {
                    return Ok((messages, is_dynamic));
                }
            }
            let desertbus = data.extract::<DesertBus>()?;

            let money_raised = match desertbus.money_raised().await {
                Ok(money_raised) => money_raised,
                Err(error) => {
                    error!(?error, "Failed to fetch the current Desert Bus total");
                    messages.push(String::from("DESERT BUS?"));
                    return Ok((messages, is_dynamic));
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
                is_dynamic = true;
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
                is_dynamic = true;
            }
        }

        Ok((messages, is_dynamic))
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
