use std::borrow::Cow;
use std::fmt::Write;
use std::future::Future;
use std::pin::Pin;

use anyhow::{Context, Error};
use twilight_http::Client as DiscordClient;
use twilight_model::channel::message::embed::EmbedField;
use twilight_model::channel::Message;
use twilight_util::builder::embed::EmbedBuilder;

use crate::cache::Cache;
use crate::command_parser::{Args, CommandHandler, Commands};
use crate::config::Config;

pub struct Help;

impl Help {
    pub fn new() -> Self {
        Self
    }

    async fn listing(
        &self,
        cache: &Cache,
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

        let guild_id = message.guild_id.unwrap_or(config.guild);

        let mut fields = vec![];
        for cmd in commands.iter() {
            if cmd.access().user_has_access(message.author.id, guild_id, cache) {
                if let Some(help) = cmd.help() {
                    fields.push(EmbedField {
                        inline: true,
                        name: format!("{}{}", config.command_prefix, help.name),
                        value: help.summary.into(),
                    });
                }
            }
        }
        fields.sort_by(|a, b| a.name.cmp(&b.name));
        for field in fields {
            embed = embed.field(field);
        }

        discord
            .create_message(message.channel_id)
            .reply(message.id)
            .embeds(&[embed.build()])
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
                    cleaned.push(' ');
                }
                cleaned.push_str(part);
            }
            cleaned
        };

        match commands.iter().filter_map(CommandHandler::help).find(|help| help.name == command) {
            Some(help) => {
                let examples = help.examples.iter().fold(String::new(), |mut examples, example| {
                    writeln!(examples, "`{}{example}`", config.command_prefix).unwrap();
                    examples
                });
                let mut embed = EmbedBuilder::new()
                    .title(format!("`{}{}`", config.command_prefix, help.usage))
                    .description(help.description);
                if !examples.is_empty() {
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
                    .await
                    .context("failed to reply to command")?;
            }
            None => {
                discord
                    .create_message(message.channel_id)
                    .reply(message.id)
                    .content(&format!("No such command: {}", crate::markdown::escape(&command)))
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
        cache: &'a Cache,
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
            None => Box::pin(self.listing(cache, config, discord, commands, message)),
        }
    }
}
