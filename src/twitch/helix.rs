use crate::config::Config;
use anyhow::{Context, Error};
use chrono::{DateTime, FixedOffset};
use reqwest::header::HeaderValue;
use reqwest::Client;
use serde::de::{Error as SerdeError, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

#[derive(Copy, Clone, Debug)]
pub enum User<'a> {
    Id(&'a str),
    Login(&'a str),
}

#[derive(Copy, Clone, Debug)]
pub enum GameDescriptor<'a> {
    Id(&'a str),
    Name(&'a str),
}

#[derive(Copy, Clone, Debug)]
pub enum StreamType {
    Live,
    Error,
}

impl<'de> Deserialize<'de> for StreamType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StreamTypeVisitor;

        impl<'de> Visitor<'de> for StreamTypeVisitor {
            type Value = StreamType;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("\"live\" or \"\"")
            }

            fn visit_str<E: SerdeError>(self, s: &str) -> Result<Self::Value, E> {
                match s {
                    "live" => Ok(StreamType::Live),
                    "" => Ok(StreamType::Error),
                    variant => Err(E::unknown_variant(variant, &["live", ""])),
                }
            }
        }

        deserializer.deserialize_str(StreamTypeVisitor)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Stream {
    pub game_id: String,
    pub id: String,
    pub language: String,
    pub started_at: DateTime<FixedOffset>,
    pub tag_ids: Vec<String>,
    pub thumbnail_url: String,
    pub title: String,
    #[serde(rename = "type")]
    pub stream_type: StreamType,
    pub user_id: String,
    pub user_name: String,
    pub viewer_count: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Follow {
    pub from_id: String,
    pub from_name: String,
    pub to_id: String,
    pub to_name: String,
    pub followed_at: DateTime<FixedOffset>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Game {
    pub id: String,
    pub name: String,
    pub box_art_url: String,
}

#[derive(Serialize)]
struct GetUsersFollowsQueryParams<'a> {
    after: Option<&'a str>,
    first: u64,
    from_id: Option<&'a str>,
    to_id: Option<&'a str>,
}

#[derive(Deserialize)]
struct Pagination {
    cursor: Option<String>,
}

#[derive(Deserialize)]
struct PaginatedResponse<T> {
    data: Vec<T>,
    pagination: Option<Pagination>,
}

/// The New Twitch API
#[derive(Clone)]
pub struct Helix {
    client: Client,
    client_id: HeaderValue,
}

impl Helix {
    pub fn new(client: Client, config: &Config) -> Result<Helix, Error> {
        Ok(Helix {
            client,
            client_id: HeaderValue::from_str(&config.twitch_client_id)
                .context("Client-ID is not valid as a header value")?,
        })
    }

    pub async fn get_streams(&self, token: &str, users: &[User<'_>]) -> Result<Vec<Stream>, Error> {
        let mut streams = vec![];

        for chunk in users.chunks(100) {
            let mut after = None::<String>;

            loop {
                let mut response = {
                    let mut params = vec![];

                    for user in chunk {
                        match *user {
                            User::Id(id) => params.push(("user_id", id)),
                            User::Login(login) => params.push(("user_login", login)),
                        }
                    }
                    params.push(("first", "100"));
                    if let Some(after) = after.as_ref() {
                        params.push(("after", after));
                    }

                    self.client
                        .get("https://api.twitch.tv/helix/streams")
                        .query(&params)
                        .header("Client-ID", self.client_id.clone())
                        .bearer_auth(token)
                        .send()
                        .await
                        .context("failed to send the request")?
                        .error_for_status()
                        .context("request failed")?
                        .json::<PaginatedResponse<Stream>>()
                        .await
                        .context("failed to read the response")?
                };

                streams.extend(response.data.drain(..));

                if let Some(cursor) = response.pagination.and_then(|p| p.cursor) {
                    after = Some(cursor);
                } else {
                    break;
                }
            }
        }

        Ok(streams)
    }

    pub async fn get_user_follows(
        &self,
        token: &str,
        from_id: Option<&str>,
        to_id: Option<&str>,
    ) -> Result<Vec<Follow>, Error> {
        let mut follows = vec![];
        let mut after = None::<String>;

        loop {
            let mut response = self
                .client
                .get("https://api.twitch.tv/helix/users/follows")
                .query(&GetUsersFollowsQueryParams {
                    from_id,
                    to_id,
                    after: after.as_ref().map(String::as_str),
                    first: 100,
                })
                .header("Client-ID", self.client_id.clone())
                .bearer_auth(token)
                .send()
                .await
                .context("failed to send the request")?
                .error_for_status()
                .context("request failed")?
                .json::<PaginatedResponse<Follow>>()
                .await
                .context("failed to read the response")?;

            follows.extend(response.data.drain(..));

            if let Some(cursor) = response.pagination.and_then(|p| p.cursor) {
                after = Some(cursor);
            } else {
                break;
            }
        }

        Ok(follows)
    }

    pub async fn get_games(
        &self,
        token: &str,
        game_descriptors: &[GameDescriptor<'_>],
    ) -> Result<Vec<Game>, Error> {
        let mut games = vec![];

        for chunk in game_descriptors.chunks(100) {
            let mut after = None::<String>;

            loop {
                let mut response = {
                    let mut params = vec![];

                    for game in chunk {
                        match *game {
                            GameDescriptor::Id(id) => params.push(("id", id)),
                            GameDescriptor::Name(name) => params.push(("name", name)),
                        }
                    }
                    if let Some(after) = after.as_ref() {
                        params.push(("after", after));
                    }

                    self.client
                        .get("https://api.twitch.tv/helix/games")
                        .query(&params)
                        .header("Client-ID", self.client_id.clone())
                        .bearer_auth(token)
                        .send()
                        .await
                        .context("failed to send the request")?
                        .error_for_status()
                        .context("request failed")?
                        .json::<PaginatedResponse<Game>>()
                        .await
                        .context("failed to read the response")?
                };

                games.extend(response.data.drain(..));

                if let Some(cursor) = response.pagination.and_then(|p| p.cursor) {
                    after = Some(cursor);
                } else {
                    break;
                }
            }
        }

        Ok(games)
    }
}
