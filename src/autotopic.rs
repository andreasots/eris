use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Error};
use sea_orm::{DatabaseConnection, EntityTrait};
use separator::FixedPlaceSeparatable;
use time::OffsetDateTime;
use tokio::sync::watch::Receiver;
use tokio::sync::RwLock;
use tracing::error;
use twilight_cache_inmemory::InMemoryCache;
use twilight_http::Client as DiscordClient;
use twitch_api::helix::streams::GetStreamsRequest;
use twitch_api::twitch_oauth2::AppAccessToken;
use twitch_api::types::UserNameRef;
use twitch_api::HelixClient;

use crate::calendar::{CalendarHub, Event};
use crate::config::Config;
use crate::desertbus::DesertBus;
use crate::models::{game, game_entry, show};
use crate::rpc::client::HeaderInfo;
use crate::rpc::LRRbot;
use crate::shorten::shorten;

const TOPIC_MAX_LEN: usize = 1024;
// Hopefully normal messages don't contain this sequence.
const DYNAMIC_TAIL_SEPARATOR: &str = " \u{2009}\u{200A}\u{200B}";
// Don't update the topic if the old and new topics have a Levenshtein distance below `SIMILARITY_THRESHOLD`.
const SIMILARITY_THRESHOLD: usize = 5;
// But even then update the topic every `SIMILAR_MIN_UPDATE_INTERVAL`.
const SIMILAR_MIN_UPDATE_INTERVAL: time::Duration = time::Duration::minutes(30);

struct EventDisplay<'a> {
    event: &'a Event,
}

impl<'a> fmt::Display for EventDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "<t:{}:R>: {} ", self.event.start.unix_timestamp(), self.event.summary)?;

        if let Some(ref location) = self.event.location {
            write!(f, "({}) ", crate::markdown::escape(location))?;
        }

        if let Some(ref desc) = self.event.description {
            let desc = crate::calendar::format_description(desc);
            write!(f, "({}) ", crate::markdown::escape(&crate::shorten::shorten(&desc, 200)))?;
        }
        write!(f, "on <t:{}:F>.", self.event.start.unix_timestamp())?;

        Ok(())
    }
}

pub async fn autotopic(
    mut running: Receiver<bool>,
    cache: Arc<InMemoryCache>,
    calendar: CalendarHub,
    config: Arc<Config>,
    db: DatabaseConnection,
    desertbus: DesertBus,
    discord: Arc<DiscordClient>,
    helix: HelixClient<'static, reqwest::Client>,
    helix_token: Arc<RwLock<AppAccessToken>>,
    lrrbot: Arc<LRRbot>,
) {
    let mut timer = tokio::time::interval(Duration::from_secs(60));
    let mut autotopic =
        Autotopic::new(cache, calendar, config, db, desertbus, discord, helix, helix_token, lrrbot);

    loop {
        tokio::select! {
            _ = running.changed() => break,
            _ = timer.tick() => {
                if let Err(error) = autotopic.update_topic().await {
                    error!(?error, "Failed to update the topic");
                }
            },
        }
    }
}

struct Autotopic {
    last_updated: Option<OffsetDateTime>,

    cache: Arc<InMemoryCache>,
    calendar: CalendarHub,
    config: Arc<Config>,
    db: DatabaseConnection,
    desertbus: DesertBus,
    discord: Arc<DiscordClient>,
    helix: HelixClient<'static, reqwest::Client>,
    helix_token: Arc<RwLock<AppAccessToken>>,
    lrrbot: Arc<LRRbot>,
}

impl Autotopic {
    fn new(
        cache: Arc<InMemoryCache>,
        calendar: CalendarHub,
        config: Arc<Config>,
        db: DatabaseConnection,
        desertbus: DesertBus,
        discord: Arc<DiscordClient>,
        helix: HelixClient<'static, reqwest::Client>,
        helix_token: Arc<RwLock<AppAccessToken>>,
        lrrbot: Arc<LRRbot>,
    ) -> Self {
        Self {
            last_updated: None,
            cache,
            calendar,
            config,
            db,
            desertbus,
            discord,
            helix,
            helix_token,
            lrrbot,
        }
    }

