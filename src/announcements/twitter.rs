use crate::config::Config;
use crate::models::State;
use failure::{Error, ResultExt, SyncFailure};
use futures::compat::Stream01CompatExt;
use futures::prelude::*;
use serenity::model::id::ChannelId;
use slog::slog_error;
use slog_scope::error;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::timer::Interval;
use crate::context::ErisContext;
use crate::typemap_keys::PgPool;
use crate::extract::Extract;
use crate::twitter::Twitter;

async fn init(ctx: &ErisContext) -> Result<HashMap<u64, Vec<ChannelId>>, Error> {
    let (twitter, twitter_users) = {
        let data = ctx.data.read();

        (data.extract::<Twitter>()?.clone(), data.extract::<Config>()?.twitter_users.clone())
    };

    let usernames = twitter_users.keys().map(|s| &s[..]).collect::<Vec<&str>>();
    let user_ids = twitter.users_lookup(&usernames)
        .await
        .context("failed to fetch the Twitter IDs for watched users")?
        .into_iter()
        .map(|user| (user.screen_name.to_lowercase(), user.id))
        .collect::<HashMap<_, _>>();

    let mut users = HashMap::new();

    for (user, channels) in twitter_users {
        users.insert(user_ids[&user], channels);
    }

    Ok(users)
}

async fn inner<'a>(
    ctx: &'a ErisContext,
    users: &'a HashMap<u64, Vec<ChannelId>>,
) -> Result<(), Error> {
    for (&user_id, channels) in users {
        let state_key = &format!("eris.announcements.twitter.{}.last_tweet_id", user_id);
        let last_tweet_id = {
            let data = ctx.data.read();
            let conn = data.extract::<PgPool>()?
                .get()
                .context("failed to get a DB connection from the connection pool")?;

            State::get::<u64, _>(&state_key, &conn).context("failed to get the last tweet ID")?
        };

        let twitter = ctx.data.read().extract::<Twitter>()?.clone();

        let mut tweets = twitter.user_timeline(user_id, true, true, 200, last_tweet_id)
            .await
            .context("failed to fetch new tweets")?;

        // Don't send an avalanche of tweets when first activated.
        if last_tweet_id.is_some() {
            tweets.sort_by_key(|tweet| tweet.id);
            for tweet in &tweets {
                // Non-reply tweet or a reply to an account we're watching.
                if tweet
                    .in_reply_to_user_id
                    .map(|user_id| users.contains_key(&user_id))
                    .unwrap_or(true)
                {
                    let message = format!(
                        "New tweet from {}: https://twitter.com/{}/status/{}",
                        tweet
                            .user
                            .name,
                        tweet
                            .user
                            .screen_name,
                        tweet.id,
                    );
                    for channel in channels {
                        // FIXME(rust-lang/rust#61579): manual `drop()` is needed because if the
                        //  result of `channel.say()` is not used the compiler ICEs.
                        let msg = channel
                            .say(ctx, &message)
                            .map_err(SyncFailure::new)
                            .context("failed to send the announcement message")?;
                        drop(msg);
                    }

                    {
                        let data = ctx.data.read();
                        let conn = data.extract::<PgPool>()?
                            .get()
                            .context("failed to get a DB connection from the connection pool")?;
                        State::set(&state_key, tweet.id, &conn)
                            .context("failed to set the new last tweet ID")?;
                    }
                }
            }
        } else {
            let data = ctx.data.read();
            let conn = data.extract::<PgPool>()?
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
        Err(err) => {
            error!("failed to initialize the tweet announcer"; "error" => ?err);
            return;
        }
    };

    let mut timer = Interval::new(Instant::now(), Duration::from_secs(10)).compat();

    loop {
        match timer.try_next().await {
            Ok(Some(_)) => if let Err(err) = inner(&ctx, &users).await {
                error!("Failed to announce new tweets"; "error" => ?err);
            },
            Ok(None) => break,
            Err(err) => error!("Timer error"; "error" => ?err),
        }
    }
}
