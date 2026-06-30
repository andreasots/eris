use std::collections::{HashMap, hash_map::Entry};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Error};
use chrono::Utc;
use deadpool::managed::Pool;
use image::ImageFormat;
use regex::{Regex, RegexSet};
use sea_orm::DatabaseConnection;
use tokio::sync::{Mutex, RwLock};
use twilight_gateway::Event;
use twilight_http::Client as DiscordClient;
use twilight_mention::Mention;
use twilight_model::http::attachment::Attachment;
use twilight_util::builder::embed::{EmbedBuilder, EmbedFieldBuilder, ImageSource};

use crate::{config::Config, tesseract::Tesseract};

pub struct RuleSet {
    all: RegexSet,
    individual: Vec<Regex>,
}

impl RuleSet {
    const RULES_STATE_KEY: &str = "eris.ocr_spam_filter.rules";

    pub async fn load(db: &DatabaseConnection) -> Result<Self, Error> {
        let rules = crate::models::state::get::<Vec<String>>(Self::RULES_STATE_KEY, db)
            .await
            .context("failed to load rules from the database")?
            .unwrap_or_default();

        Self::from_rules(rules)
    }

    pub async fn save(&self, db: &DatabaseConnection) -> Result<(), Error> {
        crate::models::state::set(Self::RULES_STATE_KEY.into(), self.rules(), db)
            .await
            .context("failed to save the rules")
    }

    pub fn rules(&self) -> &[String] {
        self.all.patterns()
    }

    pub fn from_rules<I: IntoIterator<Item = S> + Clone, S: AsRef<str>>(
        rules: I,
    ) -> Result<Self, Error> {
        Ok(Self {
            all: RegexSet::new(rules.clone()).context("failed to build a regex set")?,
            individual: rules
                .into_iter()
                .map(|rule| {
                    let rule = rule.as_ref();
                    Regex::new(rule).with_context(|| format!("failed to build rule {rule:?}"))
                })
                .collect::<Result<_, _>>()?,
        })
    }

    pub fn matches(&self, text: &str) -> Vec<&str> {
        self.all.matches(text).iter().map(|i| self.all.patterns()[i].as_str()).collect()
    }

    pub fn highlight(&self, mut text: &str) -> String {
        let mut ret = String::with_capacity(text.len());
        'outer: while !text.is_empty() {
            for re in &self.individual {
                if let Some(m) = re.find(text) {
                    ret.push_str(&crate::markdown::escape(&text[..m.start()]));
                    ret.push_str("__**");
                    ret.push_str(&crate::markdown::escape(&text[m.range()]));
                    ret.push_str("**__");
                    text = &text[m.end()..];
                    continue 'outer;
                }
            }
            ret.push_str(&crate::markdown::escape(text));
            break 'outer;
        }
        ret
    }
}

#[derive(Clone)]
pub struct OcrSpamFilter {
    config: Arc<Config>,
    discord: Arc<DiscordClient>,
    http: reqwest::Client,
    tesseract: Pool<Tesseract>,
    rules: Arc<RwLock<RuleSet>>,
    cache: Arc<Mutex<HashMap<[u8; Self::HASH_ALGO.output_len], (ImageFormat, String)>>>,
}

impl OcrSpamFilter {
    const HASH_ALGO: &'static aws_lc_rs::digest::Algorithm = &aws_lc_rs::digest::SHA256;

