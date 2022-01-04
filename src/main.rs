#![recursion_limit = "256"]

#[macro_use]
extern crate diesel;

use anyhow::{Context, Error};
use serenity::client::bridge::gateway::GatewayIntents;
use serenity::model::id::UserId;
use std::borrow::Cow;
use tracing_subscriber::EnvFilter;

use crate::context::ErisContext;
use crate::extract::Extract;
use tracing::{error, info};

mod aiomas;
mod announcements;
mod autotopic;
mod channel_reaper;
mod commands;
mod config;
mod contact;
mod context;
mod desertbus;
mod discord_events;
mod extract;
mod google;
mod influxdb;
mod inventory;
mod models;
mod pg_fts;
mod rpc;
mod schema;
mod service;
mod shorten;
mod time;
mod try_crosspost;
mod twitch;
mod twitter;
mod typemap_keys;

trait ClientBuilderExt {
    fn maybe_type_map_insert<T: serenity::prelude::TypeMapKey>(self, val: Option<T::Value>)
        -> Self;
}

impl ClientBuilderExt for serenity::client::ClientBuilder<'_> {
    fn maybe_type_map_insert<T: serenity::prelude::TypeMapKey>(
        self,
        opt: Option<T::Value>,
    ) -> Self {
        if let Some(val) = opt {
            self.type_map_insert::<T>(val)
        } else {
            self
        }
    }
}

const DEFAULT_TRACING_FILTER: &'static str = "info";

