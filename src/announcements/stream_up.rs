use crate::config::Config;
use crate::context::ErisContext;
use crate::extract::Extract;
use crate::models::{Game, GameEntry, Show};
use crate::rpc::LRRbot;
use crate::try_crosspost::TryCrosspost;
use crate::typemap_keys::PgPool;
use anyhow::{Context, Error};
use chrono::{DateTime, FixedOffset};
use diesel::OptionalExtension;
use eris_macros::rpc_handler;
use serde::Deserialize;
use std::fmt::{self, Display};
use tracing::error;

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

struct StreamUp {
    channel: Channel,
    what: String,
}

impl Display for StreamUp {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.channel.display_name.as_ref().unwrap_or(&self.channel.name))?;
        f.write_str(" is live with ")?;
        f.write_str(&self.what)?;
        if let Some(ref status) = self.channel.status {
            f.write_str(" (")?;
            f.write_str(&status)?;
            f.write_str(")")?;
        }
        f.write_str("! <")?;
        f.write_str(&self.channel.url)?;
        f.write_str(">")?;
        Ok(())
    }
}

async fn stream_up_inner(ctx: &ErisContext, channel: Channel) -> Result<(), Error> {
    let data = ctx.data.read().await;
    let lrrbot = data.extract::<LRRbot>()?;
    let announcements_channel = data.extract::<Config>()?.announcements;

    let game_id = lrrbot.get_game_id().await.context("failed to get the game ID")?;
    let show_id = lrrbot.get_show_id().await.context("failed to get the show ID")?;

    let (game, show, game_entry) = {
        let conn = ctx
            .data
            .read()
            .await
            .extract::<PgPool>()?
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

        (game, show, game_entry)
    };

    let what = {
        let game = game.as_ref();
        let game_entry = game_entry.as_ref();
        let game_display_name = game.map(|game| {
            game_entry.and_then(|entry| entry.display_name.as_ref()).unwrap_or(&game.name)
        });

        game_display_name.map(|name| format!("{} on {}", name, show.name)).unwrap_or(show.name)
    };

    announcements_channel
        .say(ctx, format!("{}", StreamUp { channel, what }))
        .await
        .context("failed to send the announcement message")?
        .try_crosspost(ctx)
        .await
        .context("failed to crosspost the announcement message")?;

    Ok(())
}

#[rpc_handler("announcements/stream_up")]
pub async fn stream_up(ctx: ErisContext, data: Channel) -> Result<(), Error> {
    let res = stream_up_inner(&ctx, data).await;

    if let Err(ref error) = res {
        error!(?error, "Failed to post a stream up announcement");
    }

    res
}