    async fn set_topic(&mut self, new_topic: &str, is_dynamic: bool) -> Result<(), Error> {
        let new_topic = shorten(new_topic, TOPIC_MAX_LEN);
        let new_topic = new_topic.as_ref();

        let channel = self
            .cache
            .channel(self.config.general_channel)
            .context("announcement channel not in cache")?;

        let old_topic = channel.topic.as_deref().unwrap_or_default();

        let new_topic_static_prefix =
            new_topic.rsplit_once(DYNAMIC_TAIL_SEPARATOR).unwrap_or((new_topic, "")).0;
        let old_topic_static_prefix =
            old_topic.rsplit_once(DYNAMIC_TAIL_SEPARATOR).unwrap_or((old_topic, "")).0;

        let now = OffsetDateTime::now_utc();

        if !is_dynamic {
            if old_topic_static_prefix == new_topic_static_prefix {
                return Ok(());
            }
        } else {
            let distance =
                levenshtein::levenshtein(old_topic_static_prefix, new_topic_static_prefix);
            if distance == 0
                || distance < SIMILARITY_THRESHOLD
                    && self
                        .last_updated
                        .map(|t| (now - t) < SIMILAR_MIN_UPDATE_INTERVAL)
                        .unwrap_or(false)
            {
                return Ok(());
            }
        }

        self.discord
            .update_channel(self.config.general_channel)
            .topic(new_topic)
            .context("new topic is invalid")?
            .await
            .context("failed to update the topic")?;
        self.last_updated = Some(now);

        Ok(())
    }

    async fn update_topic(&mut self) -> Result<(), Error> {
        let header = match self.lrrbot.get_header_info().await {
            Ok(header) => header,
            Err(error) => {
                error!(?error, "failed to fetch header info");

                HeaderInfo {
                    is_live: false,
                    channel: self.config.channel.clone(),
                    current_game: None,
                    current_show: None,
                    advice: None,
                }
            }
        };

        let mut messages = vec![];
        let mut is_dynamic = false;

        let game = if let Some(game) = header.current_game {
            game::Entity::find_by_id(game.id)
                .one(&self.db)
                .await
                .context("failed to load the game")?
        } else {
            None
        };

        let show = if let Some(show) = header.current_show {
            show::Entity::find_by_id(show.id)
                .one(&self.db)
                .await
                .context("failed to load the show")?
        } else {
            None
        };

        let game_entry =
            if let (Some(game), Some(show)) = (header.current_game, header.current_show) {
                game_entry::Entity::find_by_id((game.id, show.id))
                    .one(&self.db)
                    .await
                    .context("failed to load the game entry")?
            } else {
                None
            };

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

            match self.uptime_msg(&header.channel).await {
                Ok(msg) => messages.push(msg),
                Err(error) => error!(?error, "failed to generate the uptime message"),
            }
        } else {
            let now = OffsetDateTime::now_utc();

            let events =
                crate::calendar::get_next_event(&self.calendar, crate::calendar::LRR, now, false)
                    .await
                    .context("failed to get the next scheduled stream")?;

            let (desertbus, desertbus_is_dynamic) = self.desertbus(now, &events).await?;
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

        self.set_topic(&topic, is_dynamic).await.context("failed to update the topic")?;

        Ok(())
    }

    async fn uptime_msg(&self, channel: &str) -> Result<String, Error> {
        Ok(self
            .helix
            .req_get(
                GetStreamsRequest::user_logins([UserNameRef::from_str(channel)].as_ref()),
                &*self.helix_token.read().await,
            )
            .await
            .context("failed to get the stream")?
            .data
            .first()
            .map(|stream| {
                format!(
                    "The stream started <t:{}:R>.",
                    stream.started_at.to_fixed_offset().unix_timestamp()
                )
            })
            .unwrap_or_else(|| String::from("The stream is not live.")))
    }

    async fn desertbus(
        &self,
        now: OffsetDateTime,
        events: &[Event],
    ) -> Result<(Vec<String>, bool), Error> {
        let start = DesertBus::start_time();
        let announce_start = start - time::Duration::days(2);
        let announce_end = start + time::Duration::days(9);
        let mut messages = vec![];
        let mut is_dynamic = false;

        if announce_start <= now && now <= announce_end {
            if let Some(next_event_start) = events.get(0).map(|event| event.start) {
                if next_event_start < start {
                    return Ok((messages, is_dynamic));
                }
            }

            let money_raised = match self.desertbus.money_raised().await {
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
                            start,
                            summary: String::from("Desert Bus for Hope"),
                            end: start + time::Duration::hours(total_hours),
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
            } else if now <= start + time::Duration::hours(total_hours)
                || self.is_desertbus_live().await?
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
                    bussed.whole_hours(),
                    bussed.whole_minutes() % 60,
                    total_hours
                ));
                is_dynamic = true;
            }
        }

        Ok((messages, is_dynamic))
    }

    async fn is_desertbus_live(&self) -> Result<bool, Error> {
        Ok(!self
            .helix
            .req_get(
                GetStreamsRequest::user_logins([UserNameRef::from_str("desertbus")].as_ref()),
                &*self.helix_token.read().await,
            )
            .await
            .context("failed to get the stream")?
            .data
            .is_empty())
    }
}
