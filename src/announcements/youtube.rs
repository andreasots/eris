use std::collections::HashSet;
use std::fmt::Write;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Context, Error};
use chrono::{DateTime, Utc};
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

const MAX_RESULTS: u32 = 10;
const MAX_STATE_ENTRIES: u32 = MAX_RESULTS * 2;
const MAX_THREADS_TO_CHECK: usize = MAX_STATE_ENTRIES as usize * 2;

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
    playlists: Vec<(String, String)>,
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
            playlists.push((
                channel.id.context("channel ID is missing")?,
                channel
                    .content_details
                    .context("requested `contentDetails` but `content_details` is missing")?
                    .related_playlists
                    .context("related playlists is empty")?
                    .uploads
                    .context("no uploads playlist")?,
            ));
        }

        Ok(Self { db, cache, channel_id, playlists, discord, youtube })
    }

    fn state_key(&self, channel_id: &str) -> String {
        format!("eris.announcements.youtube.{channel_id}.announced_videos")
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

        let mut video_ids = vec![];

        for (channel_id, playlist_id) in &self.playlists {
            // Hopefully all the new videos are on the first page of results...
            let res = self
                .youtube
                .playlist_items()
                .list(&vec!["contentDetails".into()])
                .playlist_id(playlist_id)
                .max_results(MAX_RESULTS)
                .doit()
                .await;
            match res {
                Ok((_, playlist)) => {
                    if let Some(items) = playlist.items {
                        let announced =
                            state::get::<HashSet<String>>(&self.state_key(channel_id), &self.db)
                                .await
                                .with_context(|| {
                                    format!(
                                        "failed to get announced videos for channel {channel_id}"
                                    )
                                })?
                                .unwrap_or_default();

                        video_ids.extend(
                            items
                                .into_iter()
                                .filter_map(|item| item.content_details.and_then(|cd| cd.video_id))
                                .filter(|video_id| !announced.contains(video_id)),
                        );
                    }
                }
                Err(error) => error!(?error, "playlist request failed"),
            }
        }

        let mut videos =
            Video::fetch(&self.youtube, &video_ids).await.context("failed to fetch videos")?;

        videos.sort_by(|a, b| a.published_at.cmp(&b.published_at));

        for video in videos {
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

            if !is_announced && video.should_announce() {
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

            state::insert_fifo_cache(
                self.state_key(&video.channel_id),
                &video.id,
                MAX_STATE_ENTRIES,
                &self.db,
            )
            .await
            .context("failed to append video ID to state")?;
        }

        Ok(())
    }
}

pub struct Video {
    // snippet
    channel_title: String,
    channel_id: String,
    id: String,
    title: String,
    description: String,
    published_at: DateTime<Utc>,

    // contentDetails
    duration: Option<Duration>,

    // liveStreamingDetails
    has_live_streaming_details: bool,
    scheduled_start_time: Option<DateTime<Utc>>,

    // player
    player_size: Option<(i64, i64)>,
}

impl Video {
    pub async fn fetch(
        youtube: &YouTube<HttpsConnector<HttpConnector>>,
        ids: &[impl AsRef<str>],
    ) -> Result<Vec<Self>, Error> {
        if ids.is_empty() {
            return Ok(vec![]);
        }

        let mut req = youtube
            .videos()
            .list(&vec![
                "snippet".into(),
                "contentDetails".into(),
                "liveStreamingDetails".into(),
                "player".into(),
            ])
            .max_height(720); // need to specify something to get the player size

        for id in ids {
            req = req.add_id(id.as_ref());
        }

        let (_, list) = req.doit().await.context("failed to fetch video details")?;

        list.items.unwrap_or_default().into_iter().map(Self::try_from).collect()
    }

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
        threads.truncate(MAX_THREADS_TO_CHECK);

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

    fn should_announce(&self) -> bool {
        // Don't announce livestreams.
        if self.is_livestream() {
            info!(video.id = self.id, video.title = self.title, "video is a livestream");
            return false;
        }

        // Don't announce shorts.
        if self.is_short() {
            info!(video.id = self.id, video.title = self.title, "video is a short");
            return false;
        }

        true
    }

    fn is_livestream(&self) -> bool {
        // Livestreams and premieres both have `liveStreamingDetails` set but livestreams have the duration set to zero
        // until it becomes a VOD. There doesn't seem to be a way to differentiate between a VOD and a premiere.
        self.has_live_streaming_details
            && self.duration.map(|duration| duration.is_zero()).unwrap_or(false)
    }

    fn is_short(&self) -> bool {
        // The API doesn't tell you if something is a short so we need to implement the logic ourselves.
        // A short is:
        //  * up to 3 minutes long
        //  * with a square or vertical aspect ratio.
        // The API also doesn't tell you the aspect ratio or the video size so divine it from the embed player size.

        self.duration.map(|duration| duration <= Duration::from_secs(3 * 60)).unwrap_or(false)
            && self.player_size.map(|(width, height)| width <= height).unwrap_or(false)
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
        if let Some(start_time) = self.scheduled_start_time {
            write!(
                message,
                "**Note**: this video premieres <t:{}:R>.\n\n",
                start_time.timestamp(),
            )
            .unwrap();
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

        let duration = video
            .content_details
            .as_ref()
            .context("`contentDetails` is missing")?
            .duration
            .as_deref()
            .map(|s| iso8601::duration(s).map_err(Error::msg))
            .transpose()
            .context("failed to parse the video duration")?
            .map(Duration::from);

        Ok(Self {
            channel_title: snippet.channel_title.context("`channel_title` is missing")?,
            channel_id: snippet.channel_id.context("`channel_id` is missing")?,
            id: video.id.context("`id` is missing")?,
            title: snippet.title.context("`title` is missing")?,
            description: snippet.description.context("`description` is missing")?,
            published_at: snippet.published_at.context("`published_at` is missing")?,

            duration,

            has_live_streaming_details: video.live_streaming_details.is_some(),
            scheduled_start_time: video
                .live_streaming_details
                .and_then(|lsd| lsd.scheduled_start_time),

            player_size: video
                .player
                .and_then(|player| Some((player.embed_width?, player.embed_height?))),
        })
    }
}
