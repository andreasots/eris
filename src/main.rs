#![feature(await_macro, async_await, const_slice_len, core_intrinsics)]
#![recursion_limit = "256"]

#[macro_use]
extern crate diesel;

use failure::ResultExt;

use futures::future::{FutureExt, TryFutureExt};
use slog::{o, slog_error, slog_info, Drain};
use slog_scope::{error, info};
use crate::context::ErisContext;
use crate::extract::Extract;

mod aiomas;
mod announcements;
mod autotopic;
mod channel_reaper;
mod commands;
mod config;
mod contact;
mod context;
mod desertbus;
mod executor_ext;
mod extract;
mod inventory;
mod google;
mod models;
mod rpc;
mod schema;
mod service;
mod stdlog;
mod time;
mod twitch;
mod typemap_keys;
mod voice_channel_tracker;

fn main() -> Result<(), failure::Error> {
    let decorator = slog_term::TermDecorator::new().build();
    let term_drain = slog_term::FullFormat::new(decorator)
        .build()
        .filter_level(slog::Level::Info)
        .fuse();

    let limited_log = std::fs::OpenOptions::new()
        .write(true)
        .append(true)
        .create(true)
        .open("eris.log")
        .context("failed to open the log file")?;
    let debug_log = std::fs::OpenOptions::new()
        .write(true)
        .append(true)
        .create(true)
        .open("eris.debug.log")
        .context("failed to open the debug log file")?;

    let decorator = slog_term::PlainDecorator::new(limited_log);
    let limited_drain = slog_term::FullFormat::new(decorator)
        .build()
        .filter_level(slog::Level::Info)
        .fuse();

    let decorator = slog_term::PlainDecorator::new(debug_log);
    let full_drain = slog_term::FullFormat::new(decorator).build().fuse();
    let file_drain = slog::Duplicate::new(limited_drain, full_drain);

    let drain = slog::Duplicate::new(term_drain, file_drain).fuse();
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
    log::set_logger(&stdlog::LOGGER)
        .context("failed to redirect logs from the standard log crate")?;
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

    let http_client = reqwest::r#async::ClientBuilder::new()
        .build()
        .context("failed to create the HTTP client")?;

    let kraken = twitch::Kraken::new(http_client.clone(), &config)
        .context("failed to create the Twitch API v5 client")?;
    let helix = twitch::Helix::new(http_client.clone(), &config)
        .context("failed to create the New Twitch API client")?;

    let calendar = google::Calendar::new(http_client.clone(), &config);
    let spreadsheets = google::Sheets::new(
        http_client.clone(),
        matches.value_of_os("google-service-account").unwrap(),
    );

    let desertbus = desertbus::DesertBus::new(http_client.clone());

    let handler = voice_channel_tracker::VoiceChannelTracker::new(&config)
        .context("failed to create the voice channel tracker")?;

    let mut runtime = tokio::runtime::Runtime::new().context("failed to create a Tokio runtime")?;

    let mut client = serenity::Client::new(&config.discord_botsecret, handler)
        .map_err(failure::SyncFailure::new)
        .context("failed to create the Discord client")?;
    let current_application_info = client.cache_and_http.http.get_current_application_info()
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
                    "command_name" => ?command_name,
                    "message" => ?&message.content,
                    "message.id" => ?message.id.0,
                    "from.id" => ?message.author.id.0,
                    "from.name" => ?&message.author.name,
                    "from.discriminator" => ?message.author.discriminator,
                );
                true
            })
            .after(|ctx, message, _command_name, result| {
                if let Err(err) = result {
                    error!("Command resulted in an unexpected error";
                        "message.id" => ?message.id.0,
                        "error" => ?err,
                    );

                    let _ = message.reply(ctx, &format!(
                        "Command resulted in an unexpected error: {}.",
                        err.0
                    ));
                } else {
                    info!("Command processed successfully";
                        "message.id" => ?message.id.0,
                    );
                }
            })
            .unrecognised_command(commands::static_response::static_response)
            .help(&crate::commands::help::HELP_HELP_COMMAND)
            .group(&crate::commands::calendar::CALENDAR_GROUP)
            .group(&crate::commands::voice::VOICE_GROUP)
            .group(&crate::commands::time::TIME_GROUP)
            .group(&crate::commands::live::FANSTREAMS_GROUP),
    );

    #[cfg(unix)]
    std::fs::remove_file(&config.eris_socket)
        .or_else(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                Ok(())
            } else {
                Err(err)
            }
        })
        .context("failed to remove the socket file")?;

    {
        let mut data = client.data.write();

        data.insert::<crate::rpc::LRRbot>(std::sync::Arc::new(crate::rpc::LRRbot::new(&config, runtime.executor())));

        data.insert::<crate::config::Config>(config);
        data.insert::<crate::typemap_keys::Executor>(runtime.executor());
        data.insert::<crate::typemap_keys::PgPool>(pg_pool);
        data.insert::<crate::twitch::Kraken>(kraken);
        data.insert::<crate::twitch::Helix>(helix);
        data.insert::<crate::google::Calendar>(calendar);
        data.insert::<crate::google::Sheets>(spreadsheets);
        data.insert::<crate::desertbus::DesertBus>(desertbus);
    }

    let ctx = ErisContext::from_client(&client);

    let mut rpc_server = {
        let data = ctx.data.read();
        let config = data.extract::<crate::config::Config>()?;

        #[cfg(unix)]
        let server = crate::aiomas::Server::new(&config.eris_socket, runtime.executor(), ctx.clone());

        #[cfg(not(unix))]
        let server = crate::aiomas::Server::new(config.eris_port, runtime.executor(), ctx.clone());

        server
    }
        .context("failed to create the RPC server")?;
    for handler in ::inventory::iter::<crate::inventory::AiomasHandler> {
        rpc_server.register(handler.method, handler.handler);
    }
    runtime.spawn(rpc_server.serve().unit_error().boxed().compat());

    let _handle = std::thread::spawn(channel_reaper::channel_reaper(ctx.clone()));

    runtime.spawn(
        announcements::post_tweets(ctx.clone())
            .unit_error()
            .boxed()
            .compat(),
    );

    runtime.spawn(
        autotopic::autotopic(ctx.clone())
        .unit_error()
        .boxed()
        .compat(),
    );

    runtime.spawn(
        contact::post_messages(ctx.clone())
            .unit_error()
            .boxed()
            .compat(),
    );

    client
        .start()
        .map_err(failure::SyncFailure::new)
        .context("error while running the Discord client")?;

    Ok(())
}
