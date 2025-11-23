use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

use anyhow::{Context, Error};
use chrono::Utc;
use twilight_http::Client;
use twilight_model::channel::Message;

use crate::cache::Cache;
use crate::command_parser::{Args, CommandHandler, Commands, Help};
use crate::config::Config;

pub struct Time {
    pattern: &'static str,
    help: Help,
    format: &'static str,
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
            format: "%l:%M %p",
        }
    }

    pub fn new_24() -> Self {
        Self {
            pattern: "time 24",
            help: Help {
                name: "time 24".into(),
                usage: "time 24".into(),
                summary: "Post the current moonbase time using a 24-hour clock".into(),
                description: "Post the current moonbase time using a 24-hour clock.".into(),
                examples: Cow::Borrowed(&[]),
            },
            format: "%H:%M",
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
        _: &'a Cache,
        config: &'a Config,
        discord: &'a Client,
        _: Commands<'a>,
        message: &'a Message,
        _: &'a Args,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            discord
                .create_message(message.channel_id)
                .reply(message.id)
                .content(&format!(
                    "Current moonbase time: {}",
                    Utc::now().with_timezone(&config.timezone).format(self.format)
                ))
                .await
                .context("failed to reply to command")?;

            Ok(())
        })
    }
}
