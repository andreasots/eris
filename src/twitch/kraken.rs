use crate::config::Config;
use failure::{self, Error, ResultExt};
use futures::compat::Future01CompatExt;
use reqwest::header::{HeaderValue, ACCEPT, AUTHORIZATION};
use reqwest::r#async::Client;
use serde::Deserialize;
use serde_derive::Deserialize;
use serde_json::{self, Value};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct Channel {
    pub display_name: Option<String>,
    pub name: String,
    pub status: Option<String>,
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct Stream {
    pub channel: Channel,
    pub game: Option<String>,
}

/// The old v5 API. Deprecated but the New Twitch API is still lacking some features.
#[derive(Clone)]
pub struct Kraken {
    client: Client,
    config: Arc<Config>,
}

impl Kraken {
    pub fn new(client: Client, config: Arc<Config>) -> Kraken {
        Kraken { client, config }
    }

    async fn paginated_by_offset<'a, T: for<'de> Deserialize<'de>>(
        &'a self,
        url: &'a str,
        token: Option<String>,
        key: &'a str,
    ) -> Result<Vec<T>, Error> {
        let mut data = vec![];

        let token = match token {
            Some(token) => Some(
                HeaderValue::from_str(&format!("OAuth {}", token))
                    .context("failed to set the OAuth token")?,
            ),
            None => None,
        };

        loop {
            let mut req = self
                .client
                .get(url)
                .header(
                    "Client-ID",
                    HeaderValue::from_str(&self.config.twitch_client_id[..])
                        .context("failed to set the Client-ID")?,
                )
                .header(
                    ACCEPT,
                    HeaderValue::from_static("application/vnd.twitchtv.v5+json"),
                );
            if let Some(ref token) = token {
                req = req.header(AUTHORIZATION, token.clone());
            }
            let value = await!(await!(req
                .query(&[("offset", &format!("{}", data.len())[..]), ("limit", "25")])
                .send()
                .compat())
            .context("failed to send the request")?
            .json::<Value>()
            .compat())
            .context("failed to parse the response")?;
            data.extend(
                value
                    .get(key)
                    .cloned()
                    .map(serde_json::from_value::<Vec<T>>)
                    .unwrap_or_else(|| Ok(vec![]))
                    .context("failed to parse results")?,
            );

            if data.len() as u64
                >= value
                    .get("_total")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| failure::err_msg("'_total' missing or not an integer"))?
            {
                break;
            }
        }

        Ok(data)
    }

    /// https://dev.twitch.tv/docs/v5/reference/streams/#get-followed-streams
    pub async fn get_streams_followed(&self, token: String) -> Result<Vec<Stream>, Error> {
        await!(self.paginated_by_offset(
            "https://api.twitch.tv/kraken/streams/followed",
            Some(token),
            "streams",
        ))
    }
}
