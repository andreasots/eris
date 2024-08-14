use std::sync::Arc;

use anyhow::{Context as _, Error};
use sea_orm::{DatabaseConnection, EntityTrait};
use tokio::sync::RwLock;
use tracing::error;
use twilight_http::Client as DiscordClient;
use twilight_model::channel::message::MessageFlags;
use twitch_api::twitch_oauth2::AppAccessToken;
use twitch_api::HelixClient;

use crate::aiomas::server::Route;
use crate::config::Config;
use crate::models::{game, game_entry, show};
use crate::rpc::LRRbot;

async fn stream_up_inner(
    config: &Config,
    db: &DatabaseConnection,
    discord: &DiscordClient,
    helix: &HelixClient<'static, reqwest::Client>,
    helix_token: &RwLock<AppAccessToken>,
    lrrbot: &LRRbot,
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

    let channel = helix
        .get_channel_from_login(&config.channel, &*helix_token.read().await)
        .await
        .context("failed to get the channel")?
        .context("channel does not exist")?;

    let mut message = String::new();
    message.push_str(channel.broadcaster_name.as_str());
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
    if !channel.title.is_empty() {
        message.push_str(" (");
        message.push_str(&crate::markdown::escape(&channel.title));
        message.push(')');
    }
    message.push_str("! <https://twitch.tv/");
    message.push_str(&channel.broadcaster_login.as_str());
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
    helix: HelixClient<'static, reqwest::Client>,
    helix_token: Arc<RwLock<AppAccessToken>>,
    lrrbot: Arc<LRRbot>,
) -> impl Route<()> {
    move || {
        let config = config.clone();
        let db = db.clone();
        let discord = discord.clone();
        let helix = helix.clone();
        let helix_token = helix_token.clone();
        let lrrbot = lrrbot.clone();

        async move {
            stream_up_inner(&config, &db, &discord, &helix, &helix_token, &lrrbot)
                .await
                .inspect_err(|error| error!(?error, "Failed to post a stream up announcement"))
        }
    }
}
