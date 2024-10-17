use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Context, Error};
use chrono::{DateTime, Utc};
use google_youtube3::api::PlaylistItem;
use google_youtube3::hyper_rustls::HttpsConnector;
use google_youtube3::hyper_util::client::legacy::connect::HttpConnector;
use google_youtube3::YouTube;
use regex::Regex;
use sea_orm::DatabaseConnection;
use tokio::sync::watch::Receiver;
use tracing::{error, info, warn};
use twilight_http::Client as DiscordClient;
use twilight_model::channel::forum::ForumTag;
use twilight_model::channel::{Channel, ChannelType, Message};
use twilight_model::id::marker::{ChannelMarker, GuildMarker, TagMarker};
use twilight_model::id::Id;
use twilight_validate::channel::CHANNEL_NAME_LENGTH_MAX;

use crate::cache::Cache;
use crate::config::Config;
use crate::models::state;

pub async fn post_videos(
    mut running: Receiver<bool>,
    db: DatabaseConnection,
    cache: Arc<Cache>,
    config: Arc<Config>,
    discord: Arc<DiscordClient>,
    youtube: YouTube<HttpsConnector<HttpConnector>>,
) {
    let Some(channel_id) = config.lrr_videos_channel else {
        info!("video discussion forum is not set");
        return;
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
                    error!(?error, "failed to post videos");
                }
            },
        }
    }
}

struct VideoPoster {
    db: DatabaseConnection,
    cache: Arc<Cache>,
    channel_id: Id<ChannelMarker>,
    playlists: Vec<String>,
    discord: Arc<DiscordClient>,
    youtube: YouTube<HttpsConnector<HttpConnector>>,
}

impl VideoPoster {
    async fn new(
        db: DatabaseConnection,
        cache: Arc<Cache>,
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
        let mut playlists = Vec::with_capacity(config.youtube_channels.len());
        for channel in channel_list.items.context("Youtube returned no channels")? {
            playlists.push(
                channel
                    .content_details
                    .context("requested `contentDetails` but `content_details` is missing")?
                    .related_playlists
                    .context("related playlists is empty")?
                    .uploads
                    .context("no uploads playlist")?,
            );
        }

        Ok(Self { db, cache, channel_id, playlists, discord, youtube })
    }

    async fn run(&mut self) -> Result<(), Error> {
        self.cache.wait_until_ready().await;

        let (channel_type, guild_id, available_tags) = self
            .cache
            .with(|cache| {
                let channel = cache.channel(self.channel_id)?;
                Some((channel.kind, channel.guild_id, channel.available_tags.clone()))
            })
            .context("video announcements channel not in cache")?;
        let guild_id = guild_id.context("video announcements channel not in a guild")?;

        let mut videos = vec![];

        for playlist_id in &self.playlists {
            // Hopefully all the new videos are on the first page of results...
            let res = self
                .youtube
                .playlist_items()
                .list(&vec!["snippet".into()])
                .playlist_id(playlist_id)
                .doit()
                .await;
            match res {
                Ok((_, playlist)) => {
                    if let Some(items) = playlist.items {
                        for video in items {
                            videos.push(Video::try_from(video).with_context(|| {
                                format!("invalid item in playlist {playlist_id:?}")
                            })?);
                        }
                    }
                }
                Err(error) => error!(?error, "playlist request failed"),
            }
        }

        videos.sort_by(|a, b| a.published_at.cmp(&b.published_at));

        for video in videos {
            let state_key =
                format!("eris.announcements.youtube.{}.last_video_published_at", video.channel_id);
            let last_published_at = state::get::<String>(&state_key, &self.db)
                .await
                .context("failed to get the last video published timestamp")?
                .map(|ts| DateTime::parse_from_rfc3339(&ts))
                .transpose()
                .context("failed to parse the last video published timestamp")?
                .map_or(DateTime::UNIX_EPOCH, |ts| ts.with_timezone(&Utc));

            if last_published_at >= video.published_at {
                continue;
            }

            let is_announced =
                video.is_already_announced(self.channel_id, guild_id, &self.cache, &self.discord).await
                    .unwrap_or_else(|error| {
                        error!(
                            ?error,
                            video.id,
                            "failed to determine if the video is already announced, assuming that it is not"
                        );

                        false
                    });

            let should_announce =
                video.should_announce(&self.youtube).await.unwrap_or_else(|error| {
                    error!(
                        ?error,
                        video.id,
                        "failed to determine if the video should be announced, assuming that it does"
                    );

                    true
                });

            if !is_announced && should_announce {
                video
                    .announce(
                        self.channel_id,
                        channel_type,
                        available_tags.as_deref(),
                        &self.discord,
                    )
                    .await
                    .context("failed to announce video")?;
            }

            state::set(state_key, &video.published_at.to_rfc3339(), &self.db)
                .await
                .context("failed to set the last video published timestamp")?;
        }

        Ok(())
    }
}

pub struct Video {
    channel_title: String,
    channel_id: String,
    id: String,
    title: String,
    description: String,
    published_at: DateTime<Utc>,
}

impl Video {
    pub fn video_id_from_message(message: &str) -> Option<&str> {
        static RE_VIDEO_ID: OnceLock<Regex> = OnceLock::new();
        let re_video_id =
            RE_VIDEO_ID.get_or_init(|| Regex::new(r"(?m)^Video: https://youtu.be/(\S+)$").unwrap());

        Some(re_video_id.captures(message)?.get(1)?.as_str())
    }

