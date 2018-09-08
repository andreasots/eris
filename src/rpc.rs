use std::sync::Arc;
use config::Config;
use failure::{self, Error, ResultExt};
use aiomas::{Client, NewClient};
use std::collections::HashMap;
use serde_json::{self, Value};
use serde::{Deserialize, Deserializer};
use tokio::prelude::*;
use tower_service::Service;

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
/*
pub struct LRRbot {
    service: Reconnect<NewClient>,
}

impl LRRbot {
    pub fn new(config: Arc<Config>) -> LRRbot {
        #[cfg(unix)]
        let client = Client::new(&config.lrrbot_socket);

        #[cfg(not(unix))]
        let client = Client::new(&config.lrrbot_port);

        LRRbot {
            service: client,
        }
    }

    pub fn ready(self) -> impl Future<Item=LRRbot, Error=Error> {
        self.service.ready().map(|service| LRRbot { service }).map_err(from_reconnect_error)
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
*/