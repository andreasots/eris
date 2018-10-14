use chrono_tz::Tz;
use egg_mode::KeyPair;
use failure::{self, Error, Fail, ResultExt};
use ini::Ini;
use serenity::model::prelude::*;
use std::path::{Path, PathBuf};
use std::str::FromStr;

pub struct Config {
    pub username: String,

    pub database_url: String,

    pub timezone: Tz,

    #[cfg(unix)]
    pub lrrbot_socket: PathBuf,
    #[cfg(unix)]
    pub event_socket: PathBuf,
    #[cfg(unix)]
    pub eris_socket: PathBuf,

    #[cfg(not(unix))]
    pub lrrbot_port: u16,
    #[cfg(not(unix))]
    pub event_port: u16,
    #[cfg(not(unix))]
    pub eris_port: u16,

    pub google_key: String,

    pub twitch_client_id: String,

    pub discord_botsecret: String,
    pub temp_channel_prefix: String,
    pub announcements: ChannelId,
    pub voice_category: ChannelId,
    pub guild: GuildId,

    pub twitter_api_keys: KeyPair,
    pub twitter_users: Vec<String>,
}

impl Config {
    pub fn load_from_file<P: AsRef<Path>>(filename: P) -> Result<Config, Error> {
        let ini = Ini::load_from_file(filename)?;
        Ok(Config {
            username: ini
                .get_from(Some("lrrbot"), "username")
                .unwrap_or("lrrbot")
                .into(),

            database_url: ini
                .get_from(Some("lrrbot"), "postgres")
                .unwrap_or("postgres:///lrrbot")
                .into(),

            timezone: ini
                .get_from(Some("lrrbot"), "timezone")
                .unwrap_or("America/Vancouver")
                .parse::<Tz>()
                .map_err(failure::err_msg)
                .context("failed to parse the timezone")?,

            #[cfg(unix)]
            lrrbot_socket: ini
                .get_from(Some("lrrbot"), "socket_filename")
                .unwrap_or("lrrbot.sock")
                .into(),
            #[cfg(unix)]
            event_socket: ini
                .get_from(Some("lrrbot"), "eventsocket")
                .unwrap_or("/tmp/eventserver.sock")
                .into(),
            #[cfg(unix)]
            eris_socket: ini
                .get_from(Some("lrrbot"), "eris_socket")
                .unwrap_or("eris.sock")
                .into(),

            #[cfg(not(unix))]
            lrrbot_port: Config::get_option_parsed(&ini, "socket_port")?.unwrap_or(49601),
            #[cfg(not(unix))]
            event_port: Config::get_option_parsed(&ini, "event_port")?.unwrap_or(49602),
            #[cfg(not(unix))]
            eris_port: Config::get_option_parsed(&ini, "eris_port")?.unwrap_or(49603),

            google_key: Config::get_option_required(&ini, "google_key")?,

            twitch_client_id: Config::get_option_required(&ini, "twitch_clientid")?,

            discord_botsecret: Config::get_option_required(&ini, "discord_botsecret")?,

            temp_channel_prefix: ini
                .get_from(Some("lrrbot"), "discord_temp_channel_prefix")
                .unwrap_or("[TEMP]")
                .trim()
                .into(),
            announcements: ChannelId(
                Config::get_option_parsed(&ini, "discord_channel_announcements")?
                    .unwrap_or(322643668831961088),
            ),
            voice_category: ChannelId(
                Config::get_option_parsed(&ini, "discord_category_voice")?
                    .unwrap_or(360796352357072896),
            ),
            guild: GuildId(
                Config::get_option_parsed(&ini, "discord_serverid")?.unwrap_or(288920509272555520),
            ),
            twitter_api_keys: KeyPair::new(
                Config::get_option_required(&ini, "twitter_api_key")?,
                Config::get_option_required(&ini, "twitter_api_secret")?,
            ),
            twitter_users: ini
                .get_from(Some("lrrbot"), "twitter_users_to_monitor")
                .unwrap_or("loadingreadyrun")
                .trim()
                .split(",")
                .map(String::from)
                .collect(),
        })
    }

    fn get_option_required(ini: &Ini, option: &str) -> Result<String, Error> {
        Ok(ini
            .get_from(Some("lrrbot"), option)
            .ok_or_else(|| failure::err_msg(format!("{:?} is missing", option)))?
            .into())
    }

    fn get_option_parsed<T>(ini: &Ini, option: &str) -> Result<Option<T>, Error>
    where
        T: FromStr,
        T::Err: Fail,
    {
        match ini.get_from(Some("lrrbot"), option).map(str::parse) {
            Some(Ok(opt)) => Ok(Some(opt)),
            Some(Err(err)) => Err(err).with_context(|_| format!("failed to parse {:?}", option))?,
            None => Ok(None),
        }
    }
}
