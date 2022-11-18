use std::future::Future;
use std::pin::Pin;

use anyhow::{Context, Error};
use tracing_subscriber::reload::Handle;
use tracing_subscriber::EnvFilter;
use twilight_cache_inmemory::InMemoryCache;
use twilight_http::Client as DiscordClient;
use twilight_model::channel::message::MessageFlags;
use twilight_model::channel::Message;

use crate::command_parser::{Access, Args, CommandHandler, Commands};
use crate::config::Config;

pub struct TracingFilter<S> {
    reload_handle: Handle<EnvFilter, S>,
}

impl<S> TracingFilter<S> {
    pub fn new(reload_handle: Handle<EnvFilter, S>) -> Self {
        Self { reload_handle }
    }
}

impl<S> CommandHandler for TracingFilter<S> {
    fn pattern(&self) -> &str {
        "tracing-filter (.*)"
    }

    fn help(&self) -> Option<crate::command_parser::Help> {
        None
    }

    fn access(&self) -> Access {
        Access::OwnerOnly
    }

    fn handle<'a>(
        &'a self,
        _: &'a InMemoryCache,
        _: &'a Config,
        discord: &'a DiscordClient,
        _: Commands<'a>,
        message: &'a Message,
        args: &'a Args,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            let directives = args.get(0).unwrap_or("");
            let directives =
                if directives != "" { directives } else { crate::DEFAULT_TRACING_FILTER };

            let filter =
                EnvFilter::try_new(&directives).context("failed to construct the new filter")?;

            let mut old_filter = String::new();

            self.reload_handle.modify(|layer| {
                old_filter = layer.to_string();
                *layer = filter;
            })?;

            discord
                .create_message(message.channel_id)
                .reply(message.id)
                .flags(MessageFlags::SUPPRESS_EMBEDS)
                .content(&format!(
                    "Replaced `{}` with `{}`.",
                    crate::markdown::escape(&old_filter),
                    crate::markdown::escape(&directives)
                ))
                .context("response message invalid")?
                .await
                .context("failed to reply to command")?;

            Ok(())
        })
    }
}
