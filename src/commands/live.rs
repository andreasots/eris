use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

use anyhow::{Context as _, Error};
use futures::TryStreamExt;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use twilight_cache_inmemory::InMemoryCache;
use twilight_http::Client as DiscordClient;
use twilight_model::channel::message::MessageFlags;
use twilight_model::channel::Message;
use twitch_api::helix::streams::Stream;
use twitch_api::twitch_oauth2::{AccessToken, UserToken};
use twitch_api::HelixClient;

use crate::command_parser::{Args, CommandHandler, Commands, Help};
use crate::config::Config;
use crate::models::user;

pub struct Live {
    db: DatabaseConnection,
    helix: HelixClient<'static, reqwest::Client>,
}

impl Live {
    pub fn new(db: DatabaseConnection, helix: HelixClient<'static, reqwest::Client>) -> Self {
        Self { db, helix }
    }
}

impl CommandHandler for Live {
    fn pattern(&self) -> &str {
        "live"
    }

    fn help(&self) -> Option<Help> {
        Some(Help {
            name: "live".into(),
            usage: "live".into(),
            summary: "Post the currently live fanstreamers".into(),
            description: "Post the currently live fanstreamers.".into(),
            examples: Cow::Borrowed(&[]),
        })
    }

    fn handle<'a>(
        &'a self,
        _: &'a InMemoryCache,
        config: &'a Config,
        discord: &'a DiscordClient,
        _: Commands<'a>,
        message: &'a Message,
        _: &'a Args,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            let user = {
                user::Entity::find()
                    .filter(user::Column::Name.eq(&config.username[..]))
                    .one(&self.db)
                    .await
                    .context("failed to load the bot user")?
                    .context("bot user missing")?
            };

            let token = AccessToken::new(user.twitch_oauth.context("bot user token missing")?);
            let token = UserToken::from_existing(
                self.helix.get_client(),
                token,
                None,
                Some(config.twitch_client_secret.clone()),
            )
            .await
            .context("failed to validate the bot user token")?;

            let mut streams = get_followed_streams(&self.helix, &token)
                .try_collect::<Vec<_>>()
                .await
                .context("failed to fetch the streams")?;
            let mut content;

            discord
                .create_message(message.channel_id)
                .reply(message.id)
                .content(if streams.is_empty() {
                    "No fanstreamers currently live."
                } else {
                    streams.sort_by(|a, b| a.user_name.cmp(&b.user_name));
                    content = String::from("Currently live fanstreamers: ");

                    for (i, stream) in streams.iter().enumerate() {
                        if i != 0 {
                            content.push_str(", ");
                        }
                        content.push_str(&crate::markdown::escape(stream.user_name.as_str()));
                        content.push_str(" (https://twitch.tv/");
                        content.push_str(stream.user_login.as_str());
                        content.push_str(") is playing ");
                        content.push_str(&crate::markdown::escape(&stream.game_name));
                        content.push_str(" (");
                        content.push_str(&crate::markdown::escape(&stream.title));
                        content.push(')');
                    }
                    &content
                })
                .context("response message content invalid")?
                .flags(MessageFlags::SUPPRESS_EMBEDS)
                .await
                .context("failed to reply to command")?;

            Ok(())
        })
    }
}

// Literally just `HelixClient::get_followed_streams` but the returned stream is `Send`.
// The returned stream is already send but not defined as such in the interface.
fn get_followed_streams<'a, T, C>(
    client: &'a HelixClient<'a, C>,
    token: &'a T,
) -> std::pin::Pin<
    Box<
        dyn futures::Stream<
                Item = Result<
                    Stream,
                    twitch_api::helix::ClientRequestError<<C as twitch_api::HttpClient<'a>>::Error>,
                >,
            > + Send
            + 'a,
    >,
>
where
    T: twitch_api::twitch_oauth2::TwitchToken + Send + Sync + ?Sized,
    C: twitch_api::HttpClient<'a> + Sync,
{
    use futures::StreamExt;

    let user_id = match token.user_id().ok_or_else(|| {
        twitch_api::helix::ClientRequestError::Custom("no user_id found on token".into())
    }) {
        Ok(t) => t,
        Err(e) => return futures::stream::once(async { Err(e) }).boxed(),
    };
    let req = twitch_api::helix::streams::GetFollowedStreamsRequest::user_id(user_id);
    twitch_api::helix::make_stream(req, token, client, std::collections::VecDeque::from)
}
