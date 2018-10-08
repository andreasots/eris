use crate::config::Config;
use crate::models::User;
use crate::twitch::Kraken;
use crate::PgPool;
use diesel::prelude::*;
use futures::prelude::*;
use serenity::framework::standard::{Args, Command, CommandError};
use serenity::model::prelude::*;
use serenity::prelude::*;
use std::sync::Arc;
use tokio::runtime::current_thread::Runtime;

pub struct Live {
    config: Arc<Config>,
    kraken: Kraken,
    pg_pool: PgPool,
}

impl Live {
    pub fn new(config: Arc<Config>, pg_pool: PgPool, kraken: Kraken) -> Live {
        Live {
            config,
            pg_pool,
            kraken,
        }
    }
}

impl Command for Live {
    fn execute(&self, _: &mut Context, msg: &Message, _: Args) -> Result<(), CommandError> {
        let token = {
            use crate::schema::users::dsl::*;

            let conn = self.pg_pool.get()?;

            users
                .filter(name.eq(&self.config.username))
                .first::<User>(&conn)?
                .twitch_oauth
                .ok_or("Twitch token missing")?
        };

        let mut runtime = Runtime::new()?;

        match runtime.block_on(self.kraken.get_streams_followed(token).boxed().compat()) {
            Ok(ref streams) if streams.len() == 0 => {
                msg.reply("No fanstreamers currently live.")?;
            }
            Ok(mut streams) => {
                streams.sort_by(|a, b| {
                    a.channel
                        .display_name
                        .as_ref()
                        .unwrap_or(&a.channel.name)
                        .cmp(b.channel.display_name.as_ref().unwrap_or(&b.channel.name))
                });
                let streams = streams
                    .into_iter()
                    .map(|stream| {
                        let display_name = stream
                            .channel
                            .display_name
                            .as_ref()
                            .unwrap_or(&stream.channel.name);
                        let mut output = format!(
                            "{} (<{}>)",
                            markdown_escape(display_name),
                            stream.channel.url
                        );
                        if let Some(game) = stream.game {
                            output += &format!(" is playing {}", markdown_escape(&game));
                        }
                        if let Some(status) = stream.channel.status {
                            output += &format!(" ({})", markdown_escape(&status));
                        }

                        output
                    })
                    .collect::<Vec<String>>();
                msg.reply(&format!(
                    "Currently live fanstreamers: {}",
                    streams.join(", ")
                ))?;
            }
            Err(_err) => {
                msg.reply("Failed to load fanstreams currently live.")?;
            }
        }
        Ok(())
    }
}

fn markdown_escape(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '_' | '*' | '<' | '`' => vec!['\\', c],
            '#' | '@' => vec![c, '\u{200B}'],
            c => vec![c],
        })
        .collect()
}
