use anyhow::Error;
use serde_json::Value;

use crate::aiomas::{Exception, Request};
use crate::service::Reconnect;

// FIXME: this should be generic over the service but that requires generic associated types and
// existential types.
pub struct Retry {
    service: Reconnect,
    max_count: usize,
}

impl Retry {
    pub fn new(service: Reconnect, max_count: usize) -> Retry {
        Retry { service, max_count }
    }

    pub async fn call(&self, req: Request) -> Result<Result<Value, Exception>, Error> {
        let mut error = None;

        for _ in 0..self.max_count {
            match self.service.call(req.clone()).await {
                Ok(res) => return Ok(res),
                Err(err) => error = Some(err),
            }
        }

        Err(error.unwrap_or_else(|| Error::msg("max count is 0")))
    }
}
