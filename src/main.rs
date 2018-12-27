#![feature(
    futures_api,
    await_macro,
    async_await,
    transpose_result,
    const_slice_len
)]
// Remove when Diesel updates.
#![allow(proc_macro_derive_resolution_fallback)]

#[macro_use]
extern crate diesel;

use failure::ResultExt;

use futures::future::{FutureExt, TryFutureExt};
use slog::{o, slog_error, slog_info, Drain};
use slog_scope::{error, info};

mod aiomas;
mod announcements;
mod apng;
mod autotopic;
mod channel_reaper;
mod commands;
mod config;
mod desertbus;
mod executor_ext;
mod google_calendar;
mod models;
mod rpc;
mod schema;
mod service;
mod stdlog;
mod time;
mod twitch;
mod voice_channel_tracker;

type PgPool = diesel::r2d2::Pool<diesel::r2d2::ConnectionManager<diesel::pg::PgConnection>>;

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

    // TODO: determine if it should be something else. 10 ms is too short for some reason.
    serenity::CACHE.write().settings_mut().cache_lock_time = None;

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
        .get_matches();

    let config = std::sync::Arc::new(
        config::Config::load_from_file(matches.value_of_os("conf").unwrap())
            .context("failed to load the config file")?,
    );

    let pg_pool: PgPool = diesel::r2d2::Pool::new(diesel::r2d2::ConnectionManager::<
        diesel::pg::PgConnection,
    >::new(&config.database_url[..]))
    .context("failed to create the database pool")?;

    let http_client = reqwest::r#async::ClientBuilder::new()
        .build()
        .context("failed to create the HTTP client")?;

    let kraken = twitch::Kraken::new(http_client.clone(), config.clone());
    let helix = twitch::Helix::new(http_client.clone(), config.clone());

    let calendar = google_calendar::Calendar::new(http_client.clone(), config.clone());

    let desertbus = desertbus::DesertBus::new(http_client.clone());

    let handler = voice_channel_tracker::VoiceChannelTracker::new(&config)
        .context("failed to create the voice channel tracker")?;

    let mut runtime = tokio::runtime::Runtime::new().context("failed to create a Tokio runtime")?;

    let mut client = serenity::Client::new(&config.discord_botsecret, handler)
        .map_err(failure::SyncFailure::new)
        .context("failed to create the Discord client")?;
    client.with_framework(
        serenity::framework::StandardFramework::new()
            .configure(|c| {
                c.prefix(&config.command_prefix)
                    .allow_whitespace(true)
                    .on_mention(true)
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
            .after(|_, message, command_name, result| {
                if let Err(err) = result {
                    error!("Command resulted in an unexpected error";
                        "command_name" => ?command_name,
                        "message.id" => ?message.id.0,
                        "error" => ?err,
                    );

                    let _ = message.reply(&format!(
                        "Command resulted in an unexpected error: {}.",
                        err.0
                    ));
                } else {
                    info!("Command processed successfully";
                        "command_name" => ?command_name,
                        "message.id" => ?message.id.0,
                    );
                }
            })
            .unrecognised_command(commands::static_response::static_response(
                config.clone(),
                runtime.executor(),
            ))
            .customised_help(
                serenity::framework::standard::help_commands::with_embeds,
                |h| {
                    let help_url = config
                        .site_url
                        .join("help#help-section-text")
                        .expect("failed to construct the simple text response command help URL");
                    h.individual_command_tip(&format!(
                        concat!(
                            "To get help with an individual command, pass its name as an argument ",
                            "to this command. Simple text response commands (like `!advice`) are ",
                            "not listed here, for those see <{}>."
                        ),
                        help_url
                    ))
                },
            )
            .command("live", |c| {
                c.desc("Post the currently live fanstreamers.")
                    .help_available(true)
                    .num_args(0)
                    .cmd(commands::live::Live::new(
                        config.clone(),
                        pg_pool.clone(),
                        kraken.clone(),
                        runtime.executor(),
                    ))
            })
            .command("voice", |c| {
                c.desc("Create a temporary voice channel.")
                    .usage("CHANNEL NAME")
                    .example("PUBG #15")
                    .help_available(true)
                    .cmd(commands::voice::Voice::new(config.clone()))
            })
            .command("time", |c| {
                c.desc("Post the current moonbase time, optionally in the 24-hour format.")
                    .usage("[24]")
                    .example("24")
                    .help_available(true)
                    .min_args(0)
                    .max_args(1)
                    .cmd(commands::time::Time::new(config.clone()))
            })
            .command("spoiler", |c| {
                c.desc(concat!(
                    "Conceal a spoiler. If you are running Discord in a browser that's not ",
                    "Internet Explorer or Microsoft Edge you only need to click on the thumbnail ",
                    "for the message to be revealed. Otherwise you also need to click the 'Open ",
                    "original' link on desktop or the 'Open in browser' button on mobile.",
                ))
                .usage("<TOPIC> <MESSAGE>")
                .example("\"the password\" dickbutt")
                .help_available(true)
                .min_args(2)
                .cmd(commands::spoiler::Spoiler::new(config.clone()))
            })
            .group("Calendar", |g| {
                g.command("next", |c| {
                    c.desc(concat!(
                        "Gets the next scheduled stream from the LoadingReadyLive calendar. Can ",
                        "specify a timezone, to show stream in your local time. If no time zone ",
                        "is specified, times will be shown in Moonbase time. Unlike on Twitch the ",
                        "timezone is case-sensitive."
                    ))
                    .usage("[TIMEZONE]")
                    .example("America/New_York")
                    .help_available(true)
                    .min_args(0)
                    .max_args(1)
                    .cmd(commands::calendar::Calendar::lrr(
                        config.clone(),
                        calendar.clone(),
                        runtime.executor(),
                    ))
                })
                .command("nextfan", |c| {
                    c.desc(concat!(
                        "Gets the next scheduled stream from the fan-streaming calendar. Can ",
                        "specify a timezone, to show stream in your local time. If no time zone ",
                        "is specified, times will be shown in Moonbase time. Unlike on Twitch the ",
                        "timezone is case-sensitive."
                    ))
                    .usage("[TIMEZONE]")
                    .example("America/New_York")
                    .help_available(true)
                    .min_args(0)
                    .max_args(1)
                    .cmd(commands::calendar::Calendar::fan(
                        config.clone(),
                        calendar.clone(),
                        runtime.executor(),
                    ))
                })
            }),
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

    let rpc_server = rpc::Server::new(config.clone(), pg_pool.clone(), runtime.executor())
        .context("failed to create the RPC server")?;
    runtime.spawn(rpc_server.serve().unit_error().boxed().compat());

    let _handle = std::thread::spawn(channel_reaper::channel_reaper(config.clone()));

    let _handle = {
        let config = config.clone();
        let pg_pool = pg_pool.clone();
        std::thread::spawn(move || {
            let mut core =
                tokio_core::reactor::Core::new().expect("failed to create a tokio-core reactor");
            core.run(
                announcements::post_tweets(config, pg_pool, core.handle())
                    .unit_error()
                    .boxed()
                    .compat(),
            )
            .expect("failed to announce tweets");
        })
    };

    runtime.spawn(
        autotopic::autotopic(
            config,
            helix,
            calendar,
            desertbus,
            pg_pool,
            runtime.executor(),
        )
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
