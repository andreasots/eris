use serde_derive::Deserialize;
use chrono::{DateTime, FixedOffset};
use crate::aiomas::Server as AiomasServer;
use failure::{Error, ResultExt};
use crate::config::Config;
use crate::announcements;
use std::sync::Arc;
use serde_json::{self, Value};
use std::collections::HashMap;
use crate::PgPool;

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
    pub fn new(config: Arc<Config>, pg_pool: PgPool) -> Result<Server, Error> {
        #[cfg(unix)]
        let mut server = AiomasServer::new(&config.eris_socket)?;

        #[cfg(not(unix))]
        let mut server = AiomasServer::new(config.eris_port)?;

        {
            let config = config.clone();
            let pg_pool = pg_pool.clone();
            server.register("announcements/stream_up", move |mut args: Vec<Value>, kwargs: HashMap<String, Value>| {
                let config = config.clone();
                let pg_pool = pg_pool.clone();

                async move {
                    if args.len() != 1 || kwargs.len() != 0 {
                        return Err(String::from("invalid number of arguments"))
                    }

                    let data = serde_json::from_value::<Channel>(args.pop().unwrap())
                        .context("failed to deserialize arguments")
                        .map_err(|e| format!("{:?}", e))?;

                    await!(announcements::stream_up(&config, pg_pool, data));

                    Ok(serde_json::Value::Null)
                }
            });
        }

        Ok(Server {
            server,
        })
    }

    pub async fn serve(self) {
        await!(self.server.serve())
    }
}
