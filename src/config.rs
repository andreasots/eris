#![allow(clippy::unreadable_literal)]

use chrono_tz::Tz;
use failure::{self, Error, Fail, ResultExt};
use ini::Ini;
use serenity::model::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use url::Url;

#[derive(Debug)]
pub struct Config {
    pub username: String,

    pub database_url: String,

    pub command_prefix: String,

    pub site_url: Url,

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
    pub mods_channel: ChannelId,
    pub general_channel: ChannelId,
    pub guild: GuildId,

    pub twitter_api_key: String,
    pub twitter_api_secret: String,
    pub twitter_users: HashMap<String, Vec<ChannelId>>,

    pub contact_spreadsheet: Option<String>,

    /// URL for the InfluxDB's write endpoint.
    pub influxdb: Option<Url>,
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

            command_prefix: ini
                .get_from(Some("lrrbot"), "commandprefix")
                .unwrap_or("!")
                .trim()
                .into(),

            site_url: Url::parse(
                ini.get_from(Some("lrrbot"), "siteurl")
                    .unwrap_or("https://lrrbot.com/"),
            )
            .context("failed to parse `siteurl`")?,

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
            mods_channel: ChannelId(
                Config::get_option_parsed(&ini, "discord_channel_mods")?
                    .unwrap_or(289166968307712000),
            ),
            general_channel: ChannelId(
                if let Some(channel_id) =
                    Config::get_option_parsed(&ini, "discord_channel_general")?
                {
                    channel_id
                } else {
                    Config::get_option_parsed(&ini, "discord_serverid")?
                        .unwrap_or(288920509272555520)
                },
            ),
            guild: GuildId(
                Config::get_option_parsed(&ini, "discord_serverid")?.unwrap_or(288920509272555520),
            ),
            twitter_api_key: Config::get_option_required(&ini, "twitter_api_key")?,
            twitter_api_secret: Config::get_option_required(&ini, "twitter_api_secret")?,
            twitter_users: ini
                .section(Some("eris.twitter"))
                .map(|section| {
                    section
                        .iter()
                        .map(|(name, channels)| {
                            Ok((
                                name.to_lowercase(),
                                channels
                                    .split(',')
                                    .map(|id| Ok(ChannelId(str::parse(id)?)))
                                    .collect::<Result<Vec<ChannelId>, Error>>()?,
                            ))
                        })
                        .collect::<Result<HashMap<String, Vec<ChannelId>>, Error>>()
                })
                .unwrap_or_else(|| {
                    let mut twitter = HashMap::new();
                    twitter.insert(
                        String::from("loadingreadyrun"),
                        vec![ChannelId(322643668831961088)],
                    );
                    twitter.insert(
                        String::from("desertbus"),
                        vec![ChannelId(370211226564689921)],
                    );
                    Ok(twitter)
                })?,

            contact_spreadsheet: ini
                .get_from(Some("lrrbot"), "discord_contact_spreadsheet")
                .map(String::from),

            influxdb: ini
                .get_from(Some("eris"), "influxdb")
                .map(Url::parse)
                .transpose()
                .context("failed to parse `[eris].influxdb`")?,
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
