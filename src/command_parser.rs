use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context, Error};
use regex::{Captures, Regex, RegexSet};
use sea_orm::{DeriveActiveEnum, EnumIter};
use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;
use tracing::{error, info, Instrument};
use twilight_gateway::Event;
use twilight_http::Client as DiscordClient;
use twilight_model::channel::message::MessageFlags;
use twilight_model::channel::Message;
use twilight_model::gateway::payload::incoming::MessageCreate;
use twilight_model::guild::Permissions;
use twilight_model::id::marker::{ChannelMarker, GuildMarker, MessageMarker, UserMarker};
use twilight_model::id::Id;

use crate::cache::Cache;
use crate::config::Config;

pub trait CommandHandler: Send + Sync {
    fn pattern(&self) -> &str;
    fn help(&self) -> Option<Help>;
    fn handle<'a>(
        &'a self,
        cache: &'a Cache,
        config: &'a Config,
        discord: &'a DiscordClient,
        commands: Commands<'a>,
        message: &'a Message,
        args: &'a Args,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>>;

    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
    fn access(&self) -> Access {
        Access::All
    }
}

#[derive(Debug, Clone, Copy, DeriveActiveEnum, EnumIter, Eq, PartialEq)]
#[sea_orm(rs_type = "i32", db_type = "Integer")]
pub enum Access {
    /// Allow anyone to use the command
    #[sea_orm(num_value = 0)]
    All,
    /// Allow only the subscribers to use the command
    ///
    /// A 'subscriber' is someone with a coloured role.
    #[sea_orm(num_value = 1)]
    SubOnly,
    /// Allow only the moderators to use the command
    ///
    /// A 'moderator' is someone with the `ADMINISTRATOR` permission in the guild.
    #[sea_orm(num_value = 2)]
    ModOnly,
    /// Allow only the bot owners to use the command
    #[sea_orm(num_value = -1)]
    OwnerOnly,
}

impl Access {
    pub fn user_has_access(
        self,
        user_id: Id<UserMarker>,
        guild_id: Id<GuildMarker>,
        cache: &Cache,
    ) -> bool {
        match self {
            Access::All => true,
            Access::SubOnly => cache.with(|cache| {
                cache
                    .member(guild_id, user_id)
                    .as_deref()
                    .into_iter()
                    .flat_map(|member| member.roles().iter())
                    .filter_map(|&role_id| cache.role(role_id))
                    .any(|role| role.color != 0)
            }),
            Access::ModOnly => cache.with(|cache| {
                cache
                    .permissions()
                    .root(user_id, guild_id)
                    .map(|permissions| permissions.contains(Permissions::ADMINISTRATOR))
                    .unwrap_or(false)
            }),
            Access::OwnerOnly => {
                #[allow(clippy::unreadable_literal)]
                const OWNERS: [Id<UserMarker>; 3] = [
                    Id::new(101919755132227584), // defrost
                    Id::new(153674140019064832), // mrphlip
                    Id::new(144128240389324800), // qrpth
                ];
                // TODO: transfer LRRbot to a team and check against team members
                OWNERS.into_iter().any(|id| id == user_id)
            }
        }
    }

    fn refuse_reason(self) -> &'static str {
        match self {
            Access::All => "That is a unrestricted command.",
            Access::SubOnly => "That is a sub-only command.",
            Access::ModOnly => "That is a mod-only command.",
            Access::OwnerOnly => "That is a bot owner only command.",
        }
    }
}

pub struct Args {
    matches: Vec<Option<String>>,
}

impl Args {
    fn empty() -> Self {
        Self { matches: vec![] }
    }

    fn from_captures(captures: &Captures) -> Self {
        Self { matches: captures.iter().skip(1).map(|c| c.map(|c| c.as_str().into())).collect() }
    }

    pub fn get(&self, index: usize) -> Option<&str> {
        self.matches.get(index).and_then(|arg| arg.as_deref())
    }
}

pub struct Commands<'a> {
    handlers: &'a [(Regex, Box<dyn CommandHandler>)],
}

impl<'a> Commands<'a> {
    pub fn iter<'b>(&'b self) -> impl Iterator<Item = &'a dyn CommandHandler> + 'b {
        self.handlers.iter().map(|(_, handler)| &**handler)
    }
}

#[derive(Clone)]
pub struct Help {
    pub name: Cow<'static, str>,
    pub usage: Cow<'static, str>,
    pub summary: Cow<'static, str>,
    pub description: Cow<'static, str>,
    pub examples: Cow<'static, [Cow<'static, str>]>,
}

#[derive(Clone)]
pub struct CommandParser {
    cache: Arc<Cache>,
    config: Arc<Config>,
    discord: Arc<DiscordClient>,
    matcher: Arc<RegexSet>,
    handlers: Arc<Vec<(Regex, Box<dyn CommandHandler>)>>,
}

