use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Error};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use tokio::sync::mpsc::Sender;
use tokio::sync::watch::Receiver;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tower::reconnect::Reconnect;
use tower::Service;

use crate::aiomas::{Client, MakeClient, Request};
use crate::config::Config;

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
    #[cfg(unix)]
    service: Mutex<Reconnect<MakeClient, PathBuf>>,
    #[cfg(not(unix))]
    service: Mutex<Reconnect<MakeClient, u16>>,
}

impl LRRbot {
    pub fn new(
        running: Receiver<bool>,
        handler_tx: Sender<JoinHandle<()>>,
        config: &Config,
    ) -> LRRbot {
        let make_client = MakeClient::new(running, handler_tx);

        #[cfg(unix)]
        let addr = config.lrrbot_socket.clone();
        #[cfg(not(unix))]
        let addr = config.lrrbot_port;

        LRRbot { service: Mutex::new(Reconnect::new::<Client, Request>(make_client, addr)) }
    }

    async fn call(
        &self,
        name: String,
        args: Vec<Value>,
        kwargs: HashMap<String, Value>,
    ) -> Result<Value, Error> {
        // Implement retry logic here because `tower::retry::Retry` requires the service to be `Clone` which
        // `Reconnect<...>` never is.
        let mut last_error = None;

        for _ in 0..3 {
            let future = {
                let mut service = self.service.lock().await;
                if let Err(error) = std::future::poll_fn(|cx| service.poll_ready(cx)).await {
                    last_error = Some(
                        anyhow::anyhow!(error)
                            .context("failed to wait for the service to be ready"),
                    );
                    continue;
                }
                service.call((name.clone(), args.clone(), kwargs.clone()))
            };
            match future.await {
                Ok(Ok(value)) => return Ok(value),
                Ok(Err(exc)) => return Err(Error::msg(exc)),
                Err(error) => {
                    last_error = Some(anyhow::anyhow!(error).context("failed to send the request"));
                    continue;
                }
            }
        }

        Err(last_error.unwrap())
    }

    pub async fn get_header_info(&self) -> Result<HeaderInfo, Error> {
        let value = self.call("get_header_info".into(), vec![], HashMap::new()).await?;
        serde_json::from_value(value).context("failed to deserialize the response")
    }

    pub async fn get_game_id(&self) -> Result<Option<i32>, Error> {
        let value = self.call("get_game_id".into(), vec![], HashMap::new()).await?;
        serde_json::from_value(value).context("failed to deserialize the response")
    }

    pub async fn get_show_id(&self) -> Result<i32, Error> {
        let value = self.call("get_show_id".into(), vec![], HashMap::new()).await?;
        serde_json::from_value(value).context("failed to deserialize the response")
    }
}
