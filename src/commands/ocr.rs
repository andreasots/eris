use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context as _, Error};
use deadpool::managed::Pool;
use sea_orm::DatabaseConnection;
use tokio::sync::RwLock;
use twilight_http::Client as DiscordClient;
use twilight_model::channel::Message;
use twilight_util::builder::embed::EmbedBuilder;

use crate::cache::Cache;
use crate::command_parser::{Access, Args, CommandHandler, Commands, Help};
use crate::config::Config;
use crate::ocr_spam_filter::RuleSet;
use crate::tesseract::Tesseract;

pub struct RulesList {
    rules: Arc<RwLock<RuleSet>>,
}

impl RulesList {
    pub fn new(rules: Arc<RwLock<RuleSet>>) -> Self {
        Self { rules }
    }
}

impl CommandHandler for RulesList {
    fn pattern(&self) -> &'static str {
        "ocr rules"
    }

    fn help(&self) -> Option<Help> {
        Some(Help {
            name: "ocr rules".into(),
            usage: "ocr rules".into(),
            summary: "List the OCR spam rules".into(),
            description: "List the OCR spam rules.".into(),
            examples: Cow::Borrowed(&[]),
        })
    }

    fn access(&self) -> Access {
        Access::ModOnly
    }

    fn handle<'a>(
        &'a self,
        _: &'a Cache,
        _: &'a Config,
        discord: &'a DiscordClient,
        _: Commands<'a>,
        message: &'a Message,
        _: &'a Args,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            let rules = self.rules.read().await;

            let mut content = String::from("Current OCR spam rules:\n");
            for rule in rules.rules() {
                content.push_str("1. `");
                content.push_str(&rule.replace('`', "\\`"));
                content.push_str("`\n");
            }

            discord
                .create_message(message.channel_id)
                .reply(message.id)
                .content(&content)
                .await
                .context("failed to reply to command")?;

            Ok(())
        })
    }
}

pub struct RulesAdd {
    db: DatabaseConnection,
    rules: Arc<RwLock<RuleSet>>,
}

impl RulesAdd {
    pub fn new(db: DatabaseConnection, rules: Arc<RwLock<RuleSet>>) -> Self {
        Self { db, rules }
    }
}

impl CommandHandler for RulesAdd {
    fn pattern(&self) -> &'static str {
        "ocr rules add (.+)"
    }

    fn help(&self) -> Option<Help> {
        Some(Help {
            name: "ocr rules add".into(),
            usage: "ocr rules add <PATTERN>".into(),
            summary: "Add a new OCR spam rule".into(),
            description: "Add a new OCR spam rule.".into(),
            examples: Cow::Borrowed(&[Cow::Borrowed("ocr rules add crypto(currency)? casino")]),
        })
    }

    fn access(&self) -> Access {
        Access::ModOnly
    }

    fn handle<'a>(
        &'a self,
        _: &'a Cache,
        _: &'a Config,
        discord: &'a DiscordClient,
        _: Commands<'a>,
        message: &'a Message,
        args: &'a Args,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            let mut rules = self.rules.write().await;
            let mut new_rules = Vec::from(rules.rules());
            new_rules.push(args.get(0).context("rule missing")?.into());
            *rules = RuleSet::from_rules(new_rules).context("failed to compile the new rules")?;
            rules.save(&self.db).await.context("failed to save the new rules")?;

            discord
                .create_message(message.channel_id)
                .reply(message.id)
                .content("Rules updated.")
                .await
                .context("failed to reply to command")?;

            Ok(())
        })
    }
}

pub struct RulesRemove {
    db: DatabaseConnection,
    rules: Arc<RwLock<RuleSet>>,
}

impl RulesRemove {
    pub fn new(db: DatabaseConnection, rules: Arc<RwLock<RuleSet>>) -> Self {
        Self { db, rules }
    }
}

impl CommandHandler for RulesRemove {
    fn pattern(&self) -> &'static str {
        "ocr rules remove (.+)"
    }

    fn access(&self) -> Access {
        Access::ModOnly
    }

    fn help(&self) -> Option<Help> {
        Some(Help {
            name: "ocr rules remove".into(),
            usage: "ocr rules add <PATTERN>".into(),
            summary: "Remove an OCR spam rule".into(),
            description: "Remove an OCR spam rule.".into(),
            examples: Cow::Borrowed(&[Cow::Borrowed("ocr rules remove crypto(currency)? casino")]),
        })
    }

    fn handle<'a>(
        &'a self,
        _: &'a Cache,
        _: &'a Config,
        discord: &'a DiscordClient,
        _: Commands<'a>,
        message: &'a Message,
        args: &'a Args,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            let mut rules = self.rules.write().await;
            let mut new_rules = Vec::from(rules.rules());
            let filter = args.get(0).context("rule missing")?;
            new_rules.retain(|rule| rule != filter);
            *rules = RuleSet::from_rules(new_rules).context("failed to compile the new rules")?;
            rules.save(&self.db).await.context("failed to save the new rules")?;

            discord
                .create_message(message.channel_id)
                .reply(message.id)
                .content("Rules updated.")
                .await
                .context("failed to reply to command")?;

            Ok(())
        })
    }
}

pub struct Test {
    http: reqwest::Client,
    rules: Arc<RwLock<RuleSet>>,
    tesseract: Pool<Tesseract>,
}

impl Test {
    pub fn new(
        http: reqwest::Client,
        rules: Arc<RwLock<RuleSet>>,
        tesseract: Pool<Tesseract>,
    ) -> Self {
        Self { http, rules, tesseract }
    }
}

impl CommandHandler for Test {
    fn pattern(&self) -> &'static str {
        "ocr test (.+)"
    }

    fn help(&self) -> Option<Help> {
        Some(Help {
            name: "ocr test".into(),
            usage: "ocr test <URL>".into(),
            summary: "Run OCR on an image".into(),
            description: "Run OCR on an image.".into(),
            examples: Cow::Borrowed(&[Cow::Borrowed(
                "ocr test https://lrrbot.com/static/logo.png",
            )]),
        })
    }

    fn access(&self) -> Access {
        Access::ModOnly
    }

    fn handle<'a>(
        &'a self,
        _: &'a Cache,
        _: &'a Config,
        discord: &'a DiscordClient,
        _: Commands<'a>,
        message: &'a Message,
        args: &'a Args,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            let url = args.get(0).context("URL missing")?;

            let image = self
                .http
                .get(url)
                .send()
                .await
                .context("failed to request the image")?
                .error_for_status()
                .context("image request failed")?
                .bytes()
                .await
                .context("failed to download the image")?;

            let mut tesseract =
                self.tesseract.get().await.context("failed to get a Tesseract instance")?;

            let (_, text) = crate::tesseract::extract_text(&mut tesseract, &image)
                .context("failed to OCR the image")?;

            let highlighted = self.rules.read().await.highlight(&text);

            let embed = EmbedBuilder::new()
                .title("OCR results")
                .description(crate::shorten::shorten(
                    &highlighted,
                    twilight_validate::embed::DESCRIPTION_LENGTH,
                ))
                .build();

            discord
                .create_message(message.channel_id)
                .reply(message.id)
                .embeds(&[embed])
                .await
                .context("failed to reply to command")?;

            Ok(())
        })
    }
}
