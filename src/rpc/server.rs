use crate::aiomas::Server as AiomasServer;
use crate::announcements;
use crate::config::Config;
use chrono::{DateTime, FixedOffset};
use failure::{Error, ResultExt};
use serde::Deserialize;
use serde_json::{self, Value};
use std::collections::HashMap;
use crate::typemap_keys::Executor;
use crate::extract::Extract;
use crate::context::ErisContext;

#[derive(Deserialize)]
pub struct Channel {
    pub display_name: Option<String>,
    pub game: Option<String>,
    pub name: String,
    pub status: Option<String>,
    pub stream_created_at: Option<DateTime<FixedOffset>>,
    pub live: bool,
    pub url: String,
}

pub struct Server {
    server: AiomasServer,
}

impl Server {
    pub fn new(ctx: &ErisContext) -> Result<Server, Error> {
        let mut server = {
            let data = ctx.data.read();
            let config = data.extract::<Config>()?;
            let executor = data.extract::<Executor>()?.clone();

            #[cfg(unix)]
            let server = AiomasServer::new(&config.eris_socket, executor)?;
            #[cfg(not(unix))]
            let server = AiomasServer::new(config.eris_port, executor)?;

            server
        };

        {
            let ctx = ctx.clone();
            server.register(
                "announcements/stream_up",
                move |mut args: Vec<Value>, kwargs: HashMap<String, Value>| {
                    let ctx = ctx.clone();
                    async move {
                        if args.len() != 1 || kwargs.len() != 0 {
                            return Err(String::from("invalid number of arguments"));
                        }

                        let data = serde_json::from_value::<Channel>(args.pop().unwrap())
                            .context("failed to deserialize arguments")
                            .map_err(|e| format!("{:?}", e))?;

                        announcements::stream_up(&ctx, data).await;

                        Ok(serde_json::Value::Null)
                    }
                },
            );
        }

        Ok(Server { server })
    }

    pub async fn serve(self) {
        self.server.serve().await
    }
}
