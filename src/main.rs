use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Error};
use futures_util::stream::{FuturesUnordered, StreamExt as _};
use google_calendar3::common::Client as HyperClient;
use google_calendar3::hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use google_calendar3::hyper_util::client::legacy::connect::HttpConnector;
use google_calendar3::hyper_util::client::legacy::Builder as HyperClientBuilder;
use google_calendar3::hyper_util::rt::TokioExecutor;
use google_calendar3::yup_oauth2::authenticator::{Authenticator, ServiceAccountAuthenticator};
use google_calendar3::CalendarHub;
use google_sheets4::Sheets;
use google_youtube3::YouTube;
use tokio::sync::RwLock;
use tracing_subscriber::EnvFilter;
use twilight_gateway::{EventTypeFlags, Intents, StreamExt as _};
use twilight_http::Client as DiscordClient;
use twilight_model::channel::message::AllowedMentions;
use twilight_model::gateway::payload::outgoing::update_presence::UpdatePresencePayload;
use twilight_model::gateway::presence::{ActivityType, MinimalActivity, Status as PresenceStatus};

mod aiomas;
mod announcements;
mod autotopic;
mod cache;
mod calendar;
mod channel_reaper;
mod command_parser;
mod commands;
mod config;
mod contact;
mod desertbus;
mod disconnect_afk;
mod influxdb;
mod markdown;
mod metrics;
mod models;
mod rpc;
mod shorten;
mod shutdown;
#[cfg(target_os = "linux")]
mod systemd;
mod time;
mod token_renewal;
mod tz;

const DEFAULT_TRACING_FILTER: &str = "info,sqlx::query=warn";
const USER_AGENT: &str = concat!(
    "LRRbot/2.0 ",
    env!("CARGO_PKG_NAME"),
    "/",
    env!("CARGO_PKG_VERSION"),
    " (https://lrrbot.com)"
);

async fn create_google_client(
    service_account_path: impl AsRef<Path>,
) -> Result<
    (HyperClient<HttpsConnector<HttpConnector>>, Authenticator<HttpsConnector<HttpConnector>>),
    Error,
