use crate::config::Config;
use anyhow::{Context, Error};
use chrono::{DateTime, FixedOffset};
use reqwest::header::HeaderValue;
use reqwest::Client;
use serde::Deserialize;

#[derive(Copy, Clone, Debug)]
pub enum UserId<'a> {
    Id(&'a str),
    Login(&'a str),
}

#[derive(Copy, Clone, Debug)]
pub enum GameId<'a> {
    Id(&'a str),
    // Name(&'a str),
}

#[derive(Clone, Debug, Deserialize)]
pub struct Stream {
    pub game_id: String,
    pub started_at: DateTime<FixedOffset>,
    pub title: String,
    pub user_id: String,
    pub user_name: String,
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

#[derive(Clone, Debug, Deserialize)]
pub struct User {
    pub id: String,
    pub login: String,
    pub display_name: String,
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

trait FillParams {
    fn fill_params<'a>(&'a self, params: &mut Vec<(&'a str, &'a str)>);
}

impl FillParams for UserId<'_> {
    fn fill_params<'a>(&'a self, params: &mut Vec<(&'a str, &'a str)>) {
        match *self {
            UserId::Id(id) => params.push(("id", id)),
            UserId::Login(login) => params.push(("login", login)),
        }
    }
}

struct Prefixed<T>(T);

impl FillParams for Prefixed<UserId<'_>> {
    fn fill_params<'a>(&'a self, params: &mut Vec<(&'a str, &'a str)>) {
        match self.0 {
            UserId::Id(id) => params.push(("user_id", id)),
            UserId::Login(login) => params.push(("user_login", login)),
        }
    }
}

impl FillParams for GameId<'_> {
    fn fill_params<'a>(&'a self, params: &mut Vec<(&'a str, &'a str)>) {
        match *self {
            GameId::Id(id) => params.push(("id", id)),
            // GameId::Name(name) => params.push(("name", name)),
        }
    }
}

impl<T: FillParams + ?Sized> FillParams for &T {
    fn fill_params<'a>(&'a self, params: &mut Vec<(&'a str, &'a str)>) {
        (*self).fill_params(params)
    }
}

impl<T: FillParams> FillParams for [T] {
    fn fill_params<'a>(&'a self, params: &mut Vec<(&'a str, &'a str)>) {
        for elem in self {
            elem.fill_params(params);
        }
    }
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

    async fn paginated<F: FillParams, T: for<'de> Deserialize<'de>>(
        &self,
        url: &str,
        token: &str,
        params: F,
    ) -> Result<Vec<T>, Error> {
        let mut result = vec![];
        let mut after = None::<String>;

        loop {
            let mut req_params = vec![];
            params.fill_params(&mut req_params);
            req_params.push(("first", "100"));
            if let Some(after) = after.as_ref() {
                req_params.push(("after", after));
            }

            let mut response = self
                .client
                .get(url)
                .query(&req_params)
                .header("Client-ID", self.client_id.clone())
                .bearer_auth(token)
                .send()
                .await
                .context("failed to send the request")?
                .error_for_status()
                .context("request failed")?
                .json::<PaginatedResponse<T>>()
                .await
                .context("failed to read the response")?;

            result.extend(response.data.drain(..));

            if let Some(cursor) = response.pagination.and_then(|p| p.cursor) {
                after = Some(cursor);
            } else {
                break;
            }
        }

        Ok(result)
    }

    async fn lookup<I: FillParams, T: for<'de> Deserialize<'de>>(
        &self,
        url: &str,
        token: &str,
        ids: &[I],
    ) -> Result<Vec<T>, Error> {
        let mut result = vec![];

        // FIXME: do it in parallel. Blocked by rust-lang/rust#64552.
        for chunk in ids.chunks(100) {
            result.extend(self.paginated(url, token, chunk).await?);
        }

        Ok(result)
    }

    pub async fn get_streams(
        &self,
        token: &str,
        users: &[UserId<'_>],
    ) -> Result<Vec<Stream>, Error> {
        let users = users.iter().copied().map(Prefixed).collect::<Vec<_>>();
        self.lookup("https://api.twitch.tv/helix/streams", token, &users[..]).await
    }

    pub async fn get_user_follows(
        &self,
        token: &str,
        from_id: Option<&str>,
        to_id: Option<&str>,
    ) -> Result<Vec<Follow>, Error> {
        struct Params<'a> {
            from_id: Option<&'a str>,
            to_id: Option<&'a str>,
        }

        impl FillParams for Params<'_> {
            fn fill_params<'a>(&'a self, params: &mut Vec<(&'a str, &'a str)>) {
                if let Some(from_id) = self.from_id {
                    params.push(("from_id", from_id));
                }
                if let Some(to_id) = self.to_id {
                    params.push(("to_id", to_id));
                }
            }
        }

        self.paginated(
            "https://api.twitch.tv/helix/users/follows",
            token,
            Params { from_id, to_id },
        )
        .await
    }

    pub async fn get_games(&self, token: &str, games: &[GameId<'_>]) -> Result<Vec<Game>, Error> {
        self.lookup("https://api.twitch.tv/helix/games", token, games).await
    }

    pub async fn get_users(&self, token: &str, users: &[UserId<'_>]) -> Result<Vec<User>, Error> {
        self.lookup("https://api.twitch.tv/helix/users", token, users).await
    }
}
