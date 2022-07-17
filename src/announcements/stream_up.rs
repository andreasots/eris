use crate::config::Config;
use crate::context::ErisContext;
use crate::extract::Extract;
use crate::models::game;
use crate::models::game_entry;
use crate::models::show;
use crate::rpc::LRRbot;
use crate::try_crosspost::TryCrosspost;
use crate::typemap_keys::PgPool;
use anyhow::{Context, Error};
use eris_macros::rpc_handler;
use sea_orm::EntityTrait;
use serde::Deserialize;
use std::fmt::{self, Display};
use tracing::error;

#[derive(Deserialize)]
pub struct Channel {
    pub display_name: Option<String>,
    pub login: String,
    pub status: Option<String>,
    pub live: bool,
}

struct StreamUp {
    channel: Channel,
    what: String,
}

impl Display for StreamUp {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.channel.display_name.as_ref().unwrap_or(&self.channel.login))?;
        f.write_str(" is live with ")?;
        f.write_str(&self.what)?;
        if let Some(ref status) = self.channel.status {
            f.write_str(" (")?;
            f.write_str(&status)?;
            f.write_str(")")?;
        }
        f.write_str("! <https://twitch.tv/")?;
        f.write_str(&self.channel.login)?;
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
        let data = ctx.data.read().await;
        let conn = data.extract::<PgPool>()?;

        let (game, game_entry) = if let Some(game_id) = game_id {
            (
                game::Entity::find_by_id(game_id)
                    .one(conn)
                    .await
                    .context("failed to load the game")?,
                game_entry::Entity::find_by_id((game_id, show_id))
                    .one(conn)
                    .await
                    .context("failed to load the game entry")?,
            )
        } else {
            (None, None)
        };
        let show =
            show::Entity::find_by_id(show_id).one(conn).await.context("failed to load the show")?;

        (game, show, game_entry)
    };

    let what = {
        let game = game.as_ref();
        let game_entry = game_entry.as_ref();
        let game_display_name = game.map(|game| {
            game_entry.and_then(|entry| entry.display_name.as_ref()).unwrap_or(&game.name)
        });

        if let Some(show) = show {
            game_display_name.map(|name| format!("{} on {}", name, show.name)).unwrap_or(show.name)
        } else {
            game_display_name.cloned().unwrap_or_else(|| "nothing".to_string())
        }
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
