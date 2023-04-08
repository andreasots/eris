use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Error};
use google_youtube3::api::PlaylistItem;
use google_youtube3::hyper::client::HttpConnector;
use google_youtube3::hyper_rustls::HttpsConnector;
use google_youtube3::YouTube;
use regex::Regex;
use sea_orm::DatabaseConnection;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio::sync::watch::Receiver;
use tracing::{error, info, warn};
use twilight_cache_inmemory::InMemoryCache;
use twilight_http::Client as DiscordClient;
use twilight_model::channel::{Channel, ChannelType, Message};
use twilight_model::id::marker::{ChannelMarker, TagMarker};
use twilight_model::id::Id;
use twilight_validate::channel::CHANNEL_NAME_LENGTH_MAX;

use crate::config::Config;
use crate::models::state;

pub async fn post_videos(
    mut running: Receiver<bool>,
    db: DatabaseConnection,
    cache: Arc<InMemoryCache>,
    config: Arc<Config>,
    discord: Arc<DiscordClient>,
    youtube: YouTube<HttpsConnector<HttpConnector>>,
) {
    let Some(channel_id) = config.lrr_videos_channel else {
        info!("video discussion forum is not set");
        return
    };

    if config.youtube_channels.is_empty() {
        info!("Youtube channels are not set");
        return;
    }

    let mut poster = match VideoPoster::new(db, cache, channel_id, &config, discord, youtube).await
    {
        Ok(poster) => poster,
        Err(error) => {
            error!(?error, "failed to construct the video poster");
            return;
        }
    };
    let mut interval = tokio::time::interval(Duration::from_secs(300));

    loop {
        tokio::select! {
            _ = running.changed() => break,
            _ = interval.tick() => {
                if let Err(error) = poster.run().await {
                    poster.check_for_existing_threads = true;
                    error!(?error, "failed to post videos");
                }
            },
        }
    }
}

struct VideoPoster {
    db: DatabaseConnection,
    cache: Arc<InMemoryCache>,
    channel_id: Id<ChannelMarker>,
    playlists: Vec<String>,
    discord: Arc<DiscordClient>,
    youtube: YouTube<HttpsConnector<HttpConnector>>,

    check_for_existing_threads: bool,
}

impl VideoPoster {
    async fn new(
        db: DatabaseConnection,
        cache: Arc<InMemoryCache>,
        channel_id: Id<ChannelMarker>,
        config: &Config,
        discord: Arc<DiscordClient>,
        youtube: YouTube<HttpsConnector<HttpConnector>>,
    ) -> Result<Self, Error> {
        let mut req = youtube.channels().list(&vec!["contentDetails".into()]);
        for channel in &config.youtube_channels {
            req = req.add_id(channel);
        }
        let (_, channel_list) = req.doit().await.context("failed to list the channels")?;
        let mut channels = Vec::with_capacity(config.youtube_channels.len());
        for channel in channel_list.items.context("Youtube returned no channels")? {
            channels.push(
                channel
                    .content_details
                    .context("requested `contentDetails` but `content_details` is missing")?
                    .related_playlists
                    .context("related playlists is empty")?
                    .uploads
                    .context("no uploads playlist")?,
            );
        }

        Ok(Self {
            db,
            cache,
            channel_id,
            playlists: channels,
            discord,
            youtube,
            check_for_existing_threads: true,
        })
    }

