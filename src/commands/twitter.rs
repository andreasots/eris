use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

use anyhow::{Context, Error};
use egg_mode::Token;
use twilight_cache_inmemory::InMemoryCache;
use twilight_http::Client;
use twilight_model::channel::Message;

use crate::announcements::twitter::create_embeds;
use crate::command_parser::{Access, Args, CommandHandler, Commands, Help};
use crate::config::Config;

pub struct Tweet {
    token: Token,
}

impl Tweet {
    pub async fn new(config: &Config) -> Result<Self, Error> {
        let token = egg_mode::auth::bearer_token(&config.twitter_api)
            .await
            .context("failed to get the application token")?;

        Ok(Self { token })
    }
}

impl CommandHandler for Tweet {
    fn pattern(&self) -> &str {
        r"tweet (?:https?://(?:www\.)?twitter\.com/[^/]+/status/)?(\d+)(?:\?.*)?"
    }

    fn help(&self) -> Option<Help> {
        Some(Help {
            name: "tweet".into(),
            usage: "tweet <TWEET LINK>".into(),
            summary: "Display a tweet".into(),
            description: "Display a tweet".into(),
            examples: Cow::Borrowed(&[Cow::Borrowed(
                "tweet https://twitter.com/LRRbot/status/603533244881973248",
            )]),
        })
    }

    fn access(&self) -> Access {
        Access::All
    }

    fn handle<'a>(
        &'a self,
        _: &'a InMemoryCache,
        _: &'a Config,
        discord: &'a Client,
        _: Commands<'a>,
        message: &'a Message,
        args: &'a Args,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            let tweet_id = args
                .get(0)
                .context("tweet ID missing")?
                .parse::<u64>()
                .context("failed to parse tweed ID")?;

            let tweets = egg_mode::tweet::lookup([tweet_id], &self.token)
                .await
                .context("failed to load tweet")?
                .response;

            for tweet in tweets {
                let embeds = create_embeds(&tweet);

                discord
                    .create_message(message.channel_id)
                    .reply(message.id)
                    .embeds(&embeds)
                    .context("embed invalid")?
                    .await
                    .context("failed to reply to command")?;
            }

            Ok(())
        })
    }
}
