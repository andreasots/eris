use crate::config::Config;
use crate::context::ErisContext;
use crate::extract::Extract;
use crate::models::State;
use crate::twitter::Twitter;
use crate::typemap_keys::PgPool;
use anyhow::{Context, Error};
use serenity::model::id::ChannelId;
use std::collections::HashMap;
use std::time::Duration;
use tracing::{error, info};

async fn init(ctx: &ErisContext) -> Result<HashMap<u64, Vec<ChannelId>>, Error> {
    let data = ctx.data.read().await;
    let twitter = data.extract::<Twitter>()?;
    let twitter_users = &data.extract::<Config>()?.twitter_users;

    let usernames = twitter_users.keys().map(|s| &s[..]).collect::<Vec<&str>>();
    let user_ids = twitter
        .users_lookup(&usernames)
        .await
        .context("failed to fetch the Twitter IDs for watched users")?
        .into_iter()
        .map(|user| (user.screen_name.to_lowercase(), user.id))
        .collect::<HashMap<_, _>>();

    let mut users = HashMap::new();

    for (user, channels) in twitter_users {
        users.insert(user_ids[user], channels.clone());
    }

    Ok(users)
}

async fn inner<'a>(
    ctx: &'a ErisContext,
    users: &'a HashMap<u64, Vec<ChannelId>>,
) -> Result<(), Error> {
    let data = ctx.data.read().await;

    for (&user_id, channels) in users {
        let state_key = &format!("eris.announcements.twitter.{}.last_tweet_id", user_id);
        let last_tweet_id = {
            let conn = data
                .extract::<PgPool>()?
                .get()
                .context("failed to get a DB connection from the connection pool")?;

            State::get::<u64, _>(&state_key, &conn).context("failed to get the last tweet ID")?
        };

        let twitter = data.extract::<Twitter>()?;

        let mut tweets = twitter
            .user_timeline(user_id, true, true, 200, last_tweet_id)
            .await
            .context("failed to fetch new tweets")?;

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
                        || tweet
                            .entities
                            .user_mentions
                            .iter()
                            .all(|mention| mention.indices.0 != 0))
                {
                    let message = format!(
                        "New tweet from {}: https://twitter.com/{}/status/{}",
                        tweet.user.name, tweet.user.screen_name, tweet.id,
                    );
                    for channel in channels {
                        if let Some(retweeted_user_id) =
                            tweet.retweeted_status.as_ref().map(|tweet| tweet.user.id)
                        {
                            if let Some(channels) = users.get(&retweeted_user_id) {
                                if channels.contains(channel) {
                                    info!(
                                        channel = channel.0,
                                        msg = message.as_str(),
                                        "Skipping posting a retweet because the target already gets posted to this channel"
                                    );
                                    continue;
                                }
                            }
                        }
                        channel
                            .say(ctx, &message)
                            .await
                            .context("failed to send the announcement message")?;
                    }

                    {
                        let conn = data
                            .extract::<PgPool>()?
                            .get()
                            .context("failed to get a DB connection from the connection pool")?;
                        State::set(&state_key, tweet.id, &conn)
                            .context("failed to set the new last tweet ID")?;
                    }
                }
            }
        } else {
            let conn = data
                .extract::<PgPool>()?
                .get()
                .context("failed to get a DB connection from the connection pool")?;

            let last_tweet_id = tweets.iter().map(|tweet| tweet.id).max().unwrap_or(1);
            State::set(&state_key, last_tweet_id, &conn)
                .context("failed to set the new last tweet ID")?;
        }
    }

    Ok(())
}

pub async fn post_tweets(ctx: ErisContext) {
    let users = match init(&ctx).await {
        Ok(users) => users,
        Err(error) => {
            error!(?error, "failed to initialize the tweet announcer");
            return;
        }
    };

    let mut timer = tokio::time::interval(Duration::from_secs(10));

    loop {
        timer.tick().await;

        if let Err(error) = inner(&ctx, &users).await {
            error!(?error, "Failed to announce new tweets");
        }
    }
}
