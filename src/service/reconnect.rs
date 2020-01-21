use crate::aiomas::{Client, Exception, NewClient, Request};
use anyhow::{Context, Error};
use futures::channel::oneshot;
use futures::lock::Mutex;
use serde_json::Value;

// FIXME: this should be generic over the factory but that requires generic associated types and
//  existential types.
pub struct Reconnect {
    factory: NewClient,
    client: Mutex<Option<Client>>,
}

impl Reconnect {
    pub fn new(factory: NewClient) -> Reconnect {
        Reconnect { factory, client: Mutex::new(None) }
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
            let mut client = self.factory.new_client().await?;

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
