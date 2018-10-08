#![feature(
    arbitrary_self_types,
    futures_api,
    pin,
    await_macro,
    existential_type,
    async_await,
    never_type
)]
// Remove when Diesel updates.
#![allow(proc_macro_derive_resolution_fallback)]

#[macro_use]
extern crate diesel;

use failure::ResultExt;

use futures::future::{FutureExt, TryFutureExt};

mod aiomas;
mod autotopic;
mod channel_reaper;
mod commands;
mod config;
mod google_calendar;
mod models;
mod rpc;
mod schema;
mod service;
mod time;
mod twitch;

struct Handler;

impl serenity::client::EventHandler for Handler {}

type PgPool = diesel::r2d2::Pool<diesel::r2d2::ConnectionManager<diesel::pg::PgConnection>>;

fn main() -> Result<(), failure::Error> {
    simple_logger::init().unwrap();

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

    let mut client = serenity::Client::new(&config.discord_botsecret, Handler)
        .map_err(failure::SyncFailure::new)
        .context("failed to create the Discord client")?;
    client.with_framework(
        serenity::framework::StandardFramework::new()
            .configure(|c| {
                c.prefix("!")
                    .allow_whitespace(true)
                    .on_mention(true)
                    .case_insensitivity(true)
            })
            .before(|_, message, command_name| {
                println!("Got {:?} from {}", command_name, message.author.name);
                true
            })
            .help(serenity::framework::standard::help_commands::with_embeds)
            .command("live", |c| {
                c.desc("Post the currently live fanstreamers.")
                    .help_available(true)
                    .num_args(0)
                    .cmd(commands::live::Live::new(
                        config.clone(),
                        pg_pool.clone(),
                        kraken.clone(),
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
            }),
    );

    let _handle = std::thread::spawn(channel_reaper::channel_reaper(config.clone()));
    let _handle = std::thread::spawn(move || {
        tokio::run(
            autotopic::autotopic(config, helix, calendar, pg_pool)
                .unit_error()
                .boxed()
                .compat(),
        )
    });

    client
        .start()
        .map_err(failure::SyncFailure::new)
        .context("error while running the Discord client")?;

    Ok(())
}
