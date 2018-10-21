use crate::config::Config;
use crate::models::State;
use crate::PgPool;
use egg_mode::{Response, Token};
use failure::{Error, ResultExt, SyncFailure};
use futures::compat::{Future01CompatExt, Stream01CompatExt};
use futures::prelude::*;
use slog::slog_error;
use slog_scope::error;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::timer::Delay;
use tokio::timer::Interval;
use tokio_core::reactor::Handle;

async fn wait_until(rate_limit_reset: u64) -> Result<(), Error> {
    let rate_limit_reset = SystemTime::UNIX_EPOCH + Duration::from_secs(rate_limit_reset);
    let rate_limit_reset_instant =
        Instant::now() + SystemTime::now().duration_since(rate_limit_reset)?;
    await!(Delay::new(rate_limit_reset_instant).compat())
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
        match await!(f()) {
            Ok(res) => {
                if res.rate_limit_remaining == 0 {
                    await!(wait_until(res.rate_limit_reset as u64))?;
                }

                return Ok(res);
            }
            Err(egg_mode::error::Error::RateLimit(rate_limit_reset)) => {
                await!(wait_until(rate_limit_reset as u64))?;
            }
            Err(err) => return Err(err)?,
        }
    }
}

async fn init<'a>(config: &'a Config, handle: &'a Handle) -> Result<(Token, HashSet<u64>), Error> {
    let token = await!(egg_mode::bearer_token(&config.twitter_api_keys, &handle).compat())
        .context("failed to get a bearer token")?;

    let mut users = HashSet::new();

    for user in &config.twitter_users {
        let user = await!(rate_limit(
            || egg_mode::user::show(user, &token, handle).compat()
        ))
        .with_context(|err| format!("failed to get the Twitter ID of {:?}: {:?}", user, err))?;
        users.insert(user.id);
    }

    Ok((token, users))
}

async fn inner<'a>(
    config: &'a Config,
    pg_pool: &'a PgPool,
    handle: &'a Handle,
    token: &'a Token,
    users: &'a HashSet<u64>,
) -> Result<(), Error> {
    let conn = pg_pool
        .get()
        .context("failed to get a DB connection from the connection pool")?;

    for &user_id in users {
        let state_key = &format!("eris.announcements.twitter.{}.last_tweet_id", user_id);
        let last_tweet_id =
            State::get::<u64, _>(&state_key, &conn).context("failed to get the last tweet ID")?;

        let timeline =
            egg_mode::tweet::user_timeline(user_id, true, true, token, handle).with_page_size(200);

        let mut tweets = await!(rate_limit(|| timeline.call(last_tweet_id, None).compat()))
            .context("failed to fetch new tweets")?
            .response;
        // Don't send an avalanche of tweets when first activated.
        if last_tweet_id.is_some() {
            tweets.sort_by_key(|tweet| tweet.id);
            for tweet in &tweets {
                // Non-reply tweet or a reply to an account we're watching.
                if tweet
                    .in_reply_to_user_id
                    .map(|user_id| users.contains(&user_id))
                    .unwrap_or(true)
                {
                    config
                        .announcements
                        .say(format_args!(
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
                        ))
                        .map_err(SyncFailure::new)
                        .context("failed to send the annoucement message")?;
                    State::set(&state_key, tweet.id, &conn)
                        .context("failed to set the new last tweet ID")?;
                }
            }
        } else {
            let last_tweet_id = tweets.iter().map(|tweet| tweet.id).max().unwrap_or(1);
            State::set(&state_key, last_tweet_id, &conn)
                .context("failed to set the new last tweet ID")?;
        }
    }

    Ok(())
}

pub async fn post_tweets(config: Arc<Config>, pg_pool: PgPool, handle: Handle) {
    let (token, users) = match await!(init(&config, &handle)) {
        Ok((token, users)) => (token, users),
        Err(err) => {
            error!("failed to initialize the tweet announcer"; "error" => ?err);
            return;
        }
    };

    let mut timer = Interval::new(Instant::now(), Duration::from_secs(10)).compat();

    loop {
        match await!(timer.try_next()) {
            Ok(Some(_)) => match await!(inner(&config, &pg_pool, &handle, &token, &users)) {
                Ok(()) => (),
                Err(err) => error!("Failed to announce new tweets"; "error" => ?err),
            },
            Ok(None) => break,
            Err(err) => error!("Timer error"; "error" => ?err),
        }
    }
}
