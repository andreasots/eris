use crate::aiomas::NewClient;
use crate::config::Config;
use crate::service::{Reconnect, Retry};
use anyhow::{Context, Error};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Copy, Clone, Debug, Deserialize)]
pub struct GameId {
    pub id: i32,
    pub is_override: bool,
}

#[derive(Copy, Clone, Debug, Deserialize)]
pub struct ShowId {
    pub id: i32,
    pub is_override: bool,
}

fn option_bool_as_bool<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<bool>::deserialize(deserializer)?.unwrap_or(false))
}

#[derive(Clone, Debug, Deserialize)]
pub struct HeaderInfo {
    #[serde(deserialize_with = "option_bool_as_bool")]
    pub is_live: bool,
    pub channel: String,
    pub current_game: Option<GameId>,
    pub current_show: Option<ShowId>,
    pub advice: Option<String>,
}

pub struct LRRbot {
    service: Retry,
}

impl LRRbot {
    pub fn new(config: &Config) -> LRRbot {
        #[cfg(unix)]
        let client = NewClient::new(&config.lrrbot_socket);
        #[cfg(not(unix))]
        let client = NewClient::new(&config.lrrbot_port);

        LRRbot { service: Retry::new(Reconnect::new(client), 3) }
    }

    async fn call(
        &self,
        name: String,
        args: Vec<Value>,
        kwargs: HashMap<String, Value>,
    ) -> Result<Value, Error> {
        self.service.call((name, args, kwargs)).await?.map_err(Error::msg)
    }

    pub async fn get_header_info(&self) -> Result<HeaderInfo, Error> {
        let value = self.call("get_header_info".into(), vec![], HashMap::new()).await?;
        Ok(serde_json::from_value(value).context("failed to deserialize the response")?)
    }

    pub async fn get_game_id(&self) -> Result<Option<i32>, Error> {
        let value = self.call("get_game_id".into(), vec![], HashMap::new()).await?;
        Ok(serde_json::from_value(value).context("failed to deserialize the response")?)
    }

    pub async fn get_show_id(&self) -> Result<i32, Error> {
        let value = self.call("get_show_id".into(), vec![], HashMap::new()).await?;
        Ok(serde_json::from_value(value).context("failed to deserialize the response")?)
    }

    pub async fn get_data<T: DeserializeOwned>(&self, path: Vec<String>) -> Result<T, Error> {
        let value = self
            .call(
                "get_data".into(),
                vec![Value::Array(path.into_iter().map(Value::String).collect())],
                HashMap::new(),
            )
            .await?;
        Ok(serde_json::from_value(value).context("failed to deserialize the response")?)
    }
}
