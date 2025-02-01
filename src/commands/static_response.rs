use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use anyhow::{Context as _, Error};
use rand::seq::IndexedRandom;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, ModelTrait, QueryFilter};
use tracing::info;
use twilight_cache_inmemory::model::CachedMember;
use twilight_http::Client as DiscordClient;
use twilight_model::channel::message::MessageFlags;
use twilight_model::channel::Message;

use crate::cache::Cache;
use crate::command_parser::{Args, CommandHandler, Commands};
use crate::config::Config;
use crate::models::{command, command_alias, command_response};

pub struct Static {
    db: DatabaseConnection,
}

impl Static {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    fn extract_command(cmd: &str) -> String {
        let mut command = String::new();
        for (i, part) in cmd.split_whitespace().enumerate() {
            if i != 0 {
                command.push(' ');
            }
            command.push_str(part);
        }
        command
    }
}

impl CommandHandler for Static {
    fn pattern(&self) -> &str {
        r"(.*)"
    }

    fn help(&self) -> Option<crate::command_parser::Help> {
        None
    }

    fn handle<'a>(
        &'a self,
        cache: &'a Cache,
        config: &'a Config,
        discord: &'a DiscordClient,
        _: Commands<'a>,
        message: &'a Message,
        args: &'a Args,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            let Some(command) = args.get(0) else { return Ok(()) };
            let command = Self::extract_command(command);

            let Some(alias) = command_alias::Entity::find()
                .filter(command_alias::Column::Alias.eq(&command))
                .one(&self.db)
                .await
                .context("failed to search for command")?
            else {
                return Ok(());
            };

            let Some(command) = alias
                .find_related(command::Entity)
                .one(&self.db)
                .await
                .context("failed to load the command")?
            else {
                return Ok(());
            };

            let guild_id = message.guild_id.unwrap_or(config.guild);
            if command.access.user_has_access(message.author.id, guild_id, cache) {
                let responses = command
                    .find_related(command_response::Entity)
                    .all(&self.db)
                    .await
                    .context("failed to load the responses")?;

                let Some(response) = responses.choose(&mut rand::rng()) else {
                    return Ok(());
                };

                let vars = HashMap::from([(
                    "user".to_string(),
                    cache.with(|cache| {
                        message
                            .guild_id
                            .and_then(|guild_id| cache.member(guild_id, message.author.id))
                            .as_deref()
                            .and_then(CachedMember::nick)
                            .unwrap_or(&message.author.name)
                            .to_string()
                    }),
                )]);

                let response = strfmt::strfmt(&response.response, &vars)
                    .context("failed to format the reply")?;

                discord
                    .create_message(message.channel_id)
                    .reply(message.id)
                    .flags(MessageFlags::SUPPRESS_EMBEDS)
                    .content(&response)
                    .await
                    .context("failed to reply to command")?;
            } else {
                info!(?command.access, "Refusing to reply because user lacks access");
                crate::command_parser::refuse_access(
                    discord,
                    message.channel_id,
                    message.id,
                    command.access,
                )
                .await?;
            }

            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn extract_command() {
        assert_eq!(super::Static::extract_command(" \t  \t some \t command \t "), "some command");
        assert_eq!(super::Static::extract_command("command"), "command");
        assert_eq!(super::Static::extract_command("some command"), "some command");
    }
}