    async fn run(&mut self) -> Result<(), Error> {
        let Some(channel) = self.cache.channel(self.channel_id) else {
            return Err(Error::msg("video announcements channel not in cache"));
        };

        let mut videos = vec![];

        for playlist_id in &self.playlists {
            // Hopefully all the new videos are on the first page of results...
            let (_, playlist) = self
                .youtube
                .playlist_items()
                .list(&vec!["snippet".into()])
                .playlist_id(playlist_id)
                .doit()
                .await
                .context("playlist request failed")?;
            if let Some(items) = playlist.items {
                for video in items {
                    videos
                        .push(Video::try_from(video).with_context(|| {
                            format!("invalid item in playlist {playlist_id:?}")
                        })?);
                }
            }
        }

        videos.sort_by(|a, b| a.published_at.cmp(&b.published_at));

        for video in videos {
            let state_key =
                format!("eris.announcements.youtube.{}.last_video_published_at", video.channel_id);
            let last_published_at = state::get::<String>(&state_key, &self.db)
                .await
                .context("failed to get the last video published timestamp")?
                .map(|ts| OffsetDateTime::parse(&ts, &Rfc3339))
                .transpose()
                .context("failed to parse the last video published timestamp")?
                .unwrap_or(OffsetDateTime::UNIX_EPOCH);

            if last_published_at >= video.published_at {
                continue;
            }

            let is_announced = if self.check_for_existing_threads {
                video.is_already_announced(&channel, &self.cache, &self.discord).await
                    .unwrap_or_else(|error| {
                        error!(
                            ?error,
                            video.id,
                            "failed to determine if the video is already announced, assuming that it is not"
                        );

                        false
                    })
            } else {
                false
            };

            if !is_announced {
                video
                    .announce(&channel, &self.discord)
                    .await
                    .context("failed to announce video")?;
            }

            let published_at = video
                .published_at
                .format(&Rfc3339)
                .context("failed to format the video published timestamp")?;
            state::set(state_key, &published_at, &self.db)
                .await
                .context("failed to set the last video published timestamp")?;
        }

        self.check_for_existing_threads = false;

        Ok(())
    }
}

pub struct Video {
    channel_title: String,
    channel_id: String,
    id: String,
    title: String,
    description: String,
    published_at: OffsetDateTime,
}

impl Video {
    pub fn video_id_from_message(message: &str) -> Option<&str> {
        lazy_static::lazy_static! {
            static ref RE_VIDEO_ID: Regex = Regex::new(r"(?m)^Video: https://youtu.be/(\S+)$").unwrap();
        }

        Some(RE_VIDEO_ID.captures(message)?.get(1)?.as_str())
    }

    async fn is_already_announced(
        &self,
        channel: &Channel,
        cache: &InMemoryCache,
        discord: &DiscordClient,
    ) -> Result<bool, Error> {
        let mut threads = cache
            .guild_channels(channel.guild_id.context("channel not in a guild")?)
            .context("guild channels not in cache")?
            .iter()
            .copied()
            .filter_map(|id| {
                let channel = cache.channel(id);
                if channel.is_none() {
                    error!(
                        channel.id = id.get(),
                        "channel referenced by cache but not itself in cache"
                    );
                }
                channel
            })
            .filter(|thread| thread.parent_id == Some(channel.id))
            .map(|thread| thread.id)
            .collect::<Vec<_>>();

        threads.sort_by(|a, b| a.cmp(b).reverse());
        threads.truncate(10);

        for thread_id in threads {
            let mut messages = discord
                .channel_messages(thread_id)
                .after(Id::new(1))
                .limit(1)
                .context("limit invalid")?
                .await
                .context("failed to get the messages")?
                .models()
                .await
                .context("failed to deserialize the messages")?;

            let Some(original_message) = messages.pop() else {
                warn!(thread.id = thread_id.get(), "thread empty or no permissions");
                continue
            };
            let original_message = if let Some(message) = original_message.referenced_message {
                (*message).clone()
            } else {
                original_message
            };

            if Self::video_id_from_message(&original_message.content) == Some(&self.id) {
                info!(
                    thread.id = thread_id.get(),
                    video.id = self.id,
                    "announcement thread already created"
                );

                return Ok(true);
            }
        }

        Ok(false)
    }

    fn message_content(&self) -> String {
        let description = crate::shorten::shorten(
            self.description.split("Support LRR:").next().unwrap_or("").trim(),
            twilight_validate::message::MESSAGE_CONTENT_LENGTH_MAX / 2,
        );

        let mut message = String::new();
        for line in description.lines() {
            message.push_str("> ");
            message.push_str(&crate::markdown::suppress_embeds(line));
            message.push('\n');
        }
        if !message.is_empty() {
            message.push('\n');
        }
        message.push_str("Video: https://youtu.be/");
        message.push_str(&self.id);
        message.push_str("\n\u{200B}");
        message
    }