    async fn is_already_announced(
        &self,
        channel_id: Id<ChannelMarker>,
        guild_id: Id<GuildMarker>,
        cache: &Cache,
        discord: &DiscordClient,
    ) -> Result<bool, Error> {
        let mut threads = cache
            .with(|cache| {
                Some(
                    cache
                        .guild_channels(guild_id)?
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
                        .filter(|thread| thread.parent_id == Some(channel_id))
                        .map(|thread| thread.id)
                        .collect::<Vec<_>>(),
                )
            })
            .context("guild channels not in cache")?;

        threads.sort_by(|a, b| a.cmp(b).reverse());
        threads.truncate(10);

        for thread_id in threads {
            let mut messages = discord
                .channel_messages(thread_id)
                .after(Id::new(1))
                .limit(1)
                .await
                .context("failed to get the messages")?
                .models()
                .await
                .context("failed to deserialize the messages")?;

            let Some(original_message) = messages.pop() else {
                warn!(thread.id = thread_id.get(), "thread empty or no permissions");
                continue;
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

    async fn should_announce(
        &self,
        youtube: &YouTube<HttpsConnector<HttpConnector>>,
    ) -> Result<bool, Error> {
        let (_, list) = youtube
            .videos()
            .list(&vec!["contentDetails".into(), "liveStreamingDetails".into(), "player".into()])
            .max_width(720) // need to specify something to get the player height
            .add_id(&self.id)
            .doit()
            .await
            .context("failed to fetch video details")?;

        let video = list
            .items
            .context("video query returned no videos")?
            .into_iter()
            .find(|video| video.id.as_deref() == Some(&self.id))
            .context("video query didn't return this video")?;

        let duration = video
            .content_details
            .as_ref()
            .context("`contentDetails` is missing")?
            .duration
            .as_ref()
            .context("`contentDetails.duration` is missing")?;
        let duration = iso8601::duration(&duration)
            .map_err(|error| Error::msg(error))
            .context("failed to parse the video duration")?;
        let duration = Duration::from(duration);

        // Don't announce livestreams.
        match self.is_livestream(&video, duration) {
            Ok(true) => return Ok(false),
            Ok(false) => (),
            Err(error) => {
                error!(
                    ?error,
                    video.id,
                    "failed to determine if a video is a livestream, assuming that it is not"
                );
            }
        }

        // Don't announce shorts.
        match self.is_short(&video, duration) {
            Ok(true) => return Ok(false),
            Ok(false) => (),
            Err(error) => {
                error!(
                    ?error,
                    video.id, "failed to determine if a video is a short, assuming that it is not"
                );
            }
        }

        Ok(true)
    }

    fn is_livestream(
        &self,
        video: &google_youtube3::api::Video,
        duration: Duration,
    ) -> Result<bool, Error> {
        // Livestreams and premieres both have `liveStreamingDetails` set but livestreams don't have a non-zero duration
        // until it becomes a VOD. There doesn't seem to be a way to differentiate between a VOD and a premiere.
        Ok(video.live_streaming_details.is_some() && duration.is_zero())
    }

    fn is_short(
        &self,
        video: &google_youtube3::api::Video,
        duration: Duration,
    ) -> Result<bool, Error> {
        // The API doesn't tell you if something is a short so we need to implement the logic ourselves.
        // A short is:
        //  * up to 3 minutes long
        //  * with a square or vertical aspect ratio.
        // The API also doesn't tell you the aspect ratio or the video size so divine it from the embed player size.
        let player = video.player.as_ref().context("`player` is missing")?;
        let embed_width = player.embed_width.context("`player.embed_width` is missing")?;
        let embed_height = player.embed_height.context("`player.embed_height` is missing")?;

        Ok(duration <= Duration::from_secs(3 * 60) && embed_width <= embed_height)
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

    fn tags(&self, available_tags: &[ForumTag]) -> Vec<Id<TagMarker>> {
        available_tags
            .iter()
            .filter_map(|tag| (self.channel_title == tag.name).then_some(tag.id))
            .collect::<Vec<_>>()
    }

    pub async fn announce(
        &self,
        channel_id: Id<ChannelMarker>,
        channel_type: ChannelType,
        available_tags: Option<&[ForumTag]>,
        discord: &DiscordClient,
    ) -> Result<Channel, Error> {
        if channel_type == ChannelType::GuildForum {
            let thread = discord
                .create_forum_thread(
                    channel_id,
                    &crate::shorten::shorten(&self.title, CHANNEL_NAME_LENGTH_MAX),
                )
                .applied_tags(&available_tags.map(|tags| self.tags(tags)).unwrap_or_default())
                .message()
                .content(&self.message_content())
                .await
                .context("failed to create the video thread")?
                .model()
                .await
                .context("failed to deserialize the thread")?
                .channel;

            Ok(thread)
        } else {
            let message = discord
                .create_message(channel_id)
                .content(&self.message_content())
                .await
                .context("failed to send video announcement")?
                .model()
                .await
                .context("failed to deserialize the message")?;

            let thread = discord
                .create_thread_from_message(
                    channel_id,
                    message.id,
                    &crate::shorten::shorten(&self.title, CHANNEL_NAME_LENGTH_MAX),
                )
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
        message: &Message,
        available_tags: Option<&[ForumTag]>,
    ) -> Result<(), Error> {
        discord
            .update_thread(message.channel_id)
            .applied_tags(available_tags.map(|tags| self.tags(tags)).as_deref())
            .name(&crate::shorten::shorten(&self.title, CHANNEL_NAME_LENGTH_MAX))
            .await
            .context("failed to update thread name")?;

        discord
            .update_message(message.channel_id, message.id)
            .content(Some(&self.message_content()))
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
            published_at: snippet.published_at.context("`published_at` is missing")?,
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
            published_at: snippet.published_at.context("`published_at` is missing")?,
        })
    }
}
