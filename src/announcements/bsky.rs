use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Error};
use atrium_api::app::bsky::actor::get_profile;
use atrium_api::app::bsky::feed::defs::{
    FeedViewPostData, FeedViewPostReasonRefs, ReplyRefParentRefs,
};
use atrium_api::app::bsky::feed::get_author_feed;
use atrium_api::client::AtpServiceClient;
use atrium_api::types::string::{AtIdentifier, Did};
use atrium_api::types::{Object, Union};
use atrium_xrpc_client::reqwest::{ReqwestClient, ReqwestClientBuilder};
use chrono::{DateTime, FixedOffset};
use reqwest::Client as HttpClient;
use sea_orm::DatabaseConnection;
use tokio::sync::watch::Receiver;
use twilight_http::Client as DiscordClient;
use twilight_model::id::marker::ChannelMarker;
use twilight_model::id::Id;

use crate::config::Config;
use crate::models::state;

fn parse_post_id_from_uri(url: &str) -> Option<&str> {
    if !url.starts_with("at://") {
        return None;
    }

    let (start, post_id) = url.rsplit_once('/')?;

    if start.ends_with("/app.bsky.feed.post") {
        Some(post_id)
    } else {
        None
    }
}

struct SkeetAnnouncer {
    db: DatabaseConnection,
    discord: Arc<DiscordClient>,
    client: AtpServiceClient<ReqwestClient>,

    users: HashMap<Did, Vec<Id<ChannelMarker>>>,
}

impl SkeetAnnouncer {
    async fn new(
        config: Arc<Config>,
        db: DatabaseConnection,
        discord: Arc<DiscordClient>,
        client: AtpServiceClient<ReqwestClient>,
    ) -> Result<Self, Error> {
        let mut users: HashMap<Did, Vec<Id<ChannelMarker>>> =
            HashMap::with_capacity(config.bsky_users.len());

        for (user, channels) in config.bsky_users.iter() {
            let did = match user {
                AtIdentifier::Did(did) => did.clone(),
                id => {
                    let profile = client
                        .service
                        .app
                        .bsky
                        .actor
                        .get_profile(Object::from(get_profile::ParametersData {
                            actor: id.clone(),
                        }))
                        .await
                        .with_context(|| format!("failed to look up {id:?}"))?;

                    profile.data.did
                }
            };

            users.insert(did, channels.clone());
        }

        Ok(Self { db, discord, client, users })
    }

    fn is_reply_to_watched_in(
        &self,
        skeet: &Object<FeedViewPostData>,
        channel: Id<ChannelMarker>,
    ) -> bool {
        let Some(reply) = skeet.reply.as_ref() else { return false };

        match reply.parent {
            Union::Refs(ReplyRefParentRefs::PostView(ref post)) => self
                .users
                .get(&post.author.did)
                .map(Vec::as_slice)
                .unwrap_or_default()
                .contains(&channel),
            Union::Refs(ReplyRefParentRefs::BlockedPost(ref post)) => self
                .users
                .get(&post.author.did)
                .map(Vec::as_slice)
                .unwrap_or_default()
                .contains(&channel),
            Union::Refs(ReplyRefParentRefs::NotFoundPost(_)) => false,
            Union::Unknown(_) => false,
        }
    }

