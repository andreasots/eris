use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Error};
use google_youtube3::api::PlaylistItem;
use google_youtube3::hyper::client::HttpConnector;
use google_youtube3::hyper_rustls::HttpsConnector;
use google_youtube3::YouTube;
use sea_orm::DatabaseConnection;
use time::format_description::well_known::{Iso8601, Rfc3339};
use time::OffsetDateTime;
use tracing::{error, info};
use twilight_cache_inmemory::InMemoryCache;
use twilight_http::Client as DiscordClient;
use twilight_model::channel::ChannelType;
use twilight_model::id::marker::ChannelMarker;
use twilight_model::id::Id;

use crate::config::Config;
use crate::models::state;

pub async fn post_videos(
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

    let poster = match VideoPoster::new(db, cache, channel_id, &config, discord, youtube).await {
        Ok(poster) => poster,
        Err(error) => {
            error!(?error, "failed to construct the video poster");
            return;
        }
    };
    let mut interval = tokio::time::interval(Duration::from_secs(300));

    loop {
        interval.tick().await;

        if let Err(error) = poster.run().await {
            error!(?error, "failed to post videos");
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

        Ok(Self { db, cache, channel_id, playlists: channels, discord, youtube })
    }

    async fn run(&self) -> Result<(), Error> {
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

        let Some(channel) = self.cache.channel(self.channel_id) else {
            return Err(Error::msg("video announcements channel not in cache"));
        };

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

            if last_published_at > video.published_at {
                continue;
            }

            let description = video.description.split("Support LRR:").next().unwrap_or("").trim();

            let mut message = String::new();
            for line in description.lines() {
                message.push_str("> ");
                message.push_str(&crate::markdown::suppress_embeds(line));
                message.push_str("\n");
            }
            message.push_str("https://youtu.be/");
            message.push_str(&video.id);

            if channel.kind == ChannelType::GuildForum {
                let tags = channel
                    .available_tags
                    .as_deref()
                    .into_iter()
                    .flatten()
                    .filter_map(|tag| {
                        (video.title.contains(&tag.name) || video.channel_title == tag.name)
                            .then_some(tag.id)
                    })
                    .collect::<Vec<_>>();

                self.discord
                    .create_forum_thread(channel.id, &video.title)
                    .applied_tags(&tags)
                    .message()
                    .content(&message)
                    .context("video announcement invalid")?
                    .await
                    .context("failed to create the video thread")?;
            } else {
                self.discord
                    .create_message(self.channel_id)
                    .content(&format!("**{}**\n{}", crate::markdown::escape(&video.title), message))
                    .context("video announcement invalid")?
                    .await
                    .context("failed to send video announcement")?;
            }

            let published_at = video
                .published_at
                .format(&Rfc3339)
                .context("failed to format the video published timestamp")?;
            state::set(state_key, &published_at, &self.db)
                .await
                .context("failed to set the last video published timestamp")?;
        }

        Ok(())
    }
}

struct Video {
    channel_title: String,
    channel_id: String,
    id: String,
    title: String,
    description: String,
    published_at: OffsetDateTime,
}

impl TryFrom<PlaylistItem> for Video {
    type Error = Error;

    fn try_from(video: PlaylistItem) -> Result<Self, Self::Error> {
        let snippet = video.snippet.context("asked for `snippet` yet it is missing")?;
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
            published_at: OffsetDateTime::parse(
                &snippet.published_at.context("`published_at` is missing")?,
                &Iso8601::DEFAULT,
            )
            .context("failed to parse `published_at`")?,
        })
    }
}