    pub fn new(
        config: Arc<Config>,
        discord: Arc<DiscordClient>,
        http: reqwest::Client,
        tesseract: Pool<Tesseract>,
        rules: Arc<RwLock<RuleSet>>,
    ) -> Result<Self, Error> {
        Ok(Self {
            config,
            discord,
            http,
            tesseract,
            rules,
            cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    async fn download(&self, url: &str) -> Result<Vec<u8>, Error> {
        Ok(self
            .http
            .get(url)
            .send()
            .await
            .context("failed to request the image")?
            .error_for_status()
            .context("image request failed")?
            .bytes()
            .await
            .context("failed to download the image")?
            .into())
    }

    async fn extract_text(&self, image: &[u8]) -> Result<(ImageFormat, String), Error> {
        let hash = aws_lc_rs::digest::digest(Self::HASH_ALGO, image)
            .as_ref()
            .try_into()
            .context("wrong hash algo output size")?;
        let mut cache = self.cache.lock().await;
        match cache.entry(hash) {
            Entry::Occupied(entry) => Ok(entry.get().clone()),
            Entry::Vacant(entry) => {
                let mut tesseract =
                    self.tesseract.get().await.context("failed to get a Tesseract instance")?;
                let res = crate::tesseract::extract_text(&mut tesseract, image)
                    .context("failed to extract text from image")?;
                Ok(entry.insert(res).clone())
            }
        }
    }

    pub async fn on_event(self, event: Event) {
        let message = match &event {
            Event::MessageCreate(event) => &event.0,
            Event::MessageUpdate(event) => &event.0,
            _ => return,
        };

        if message.author.bot {
            // The bots are presumably verified to not send spam.
            // Also it'd kinda suck if we deleted logs of spam getting deleted.
            return;
        }

        let urls_from_embeds = message
            .embeds
            .iter()
            .flat_map(|embed| embed.thumbnail.as_ref())
            .map(|thumbnail| thumbnail.proxy_url.as_deref().unwrap_or(thumbnail.url.as_str()));
        let urls_from_attachments =
            message.attachments.iter().map(|attachment| attachment.proxy_url.as_str());
        let urls = urls_from_embeds.chain(urls_from_attachments);

        let mut embeds = vec![];
        let mut attachments = vec![];

        for url in urls {
            let image = match self.download(url).await {
                Ok(image) => image,
                Err(error) => {
                    tracing::error!(?error, url, "failed to download the image");
                    continue;
                }
            };

            match self.extract_text(&image).await {
                Ok((format, text)) => {
                    let rules = self.rules.read().await;
                    let matched_rules = rules.matches(&text);
                    if matched_rules.is_empty() {
                        continue;
                    };
                    let filename = format!(
                        "{}.{}.{}",
                        message.id,
                        attachments.len(),
                        format.extensions_str().get(0).unwrap_or(&"bin"),
                    );

                    let embed = EmbedBuilder::new()
                        .color(0xFF_00_00)
                        .title("Image spam detected")
                        .image(ImageSource::attachment(&filename).expect("invalid image file name"))
                        .field(EmbedFieldBuilder::new(
                            "User",
                            format!(
                                "{} {} (ID: {})",
                                message.author.id.mention(),
                                message.author.name,
                                message.author.id,
                            ),
                        ))
                        .field(EmbedFieldBuilder::new(
                            "Channel",
                            message.channel_id.mention().to_string(),
                        ))
                        .field(EmbedFieldBuilder::new("Message ID", message.id.to_string()))
                        .field(EmbedFieldBuilder::new(
                            "Matched rules",
                            crate::shorten::shorten(
                                &matched_rules.join(", "),
                                twilight_validate::embed::FIELD_VALUE_LENGTH,
                            ),
                        ))
                        .description(crate::shorten::shorten(
                            &rules.highlight(&text),
                            twilight_validate::embed::DESCRIPTION_LENGTH,
                        ))
                        .timestamp(message.timestamp);
                    embeds.push(embed.build());
                    attachments.push(Attachment {
                        description: Some(
                            crate::shorten::shorten(
                                &text,
                                twilight_validate::message::ATTACHMENT_DESCIPTION_LENGTH_MAX,
                            )
                            .into_owned(),
                        ),
                        file: image,
                        filename,
                        id: attachments.len() as u64,
                    });
                }
                Err(error) => tracing::error!(?error, "failed to check {url:?} for spam"),
            }
        }

        if !embeds.is_empty() {
            let res = self
                .discord
                .create_message(self.config.logs_channel)
                .embeds(&embeds)
                .attachments(&attachments)
                .await;
            if let Err(error) = res {
                tracing::error!(?error, "failed to log the spam message");
            }
            if let Err(error) = self.discord.delete_message(message.channel_id, message.id).await {
                tracing::error!(?error, "failed to delete the spam message");
            }
            let reaction_time = Duration::from_micros(
                (Utc::now().timestamp_micros() - message.timestamp.as_micros()) as u64,
            );
            tracing::info!(
                ?reaction_time,
                message.id = message.id.get(),
                message.author.id = message.author.id.get(),
                message.author.name = message.author.name,
                "spam detected",
            );
        }
    }
}
