use crate::config::Config;
use crate::models::State;
use egg_mode::{Response, Token};
use failure::{Error, ResultExt, SyncFailure};
use futures::compat::{Future01CompatExt, Stream01CompatExt};
use futures::prelude::*;
use serenity::model::id::ChannelId;
use slog::slog_error;
use slog_scope::error;
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};
use tokio::timer::Delay;
use tokio::timer::Interval;
use crate::context::ErisContext;
use crate::typemap_keys::PgPool;
use crate::extract::Extract;

async fn wait_until(rate_limit_reset: u64) -> Result<(), Error> {
    let rate_limit_reset = SystemTime::UNIX_EPOCH + Duration::from_secs(rate_limit_reset);
    let rate_limit_reset_instant =
        Instant::now() + SystemTime::now().duration_since(rate_limit_reset)?;
    Delay::new(rate_limit_reset_instant).compat()
        .await
        .context("failed to wait until the rate-limit is reset")?;
    Ok(())
}

async fn rate_limit<
    T,
    Fn: FnMut() -> Fut,
    Fut: Future<Output = Result<Response<T>, egg_mode::error::Error>>,
>(
    mut f: Fn,
) -> Result<Response<T>, Error> {
    loop {
        match f().await {
            Ok(res) => {
                if res.rate_limit_remaining == 0 {
                    wait_until(res.rate_limit_reset as u64).await?;
                }

                return Ok(res);
            }
            Err(egg_mode::error::Error::RateLimit(rate_limit_reset)) => {
                wait_until(rate_limit_reset as u64).await?;
            }
            Err(err) => return Err(err)?,
        }
    }
}

async fn init(ctx: &ErisContext) -> Result<(Token, HashMap<u64, Vec<ChannelId>>), Error> {
    let (token_future, twitter_users) = {
        let data = ctx.data.read();
        let config = data.extract::<Config>()?;
        let token_future = egg_mode::bearer_token(&config.twitter_api_keys);

        (token_future, config.twitter_users.clone())
    };
    let token = token_future.compat()
        .await
        .context("failed to get a bearer token")?;

    let mut users = HashMap::new();

    for (user, channels) in twitter_users {
        let user = rate_limit(|| egg_mode::user::show(&user, &token).compat())
            .await
            .with_context(|err| format!("failed to get the Twitter ID of {:?}: {:?}", user, err))?;
        users.insert(user.id, channels);
    }

    Ok((token, users))
}

async fn inner<'a>(
    ctx: &'a ErisContext,
    token: &'a Token,
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

        let timeline =
            egg_mode::tweet::user_timeline(user_id, true, true, token).with_page_size(200);

        let mut tweets = rate_limit(|| timeline.call(last_tweet_id, None).compat())
            .await
            .context("failed to fetch new tweets")?
            .response;
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
                            .as_ref()
                            .map(|user| &user.name[..])
                            .unwrap_or("???"),
                        tweet
                            .user
                            .as_ref()
                            .map(|user| &user.screen_name[..])
                            .unwrap_or("a"),
                        tweet.id,
                    );
                    for channel in channels {
                        channel
                            .say(ctx, &message)
                            .map_err(SyncFailure::new)
                            .context("failed to send the annoucement message")?;
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
    let (token, users) = match init(&ctx).await {
        Ok((token, users)) => (token, users),
        Err(err) => {
            error!("failed to initialize the tweet announcer"; "error" => ?err);
            return;
        }
    };

    let mut timer = Interval::new(Instant::now(), Duration::from_secs(10)).compat();

    loop {
        match timer.try_next().await {
            Ok(Some(_)) => if let Err(err) = inner(&ctx, &token, &users).await {
                error!("Failed to announce new tweets"; "error" => ?err);
            },
            Ok(None) => break,
            Err(err) => error!("Timer error"; "error" => ?err),
        }
    }
}
