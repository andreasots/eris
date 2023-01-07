use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

use anyhow::{Context, Error};
use twilight_cache_inmemory::InMemoryCache;
use twilight_http::Client as DiscordClient;
use twilight_mention::Mention;
use twilight_model::channel::message::MessageFlags;
use twilight_model::channel::{Channel, ChannelType, Message};

use crate::command_parser::{Args, CommandHandler, Commands, Help};
use crate::config::Config;

pub struct Voice;

impl Voice {
    pub fn new() -> Self {
        Self
    }

    pub async fn exec(
        &self,
        config: &Config,
        discord: &DiscordClient,
        name: &str,
    ) -> Result<Channel, Error> {
        let name = format!("{} {name}", config.temp_channel_prefix);
        let channel = discord
            .create_guild_channel(config.guild, &name)
            .context("invalid channel name")?
            .kind(ChannelType::GuildVoice)
            .parent_id(config.voice_category)
            .await
            .context("failed to create the temporary voice channel")?
            .model()
            .await
            .context("failed to parse the response")?;
        Ok(channel)
    }
}

impl CommandHandler for Voice {
    fn pattern(&self) -> &str {
        "voice (.+)"
    }

    fn help(&self) -> Option<Help> {
        Some(Help {
            name: "voice".into(),
            usage: "voice <CHANNEL NAME>".into(),
            summary: "Create a temporary voice channel".into(),
            description: concat!(
                "Create a temporary voice channel.\n\n",
                "Unused temporary voice channels will be automatically deleted if they're older ",
                "than 15 minutes.",
            )
            .into(),
            examples: Cow::Borrowed(&[Cow::Borrowed("voice PUBG #15")]),
        })
    }

    fn handle<'a>(
        &'a self,
        _: &'a InMemoryCache,
        config: &'a Config,
        discord: &'a DiscordClient,
        _: Commands<'a>,
        message: &'a Message,
        args: &'a Args,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            let content = match self.exec(config, discord, args.get(0).unwrap()).await {
                Ok(channel) => format!("Created a temporary voice channel {}", channel.mention()),
                Err(error) => format!("Failed to create a temporary voice channel: {}", error),
            };

            discord
                .create_message(message.channel_id)
                .reply(message.id)
                .flags(MessageFlags::SUPPRESS_EMBEDS)
                .content(&content)
                .context("invalid response message")?
                .await
                .context("failed to respond to command")?;

            Ok(())
        })
    }
}