    async fn post_skeets(&self) -> Result<(), Error> {
        for (user, channels) in self.users.iter() {
            let state_key =
                format!("eris.announcements.bsky.{}.last_skeet_indexed_at", user.as_ref());

            let last_skeet_indexed_at = state::get::<DateTime<FixedOffset>>(&state_key, &self.db)
                .await
                .context("failed to get the last skeet ID")?;

            let mut skeets = self
                .client
                .service
                .app
                .bsky
                .feed
                .get_author_feed(Object::from(get_author_feed::ParametersData {
                    actor: AtIdentifier::Did(user.clone()),
                    cursor: None,
                    filter: None,
                    include_pins: Some(false),
                    limit: None,
                }))
                .await
                .with_context(|| format!("failed to fetch skeets from {user:?}"))?
                .data
                .feed;

            if let Some(last_skeet_indexed_at) = last_skeet_indexed_at {
                skeets.retain(|skeet| *skeet.post.indexed_at.as_ref() > last_skeet_indexed_at);
            }
            skeets.sort_by_key(|skeet| *skeet.post.indexed_at.as_ref());

            // Don't send an avalanche of toots when first activated.
            if last_skeet_indexed_at.is_some() {
                for skeet in skeets {
                    let Some(post_id) = parse_post_id_from_uri(&skeet.post.uri) else {
                        tracing::warn!(uri = skeet.post.uri, "failed to parse skeet URI");
                        continue;
                    };

                    let (message, is_reskeet) = match skeet.reason {
                        Some(Union::Refs(FeedViewPostReasonRefs::ReasonRepost(ref repost))) => (
                            format!(
                                "{} reposted a post: https://bsky.app/profile/{}/post/{post_id}",
                                repost
                                    .by
                                    .display_name
                                    .as_deref()
                                    .unwrap_or(repost.by.handle.as_str()),
                                skeet.post.author.handle.as_str(),
                            ),
                            true,
                        ),
                        _ => (
                            format!(
                                "New post from {}: https://bsky.app/profile/{}/post/{post_id}",
                                skeet
                                    .post
                                    .author
                                    .display_name
                                    .as_deref()
                                    .unwrap_or(skeet.post.author.handle.as_str()),
                                skeet.post.author.handle.as_str(),
                            ),
                            false,
                        ),
                    };

                    for channel in channels.iter().copied() {
                        if skeet.reply.is_some() && !self.is_reply_to_watched_in(&skeet, channel) {
                            tracing::info!(
                                ?channel,
                                message,
                                "Skipping posting a reply because the target does not get posted to this channel",
                            );
                            continue;
                        }

                        if is_reskeet
                            && self
                                .users
                                .get(&skeet.post.author.did)
                                .map(Vec::as_slice)
                                .unwrap_or_default()
                                .contains(&channel)
                        {
                            tracing::info!(
                                ?channel,
                                message,
                                "Skipping posting a reskeet because the target already gets posted to this channel",
                            );
                        }

                        let message = self
                            .discord
                            .create_message(channel)
                            .content(&message)
                            .await
                            .context("failed to send the announcement message")?
                            .model()
                            .await
                            .context("failed to parse the announcement message")?;
                        if let Err(error) =
                            self.discord.crosspost_message(channel, message.id).await
                        {
                            tracing::error!(?error, "failed to crosspost the announcement message");
                        }
                    }

                    state::set(state_key.clone(), *skeet.post.indexed_at.as_ref(), &self.db)
                        .await
                        .context("failed to update the last skeet indexed at timestamp")?;
                }
            } else {
                let last_skeet_indexed_at = skeets
                    .last()
                    .map(|skeet| *skeet.post.indexed_at.as_ref())
                    .unwrap_or(DateTime::<FixedOffset>::MIN_UTC.fixed_offset());

                state::set(state_key, last_skeet_indexed_at, &self.db)
                    .await
                    .context("failed to update the last skeet indexed at timestamp")?;
            }
        }
        Ok(())
    }
}

pub async fn post_skeets(
    mut running: Receiver<bool>,
    config: Arc<Config>,
    db: DatabaseConnection,
    discord: Arc<DiscordClient>,
    http_client: HttpClient,
) {
    let client = AtpServiceClient::new(
        ReqwestClientBuilder::new("https://public.api.bsky.app").client(http_client).build(),
    );

    let annoucer = match SkeetAnnouncer::new(config, db, discord, client).await {
        Ok(res) => res,
        Err(error) => {
            tracing::error!(?error, "failed to initialize the skeet announcer");
            return;
        }
    };

    let mut timer = tokio::time::interval(Duration::from_secs(10));

    loop {
        tokio::select! {
            _ = running.changed() => break,
            _ = timer.tick() => {
                if let Err(error) = annoucer.post_skeets().await {
                    tracing::error!(?error, "Failed to announce new skeets");
                }
            }
        }
    }
}
