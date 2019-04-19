use crate::aiomas::{Client, Exception, NewClient, Request};
use failure::Error;
use serde_json::Value;
use crate::rwlock::RwLock;

// FIXME: this should be generic over the factory but that requires generic associated types and
//  existential types.
pub struct Reconnect {
    factory: NewClient,
    client: RwLock<Option<Client>>,
}

impl Reconnect {
    pub fn new(factory: NewClient) -> Reconnect {
        Reconnect {
            factory,
            client: RwLock::new(None),
        }
    }

    pub async fn call(&self, req: Request) -> Result<Result<Value, Exception>, Error> {
        {
            let guard = self.client.read().await;

            if let Some(client) = &*guard {
                match client.call(req).await {
                    Ok(res) => return Ok(res),
                    Err(err) => {
                        drop(guard);
                        *self.client.write().await = None;
                        return Err(err);
                    }
                }
            }
        }

        let mut client_guard = self.client.write().await;
        let client = self.factory.new_client().await?;

        let res = client.call(req).await;

        if res.is_ok() {
            *client_guard = Some(client);
        } else {
            *client_guard = None;
        }

        res
    }
}
