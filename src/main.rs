#![recursion_limit = "256"]

#[macro_use]
extern crate diesel;

use anyhow::{Context, Error};

use crate::context::ErisContext;
use crate::extract::Extract;
use slog::{o, Drain};
use slog_scope::{error, info};

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
mod time;
mod truncate;
mod twitch;
mod twitter;
mod typemap_keys;

struct DualWriter<W1: std::io::Write, W2: std::io::Write>(W1, W2);

impl<W1: std::io::Write, W2: std::io::Write> std::io::Write for DualWriter<W1, W2> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write_all(buf)?;
        self.1.write_all(buf)?;

        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()?;
        self.1.flush()
    }
}

fn main() -> Result<(), Error> {
    let log_file = std::fs::OpenOptions::new()
        .write(true)
        .append(true)
        .create(true)
        .open("eris.log")
        .context("failed to open the log file")?;

    let drain = slog_json::Json::new(DualWriter(log_file, std::io::stdout()))
        .set_flush(true)
        .add_key_value(o! {
            "ts" => slog::PushFnValue(|_, ser| ser.emit(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true))),
            "level" => slog::FnValue(|record| record.level().as_str()),
            "msg" => slog::PushFnValue(|record, ser| ser.emit(record.msg())),
            "module" => slog::FnValue(|record| record.module()),
        })
        .build()
        .fuse();
    let drain = slog_async::Async::new(drain)
        .overflow_strategy(slog_async::OverflowStrategy::Block)
        .build()
        .fuse();
    let logger = slog::Logger::root(
        drain,
        o!(
            "version" => env!("CARGO_PKG_VERSION"),
            "build" => option_env!("TRAVIS_BUILD_NUMBER").unwrap_or("local build")
        ),
    );
    let _handle = slog_scope::set_global_logger(logger);
    slog_stdlog::init().context("failed to redirect logs from the standard log crate")?;
    log::set_max_level(log::LevelFilter::max());

    let matches = clap::App::new(env!("CARGO_PKG_NAME"))
        .version(env!("CARGO_PKG_VERSION"))
        .author(env!("CARGO_PKG_AUTHORS"))
        .about(env!("CARGO_PKG_DESCRIPTION"))
        .arg(
            clap::Arg::with_name("conf")
                .short("c")
                .value_name("FILE")
                .help("Config file")
                .default_value("lrrbot.conf"),
        )
        .arg(
            clap::Arg::with_name("google-service-account")
                .short("k")
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

    let mut runtime = tokio::runtime::Runtime::new().context("failed to create a Tokio runtime")?;
    let handle = runtime.handle().clone();

    runtime.block_on(async move {
        // FIXME(tokio-rs/tokio#1838): `block_in_place` panics when used inside `Runtime::block_on`
        tokio::spawn(async move {
            let http_client = reqwest::ClientBuilder::new()
                .user_agent("LRRbot/2.0 (https://lrrbot.com)")
                .build()
                .context("failed to create the HTTP client")?;

            let kraken = twitch::Kraken::new(http_client.clone(), &config)
                .context("failed to create the Twitch API v5 client")?;
            let helix = twitch::Helix::new(http_client.clone(), &config)
                .context("failed to create the New Twitch API client")?;

            let google_keys_json_path = matches.value_of_os("google-service-account").unwrap();

            let calendar = google::Calendar::new(http_client.clone(), &google_keys_json_path);
            let spreadsheets = google::Sheets::new(http_client.clone(), &google_keys_json_path);

            let desertbus = desertbus::DesertBus::new(http_client.clone());

            let handler = crate::discord_events::DiscordEvents::new();

            let twitter = crate::twitter::Twitter::new(
                http_client.clone(),
                config.twitter_api_key.clone(),
                config.twitter_api_secret.clone(),
            )
            .await
            .context("failed to initialise the Twitter client")?;

            let mut client = tokio::task::block_in_place(|| {
                serenity::Client::new(&config.discord_botsecret, handler)
            })
            .context("failed to create the Discord client")?;
            let current_application_info = tokio::task::block_in_place(|| {
                client.cache_and_http.http.get_current_application_info()
            })
            .context("failed to fetch the current application information")?;
            client.with_framework(
                serenity::framework::StandardFramework::new()
                    .configure(|c| {
                        c.prefix(&config.command_prefix)
                            .with_whitespace((true, true, true))
                            .on_mention(Some(current_application_info.id))
                            .case_insensitivity(true)
                    })
                    .before(|_, message, command_name| {
                        info!("Command received";
                            "command_name" => command_name,
                            "message" => &message.content,
                            "message.id" => message.id.0,
                            "from.id" => message.author.id.0,
                            "from.name" => &message.author.name,
                            "from.discriminator" => message.author.discriminator,
                        );
                        true
                    })
                    .after(|ctx, message, _command_name, result| {
                        if let Err(err) = result {
                            error!("Command resulted in an unexpected error";
                                "message.id" => message.id.0,
                                "error" => &err.0,
                            );

                            let _ = message.reply(
                                ctx,
                                &format!("Command resulted in an unexpected error: {}.", err.0),
                            );
                        } else {
                            info!("Command processed successfully";
                                "message.id" => message.id.0,
                            );
                        }
                    })
                    .unrecognised_command(commands::static_response::static_response)
                    .help(&crate::commands::help::HELP)
                    .group(&crate::commands::calendar::CALENDAR_GROUP)
                    .group(&crate::commands::date::DATE_GROUP)
                    .group(&crate::commands::live::FANSTREAMS_GROUP)
                    .group(&crate::commands::quote::QUOTE_GROUP)
                    .group(&crate::commands::time::TIME_GROUP)
                    .group(&crate::commands::voice::VOICE_GROUP),
            );

            #[cfg(unix)]
            {
                if let Err(err) = tokio::fs::remove_file(&config.eris_socket).await {
                    if err.kind() != std::io::ErrorKind::NotFound {
                        return Err(err).context("failed to remove the socket file")?;
                    }
                }
            }

            {
                let mut data = client.data.write();

                data.insert::<crate::rpc::LRRbot>(std::sync::Arc::new(crate::rpc::LRRbot::new(
                    &config,
                )));

                if let Some(url) = config.influxdb.as_ref() {
                    data.insert::<crate::influxdb::InfluxDB>(crate::influxdb::InfluxDB::new(
                        http_client.clone(),
                        url.clone(),
                    ));
                }

                data.insert::<crate::config::Config>(config);
                data.insert::<crate::typemap_keys::Executor>(handle);
                data.insert::<crate::typemap_keys::PgPool>(pg_pool);
                data.insert::<crate::twitch::Kraken>(kraken);
                data.insert::<crate::twitch::Helix>(helix);
                data.insert::<crate::google::Calendar>(calendar);
                data.insert::<crate::google::Sheets>(spreadsheets);
                data.insert::<crate::desertbus::DesertBus>(desertbus);
                data.insert::<crate::twitter::Twitter>(twitter);
            }

            let ctx = ErisContext::from_client(&client);

            let mut rpc_server = {
                let data = ctx.data.read();
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

            let _handle = std::thread::spawn(channel_reaper::channel_reaper(ctx.clone()));

            tokio::spawn(announcements::post_tweets(ctx.clone()));

            tokio::spawn(autotopic::autotopic(ctx.clone()));

            tokio::spawn(contact::post_messages(ctx));

            tokio::task::block_in_place(|| client.start())
                .context("error while running the Discord client")?;

            Ok(())
        })
        .await
        .context("failed to wait for the bot to stop")?
    })
}