> {
    let connector = HttpsConnectorBuilder::new()
        .with_native_roots()
        .context("failed to set the TLS configuration")?
        .https_or_http()
        .enable_http1()
        .build();

    let builder = HyperClientBuilder::new(TokioExecutor::new());

    let auth: google_sheets4::yup_oauth2::ServiceAccountKey =
        google_calendar3::yup_oauth2::read_service_account_key(service_account_path)
            .await
            .context("failed to read the Google service account key")?;
    let auth = ServiceAccountAuthenticator::with_client(auth, builder.build(connector.clone()))
        .build()
        .await
        .context("failed to create the Google service account authenticator")?;

    Ok((builder.build(connector), auth))
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let builder = tracing_subscriber::fmt::fmt()
        .json()
        .flatten_event(true)
        .with_current_span(true)
        .with_span_list(true)
        .with_timer(tracing_subscriber::fmt::time::ChronoUtc::rfc_3339())
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

    let matches = clap::Command::new(env!("CARGO_PKG_NAME"))
        .version(env!("CARGO_PKG_VERSION"))
        .author(env!("CARGO_PKG_AUTHORS"))
        .about(env!("CARGO_PKG_DESCRIPTION"))
        .arg(
            clap::Arg::new("conf")
                .short('c')
                .value_name("FILE")
                .help("Config file")
                .value_parser(clap::value_parser!(PathBuf))
                .default_value("lrrbot.conf"),
        )
        .arg(
            clap::Arg::new("google-service-account")
                .short('k')
                .value_name("FILE")
                .help("JSON file containing the Google service account key")
                .value_parser(clap::value_parser!(PathBuf))
                .default_value("keys.json"),
        )
        .get_matches();

    let mut tasks = FuturesUnordered::new();
    let (running_tx, mut running_rx) = tokio::sync::watch::channel(true);

    let (handle, handler_tx) = crate::shutdown::wait_for_outstanding(running_rx.clone());
    tasks.push(handle);

    let config = crate::config::Config::load_from_file(matches.get_one::<PathBuf>("conf").unwrap())
        .context("failed to load the config file")?;
    let config = Arc::new(config);

    let db = sea_orm::Database::connect(&config.database_url)
        .await
        .context("failed to create the database pool")?;

    let http_client = reqwest::ClientBuilder::new()
        .user_agent(USER_AGENT)
        .build()
        .context("failed to create the HTTP client")?;

    let helix = twitch_api::HelixClient::with_client(http_client.clone());
    let helix_token = twitch_api::twitch_oauth2::AppAccessToken::get_app_access_token(
        &http_client,
        config.twitch_client_id.clone(),
        config.twitch_client_secret.clone(),
        vec![],
    )
    .await
    .context("failed to request the Twitch app access token")?;
    let helix_token = Arc::new(RwLock::new(helix_token));

    let (google_client, google_auth) =
        create_google_client(matches.get_one::<PathBuf>("google-service-account").unwrap())
            .await
            .context("failed to create the Google API client")?;

    let mut calendar = CalendarHub::new(google_client.clone(), google_auth.clone());
    calendar.user_agent(USER_AGENT.into());
    let mut sheets = Sheets::new(google_client.clone(), google_auth.clone());
    sheets.user_agent(USER_AGENT.into());
    let mut youtube = YouTube::new(google_client.clone(), google_auth.clone());
    youtube.user_agent(USER_AGENT.into());

    let influxdb = config
        .influxdb
        .as_ref()
        .map(|(url, database)| crate::influxdb::InfluxDb::new(http_client.clone(), url, database))
        .transpose()
        .context("failed to create the InfluxDB client")?;

    let desertbus = crate::desertbus::DesertBus::new(http_client.clone());

    let discord = DiscordClient::builder()
        .token(config.discord_botsecret.clone())
        // prevent any mentions by default
        .default_allowed_mentions(AllowedMentions::default())
        .build();
    let discord = Arc::new(discord);

    let cache = Arc::new(crate::cache::Cache::new(config.guild));
    let lrrbot = Arc::new(crate::rpc::LRRbot::new(running_rx.clone(), handler_tx.clone(), &config));

    let mut rpc_server = {
        #[cfg(unix)]
        let server = {
            if let Err(err) = tokio::fs::remove_file(&config.eris_socket).await {
                if err.kind() != std::io::ErrorKind::NotFound {
                    return Err(Error::from(err).context("failed to remove the socket file"));
                }
            }

            crate::aiomas::server::Server::new(&config.eris_socket)
        };

        #[cfg(not(unix))]
        let server = crate::aiomas::server::Server::new(config.eris_port).await;

        server
    }
    .context("failed to create the RPC server")?;

    rpc_server.register(
        "announcements/stream_up",
        crate::announcements::stream_up(
            config.clone(),
            db.clone(),
            discord.clone(),
            helix.clone(),
            helix_token.clone(),
            lrrbot.clone(),
        ),
    );

    tasks.push(tokio::spawn(rpc_server.serve(running_rx.clone(), handler_tx.clone())));
    tasks.push(tokio::spawn(crate::announcements::post_skeets(
        running_rx.clone(),
        config.clone(),
        db.clone(),
        discord.clone(),
        http_client.clone(),
    )));
    tasks.push(tokio::spawn(crate::announcements::post_toots(
        running_rx.clone(),
        config.clone(),
        db.clone(),
        discord.clone(),
        http_client.clone(),
    )));
    tasks.push(tokio::spawn(crate::announcements::post_videos(
        running_rx.clone(),
        db.clone(),
        cache.clone(),
        config.clone(),
        discord.clone(),
        youtube.clone(),
    )));
    tasks.push(tokio::spawn(crate::autotopic::autotopic(
        running_rx.clone(),
        cache.clone(),
        calendar.clone(),
        config.clone(),
        db.clone(),
        desertbus.clone(),
        discord.clone(),
        helix.clone(),
        helix_token.clone(),
        lrrbot.clone(),
    )));
    tasks.push(tokio::spawn(crate::channel_reaper::channel_reaper(
        running_rx.clone(),
        cache.clone(),
        config.clone(),
        discord.clone(),
    )));
    tasks.push(tokio::spawn(crate::contact::post_messages(
        running_rx.clone(),
        config.clone(),
        discord.clone(),
        sheets.clone(),
    )));
    tasks.push(tokio::spawn(crate::token_renewal::renew_helix(
        running_rx.clone(),
        helix_token.clone(),
        http_client.clone(),
    )));

    let command_parser = crate::command_parser::CommandParser::builder()
        .command(crate::commands::calendar::Next::fan(calendar.clone()))
        .command(crate::commands::calendar::Next::lrr(calendar.clone()))
        .command(crate::commands::help::Help::new())
        .command(crate::commands::live::Live::new(db.clone(), helix.clone()))
        .command(crate::commands::quote::Details::new(db.clone()))
        .command(crate::commands::quote::QueryDebugger::new())
        .command(crate::commands::time::Time::new_12())
        .command(crate::commands::time::Time::new_24())
        .command(crate::commands::tracing::TracingFilter::new(reload_handle.clone()))
        .command_opt(crate::commands::video::New::new(&config, youtube.clone()))
        .command_opt(crate::commands::video::Refresh::new(&config, youtube.clone()))
        .command(crate::commands::voice::Voice::new())
        // this command is after all other quote commands to avoid conflicts
        .command(crate::commands::quote::Find::new(db.clone()))
        // this is the last command on purpose to avoid conflicts
        .command(crate::commands::static_response::Static::new(db.clone()))
        .build(cache.clone(), config.clone(), discord.clone())
        .context("failed to build the command parser")?;

    #[cfg(target_os = "linux")]
    let sd_notify = match crate::systemd::Notify::new() {
        Ok(notify) => Some(Arc::new(notify)),
        Err(error) => {
            tracing::warn!(?error, "failed to create the systemd notifier");
            None
        }
    };

    let intents = Intents::GUILDS
        | Intents::GUILD_MEMBERS
        | Intents::GUILD_EMOJIS_AND_STICKERS
        | Intents::GUILD_VOICE_STATES
        | Intents::GUILD_MESSAGES
        | Intents::DIRECT_MESSAGES
        | Intents::MESSAGE_CONTENT;

    let shard_config = twilight_gateway::Config::new(config.discord_botsecret.clone(), intents);
    let presence = UpdatePresencePayload::new(
        vec![MinimalActivity {
            kind: ActivityType::Listening,
            name: format!("{}help || v{}", config.command_prefix, env!("CARGO_PKG_VERSION")),
            url: Some("https://lrrbot.com/".into()),
        }
        .into()],
        false,
        None,
        PresenceStatus::Online,
    )
    .context("failed to construct the presence")?;
    let shards = twilight_gateway::create_recommended(&discord, shard_config, |_, builder| {
        builder.presence(presence.clone()).build()
    })
    .await
    .context("failed to create the shards")?;

    for mut shard in shards {
        let cache = cache.clone();
        let command_parser = command_parser.clone();
        let discord = discord.clone();
        let influxdb = influxdb.clone();
        let mut running_rx = running_rx.clone();
        let handler_tx = handler_tx.clone();
        #[cfg(target_os = "linux")]
        let sd_notify = sd_notify.clone();

        tasks.push(tokio::spawn(async move {
            let shard_id = shard.id();

            loop {
                tokio::select! {
                    _ = running_rx.changed() => break,
                    res = shard.next_event(EventTypeFlags::all()) => match res {
                        Some(Ok(event)) => {
                            #[cfg(target_os = "linux")]
                            if let Some(sd_notify) = sd_notify.as_ref() {
                                if let Err(error) = sd_notify.feed_watchdog().await {
                                    tracing::warn!(?error, "failed to feed the systemd watchdog");
                                }
                            }

                            if let Some(ref influxdb) = influxdb {
                                if let Err(error) =
                                    crate::metrics::on_event(&cache, influxdb, &event).await
                                {
                                    tracing::error!(?error, "failed to collect metrics");
                                }
                            }

                            cache.update(&event);

                            crate::disconnect_afk::on_event(&cache, &discord, &event).await;

                            command_parser.on_event(&handler_tx, &event).await;
                        }
                        Some(Err(error)) => {
                            tracing::error!(
                                ?error,
                                shard.id = ?shard_id,
                                "failed to receive an event from the shard"
                            );
                        }
                        None => break,
                    }
                }
            }
        }));
    }

    tasks.push(tokio::spawn(async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => (),
            _ = running_rx.changed() => (),
        }
    }));

    #[cfg(target_os = "linux")]
    if let Some(sd_notify) = sd_notify.as_ref() {
        if let Err(error) = sd_notify.ready().await {
            tracing::warn!(?error, "failed to notify systemd that the bot is up");
        }
    }

    if let Some(Err(error)) = tasks.next().await {
        tracing::error!(?error, "task failed");
    }
    tracing::info!("stopping bot");
    running_tx.send_replace(false);

    while let Some(res) = tasks.next().await {
        if let Err(error) = res {
            tracing::error!(?error, "task failed");
        }
    }

    Ok(())
}
