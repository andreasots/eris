//! Creating OAuth2 bearer tokens for Google service accounts

use chrono::{DateTime, Duration, TimeZone, Utc};
use failure::{Error, ResultExt};
use futures::lock::Mutex;
use jsonwebtoken::{Algorithm, Header};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

const TOKEN_URI: &str = "https://www.googleapis.com/oauth2/v4/token";

/// Type of a service account key JSON. There are more fields but we're only interested in these.
#[derive(Deserialize)]
struct ServiceAccountKey {
    private_key: String,
    client_email: String,
}

#[derive(Serialize)]
struct Claims<'a> {
    iss: &'a str,
    scope: &'a str,
    aud: &'a str,
    iat: i64,
    exp: i64,
}

#[derive(Deserialize)]
struct NewToken {
    access_token: String,
    token_type: String,
    expires_in: i64,
}

struct Token {
    token: String,
    expires: DateTime<Utc>,
}

pub struct ServiceAccount {
    client: Client,
    key_path: PathBuf,
    scopes: String,
    token: Mutex<Token>,
}

impl ServiceAccount {
    pub fn new(key_path: PathBuf, client: Client, scopes: &[&str]) -> ServiceAccount {
        ServiceAccount {
            key_path,
            client,
            scopes: scopes.join(" "),
            token: Mutex::new(Token {
                token: String::new(),
                expires: Utc.timestamp(0, 0),
            }),
        }
    }

    pub async fn get_token(&self) -> Result<String, Error> {
        let mut token = self.token.lock().await;
        let now = Utc::now();

        if token.expires <= now {
            let mut file = File::open(&self.key_path)
                .await
                .context("failed to open the service account key JSON file")?;
            let mut content = vec![];
            file.read_to_end(&mut content)
                .await
                .context("failed to read the service account key JSON file")?;
            let key = serde_json::from_slice::<'_, ServiceAccountKey>(&content)
                .context("failed to parse the service account key JSON")?;

            let jwt = jsonwebtoken::encode(
                &Header::new(Algorithm::RS256),
                &Claims {
                    iss: &key.client_email,
                    scope: &self.scopes,
                    aud: TOKEN_URI,
                    iat: now.timestamp(),
                    exp: (now + Duration::seconds(3600)).timestamp(),
                },
                key.private_key.as_bytes(),
            )
            .context("failed to create a JWT token")?;

            let new_token = self
                .client
                .post(TOKEN_URI)
                .form(&[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                    ("assertion", &jwt),
                ])
                .send()
                .await
                .context("failed to request a OAuth2 token")?
                .error_for_status()
                .context("request failed")?
                .json::<NewToken>()
                .await
                .context("failed to read the response")?;
            if new_token.token_type != "Bearer" {
                return Err(failure::err_msg(format!(
                    "{:?} token returned, expected Bearer",
                    new_token.token_type
                )));
            }
            *token = Token {
                token: new_token.access_token,
                expires: Utc::now() + Duration::seconds(new_token.expires_in),
            }
        }

        Ok(token.token.clone())
    }
}
