use crate::aiomas::NewClient;
use crate::config::Config;
use crate::service::{Reconnect, Retry};
use failure::{self, Error, ResultExt};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::{self, Value};
use std::collections::HashMap;
use tokio::runtime::TaskExecutor;

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
    pub fn new(config: &Config, executor: TaskExecutor) -> LRRbot {
        #[cfg(unix)]
        let client = NewClient::new(&config.lrrbot_socket, executor);

        #[cfg(not(unix))]
        let client = NewClient::new(&config.lrrbot_port, executor);

        LRRbot {
            service: Retry::new(Reconnect::new(client), 3),
        }
    }

    // FIXME: `&mut self` prevents sending multiple requests at once. Maybe return the channel from
    // `aiomas::Client::call` so that even though sending still requires a `&mut self` receiving
    // doesn't and so multiple requests can be in flight at once.
    async fn call(
        &mut self,
        name: String,
        args: Vec<Value>,
        kwargs: HashMap<String, Value>,
    ) -> Result<Value, Error> {
        await!(self.service.call((name, args, kwargs)))?.map_err(failure::err_msg)
    }

    pub async fn get_header_info(&mut self) -> Result<HeaderInfo, Error> {
        let value = await!(self.call("get_header_info".into(), vec![], HashMap::new()))?;
        Ok(serde_json::from_value(value).context("failed to deserialize the response")?)
    }

    pub async fn get_game_id(&mut self) -> Result<Option<i32>, Error> {
        let value = await!(self.call("get_game_id".into(), vec![], HashMap::new()))?;
        Ok(serde_json::from_value(value).context("failed to deserialize the response")?)
    }

    pub async fn get_show_id(&mut self) -> Result<i32, Error> {
        let value = await!(self.call("get_show_id".into(), vec![], HashMap::new()))?;
        Ok(serde_json::from_value(value).context("failed to deserialize the response")?)
    }

    pub async fn get_data<T: DeserializeOwned>(&mut self, path: Vec<String>) -> Result<T, Error> {
        let value = await!(self.call(
            "get_data".into(),
            vec![Value::Array(path.into_iter().map(Value::String).collect())],
            HashMap::new()
        ))?;
        Ok(serde_json::from_value(value).context("failed to deserialize the response")?)
    }
}
