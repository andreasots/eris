use crate::aiomas::{Exception, Request};
use crate::service::Reconnect;
use failure::{self, Error};
use serde_json::Value;

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

    pub async fn call(&mut self, req: Request) -> Result<Result<Value, Exception>, Error> {
        let mut error = None;

        for _ in 0..self.max_count {
            match await!(self.service.call(req.clone())) {
                Ok(res) => return Ok(res),
                Err(err) => error = Some(err),
            }
        }

        Err(error.unwrap_or_else(|| failure::err_msg("max count is 0")))
    }
}
