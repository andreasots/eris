use config::Config;
use failure::{self, Error, ResultExt};
use reqwest::header::{Authorization, Headers, Scheme};
use reqwest::Error as ReqwestError;
use reqwest::{Client, ClientBuilder};
use serde::Deserialize;
use serde_json::{self, Value};
use std::fmt;
use std::str::FromStr;
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

#[derive(Clone, Debug)]
struct OAuth(String);

impl FromStr for OAuth {
    type Err = ReqwestError;

    fn from_str(s: &str) -> Result<OAuth, Self::Err> {
        Ok(OAuth(s.into()))
    }
}

impl Scheme for OAuth {
    fn scheme() -> Option<&'static str> {
        Some("OAuth")
    }

    fn fmt_scheme(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The old v5 API. Deprecated but the New Twitch API is still lacking some features.
pub struct Kraken {
    client: Client,
}

impl Kraken {
    pub fn new(config: Arc<Config>) -> Result<Kraken, Error> {
        let mut default_headers = Headers::new();

        default_headers.set_raw("Client-ID", &config.twitch_client_id[..]);
        default_headers.set_raw("Accept", "application/vnd.twitchtv.v5+json");

        Ok(Kraken {
            client: ClientBuilder::new()
                .default_headers(default_headers)
                .build()?,
        })
    }

    fn paginated_by_offset<T: for<'de> Deserialize<'de>>(
        &self,
        url: &str,
        token: Option<String>,
        key: &str,
    ) -> Result<Vec<T>, Error> {
        let mut data = vec![];

        loop {
            let mut req = self.client.get(url);
            if let Some(ref token) = token {
                req.header(Authorization(OAuth(token.clone())));
            }
            req.query(&[("offset", &format!("{}", data.len())[..]), ("limit", "25")]);
            let value = req
                .send()
                .context("failed to send the request")?
                .json::<Value>()
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
    pub fn get_streams_followed(&self, token: String) -> Result<Vec<Stream>, Error> {
        self.paginated_by_offset(
            "https://api.twitch.tv/kraken/streams/followed",
            Some(token),
            "streams",
        )
    }
}
