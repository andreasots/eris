use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Error};
use chrono::{DateTime, Utc};
use sea_orm::{DatabaseConnection, EntityTrait};
use tokio::sync::RwLock;
use tokio::sync::watch::Receiver;
use tracing::error;
use twilight_http::Client as DiscordClient;
use twitch_api::HelixClient;
use twitch_api::helix::streams::GetStreamsRequest;
use twitch_api::twitch_oauth2::AppAccessToken;
use twitch_api::types::UserNameRef;

use crate::cache::Cache;
use crate::calendar::{CalendarHub, Event};
use crate::config::Config;
use crate::models::{game, game_entry, show};
use crate::rpc::LRRbot;
use crate::rpc::client::HeaderInfo;
use crate::shorten::shorten;

const TOPIC_MAX_LEN: usize = 1024;
// Hopefully normal messages don't contain this sequence.
const DYNAMIC_TAIL_SEPARATOR: &str = " \u{2009}\u{200A}\u{200B}";

struct EventDisplay<'a> {
    event: &'a Event,
}

impl fmt::Display for EventDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "<t:{}:R>: {} ", self.event.start.timestamp(), self.event.summary)?;

        if let Some(ref location) = self.event.location {
            write!(f, "({}) ", crate::markdown::escape(location))?;
        }

        if let Some(ref desc) = self.event.description {
            let desc = crate::calendar::format_description(desc);
            write!(f, "({}) ", crate::markdown::escape(&crate::shorten::shorten(&desc, 200)))?;
        }
        write!(f, "on <t:{}:F>.", self.event.start.timestamp())?;

        Ok(())
    }
}

pub async fn autotopic(
    mut running: Receiver<bool>,
    cache: Arc<Cache>,
    calendar: CalendarHub,
    config: Arc<Config>,
    db: DatabaseConnection,
    discord: Arc<DiscordClient>,
    helix: HelixClient<'static, reqwest::Client>,
    helix_token: Arc<RwLock<AppAccessToken>>,
    lrrbot: Arc<LRRbot>,
) {
    let mut timer = tokio::time::interval(Duration::from_secs(60));
    let mut autotopic =
        Autotopic::new(cache, calendar, config, db, discord, helix, helix_token, lrrbot);

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
    last_updated: Option<DateTime<Utc>>,

    cache: Arc<Cache>,
    calendar: CalendarHub,
    config: Arc<Config>,
    db: DatabaseConnection,
    discord: Arc<DiscordClient>,
    helix: HelixClient<'static, reqwest::Client>,
    helix_token: Arc<RwLock<AppAccessToken>>,
    lrrbot: Arc<LRRbot>,
}

impl Autotopic {
    fn new(
        cache: Arc<Cache>,
        calendar: CalendarHub,
        config: Arc<Config>,
        db: DatabaseConnection,
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
            discord,
            helix,
            helix_token,
            lrrbot,
        }
    }

    async fn set_topic(&mut self, new_topic: &str) -> Result<(), Error> {
        self.cache.wait_until_ready().await;

        let new_topic = shorten(new_topic, TOPIC_MAX_LEN);
        let new_topic = new_topic.as_ref();

        let old_topic = self
            .cache
            .with(|cache| {
                cache.channel(self.config.general_channel).map(|channel| channel.topic.clone())
            })
            .context("announcement channel not in cache")?
            .unwrap_or_default();

        let new_topic_static_prefix =
            new_topic.rsplit_once(DYNAMIC_TAIL_SEPARATOR).unwrap_or((new_topic, "")).0;
        let old_topic_static_prefix =
            old_topic.rsplit_once(DYNAMIC_TAIL_SEPARATOR).unwrap_or((&old_topic, "")).0;

        let now = Utc::now();

        if old_topic_static_prefix == new_topic_static_prefix {
            return Ok(());
        }

        self.discord
            .update_channel(self.config.general_channel)
            .topic(new_topic)
            .await
            .context("failed to update the topic")?;
        self.last_updated = Some(now);

        Ok(())
    }

    async fn update_topic(&mut self) -> Result<(), Error> {
        let header = self.lrrbot.get_header_info().await.unwrap_or_else(|error| {
            error!(?error, "failed to fetch header info");

            HeaderInfo {
                is_live: false,
                channel: self.config.channel.clone(),
                current_game: None,
                current_show: None,
                advice: None,
            }
        });

        let mut messages = vec![];

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
            let now = Utc::now();

            let events =
                crate::calendar::get_next_event(&self.calendar, crate::calendar::LRR, now, false)
                    .await
                    .context("failed to get the next scheduled stream")?;

            messages.extend(events.iter().map(|event| EventDisplay { event }.to_string()));
        }

        let mut topic = messages.join(" ");
        if let Some(advice) = header.advice {
            if !topic.is_empty() {
                topic.push_str(DYNAMIC_TAIL_SEPARATOR);
            }
            topic.push_str(&advice);
        }

        self.set_topic(&topic).await.context("failed to update the topic")?;

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
            .map_or_else(
                || String::from("The stream is not live."),
                |stream| {
                    format!(
                        "The stream started <t:{}:R>.",
                        stream.started_at.to_fixed_offset().unix_timestamp()
                    )
                },
            ))
    }
}
