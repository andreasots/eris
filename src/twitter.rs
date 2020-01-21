use anyhow::{Context, Error, bail};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::borrow::Borrow;

#[derive(Serialize)]
struct OAuth2TokenRequest {
    grant_type: &'static str,
}

#[derive(Deserialize)]
struct OAuth2TokenResponse {
    token_type: String,
    access_token: String,
}

/// https://developer.twitter.com/en/docs/tweets/data-dictionary/overview/user-object
#[derive(Deserialize)]
pub struct User {
    pub id: u64,
    pub name: String,
    pub screen_name: String,
}

#[derive(Serialize)]
struct UserTimelineRequest {
    user_id: u64,
    exclude_replies: bool,
    include_rts: bool,
    count: u32,
    since_id: Option<u64>,
}

/// https://developer.twitter.com/en/docs/tweets/data-dictionary/overview/tweet-object
#[derive(Deserialize)]
pub struct Tweet {
    pub id: u64,
    pub user: User,
    pub in_reply_to_user_id: Option<u64>,
    pub retweeted_status: Option<Box<Tweet>>,
    pub entities: Entities,
}

#[derive(Deserialize)]
pub struct Entities {
    pub user_mentions: Vec<UserMention>,
}

#[derive(Deserialize)]
pub struct UserMention {
    pub indices: (usize, usize),
}

#[derive(Clone)]
pub struct Twitter {
    client: Client,
    token: String,
}

// FIXME: rate limiting? Do we even care?
impl Twitter {
    pub async fn new(client: Client, key: String, secret: String) -> Result<Twitter, Error> {
        let token = Self::fetch_token(&client, &key, &secret).await?;

        Ok(Twitter { client, token })
    }

    async fn fetch_token<'a>(
        client: &'a Client,
        key: &'a str,
        secret: &'a str,
    ) -> Result<String, Error> {
        let res = client
            .post("https://api.twitter.com/oauth2/token")
            .basic_auth(key, Some(secret))
            .form(&OAuth2TokenRequest { grant_type: "client_credentials" })
            .send()
            .await
            .context("failed to send the bearer token request")?
            .error_for_status()
            .context("bearer token request failed")?
            .json::<OAuth2TokenResponse>()
            .await
            .context("failed to parse the bearer token")?;

        if res.token_type == "bearer" {
            Ok(res.access_token)
        } else {
            bail!("OAuth2 token request returned a non-Bearer token, got {:?}", res.token_type)
        }
    }

    pub async fn users_lookup<'a>(
        &'a self,
        users: &'a [impl Borrow<str>],
    ) -> Result<Vec<User>, Error> {
        let users = self
            .client
            .get("https://api.twitter.com/1.1/users/lookup.json")
            .bearer_auth(&self.token)
            .query(&[("screen_name", users.join(","))])
            .send()
            .await
            .context("failed to send the users lookup request")?
            .error_for_status()
            .context("users lookup request failed")?
            .json::<Vec<User>>()
            .await
            .context("failed to parse users")?;
        Ok(users)
    }

    pub async fn user_timeline(
        &self,
        user_id: u64,
        with_replies: bool,
        with_retweets: bool,
        count: u32,
        since_id: Option<u64>,
    ) -> Result<Vec<Tweet>, Error> {
        let tweets = self
            .client
            .get("https://api.twitter.com/1.1/statuses/user_timeline.json")
            .bearer_auth(&self.token)
            .query(&UserTimelineRequest {
                user_id,
                exclude_replies: !with_replies,
                include_rts: with_retweets,
                count,
                since_id,
            })
            .send()
            .await
            .context("failed to send the timeline request")?
            .error_for_status()
            .context("timeline request failed")?
            .json::<Vec<Tweet>>()
            .await
            .context("failed to parse the timeline")?;
        Ok(tweets)
    }
}
