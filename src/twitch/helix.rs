use crate::config::Config;
use chrono::{DateTime, FixedOffset};
use failure::{Error, ResultExt};
use futures::compat::Future01CompatExt;
use reqwest::header::HeaderValue;
use reqwest::r#async::Client;
use serde::de::{Error as SerdeError, Visitor};
use serde::{Deserialize, Deserializer};
use serde_derive::Deserialize;
use std::fmt;
use std::sync::Arc;

#[derive(Copy, Clone, Debug)]
pub enum User<'a> {
    Id(&'a str),
    Login(&'a str),
}

impl<'a> User<'a> {
    fn as_query(self) -> [(&'static str, &'a str); 1] {
        match self {
            User::Id(id) => [("user_id", id)],
            User::Login(login) => [("user_login", login)],
        }
    }
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
    pub community_ids: Vec<String>,
    pub game_id: String,
    pub id: String,
    pub language: String,
    pub started_at: DateTime<FixedOffset>,
    pub thumbnail_url: String,
    pub title: String,
    #[serde(rename = "type")]
    pub stream_type: StreamType,
}

#[derive(Deserialize)]
struct Pagination {
    cursor: Option<String>,
}

#[derive(Deserialize)]
struct PaginatedResponse<T> {
    data: Vec<T>,
    pagination: Pagination,
}

/// The New Twitch API
#[derive(Clone)]
pub struct Helix {
    client: Client,
    config: Arc<Config>,
}

impl Helix {
    pub fn new(client: Client, config: Arc<Config>) -> Helix {
        Helix { client, config }
    }

    pub async fn get_stream<'a>(&'a self, user: User<'a>) -> Result<Option<Stream>, Error> {
        Ok(await!(await!(self
            .client
            .get("https://api.twitch.tv/helix/streams")
            .header(
                "Client-ID",
                HeaderValue::from_str(&self.config.twitch_client_id)
                    .context("failed to set the client ID")?
            )
            .query(&user.as_query()[..])
            .send()
            .compat())
        .context("failed to send the request")?
        .json::<PaginatedResponse<Stream>>()
        .compat())
        .context("failed to read the response")?
        .data
        .pop())
    }
}
