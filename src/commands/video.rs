use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

use anyhow::{Context, Error};
use google_youtube3::YouTube;
use google_youtube3::hyper_rustls::HttpsConnector;
use google_youtube3::hyper_util::client::legacy::connect::HttpConnector;
use twilight_http::Client;
use twilight_mention::Mention;
use twilight_model::channel::Message;
use twilight_model::channel::message::MessageFlags;
use twilight_model::id::Id;
use twilight_model::id::marker::ChannelMarker;

use crate::announcements::youtube::Video;
use crate::cache::Cache;
use crate::command_parser::{Access, Args, CommandHandler, Commands, Help};
use crate::config::Config;

pub struct New {
    channel_id: Id<ChannelMarker>,
    youtube: YouTube<HttpsConnector<HttpConnector>>,
}

impl New {
    #[allow(clippy::self_named_constructors)]
    pub fn new(config: &Config, youtube: YouTube<HttpsConnector<HttpConnector>>) -> Option<Self> {
        Some(Self { channel_id: config.lrr_videos_channel?, youtube })
    }
}

impl CommandHandler for New {
    fn pattern(&self) -> &'static str {
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
        cache: &'a Cache,
        _: &'a Config,
        discord: &'a Client,
        _: Commands<'a>,
        message: &'a Message,
        args: &'a Args,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            let (channel_type, available_tags) = cache
                .with(|cache| {
                    let channel = cache.channel(self.channel_id)?;
                    Some((channel.kind, channel.available_tags.clone()))
                })
                .context("channel not in cache")?;

            let videos = Video::fetch(&self.youtube, &[args.get(0).context("video ID missing")?])
                .await
                .context("failed to get the video")?;

            if videos.is_empty() {
                discord
                    .create_message(message.channel_id)
                    .reply(message.id)
                    .flags(MessageFlags::SUPPRESS_EMBEDS)
                    .content("No such video.")
                    .await
                    .context("failed to reply to command")?;
            } else {
                for video in videos {
                    let thread = video
                        .announce(self.channel_id, channel_type, available_tags.as_deref(), discord)
                        .await
                        .context("failed to create the video thread")?;
                    discord
                        .create_message(message.channel_id)
                        .reply(message.id)
                        .flags(MessageFlags::SUPPRESS_EMBEDS)
                        .content(&format!("Created {}.", thread.mention()))
                        .await
                        .context("failed to reply to command")?;
                }
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
    fn pattern(&self) -> &'static str {
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
        cache: &'a Cache,
        _: &'a Config,
        discord: &'a Client,
        _: Commands<'a>,
        message: &'a Message,
        args: &'a Args,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            let bot_id = cache
                .with(|cache| Some(cache.current_user()?.id))
                .ok_or_else(|| Error::msg("bot not in cache"))?;

            let available_tags = cache
                .with(|cache| Some(cache.channel(self.channel_id)?.available_tags.clone()))
                .ok_or_else(|| Error::msg("channel not in cache"))?;
            let thread_parent_id = cache
                .with(|cache| Some(cache.channel(message.channel_id)?.parent_id))
                .ok_or_else(|| Error::msg("thread not in cache"))?;
            if thread_parent_id != Some(self.channel_id) {
                discord
                    .create_message(message.channel_id)
                    .reply(message.id)
                    .flags(MessageFlags::SUPPRESS_EMBEDS)
                    .content(&format!(
                        "Command must be used in a thread in {}.",
                        self.channel_id.mention()
                    ))
                    .await
                    .context("failed to report an error")?;
            }

            let mut messages = discord
                .channel_messages(message.channel_id)
                .after(Id::new(1))
                .limit(1)
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

            if original_message.author.id != bot_id {
                discord
                    .create_message(message.channel_id)
                    .reply(message.id)
                    .flags(MessageFlags::SUPPRESS_EMBEDS)
                    .content("Can't edit the first post in thread because it was created by someone else.")
                    .await
                    .context("failed to report an error")?;

                return Ok(());
            }

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
                    .await
                    .context("failed to report an error")?;
                return Ok(());
            };

            let videos = Video::fetch(&self.youtube, &[video_id])
                .await
                .context("failed to get the video")?;

            if videos.is_empty() {
                discord
                    .create_message(message.channel_id)
                    .reply(message.id)
                    .flags(MessageFlags::SUPPRESS_EMBEDS)
                    .content("No such video.")
                    .await
                    .context("failed to reply to command")?;
            } else {
                for video in videos {
                    video
                        .edit(discord, &original_message, available_tags.as_deref())
                        .await
                        .context("failed to update the video thread")?;

                    discord
                        .create_message(message.channel_id)
                        .reply(message.id)
                        .flags(MessageFlags::SUPPRESS_EMBEDS)
                        .content("Message updated.")
                        .await
                        .context("failed to reply to command")?;
                }
            }

            Ok(())
        })
    }
}
