use crate::config::Config;
use crate::models::{Game, GameEntry, Show};
use crate::rpc::server::Channel;
use crate::rpc::LRRbot;
use crate::PgPool;
use diesel::OptionalExtension;
use failure::{Error, ResultExt, SyncFailure};
use slog::slog_error;
use slog_scope::error;
use std::fmt::{self, Display};
use tokio::runtime::TaskExecutor;

struct StreamUp {
    data: Channel,
    what: String,
}

impl Display for StreamUp {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.data.display_name.as_ref().unwrap_or(&self.data.name))?;
        f.write_str(" is live with ")?;
        f.write_str(&self.what)?;
        if let Some(ref status) = self.data.status {
            f.write_str(" (")?;
            f.write_str(&status)?;
            f.write_str(")")?;
        }
        f.write_str("! <")?;
        f.write_str(&self.data.url)?;
        f.write_str(">")?;
        Ok(())
    }
}

async fn stream_up_inner<'a>(
    config: &'a Config,
    lrrbot: &'a mut LRRbot,
    pg_pool: PgPool,
    data: Channel,
) -> Result<(), Error> {
    let game_id = await!(lrrbot.get_game_id()).context("failed to get the game ID")?;
    let show_id = await!(lrrbot.get_show_id()).context("failed to get the show ID")?;

    let conn = pg_pool
        .get()
        .context("failed to get a database connection from the pool")?;

    let game = game_id
        .map(|game_id| Game::find(game_id, &conn))
        .transpose()
        .context("failed to load the game")?;
    let show = Show::find(show_id, &conn).context("failed to load the show")?;
    let game_entry = game_id
        .map(|game_id| GameEntry::find(game_id, show_id, &conn))
        .transpose()
        .optional()
        .context("failed to load the game entry")?
        .and_then(|entry| entry);

    let what = {
        let game = game.as_ref();
        let game_entry = game_entry.as_ref();
        let game_display_name = game.map(|game| {
            game_entry
                .and_then(|entry| entry.display_name.as_ref())
                .unwrap_or(&game.name)
        });

        game_display_name
            .map(|name| format!("{} on {}", name, show.name))
            .unwrap_or(show.name)
    };

    config
        .announcements
        .say(format_args!("{}", StreamUp { data, what }))
        .map_err(SyncFailure::new)
        .context("failed to send the annoucement message")?;

    Ok(())
}

pub async fn stream_up(config: &Config, pg_pool: PgPool, data: Channel, executor: TaskExecutor) {
    let mut lrrbot = LRRbot::new(config, executor);

    match await!(stream_up_inner(config, &mut lrrbot, pg_pool, data)) {
        Ok(()) => (),
        Err(err) => error!("Failed to post a stream up announcement"; "error" => ?err),
    }
}
