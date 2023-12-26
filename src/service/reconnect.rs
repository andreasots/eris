use anyhow::{Context, Error};
use serde_json::Value;
use tokio::sync::{oneshot, Mutex};

use crate::aiomas::{Client, Connector, Exception, Request};

// FIXME: this should be generic over the connector but that requires Rust 1.75
pub struct Reconnect {
    connector: Connector,
    client: Mutex<Option<Client>>,
}

impl Reconnect {
    pub fn new(factory: Connector) -> Reconnect {
        Reconnect { connector: factory, client: Mutex::new(None) }
    }

    async fn call_inner(
        &self,
        req: Request,
    ) -> Result<oneshot::Receiver<Result<Value, Exception>>, Error> {
        let mut client_guard = self.client.lock().await;

        if let Some(client) = &mut *client_guard {
            let res = client.call(req).await;

            if res.is_err() {
                *client_guard = None;
            }

            res
        } else {
            let mut client = self.connector.connect().await?;

            let res = client.call(req).await;

            if res.is_ok() {
                *client_guard = Some(client);
            } else {
                *client_guard = None;
            }

            res
        }
    }

    pub async fn call(&self, req: Request) -> Result<Result<Value, Exception>, Error> {
        let res = self.call_inner(req).await?.await.context("client disconnected mid-request")?;
        Ok(res)
    }
}
