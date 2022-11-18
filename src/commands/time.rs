use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

use anyhow::{Context, Error};
use time::format_description::FormatItem;
use time::macros::format_description;
use time::OffsetDateTime;
use time_tz::OffsetDateTimeExt;
use twilight_cache_inmemory::InMemoryCache;
use twilight_http::Client;
use twilight_model::channel::Message;

use crate::command_parser::{Args, CommandHandler, Commands, Help};
use crate::config::Config;

pub struct Time {
    pattern: &'static str,
    help: Help,
    format: &'static [FormatItem<'static>],
}

impl Time {
    pub fn new_12() -> Self {
        Self {
            pattern: "time",
            help: Help {
                name: "time".into(),
                usage: "time".into(),
                summary: "Post the current moonbase time".into(),
                description: "Post the current moonbase time.".into(),
                examples: Cow::Borrowed(&[]),
            },
            format: format_description!("[hour repr:12]:[minute] [period]"),
        }
    }

    pub fn new_24() -> Self {
        Self {
            pattern: "time 24",
            help: Help {
                name: "time".into(),
                usage: "time".into(),
                summary: "Post the current moonbase time using a 24-hour clock".into(),
                description: "Post the current moonbase time using a 24-hour clock.".into(),
                examples: Cow::Borrowed(&[]),
            },
            format: format_description!("[hour repr:24]:[minute]"),
        }
    }
}

impl CommandHandler for Time {
    fn pattern(&self) -> &str {
        self.pattern
    }

    fn help(&self) -> Option<Help> {
        Some(self.help.clone())
    }

    fn handle<'a>(
        &'a self,
        _: &'a InMemoryCache,
        config: &'a Config,
        discord: &'a Client,
        _: Commands<'a>,
        message: &'a Message,
        _: &'a Args,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            let now = OffsetDateTime::now_utc()
                .to_timezone(config.timezone)
                .format(self.format)
                .context("failed to format the current time")?;

            discord
                .create_message(message.channel_id)
                .reply(message.id)
                .content(&format!("Current moonbase time: {now}"))
                .context("reply message invalid")?
                .await
                .context("failed to reply to command")?;

            Ok(())
        })
    }
}
