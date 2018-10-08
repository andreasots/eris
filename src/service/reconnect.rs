use crate::aiomas::{Client, Exception, NewClient, Request};
use failure::Error;
use serde_json::Value;

// FIXME: this should be generic over the factory but that requires generic associated types and
// existential types.
pub struct Reconnect {
    factory: NewClient,
    client: Option<Client>,
}

impl Reconnect {
    pub fn new(factory: NewClient) -> Reconnect {
        Reconnect {
            factory,
            client: None,
        }
    }

    pub async fn call(&mut self, req: Request) -> Result<Result<Value, Exception>, Error> {
        if self.client.is_none() {
            self.client = Some(await!(self.factory.new_client())?);
        }

        let res = await!(self.client.as_mut().unwrap().call(req));

        if res.is_err() {
            self.client = None;
        }

        res
    }
}
