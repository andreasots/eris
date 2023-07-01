use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Error};
use egg_mode::entities::MediaType;
use egg_mode::tweet::Tweet;
use egg_mode::Token;
use sea_orm::DatabaseConnection;
use tokio::sync::watch::Receiver;
use tracing::{error, info};
use twilight_http::Client as DiscordClient;
use twilight_model::channel::message::Embed;
use twilight_model::id::marker::ChannelMarker;
use twilight_model::id::Id;
use twilight_model::util::Timestamp;
use twilight_util::builder::embed::{
    EmbedAuthorBuilder, EmbedBuilder, EmbedFooterBuilder, ImageSource,
};
use twilight_validate::embed::{AUTHOR_NAME_LENGTH, DESCRIPTION_LENGTH};

use crate::config::Config;
use crate::models::state;
use crate::shorten::shorten;

pub fn create_embeds(tweet: &Tweet) -> Vec<Embed> {
    let mut embeds = vec![];

    let mut text = tweet.text.clone();

    for url in &tweet.entities.urls {
        text = text.replace(
            url.url.as_str(),
            url.expanded_url.as_deref().unwrap_or(url.display_url.as_str()),
        );
    }

    if let Some(ref media) = tweet.entities.media {
        for media in media {
            text = text.replace(media.url.as_str(), &media.expanded_url);
        }
    }

    let mut embed = EmbedBuilder::new().description(shorten(&text, DESCRIPTION_LENGTH));

    let unsupported_media = tweet
        .extended_entities
        .as_ref()
        .map(|entities| {
            entities
                .media
                .iter()
                .filter_map(|e| match e.media_type {
                    MediaType::Photo => None,
                    MediaType::Video => Some("a video"),
                    MediaType::Gif => Some("a \"GIF\""),
                })
                .collect()
        })
        .unwrap_or_else(Vec::new);

    if !unsupported_media.is_empty() {
        let types = unsupported_media.join(", ");
        let replacement = if unsupported_media.len() == 1 { "a thumbnail" } else { "thumbnails" };
        embed = embed.footer(EmbedFooterBuilder::new(format!(
            "NOTE: unsupported media ({types}) replaced with {replacement}"
        )));
    }

    if let Ok(timestamp) = Timestamp::from_micros(tweet.created_at.timestamp_micros()) {
        embed = embed.timestamp(timestamp);
    }

    if let Some(ref user) = tweet.user {
        let mut author = EmbedAuthorBuilder::new(shorten(
            &format!("{} (@{})", user.name, user.screen_name),
            AUTHOR_NAME_LENGTH,
        ))
        .url(format!("https://twitter.com/{}", user.screen_name));
        if let Ok(profile_image) = ImageSource::url(&user.profile_image_url_https) {
            author = author.icon_url(profile_image);
        }
        embed = embed
            .author(author)
            .url(format!("https://twitter.com/{}/status/{}", user.screen_name, tweet.id));
    } else {
        embed = embed.url(format!("https://twitter.com/i/status/{}", tweet.id));
    }

    if let Some(ref entities) = tweet.extended_entities {
        for entity in &entities.media {
            if let Ok(image) = ImageSource::url(&entity.media_url_https) {
                embed = embed.image(image);
                embeds.push(embed.build());

                embed = EmbedBuilder::new().url(embeds[0].url.as_ref().unwrap());
            }
        }
    } else {
        embeds.push(embed.build());
    }

    embeds
}

async fn init(config: &Config) -> Result<(Token, HashMap<u64, Vec<Id<ChannelMarker>>>), Error> {
    let token = egg_mode::auth::bearer_token(&config.twitter_api)
        .await
        .context("failed to get the application token")?;

    let user_ids =
        egg_mode::user::lookup(config.twitter_users.keys().cloned().collect::<Vec<_>>(), &token)
            .await
            .context("failed to fetch the Twitter IDs for watched users")?
            .into_iter()
            .map(|user| (user.screen_name.to_lowercase(), user.id))
            .collect::<HashMap<_, _>>();

    let users = config
        .twitter_users
        .iter()
        .map(|(user, channels)| (user_ids[user], channels.clone()))
        .collect();

    Ok((token, users))
}

