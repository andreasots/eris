use std::sync::Arc;
use crate::config::Config;
use failure::{self, Error, ResultExt};
use crate::aiomas::{NewClient, Reconnect, Retry};
use std::collections::HashMap;
use serde_json::{self, Value};
use serde::{Deserialize, Deserializer};
use tokio::prelude::*;
use serde_derive::Deserialize;

#[derive(Copy, Clone, Debug, Deserialize)]
pub struct GameId {
    pub id: i64,
    pub is_override: bool,
}

#[derive(Copy, Clone, Debug, Deserialize)]
pub struct ShowId {
    pub id: i64,
    pub is_override: bool,
}

fn option_bool_as_bool<'de, D>(deserializer: D) -> Result<bool, D::Error> where D: Deserializer<'de> {
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
    pub fn new(config: Arc<Config>) -> LRRbot {
        #[cfg(unix)]
        let client = NewClient::new(&config.lrrbot_socket);

        #[cfg(not(unix))]
        let client = NewClient::new(&config.lrrbot_port);

        LRRbot {
            service: Retry::new(Reconnect::new(client), 3),
        }
    }

    fn call(&mut self, name: String, args: Vec<Value>, kwargs: HashMap<String, Value>) -> impl Future<Item=Value, Error=Error> {
        self.service.call((name, args, kwargs)).map_err(from_reconnect_error)
            .and_then(|res| res.map_err(failure::err_msg))
    }

    pub fn get_header_info(&mut self) -> impl Future<Item=HeaderInfo, Error=Error> {
        self.call("get_header_info".into(), vec![], HashMap::new())
            .and_then(|value| Ok(serde_json::from_value(value).context("failed to deserialize the response")?))
    }
}