#[tokio::main]
async fn main() -> Result<(), Error> {
    let builder = tracing_subscriber::fmt::fmt()
        .json()
        .flatten_event(true)
        .with_current_span(true)
        .with_span_list(true)
        .with_timer(tracing_subscriber::fmt::time::ChronoUtc::rfc3339())
        .with_env_filter(EnvFilter::new(match std::env::var(EnvFilter::DEFAULT_ENV) {
            Ok(filter) => Cow::Owned(filter),
            Err(std::env::VarError::NotPresent) => Cow::Borrowed(DEFAULT_TRACING_FILTER),
            Err(e) => {
                panic!("failed to read the tracing filter from ${}: {}", EnvFilter::DEFAULT_ENV, e)
            }
        }))
        .with_filter_reloading();
    let reload_handle = builder.reload_handle();
    builder
        .try_init()
        .map_err(|err| anyhow::anyhow!(err))
        .context("failed to initialize tracing")?;

    let matches = clap::App::new(env!("CARGO_PKG_NAME"))
        .version(env!("CARGO_PKG_VERSION"))
        .author(env!("CARGO_PKG_AUTHORS"))
        .about(env!("CARGO_PKG_DESCRIPTION"))
        .arg(
            clap::Arg::new("conf")
                .short('c')
                .value_name("FILE")
                .help("Config file")
                .default_value("lrrbot.conf"),
        )
        .arg(
            clap::Arg::new("google-service-account")
                .short('k')
                .value_name("FILE")
                .help("JSON file containing the Google service account key")
                .default_value("keys.json"),
        )
        .get_matches();

    let config = config::Config::load_from_file(matches.value_of_os("conf").unwrap())
        .context("failed to load the config file")?;

    let pg_pool = diesel::r2d2::Pool::new(diesel::r2d2::ConnectionManager::<
        diesel::pg::PgConnection,
    >::new(&config.database_url[..]))
    .context("failed to create the database pool")?;

    let http_client = reqwest::ClientBuilder::new()
        .user_agent(concat!(
            "LRRbot/2.0 ",
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION"),
            " (https://lrrbot.com)"
        ))
        .build()
        .context("failed to create the HTTP client")?;

    let helix = twitch::Helix::new(http_client.clone(), &config)
        .context("failed to create the New Twitch API client")?;

    let google_keys_json_path = matches.value_of_os("google-service-account").unwrap();

    let calendar = google::Calendar::new(http_client.clone(), &google_keys_json_path);
    let spreadsheets = google::Sheets::new(http_client.clone(), &google_keys_json_path);

    let desertbus = desertbus::DesertBus::new(http_client.clone());

    let twitter = crate::twitter::Twitter::new(
        http_client.clone(),
        config.twitter_api_key.clone(),
        config.twitter_api_secret.clone(),
    )
    .await
    .context("failed to initialise the Twitter client")?;

    #[cfg(unix)]
    {
        if let Err(err) = tokio::fs::remove_file(&config.eris_socket).await {
            if err.kind() != std::io::ErrorKind::NotFound {
                return Err(err).context("failed to remove the socket file")?;
            }
        }
    }

    let http = serenity::http::Http::new_with_token(&config.discord_botsecret);
    let current_application_info = http
        .get_current_application_info()
        .await
        .context("failed to get the current application info")?;

    let mut client = serenity::Client::builder(&config.discord_botsecret)
        .intents(
            GatewayIntents::GUILDS
                | GatewayIntents::GUILD_MEMBERS
                | GatewayIntents::GUILD_EMOJIS
                | GatewayIntents::GUILD_VOICE_STATES
                | GatewayIntents::GUILD_MESSAGES
                | GatewayIntents::DIRECT_MESSAGES,
        )
        .event_handler(crate::discord_events::DiscordEvents::new())
        .framework(
            serenity::framework::StandardFramework::new()
                .configure(|c| {
                    c.prefix(&config.command_prefix)
                        .with_whitespace((true, true, true))
                        .on_mention(Some(current_application_info.id))
                        .case_insensitivity(true)
                        .owners(
                            [
                                // Defrost#0001
                                UserId(101919755132227584),
                                // phlip#6324
                                UserId(153674140019064832),
                                // qrpth#6704
                                UserId(144128240389324800),
                            ]
                            .iter()
                            .copied()
                            .collect(),
                        )
                })
                .before(|_, message, command_name| {
                    Box::pin(async move {
                        info!(
                            command_name = command_name,
                            message = message.content.as_str(),
                            message.id = message.id.0,
                            from.id = message.author.id.0,
                            from.name = message.author.name.as_str(),
                            from.discriminator = message.author.discriminator,
                            "Command received",
                        );
                        true
                    })
                })
                .after(|ctx, message, _command_name, result| {
                    Box::pin(async move {
                        if let Err(error) = result {
                            error!(
                                message.id = message.id.0,
                                ?error,
                                "Command resulted in an unexpected error"
                            );

                            let _ = message.reply(
                                ctx,
                                &format!("Command resulted in an unexpected error: {}.", error),
                            );
                        } else {
                            info!(message.id = message.id.0, "Command processed successfully",);
                        }
                    })
                })
                .unrecognised_command(commands::static_response::static_response)
                .help(&crate::commands::help::HELP)
                .group(&crate::commands::calendar::CALENDAR_GROUP)
                .group(&crate::commands::live::FANSTREAMS_GROUP)
                .group(&crate::commands::quote::QUOTE_GROUP)
                .group(&crate::commands::time::TIME_GROUP)
                .group(&crate::commands::tracing::TRACING_GROUP)
                .group(&crate::commands::voice::VOICE_GROUP),
        )
        .type_map_insert::<crate::rpc::LRRbot>(std::sync::Arc::new(crate::rpc::LRRbot::new(
            &config,
        )))
        .maybe_type_map_insert::<crate::influxdb::InfluxDB>(
            config
                .influxdb
                .as_ref()
                .map(|url| crate::influxdb::InfluxDB::new(http_client.clone(), url.clone())),
        )
        .type_map_insert::<crate::config::Config>(config)
        .type_map_insert::<crate::typemap_keys::PgPool>(pg_pool)
        .type_map_insert::<crate::twitch::Helix>(helix)
        .type_map_insert::<crate::google::Calendar>(calendar)
        .type_map_insert::<crate::google::Sheets>(spreadsheets)
        .type_map_insert::<crate::desertbus::DesertBus>(desertbus)
        .type_map_insert::<crate::twitter::Twitter>(twitter)
        .type_map_insert::<crate::typemap_keys::ReloadHandle>(reload_handle)
        .await
        .context("failed to create the Discord client")?;

    let ctx = ErisContext::from_client(&client);

    let mut rpc_server = {
        let data = ctx.data.read().await;
        let config = data.extract::<crate::config::Config>()?;

        #[cfg(unix)]
        let server = crate::aiomas::Server::new(&config.eris_socket, ctx.clone());

        #[cfg(not(unix))]
        let server = crate::aiomas::Server::new(config.eris_port, ctx.clone());

        server
    }
    .context("failed to create the RPC server")?;
    for handler in ::inventory::iter::<crate::inventory::AiomasHandler> {
        rpc_server.register(handler.method, handler.handler);
    }

    tokio::spawn(rpc_server.serve());
    tokio::spawn(channel_reaper::channel_reaper(ctx.clone()));
    tokio::spawn(announcements::post_tweets(ctx.clone()));
    tokio::spawn(autotopic::autotopic(ctx.clone()));
    tokio::spawn(contact::post_messages(ctx));

    client.start().await.context("error while running the Discord client")
}
