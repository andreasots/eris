use std::sync::Arc;

use anyhow::{Context as _, Error};
use sea_orm::{DatabaseConnection, EntityTrait};
use serde::Deserialize;
use tracing::error;
use twilight_http::Client as DiscordClient;
use twilight_model::channel::message::MessageFlags;

use crate::aiomas::Route;
use crate::config::Config;
use crate::models::{game, game_entry, show};
use crate::rpc::LRRbot;

#[derive(Deserialize)]
pub struct Channel {
    display_name: Option<String>,
    login: String,
    title: Option<String>,
}

async fn stream_up_inner(
    config: &Config,
    db: &DatabaseConnection,
    discord: &DiscordClient,
    lrrbot: &LRRbot,

    channel: Channel,
) -> Result<(), Error> {
    let game_id = lrrbot.get_game_id().await.context("failed to get the game ID")?;
    let show_id = lrrbot.get_show_id().await.context("failed to get the show ID")?;

    let (game, show, game_entry) = {
        let (game, game_entry) = if let Some(game_id) = game_id {
            (
                game::Entity::find_by_id(game_id)
                    .one(db)
                    .await
                    .context("failed to load the game")?,
                game_entry::Entity::find_by_id((game_id, show_id))
                    .one(db)
                    .await
                    .context("failed to load the game entry")?,
            )
        } else {
            (None, None)
        };
        let show =
            show::Entity::find_by_id(show_id).one(db).await.context("failed to load the show")?;

        (game, show, game_entry)
    };

    let mut message = String::new();
    message.push_str(channel.display_name.as_ref().unwrap_or(&channel.login));
    message.push_str(" is live with ");
    {
        let game = game.as_ref();
        let game_entry = game_entry.as_ref();
        let game_display_name = game.map(|game| {
            game_entry.and_then(|entry| entry.display_name.as_deref()).unwrap_or(&game.name)
        });

        match (game_display_name, show.as_ref()) {
            (Some(game), Some(show)) => {
                message.push_str(game);
                message.push_str(" on ");
                message.push_str(&show.name);
            }
            (Some(game), None) => message.push_str(game),
            (None, Some(show)) => message.push_str(&show.name),
            (None, None) => message.push_str("nothing"),
        }
    }
    if let Some(title) = channel.title.as_deref().filter(|s| !s.is_empty()) {
        message.push_str(" (");
        message.push_str(&crate::markdown::escape(title));
        message.push(')');
    }
    message.push_str("! <https://twitch.tv/");
    message.push_str(&channel.login);
    message.push('>');

    let message = discord
        .create_message(config.announcements)
        .flags(MessageFlags::SUPPRESS_EMBEDS)
        .content(&message)
        .await
        .context("failed to send the announcement message request")?
        .model()
        .await
        .context("failed to parse the annoucement message response")?;

    if let Err(error) = discord.crosspost_message(message.channel_id, message.id).await {
        error!(?error, "failed to crosspost the stream up announcement");
    }

    Ok(())
}

pub fn stream_up(
    config: Arc<Config>,
    db: DatabaseConnection,
    discord: Arc<DiscordClient>,
    lrrbot: Arc<LRRbot>,
) -> impl Route<(Channel,)> {
    move |data| {
        let config = config.clone();
        let db = db.clone();
        let discord = discord.clone();
        let lrrbot = lrrbot.clone();

        async move {
            let ret = stream_up_inner(&config, &db, &discord, &lrrbot, data).await;
            // `Result::inspect_err` is unstable :(
            if let Err(ref error) = ret {
                error!(?error, "Failed to post a stream up announcement");
            }
            ret
        }
    }
}