async fn inner<'a>(
    db: &'a DatabaseConnection,
    discord: &'a DiscordClient,
    token: &'a Token,
    users: &'a HashMap<u64, Vec<Id<ChannelMarker>>>,
) -> Result<(), Error> {
    for (&user_id, channels) in users {
        let state_key = format!("eris.announcements.twitter.{}.last_tweet_id", user_id);
        let last_tweet_id =
            state::get::<u64>(&state_key, db).await.context("failed to get the last tweet ID")?;

        let mut tweets = egg_mode::tweet::user_timeline(user_id, true, true, token)
            .call(last_tweet_id, None)
            .await
            .context("failed to fetch new tweets")?
            .response;

        // Don't send an avalanche of tweets when first activated.
        if last_tweet_id.is_some() {
            tweets.sort_by_key(|tweet| tweet.id);
            for tweet in &tweets {
                // (Non-reply tweet or a reply to an account we're watching) and (a retweet or doesn't start with a user mention)
                if tweet
                    .in_reply_to_user_id
                    .map(|user_id| users.contains_key(&user_id))
                    .unwrap_or(true)
                    && (tweet.retweeted_status.is_some()
                        || tweet.entities.user_mentions.iter().all(|mention| mention.range.0 != 0))
                {
                    let message = if let Some(ref user) = tweet.user {
                        format!(
                            "New tweet from {}: <https://twitter.com/{}/status/{}>",
                            user.name, user.screen_name, tweet.id,
                        )
                    } else {
                        format!("New tweet: <https://twitter.com/i/status/{}>", tweet.id)
                    };

                    let embeds = create_embeds(&tweet);

                    for channel in channels.iter().copied() {
                        if let Some(retweeted_user_id) = tweet
                            .retweeted_status
                            .as_ref()
                            .and_then(|tweet| tweet.user.as_ref())
                            .map(|user| user.id)
                        {
                            if let Some(channels) = users.get(&retweeted_user_id) {
                                if channels.contains(&channel) {
                                    info!(
                                        ?channel,
                                        msg = message.as_str(),
                                        "Skipping posting a retweet because the target already gets posted to this channel"
                                    );
                                    continue;
                                }
                            }
                        }

                        let message = discord
                            .create_message(channel)
                            .content(&message)
                            .context("announcement message is invalid")?
                            .embeds(&embeds)
                            .context("embed is invalid")?
                            .await
                            .context("failed to send the announcement message")?
                            .model()
                            .await
                            .context("failed to parse the announcement message")?;
                        if let Err(error) = discord.crosspost_message(channel, message.id).await {
                            error!(?error, "failed to crosspost the announcement message");
                        }
                    }

                    state::set(state_key.clone(), tweet.id, db)
                        .await
                        .context("failed to set the new last tweet ID")?;
                }
            }
        } else {
            let last_tweet_id = tweets.iter().map(|tweet| tweet.id).max().unwrap_or(1);
            state::set(state_key, last_tweet_id, db)
                .await
                .context("failed to set the new last tweet ID")?;
        }
    }

    Ok(())
}

pub async fn post_tweets(
    mut running: Receiver<bool>,
    config: Arc<Config>,
    db: DatabaseConnection,
    discord: Arc<DiscordClient>,
) {
    let (token, users) = match init(&config).await {
        Ok(res) => res,
        Err(error) => {
            error!(?error, "failed to initialize the tweet announcer");
            return;
        }
    };

    let mut timer = tokio::time::interval(Duration::from_secs(10));

    loop {
        tokio::select! {
            _ = running.changed() => break,
            _ = timer.tick() => {
                if let Err(error) = inner(&db, &discord, &token, &users).await {
                    error!(?error, "Failed to announce new tweets");
                }
            }
        }
    }
}
