use crate::aiomas::Server as AiomasServer;
use crate::announcements;
use crate::config::Config;
use crate::PgPool;
use chrono::{DateTime, FixedOffset};
use failure::{Error, ResultExt};
use serde_derive::Deserialize;
use serde_json::{self, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::runtime::TaskExecutor;

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
    pub fn new(
        config: Arc<Config>,
        pg_pool: PgPool,
        executor: TaskExecutor,
    ) -> Result<Server, Error> {
        #[cfg(unix)]
        let mut server = AiomasServer::new(&config.eris_socket, executor.clone())?;

        #[cfg(not(unix))]
        let mut server = AiomasServer::new(config.eris_port, executor.clone())?;

        {
            let config = config.clone();
            let pg_pool = pg_pool.clone();
            let executor = executor.clone();
            server.register(
                "announcements/stream_up",
                move |mut args: Vec<Value>, kwargs: HashMap<String, Value>| {
                    let config = config.clone();
                    let pg_pool = pg_pool.clone();
                    let executor = executor.clone();

                    async move {
                        if args.len() != 1 || kwargs.len() != 0 {
                            return Err(String::from("invalid number of arguments"));
                        }

                        let data = serde_json::from_value::<Channel>(args.pop().unwrap())
                            .context("failed to deserialize arguments")
                            .map_err(|e| format!("{:?}", e))?;

                        await!(announcements::stream_up(
                            &config,
                            pg_pool,
                            data,
                            executor.clone()
                        ));

                        Ok(serde_json::Value::Null)
                    }
                },
            );
        }

        Ok(Server { server })
    }

    pub async fn serve(self) {
        await!(self.server.serve())
    }
}
