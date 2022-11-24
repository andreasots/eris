#![allow(clippy::unreadable_literal)]

use std::collections::HashMap;
use std::error::Error as StdError;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{anyhow, Error};
use egg_mode::KeyPair;
use ini::Ini;
use time_tz::Tz;
use twilight_model::id::marker::{ChannelMarker, GuildMarker};
use twilight_model::id::Id;
use twitch_api::twitch_oauth2::{ClientId, ClientSecret};

#[derive(Debug)]
pub struct Config {
    pub username: String,
    pub channel: String,

    pub database_url: String,

    pub command_prefix: String,

    pub timezone: &'static Tz,

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

    pub twitch_client_id: ClientId,
    pub twitch_client_secret: ClientSecret,

    pub discord_botsecret: String,
    pub temp_channel_prefix: String,
    pub announcements: Id<ChannelMarker>,
    pub voice_category: Id<ChannelMarker>,
    pub mods_channel: Id<ChannelMarker>,
    pub general_channel: Id<ChannelMarker>,
    pub lrr_videos_channel: Option<Id<ChannelMarker>>,
    pub guild: Id<GuildMarker>,

    pub twitter_api: KeyPair,
    pub twitter_users: HashMap<String, Vec<Id<ChannelMarker>>>,

    pub contact_spreadsheet: Option<String>,

    pub influxdb: Option<(String, String)>,

    pub youtube_channels: Vec<String>,
}

impl Config {
    pub fn load_from_file<P: AsRef<Path>>(filename: P) -> Result<Config, Error> {
        let ini = Ini::load_from_file(filename)?;
        Ok(Config {
            username: ini.get_from(Some("lrrbot"), "username").unwrap_or("lrrbot").into(),
            channel: ini.get_from(Some("lrrbot"), "channel").unwrap_or("loadingreadyrun").into(),

            database_url: ini
                .get_from(Some("lrrbot"), "postgres")
                .unwrap_or("postgres:///lrrbot")
                .into(),

            command_prefix: ini
                .get_from(Some("lrrbot"), "commandprefix")
                .unwrap_or("!")
                .trim()
                .into(),

            timezone: {
                let timezone =
                    ini.get_from(Some("lrrbot"), "timezone").unwrap_or("America/Vancouver");
                time_tz::timezones::get_by_name(timezone)
                    .ok_or_else(|| Error::msg(format!("unknown timezone: {timezone}")))?
            },

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
            eris_socket: ini.get_from(Some("lrrbot"), "eris_socket").unwrap_or("eris.sock").into(),

            #[cfg(not(unix))]
            lrrbot_port: Config::get_option_parsed(&ini, "socket_port")?.unwrap_or(49601),
            #[cfg(not(unix))]
            event_port: Config::get_option_parsed(&ini, "event_port")?.unwrap_or(49602),
            #[cfg(not(unix))]
            eris_port: Config::get_option_parsed(&ini, "eris_port")?.unwrap_or(49603),

            google_key: Config::get_option_required(&ini, "google_key")?,

            twitch_client_id: ClientId::new(Config::get_option_required(&ini, "twitch_clientid")?),
            twitch_client_secret: ClientSecret::new(Config::get_option_required(
                &ini,
                "twitch_clientsecret",
            )?),

            discord_botsecret: Config::get_option_required(&ini, "discord_botsecret")?,

            temp_channel_prefix: ini
                .get_from(Some("lrrbot"), "discord_temp_channel_prefix")
                .unwrap_or("[TEMP]")
                .trim()
                .into(),
            announcements: Config::get_option_parsed(&ini, "discord_channel_announcements")?
                .unwrap_or(Id::new(322643668831961088)),
            voice_category: Config::get_option_parsed(&ini, "discord_category_voice")?
                .unwrap_or(Id::new(360796352357072896)),
            mods_channel: Config::get_option_parsed(&ini, "discord_channel_mods")?
                .unwrap_or(Id::new(289166968307712000u64)),
            general_channel: if let Some(channel_id) =
                Config::get_option_parsed(&ini, "discord_channel_general")?
            {
                channel_id
            } else {
                Config::get_option_parsed(&ini, "discord_serverid")?
                    .unwrap_or(Id::new(288920509272555520))
            },
            lrr_videos_channel: Config::get_option_parsed(&ini, "discord_channel_lrr_videos")?,
            guild: Config::get_option_parsed(&ini, "discord_serverid")?
                .unwrap_or(Id::new(288920509272555520)),
            twitter_api: KeyPair::new(
                Config::get_option_required(&ini, "twitter_api_key")?,
                Config::get_option_required(&ini, "twitter_api_secret")?,
            ),
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
                                    .map(|id| Ok(str::parse(id)?))
                                    .collect::<Result<Vec<Id<ChannelMarker>>, Error>>()?,
                            ))
                        })
                        .collect::<Result<HashMap<String, Vec<Id<ChannelMarker>>>, Error>>()
                })
                .unwrap_or_else(|| {
                    Ok(HashMap::from([
                        (String::from("loadingreadyrun"), vec![Id::new(322643668831961088)]),
                        (String::from("desertbus"), vec![Id::new(370211226564689921)]),
                    ]))
                })?,

            contact_spreadsheet: ini
                .get_from(Some("lrrbot"), "discord_contact_spreadsheet")
                .map(String::from),

            influxdb: {
                let url = ini.get_from(Some("eris"), "influxdb").map(String::from);
                let db = ini.get_from(Some("eris"), "influxdb_database").map(String::from);

                url.and_then(|url| db.map(|db| (url, db)))
            },

            youtube_channels: ini
                .get_from(Some("lrrbot"), "youtube_channels")
                .map(str::trim)
                .filter(|opt| !opt.is_empty())
                .into_iter()
                .flat_map(|opt| opt.split(','))
                .map(str::trim)
                .map(String::from)
                .collect(),
        })
    }

    fn get_option_required(ini: &Ini, option: &str) -> Result<String, Error> {
        Ok(ini
            .get_from(Some("lrrbot"), option)
            .ok_or_else(|| anyhow!("{:?} is missing", option))?
            .into())
    }

    fn get_option_parsed<T>(ini: &Ini, option: &str) -> Result<Option<T>, Error>
    where
        T: FromStr,
        T::Err: StdError + Send + Sync + 'static,
    {
        match ini.get_from(Some("lrrbot"), option).map(str::parse) {
            Some(Ok(opt)) => Ok(Some(opt)),
            Some(Err(err)) => {
                Err(Error::from(err).context(format!("failed to parse {:?}", option)))
            }
            None => Ok(None),
        }
    }
}