    fn tags(&self, channel: &Channel) -> Vec<Id<TagMarker>> {
        channel
            .available_tags
            .as_deref()
            .into_iter()
            .flatten()
            .filter_map(|tag| (self.channel_title == tag.name).then_some(tag.id))
            .collect::<Vec<_>>()
    }

    pub async fn announce(
        &self,
        channel: &Channel,
        discord: &DiscordClient,
    ) -> Result<Channel, Error> {
        if channel.kind == ChannelType::GuildForum {
            let thread = discord
                .create_forum_thread(
                    channel.id,
                    &crate::shorten::shorten(&self.title, CHANNEL_NAME_LENGTH_MAX),
                )
                .applied_tags(&self.tags(channel))
                .message()
                .content(&self.message_content())
                .context("video announcement invalid")?
                .await
                .context("failed to create the video thread")?
                .model()
                .await
                .context("failed to deserialize the thread")?
                .channel;

            Ok(thread)
        } else {
            let message = discord
                .create_message(channel.id)
                .content(&self.message_content())
                .context("video announcement invalid")?
                .await
                .context("failed to send video announcement")?
                .model()
                .await
                .context("failed to deserialize the message")?;

            let thread = discord
                .create_thread_from_message(
                    channel.id,
                    message.id,
                    &crate::shorten::shorten(&self.title, CHANNEL_NAME_LENGTH_MAX),
                )
                .context("thread name invalid")?
                .await
                .context("failed to create the thread")?
                .model()
                .await
                .context("failed to deserialize the thread")?;

            Ok(thread)
        }
    }

    pub async fn edit(
        &self,
        discord: &DiscordClient,
        channel: &Channel,
        message: &Message,
        thread: &Channel,
    ) -> Result<(), Error> {
        let mut req = discord.update_thread(thread.id);
        let tags;
        if channel.available_tags.is_some() {
            tags = self.tags(channel);
            req = req.applied_tags(Some(&tags));
        }
        req.name(&crate::shorten::shorten(&self.title, CHANNEL_NAME_LENGTH_MAX))
            .context("thread name invalid")?
            .await
            .context("failed to update thread name")?;

        discord
            .update_message(message.channel_id, message.id)
            .content(Some(&self.message_content()))
            .context("video announcement invalid")?
            .await
            .context("failed to update the video announcement")?;
        Ok(())
    }
}

impl TryFrom<google_youtube3::api::Video> for Video {
    type Error = Error;

    fn try_from(video: google_youtube3::api::Video) -> Result<Self, Self::Error> {
        let snippet = video.snippet.context("`snippet` is missing")?;
        Ok(Self {
            channel_title: snippet.channel_title.context("`channel_title` is missing")?,
            channel_id: snippet.channel_id.context("`channel_id` is missing")?,
            id: video.id.context("`id` is missing")?,
            title: snippet.title.context("`title` is missing")?,
            description: snippet.description.context("`description` is missing")?,
            published_at: OffsetDateTime::from_unix_timestamp_nanos(
                snippet.published_at.context("`published_at` is missing")?.timestamp_nanos()
                    as i128,
            )
            .context("failed to convert `published_at`")?,
        })
    }
}

impl TryFrom<PlaylistItem> for Video {
    type Error = Error;

    fn try_from(video: PlaylistItem) -> Result<Self, Self::Error> {
        let snippet = video.snippet.context("`snippet` is missing")?;
        Ok(Self {
            channel_title: snippet.channel_title.context("`channel_title` is missing")?,
            channel_id: snippet.channel_id.context("`channel_id` is missing")?,
            id: snippet
                .resource_id
                .context("`resource_id` missing")?
                .video_id
                .context("`resource_id.video_id` is missing")?,
            title: snippet.title.context("`title` is missing")?,
            description: snippet.description.context("`description` is missing")?,
            published_at: OffsetDateTime::from_unix_timestamp_nanos(
                snippet.published_at.context("`published_at` is missing")?.timestamp_nanos()
                    as i128,
            )
            .context("failed to convert `published_at`")?,
        })
    }
}