impl CommandParser {
    pub fn builder() -> Builder {
        Builder { handlers: vec![] }
    }

    pub async fn on_event(&self, handler_tx: &Sender<JoinHandle<()>>, event: &Event) {
        let Event::MessageCreate(event) = event else { return };
        let MessageCreate(ref message) = **event;

        if message.author.bot {
            return;
        }

        if let Some(i) = self.matcher.matches(&message.content).into_iter().next() {
            let _ = handler_tx
                .send(tokio::spawn({
                    let cache = self.cache.clone();
                    let config = self.config.clone();
                    let discord = self.discord.clone();
                    let handlers = self.handlers.clone();
                    let message = message.clone();

                    async move {
                        let Some((pattern, handler)) = handlers.get(i) else { return };

                        let span = tracing::info_span!(
                            "handle_command",
                            handler.name = handler.name(),
                            message.content = message.content.as_str(),
                            message.id = message.id.get(),
                            message.author.id = message.author.id.get(),
                            message.author.name = message.author.name.as_str(),
                            message.author.discriminator = message.author.discriminator,
                        );

                        async {
                            info!("Command received");

                            let guild_id = message.guild_id.unwrap_or(config.guild);
                            let access = handler.access();
                            if !access.user_has_access(message.author.id, guild_id, &cache) {
                                info!(?access, guild.id = guild_id.get(), "refusing access");

                                if let Err(error) =
                                    refuse_access(&discord, message.channel_id, message.id, access)
                                        .await
                                {
                                    error!(?error, "failed to report access refusal to the user");
                                }

                                return;
                            }

                            let args = (pattern.captures_len() > 1)
                                .then_some(())
                                .and_then(|()| pattern.captures(&message.content))
                                .map_or_else(Args::empty, |captures| {
                                    Args::from_captures(&captures)
                                });

                            let cmds = Commands { handlers: &handlers };

                            if let Err(error) = handler
                                .handle(&cache, &config, &discord, cmds, &message, &args)
                                .await
                            {
                                error!(?error, "command handler failed");
                                if let Err(error) =
                                    error_feedback(&discord, message.channel_id, message.id, error)
                                        .await
                                {
                                    error!(?error, "failed to report the error to the user");
                                }
                            } else {
                                info!("Command processed successfully");
                            }
                        }
                        .instrument(span)
                        .await;
                    }
                }))
                .await;
        }
    }
}

async fn error_feedback(
    discord: &DiscordClient,
    channel_id: Id<ChannelMarker>,
    message_id: Id<MessageMarker>,
    error: Error,
) -> Result<(), Error> {
    discord
        .create_message(channel_id)
        .reply(message_id)
        .flags(MessageFlags::SUPPRESS_EMBEDS)
        .content(&format!("Command resulted in an unexpected error: {error}"))
        .await
        .context("failed to send the error message")?;
    Ok(())
}

pub async fn refuse_access(
    discord: &DiscordClient,
    channel_id: Id<ChannelMarker>,
    message_id: Id<MessageMarker>,
    access: Access,
) -> Result<(), Error> {
    discord
        .create_message(channel_id)
        .reply(message_id)
        .content(access.refuse_reason())
        .await
        .context("failed to reply to command")?;
    Ok(())
}

pub struct Builder {
    handlers: Vec<Box<dyn CommandHandler>>,
}

impl Builder {
    pub fn command(mut self, command: impl CommandHandler + 'static) -> Self {
        self.handlers.push(Box::new(command));
        self
    }

    pub fn command_opt(mut self, command: Option<impl CommandHandler + 'static>) -> Self {
        if let Some(command) = command {
            self.handlers.push(Box::new(command));
        }
        self
    }

    fn expand_pattern(prefix: &str, pattern: &str) -> Result<Regex, Error> {
        let prefix = regex::escape(prefix);
        let expanded = pattern.replace(' ', r"(?:\s+)");
        Regex::new(&format!(r"^\s*{prefix}\s*{expanded}\s*$")).map_err(|err| {
            Error::new(err).context(format!("failed to compile pattern {pattern:?}"))
        })
    }

    pub fn build(
        self,
        cache: Arc<Cache>,
        config: Arc<Config>,
        discord: Arc<DiscordClient>,
    ) -> Result<CommandParser, Error> {
        let handlers = self
            .handlers
            .into_iter()
            .map(|handler| {
                let pattern = Self::expand_pattern(&config.command_prefix, handler.pattern())?;
                Ok((pattern, handler))
            })
            .collect::<Result<Vec<_>, Error>>()
            .context("failed to expand patterns")?;

        let matcher = RegexSet::new(handlers.iter().map(|(pattern, _)| pattern.as_str()))
            .context("failed to build the matcher")?;
        let matcher = Arc::new(matcher);

        Ok(CommandParser { cache, config, discord, matcher, handlers: Arc::new(handlers) })
    }
}
