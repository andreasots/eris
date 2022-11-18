use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

use anyhow::{Context, Error};
use twilight_cache_inmemory::InMemoryCache;
use twilight_http::Client as DiscordClient;
use twilight_model::channel::message::embed::EmbedField;
use twilight_model::channel::Message;
use twilight_util::builder::embed::EmbedBuilder;

use crate::command_parser::{Args, CommandHandler, Commands};
use crate::config::Config;

pub struct Help;

impl Help {
    pub fn new() -> Self {
        Self
    }

    async fn listing(
        &self,
        config: &Config,
        discord: &DiscordClient,
        commands: Commands<'_>,
        message: &Message,
    ) -> Result<(), Error> {
        let mut embed = EmbedBuilder::new().description(concat!(
            "To get help with an individual command, pass its name as an argument to this ",
            "command. Simple text response commands (like `!advice`) are not listed here, ",
            "for those see [LRRbot's website](https://lrrbot.com/help#help-section-text).",
        ));

        let mut fields = vec![];
        for cmd in commands.help() {
            fields.push(EmbedField {
                inline: true,
                name: format!("{}{}", config.command_prefix, cmd.name),
                value: cmd.summary.into(),
            });
        }
        fields.sort_by(|a, b| a.name.cmp(&b.name));
        for field in fields {
            embed = embed.field(field);
        }

        discord
            .create_message(message.channel_id)
            .reply(message.id)
            .embeds(&[embed.build()])
            .context("command listing invalid")?
            .await
            .context("failed to reply to command")?;
        Ok(())
    }

    async fn single_command(
        &self,
        config: &Config,
        discord: &DiscordClient,
        commands: Commands<'_>,
        message: &Message,
        command: &str,
    ) -> Result<(), Error> {
        let command = {
            let mut cleaned = String::with_capacity(command.len());
            for (i, part) in command.split_whitespace().enumerate() {
                if i != 0 {
                    cleaned.push_str(" ");
                }
                cleaned.push_str(part);
            }
            cleaned
        };

        match commands.help().find(|help| help.name == command) {
            Some(help) => {
                let examples = help
                    .examples
                    .iter()
                    .map(|example| format!("`{}{example}`\n", config.command_prefix))
                    .collect::<String>();
                let mut embed = EmbedBuilder::new()
                    .title(format!("`{}{}`", config.command_prefix, help.usage))
                    .description(help.description);
                if examples.len() > 0 {
                    embed = embed.field(EmbedField {
                        inline: false,
                        name: "Examples".into(),
                        value: examples,
                    });
                }
                discord
                    .create_message(message.channel_id)
                    .reply(message.id)
                    .embeds(&[embed.build()])
                    .context("detailed command help invalid")?
                    .await
                    .context("failed to reply to command")?;
            }
            None => {
                discord
                    .create_message(message.channel_id)
                    .reply(message.id)
                    .content(&format!("No such command: {}", crate::markdown::escape(&command)))
                    .context("error message invalid")?
                    .await
                    .context("failed to reply to command")?;
            }
        }

        Ok(())
    }
}

impl CommandHandler for Help {
    fn pattern(&self) -> &str {
        r"help(?: ((?:\w+)(?: \w+)*))?"
    }

    fn help(&self) -> Option<crate::command_parser::Help> {
        Some(crate::command_parser::Help {
            name: "help".into(),
            usage: "help [COMMAND]".into(),
            summary: "Get information on available commands".into(),
            description: concat!(
                "Get information on available commands.\n\n",
                "To get more detailed information on a specific command, pass its name as an ",
                "argument."
            )
            .into(),
            examples: Cow::Borrowed(&[Cow::Borrowed("help"), Cow::Borrowed("help voice")]),
        })
    }

    fn handle<'a>(
        &'a self,
        _: &'a InMemoryCache,
        config: &'a Config,
        discord: &'a DiscordClient,
        commands: Commands<'a>,
        message: &'a Message,
        args: &'a Args,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        match args.get(0) {
            Some(command) => {
                Box::pin(self.single_command(config, discord, commands, message, command))
            }
            None => Box::pin(self.listing(config, discord, commands, message)),
        }
    }
}
