#![feature(rust_2018_preview)]

extern crate byteorder;
extern crate bytes;
extern crate chrono_tz;
extern crate chrono;
extern crate clap;
extern crate egg_mode;
extern crate failure;
extern crate ini;
extern crate futures;
extern crate reqwest;
extern crate serde_json;
extern crate serde;
extern crate serenity;
extern crate tokio;
extern crate tower_service;

extern crate simple_logger;

#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate diesel;

use failure::ResultExt;
use tokio::prelude::*;

mod aiomas;
mod channel_reaper;
mod commands;
mod config;
mod models;
mod rpc;
mod schema;
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
        .context(
        "failed to create the database pool",
    )?;

    let kraken = std::sync::Arc::new(
        twitch::Kraken::new(config.clone()).context("failed to create the Twitch v5 client")?,
    );

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
    /*let _handle = std::thread::spawn(move || {
        let mut lrrbot = Some(rpc::LRRbot::new(config));

        tokio::run(tokio::timer::Interval::new_interval(std::time::Duration::from_secs(1))
            .map_err(|err| eprintln!("timer error: {}", err))
            .for_each(move |_| {
                lrrbot.take().unwrap().ready()
                    .and_then(|mut lrrbot| {
                        lrrbot.get_header_info()
                            .then(|res| {
                                println!("header: {:?}", res);
                                Ok(())
                            })
                    })
                    .map_err(|err| {
                        eprintln!("error connecting: {:?}", err);
                    })
            })
        );
    });*/

    client
        .start()
        .map_err(failure::SyncFailure::new)
        .context("error while running the Discord client")?;

    Ok(())
}
