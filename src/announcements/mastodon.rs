use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Error};
use reqwest::Client as HttpClient;
use sea_orm::DatabaseConnection;
use tokio::sync::watch::Receiver;
use tracing::{error, info};
use twilight_http::Client as DiscordClient;
use twilight_model::id::marker::ChannelMarker;
use twilight_model::id::Id;
use url::Url;

use crate::config::Config;
use crate::models::state;

mod mastodon_api {
    use serde::Deserialize;
    use time::OffsetDateTime;
    use url::Url;

    #[derive(Deserialize)]
    pub struct SearchResponse {
        pub accounts: Vec<Account>,
    }

    #[derive(Deserialize)]
    pub struct Account {
        /// Account ID
        pub id: String,
        /// The Webfinger account URI. Equal to username for local users, or username@domain for remote users.
        pub acct: String,
        /// The profile’s display name.
        pub display_name: String,
    }

    #[derive(Deserialize)]
    pub struct Status {
        pub id: String,
        /// A link to the status’s HTML representation.
        pub url: Option<Url>,
        /// URI of the status used for federation.
        pub uri: Url,
        /// The account that authored this status.
        pub account: Account,
        /// The status being reblogged.
        pub reblog: Option<Box<Status>>,
        /// ID of the account that authored the status being replied to.
        pub in_reply_to_account_id: Option<String>,
        /// The date when this status was created.
        #[serde(with = "time::serde::iso8601")]
        pub created_at: OffsetDateTime,
    }
}

struct TootAnnouncer {
    config: Arc<Config>,
    db: DatabaseConnection,
    discord: Arc<DiscordClient>,
    http_client: HttpClient,

    users: HashMap<String, Vec<Id<ChannelMarker>>>,
}

impl TootAnnouncer {
    async fn new(
        config: Arc<Config>,
        db: DatabaseConnection,
        discord: Arc<DiscordClient>,
        http_client: HttpClient,
    ) -> Result<Self, Error> {
        let mut this = Self { config, db, discord, http_client, users: HashMap::new() };
        this.populate_users().await?;
        Ok(this)
    }

    fn url(&self, path: &str) -> Result<Url, Error> {
        self.config.mastodon_server.join(path).with_context(|| {
            format!("failed to join {path:?} to {:?}", self.config.mastodon_server)
        })
    }

    async fn populate_users(&mut self) -> Result<(), Error> {
        let search_url = self.url("api/v2/search").context("failed to construct the search URL")?;

        for (username, channels) in &self.config.mastodon_users {
            let res = self
                .http_client
                .get(search_url.clone())
                .query(&[("q", username.as_str()), ("type", "accounts")])
                .send()
                .await
                .with_context(|| format!("failed to send a search request for {username:?}"))?
                .error_for_status()
                .with_context(|| format!("failed to search for {username:?}"))?
                .json::<self::mastodon_api::SearchResponse>()
                .await
                .with_context(|| format!("failed to parse the search response for {username:?}"))?;

            let account = res
                .accounts
                .iter()
                .find(|account| caseless::canonical_caseless_match_str(&account.acct, username))
                .ok_or_else(|| {
                    let accounts = res
                        .accounts
                        .iter()
                        .map(|account| account.acct.as_str())
                        .collect::<Vec<_>>();
                    Error::msg(format!(
                        "failed to find {username:?}: server returned: {accounts:?}"
                    ))
                })?;

            self.users.insert(account.id.clone(), channels.clone());
        }

        Ok(())
    }

    async fn post_toots(&self) -> Result<(), Error> {
        for (user_id, channels) in &self.users {
            let state_key = format!("eris.announcements.mastodon.{}.last_toot_id", user_id);
            let last_toot_id = state::get::<String>(&state_key, &self.db)
                .await
                .context("failed to get the last toot ID")?;

            let mut toots = self
                .http_client
                .get(
                    self.url(&format!("api/v1/accounts/{user_id}/statuses"))
                        .context("failed to construct the toots URL")?,
                )
                .query(&[("min_id", last_toot_id.as_deref())])
                .send()
                .await
                .with_context(|| format!("failed to request new toots from {user_id}"))?
                .error_for_status()
                .with_context(|| format!("failed to get new toots from {user_id}"))?
                .json::<Vec<self::mastodon_api::Status>>()
                .await
                .with_context(|| format!("failed to parse the new toots from {user_id}"))?;

            toots.sort_by_key(|toot| toot.created_at);

            // Don't send an avalanche of toots when first activated.
            if last_toot_id.is_some() {
                for toot in &toots {
                    // (Non-reply toot or a reply to an account we're watching) and (a boost or ~~doesn't start with a user mention~~)
                    // TODO: figure out where user mentions are in the HTML soup
                    // NOTE: unlike Twitter Mastodon does show toots that start with a mention under posts instead of replies...
                    if toot
                        .in_reply_to_account_id
                        .as_deref()
                        .map(|user_id| self.users.contains_key(user_id))
                        .unwrap_or(true)
                        && (toot.reblog.is_some() || true)
                    {
                        let message = if let Some(ref boosted_toot) = toot.reblog {
                            format!(
                                "{} boosted a toot: {}",
                                toot.account.display_name,
                                boosted_toot.url.as_ref().unwrap_or(&toot.uri)
                            )
                        } else {
                            format!(
                                "New toot from {}: {}",
                                toot.account.display_name,
                                toot.url.as_ref().unwrap_or(&toot.uri)
                            )
                        };

                        for channel in channels.iter().copied() {
                            if let Some(boosted_user_id) =
                                toot.reblog.as_deref().map(|toot| toot.account.id.as_str())
                            {
                                if let Some(channels) = self.users.get(boosted_user_id) {
                                    if channels.contains(&channel) {
                                        info!(
                                            ?channel,
                                            msg = message.as_str(),
                                            "Skipping posting a boost because the target already gets posted to this channel"
                                        );
                                        continue;
                                    }
                                }
                            }

                            let message = self
                                .discord
                                .create_message(channel)
                                .content(&message)
                                .context("announcement message is invalid")?
                                .await
                                .context("failed to send the announcement message")?
                                .model()
                                .await
                                .context("failed to parse the announcement message")?;
                            if let Err(error) =
                                self.discord.crosspost_message(channel, message.id).await
                            {
                                error!(?error, "failed to crosspost the announcement message");
                            }
                        }
                    }

                    state::set(state_key.clone(), &toot.id, &self.db)
                        .await
                        .context("failed to set the new last toot ID")?;
                }
            } else {
                let last_toot_id = toots.last().map(|toot| toot.id.as_str()).unwrap_or("0");
                state::set(state_key, last_toot_id, &self.db)
                    .await
                    .context("failed to set the new last toot ID")?;
            }
        }

        Ok(())
    }
}

pub async fn post_toots(
    mut running: Receiver<bool>,
    config: Arc<Config>,
    db: DatabaseConnection,
    discord: Arc<DiscordClient>,
    http_client: HttpClient,
) {
    let annoucer = match TootAnnouncer::new(config, db, discord, http_client).await {
        Ok(res) => res,
        Err(error) => {
            error!(?error, "failed to initialize the toot announcer");
            return;
        }
    };

    let mut timer = tokio::time::interval(Duration::from_secs(10));

    loop {
        tokio::select! {
            _ = running.changed() => break,
            _ = timer.tick() => {
                if let Err(error) = annoucer.post_toots().await {
                    error!(?error, "Failed to announce new toots");
                }
            }
        }
    }
}
