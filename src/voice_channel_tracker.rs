use crate::config::Config;
use crate::extract::Extract;
use chrono::{DateTime, Utc};
use csv::{Writer, WriterBuilder};
use failure::{bail, format_err, Error, ResultExt};
use serde::Serialize;
use serenity::model::prelude::*;
use serenity::prelude::*;
use slog_scope::error;
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom};
use std::sync::{Arc, Mutex};

#[derive(Serialize)]
enum Event {
    GuildReady,
    Create,
    Update,
    Delete,
    StateUpdate,
}

#[derive(Serialize)]
struct Row<'a> {
    timestamp: DateTime<Utc>,
    channel_id: ChannelId,
    channel_name: &'a str,
    user_count: u64,
    event: Event,
}

pub struct VoiceChannelTracker {
    writer: Mutex<Writer<File>>,
}

impl VoiceChannelTracker {
    pub fn new(config: &Config) -> Result<VoiceChannelTracker, Error> {
        let mut file = OpenOptions::new()
            .write(true)
            .append(true)
            .create(true)
            .open(&config.voice_channel_data)
            .context("failed to open the voice channel data file")?;
        let end = file
            .seek(SeekFrom::End(0))
            .context("failed to determine file size")?;
        let writer = WriterBuilder::new().has_headers(end == 0).from_writer(file);

        Ok(VoiceChannelTracker {
            writer: Mutex::new(writer),
        })
    }
}

fn user_count_for(guild: &Guild, channel: ChannelId) -> u64 {
    let mut count = 0u64;

    for state in guild.voice_states.values() {
        if let Some(channel_id) = state.channel_id {
            if channel_id == channel {
                count += 1;
            }
        }
    }

    count
}

fn log_error<F: FnOnce() -> Result<(), Error>>(f: F) {
    match f() {
        Ok(()) => (),
        Err(err) => error!("Error in event handler"; "error" => ?err),
    }
}

impl EventHandler for VoiceChannelTracker {
    fn ready(&self, ctx: Context, _data_about_bot: Ready) {
        let data = ctx.data.read();
        let config = data.extract::<Config>().unwrap();
        ctx.set_activity(Activity::listening(&format!(
            "{}help || v{}-pre.{}",
            config.command_prefix,
            env!("CARGO_PKG_VERSION"),
            option_env!("TRAVIS_BUILD_NUMBER").unwrap_or("---")
        )));
    }

    fn channel_create(&self, ctx: Context, channel: Arc<RwLock<GuildChannel>>) {
        log_error(|| {
            let channel = channel.read();
            if channel.kind != ChannelType::Voice {
                return Ok(());
            }

            let guild = match channel.guild(&ctx) {
                Some(guild) => guild,
                None => bail!("failed to get the guild for the channel {:?}", channel.name),
            };

            let mut writer = self
                .writer
                .lock()
                .map_err(|err| format_err!("{}", err))
                .context("writer is poisoned")?;
            writer
                .serialize(Row {
                    timestamp: Utc::now(),
                    channel_id: channel.id,
                    channel_name: &channel.name,
                    user_count: user_count_for(&guild.read(), channel.id),
                    event: Event::Create,
                })
                .context("failed to append to the voice channel log")?;
            writer
                .flush()
                .context("failed to flush the voice channel log")?;

            Ok(())
        });
    }

    fn channel_delete(&self, ctx: Context, channel: Arc<RwLock<GuildChannel>>) {
        log_error(|| {
            let channel = channel.read();
            if channel.kind != ChannelType::Voice {
                return Ok(());
            }

            let guild = match channel.guild(&ctx) {
                Some(guild) => guild,
                None => bail!("failed to get the guild for the channel {:?}", channel.name),
            };

            let mut writer = self
                .writer
                .lock()
                .map_err(|err| format_err!("{}", err))
                .context("writer is poisoned")?;
            writer
                .serialize(Row {
                    timestamp: Utc::now(),
                    channel_id: channel.id,
                    channel_name: &channel.name,
                    user_count: user_count_for(&guild.read(), channel.id),
                    event: Event::Delete,
                })
                .context("failed to append to the voice channel log")?;
            writer
                .flush()
                .context("failed to flush the voice channel log")?;

            Ok(())
        });
    }

    fn channel_update(&self, ctx: Context, _old: Option<Channel>, new: Channel) {
        log_error(|| {
            let channel = if let Some(channel) = new.guild() {
                channel
            } else {
                return Ok(());
            };

            let channel = channel.read();
            if channel.kind != ChannelType::Voice {
                return Ok(());
            }

            let guild = match channel.guild(&ctx) {
                Some(guild) => guild,
                None => bail!("failed to get the guild for the channel {:?}", channel.name),
            };

            let mut writer = self
                .writer
                .lock()
                .map_err(|err| format_err!("{}", err))
                .context("writer is poisoned")?;

            writer
                .serialize(Row {
                    timestamp: Utc::now(),
                    channel_id: channel.id,
                    channel_name: &channel.name,
                    user_count: user_count_for(&guild.read(), channel.id),
                    event: Event::Update,
                })
                .context("failed to append to the voice channel log")?;
            writer
                .flush()
                .context("failed to flush the voice channel log")?;

            Ok(())
        });
    }

    fn guild_create(&self, _ctx: Context, guild: Guild, _is_new: bool) {
        log_error(|| {
            let mut writer = self
                .writer
                .lock()
                .map_err(|err| format_err!("{}", err))
                .context("writer is poisoned")?;

            let now = Utc::now();

            for channel in guild.channels.values() {
                let channel = channel.read();

                if channel.kind != ChannelType::Voice {
                    continue;
                }

                writer
                    .serialize(Row {
                        timestamp: now,
                        channel_id: channel.id,
                        channel_name: &channel.name,
                        user_count: user_count_for(&guild, channel.id),
                        event: Event::GuildReady,
                    })
                    .context("failed to append to the voice channel log")?;
            }
            writer
                .flush()
                .context("failed to flush the voice channel log")?;

            Ok(())
        });
    }

    fn voice_state_update(
        &self,
        ctx: Context,
        guild: Option<GuildId>,
        old: Option<VoiceState>,
        new: VoiceState,
    ) {
        log_error(|| {
            let mut writer = self
                .writer
                .lock()
                .map_err(|err| format_err!("{}", err))
                .context("writer is poisoned")?;

            let guild = match guild {
                Some(guild) => guild,
                None => return Ok(()),
            };

            let now = Utc::now();

            let channels = (&[old.and_then(|state| state.channel_id), new.channel_id][..])
                .iter()
                .flat_map(Clone::clone)
                .collect::<HashSet<ChannelId>>();

            let cache = ctx.cache.read();

            for channel_id in channels {
                let guild = match cache.guilds.get(&guild) {
                    Some(guild) => guild,
                    None => {
                        error!("Failed to get the guild"; "guild" => ?guild.0);
                        continue;
                    }
                };
                let guild = guild.read();

                let channel = match guild.channels.get(&channel_id).cloned() {
                    Some(channel) => channel,
                    None => continue,
                };
                let channel = channel.read();

                writer
                    .serialize(Row {
                        timestamp: now,
                        channel_id,
                        channel_name: &channel.name,
                        user_count: user_count_for(&guild, channel_id),
                        event: Event::StateUpdate,
                    })
                    .context("failed to append to the voice channel log")?;
            }
            writer
                .flush()
                .context("failed to flush the voice channel log")?;

            Ok(())
        });
    }
}
