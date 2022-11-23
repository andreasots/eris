use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

use anyhow::{Context, Error};
use google_youtube3::hyper::client::HttpConnector;
use google_youtube3::hyper_rustls::HttpsConnector;
use google_youtube3::YouTube;
use twilight_cache_inmemory::InMemoryCache;
use twilight_http::Client;
use twilight_mention::Mention;
use twilight_model::channel::message::MessageFlags;
use twilight_model::channel::Message;
use twilight_model::id::marker::ChannelMarker;
use twilight_model::id::Id;

use crate::announcements::youtube::Video;
use crate::command_parser::{Access, Args, CommandHandler, Commands, Help};
use crate::config::Config;

pub struct New {
    channel_id: Id<ChannelMarker>,
    youtube: YouTube<HttpsConnector<HttpConnector>>,
}

impl New {
    pub fn new(config: &Config, youtube: YouTube<HttpsConnector<HttpConnector>>) -> Option<Self> {
        Some(Self { channel_id: config.lrr_videos_channel?, youtube })
    }
}

impl CommandHandler for New {
    fn pattern(&self) -> &str {
        r"video new (\S+)"
    }

    fn help(&self) -> Option<Help> {
        Some(Help {
            name: "video new".into(),
            usage: "video new <VIDEO ID>".into(),
            summary: "Create a new video thread".into(),
            description: "Create a new video thread.".into(),
            examples: Cow::Borrowed(&[Cow::Borrowed("video new dQw4w9WgXcQ")]),
        })
    }

    fn access(&self) -> Access {
        Access::ModOnly
    }

    fn handle<'a>(
        &'a self,
        cache: &'a InMemoryCache,
        _: &'a Config,
        discord: &'a Client,
        _: Commands<'a>,
        message: &'a Message,
        args: &'a Args,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            let channel = cache.channel(self.channel_id).context("channel not in cache")?;
            let video_id = args.get(0).context("video ID missing")?;
            let (_, videos) = self
                .youtube
                .videos()
                .list(&vec!["snippet".into()])
                .add_id(video_id)
                .doit()
                .await
                .context("failed to get the video")?;

            let videos = videos.items.unwrap_or_else(Vec::new);
            if !videos.is_empty() {
                for video in videos {
                    let thread = Video::try_from(video)
                        .context("failed to deserialize the video")?
                        .announce(&channel, discord)
                        .await
                        .context("failed to create the video thread")?;
                    discord
                        .create_message(message.channel_id)
                        .reply(message.id)
                        .flags(MessageFlags::SUPPRESS_EMBEDS)
                        .content(&format!("Created {}.", thread.mention()))
                        .context("message invalid")?
                        .await
                        .context("failed to reply to command")?;
                }
            } else {
                discord
                    .create_message(message.channel_id)
                    .reply(message.id)
                    .flags(MessageFlags::SUPPRESS_EMBEDS)
                    .content("No such video.")
                    .context("message invalid")?
                    .await
                    .context("failed to reply to command")?;
            }

            Ok(())
        })
    }
}

pub struct Refresh {
    channel_id: Id<ChannelMarker>,
    youtube: YouTube<HttpsConnector<HttpConnector>>,
}

impl Refresh {
    pub fn new(config: &Config, youtube: YouTube<HttpsConnector<HttpConnector>>) -> Option<Self> {
        Some(Self { channel_id: config.lrr_videos_channel?, youtube })
    }
}

impl CommandHandler for Refresh {
    fn pattern(&self) -> &str {
        r"video refresh(?: (\S+))?"
    }

    fn help(&self) -> Option<Help> {
        Some(Help {
            name: "video refresh".into(),
            usage: "video refresh [VIDEO ID]".into(),
            summary: "Update the video thread to have up to date video information".into(),
            description: Cow::Owned(format!(
                concat!(
                "Update the video thread to have up to date video information. Optionally pass a ",
                "YouTube video ID to replace the current video\n\n",
                "Must be used in a thread in {}."
            ),
                self.channel_id.mention()
            )),
            examples: Cow::Borrowed(&[Cow::Borrowed("video refresh dQw4w9WgXcQ")]),
        })
    }

    fn access(&self) -> Access {
        Access::ModOnly
    }

    fn handle<'a>(
        &'a self,
        cache: &'a InMemoryCache,
        _: &'a Config,
        discord: &'a Client,
        _: Commands<'a>,
        message: &'a Message,
        args: &'a Args,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            let bot = cache.current_user().ok_or_else(|| Error::msg("bot not in cache"))?;

            let channel =
                cache.channel(self.channel_id).ok_or_else(|| Error::msg("channel not in cache"))?;
            let thread = cache
                .channel(message.channel_id)
                .ok_or_else(|| Error::msg("thread not in cache"))?;
            if thread.parent_id != Some(self.channel_id) {
                discord
                    .create_message(message.channel_id)
                    .reply(message.id)
                    .flags(MessageFlags::SUPPRESS_EMBEDS)
                    .content(&format!(
                        "Command must be used in a thread in {}.",
                        self.channel_id.mention()
                    ))
                    .context("error message invalid")?
                    .await
                    .context("failed to report an error")?;
            }

            let mut messages = discord
                .channel_messages(thread.id)
                .after(Id::new(1))
                .limit(1)
                .context("limit invalid")?
                .await
                .context("failed to get the messages")?
                .models()
                .await
                .context("failed to deserialize the messages")?;
            let original_message =
                messages.pop().ok_or_else(|| Error::msg("thread empty or no permissions"))?;
            let original_message = if let Some(message) = original_message.referenced_message {
                (*message).clone()
            } else {
                original_message
            };

            if original_message.author.id != bot.id {
                discord
                    .create_message(message.channel_id)
                    .reply(message.id)
                    .flags(MessageFlags::SUPPRESS_EMBEDS)
                    .content("Can't edit the first post in thread because it was created by someone else.")
                    .context("error message invalid")?
                    .await
                    .context("failed to report an error")?;

                return Ok(());
            };

            let video_id = if let Some(video_id) = args.get(0) {
                video_id
            } else if let Some(video_id) = Video::video_id_from_message(&original_message.content) {
                video_id
            } else {
                discord
                    .create_message(message.channel_id)
                    .reply(message.id)
                    .flags(MessageFlags::SUPPRESS_EMBEDS)
                    .content("Could not find a YouTube video ID in the first message of the thread")
                    .context("error message invalid")?
                    .await
                    .context("failed to report an error")?;
                return Ok(());
            };

            let (_, videos) = self
                .youtube
                .videos()
                .list(&vec!["snippet".into()])
                .add_id(video_id)
                .doit()
                .await
                .context("failed to get the video")?;

            let videos = videos.items.unwrap_or_else(Vec::new);
            if !videos.is_empty() {
                for video in videos {
                    Video::try_from(video)
                        .context("failed to deserialize the video")?
                        .edit(discord, &channel, &original_message, &thread)
                        .await
                        .context("failed to update the video thread")?;

                    discord
                        .create_message(message.channel_id)
                        .reply(message.id)
                        .flags(MessageFlags::SUPPRESS_EMBEDS)
                        .content("Message updated.")
                        .context("message invalid")?
                        .await
                        .context("failed to reply to command")?;
                }
            } else {
                discord
                    .create_message(message.channel_id)
                    .reply(message.id)
                    .flags(MessageFlags::SUPPRESS_EMBEDS)
                    .content("No such video.")
                    .context("message invalid")?
                    .await
                    .context("failed to reply to command")?;
            }

            Ok(())
        })
    }
}
