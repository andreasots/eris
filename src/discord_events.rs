use crate::config::Config;
use crate::extract::Extract;
use failure::{bail, Error, ResultExt};
use serenity::model::prelude::*;
use serenity::prelude::*;
use slog_scope::error;
use std::sync::Arc;
use crate::influxdb::{InfluxDB, Timestamp, Measurement};
use joinery::Joinable;
use std::convert::TryFrom;
use crate::typemap_keys::Executor;
use crate::executor_ext::ExecutorExt;

pub struct DiscordEvents;

impl DiscordEvents {
    pub fn new() -> Self {
        Self
    }
}

impl DiscordEvents {
    fn users_for(guild: &Guild, channel: ChannelId) -> Vec<UserId> {
        guild.voice_states.values()
            .filter(|state| state.channel_id == Some(channel))
            .map(|state| state.user_id)
            .collect()
    }

    fn log_error<F: FnOnce() -> Result<(), Error>>(f: F) {
        match f() {
            Ok(()) => (),
            Err(err) => error!("Error in event handler"; "error" => ?err),
        }
    }

    fn set_activity(&self, ctx: Context) {
        let data = ctx.data.read();
        let config = data.extract::<Config>().unwrap();
        let activity = if let Some(build_number) = option_env!("TRAVIS_BUILD_NUMBER") {
            format!("{}help || v{}+{}", config.command_prefix, env!("CARGO_PKG_VERSION"), build_number)
        } else {
            format!("{}help || v{}", config.command_prefix, env!("CARGO_PKG_VERSION"))
        };
        ctx.set_activity(Activity::listening(&activity));
    }
}

impl EventHandler for DiscordEvents {
    fn ready(&self, ctx: Context, _data_about_bot: Ready) {
        self.set_activity(ctx);
    }

    fn resume(&self, ctx: Context, _event: ResumedEvent) {
        self.set_activity(ctx);
    }

    fn channel_create(&self, ctx: Context, channel: Arc<RwLock<GuildChannel>>) {
        Self::log_error(|| {
            let data = ctx.data.read();
            if let Some(influxdb) = data.get::<InfluxDB>() {
                let executor = data.extract::<Executor>()?;

                let channel = channel.read();
                if channel.kind != ChannelType::Voice {
                    return Ok(());
                }

                let guild = match channel.guild(&ctx) {
                    Some(guild) => guild,
                    None => bail!("failed to get the guild for the channel {:?}", channel.name),
                };

                let users = Self::users_for(&guild.read(), channel.id);

                let mut measurement = Measurement::new("voice_channels", Timestamp::Now)
                    .add_tag("channel_id", channel.id.to_string())
                    .add_tag("channel_name", channel.name.clone())
                    .add_tag("event", "create")
                    .add_field("count", i64::try_from(users.len()).unwrap_or(std::i64::MAX));
                if ! users.is_empty() {
                    measurement = measurement.add_field("users", users.join_with(',').to_string());
                }

                let influxdb = influxdb.clone();
                executor.block_on(async move { influxdb.write(&[measurement]).await })
                    .context("failed to write the user count to InfluxDB")?;
            }
            Ok(())
        })
    }

    fn channel_delete(&self, ctx: Context, channel: Arc<RwLock<GuildChannel>>) {
        Self::log_error(|| {
            let data = ctx.data.read();
            if let Some(influxdb) = data.get::<InfluxDB>() {
                let executor = data.extract::<Executor>()?;

                let channel = channel.read();
                if channel.kind != ChannelType::Voice {
                    return Ok(());
                }

                let measurement = Measurement::new("voice_channels", Timestamp::Now)
                    .add_tag("channel_id", channel.id.to_string())
                    .add_tag("channel_name", channel.name.clone())
                    .add_tag("event", "delete")
                    .add_field("count", 0);

                let influxdb = influxdb.clone();
                executor.block_on(async move { influxdb.write(&[measurement]).await })
                    .context("failed to write the user count to InfluxDB")?;
            }
            Ok(())
        })
    }

    fn channel_update(&self, ctx: Context, _old: Option<Channel>, new: Channel) {
        Self::log_error(|| {
            let data = ctx.data.read();
            if let Some(influxdb) = data.get::<InfluxDB>() {
                let executor = data.extract::<Executor>()?;

                let channel = if let Some(channel) = new.guild() {
                    channel
                } else {
                    return Ok(())
                };
                let channel = channel.read();

                if channel.kind != ChannelType::Voice {
                    return Ok(());
                }

                let guild = match channel.guild(&ctx) {
                    Some(guild) => guild,
                    None => bail!("failed to get the guild for the channel {:?}", channel.name),
                };

                let users = Self::users_for(&guild.read(), channel.id);

                let mut measurement = Measurement::new("voice_channels", Timestamp::Now)
                    .add_tag("channel_id", channel.id.to_string())
                    .add_tag("channel_name", channel.name.clone())
                    .add_tag("event", "update")
                    .add_field("count", i64::try_from(users.len()).unwrap_or(std::i64::MAX));
                if ! users.is_empty() {
                    measurement = measurement.add_field("users", users.join_with(',').to_string());
                }

                let influxdb = influxdb.clone();
                executor.block_on(async move { influxdb.write(&[measurement]).await })
                    .context("failed to write the user count to InfluxDB")?;
            }
            Ok(())
        })
    }

    fn guild_create(&self, ctx: Context, guild: Guild, _is_new: bool) {
        Self::log_error(|| {
            let data = ctx.data.read();
            if let Some(influxdb) = data.get::<InfluxDB>() {
                let executor = data.extract::<Executor>()?;

                let measurements = guild.channels.values()
                    .map(|channel| channel.read())
                    .filter(|channel| channel.kind == ChannelType::Voice)
                    .map(|channel| {
                        let users = Self::users_for(&guild, channel.id);

                        let mut measurement = Measurement::new("voice_channels", Timestamp::Now)
                            .add_tag("channel_id", channel.id.to_string())
                            .add_tag("channel_name", channel.name.clone())
                            .add_tag("event", "create")
                            .add_field("count", i64::try_from(users.len()).unwrap_or(std::i64::MAX));
                        if ! users.is_empty() {
                            measurement = measurement.add_field("users", users.join_with(',').to_string());
                        }
                        measurement
                    })
                    .collect::<Vec<_>>();

                let influxdb = influxdb.clone();
                executor.block_on(async move { influxdb.write(&measurements).await })
                    .context("failed to write the user count to InfluxDB")?;
            }
            Ok(())
        })
    }

    fn voice_state_update(
        &self,
        ctx: Context,
        guild: Option<GuildId>,
        old: Option<VoiceState>,
        new: VoiceState,
    ) {
        Self::log_error(|| {
            let data = ctx.data.read();

            if let Some(influxdb) = data.get::<InfluxDB>() {
                let executor = data.extract::<Executor>()?;

                let guild = match guild {
                    Some(guild) => guild.to_guild_cached(&ctx).ok_or_else(|| failure::err_msg("failed to get the guild"))?,
                    None => return Ok(()),
                };

                let channels = old.and_then(|state| state.channel_id)
                    .into_iter()
                    .filter(|channel_id| Some(*channel_id) != new.channel_id)
                    .chain(new.channel_id);

                let mut measurements = Vec::with_capacity(2);
                for channel_id in channels {
                    let guild = guild.read();

                    let channel = match guild.channels.get(&channel_id).cloned() {
                        Some(channel) => channel,
                        None => continue,
                    };
                    let channel = channel.read();

                    let users = Self::users_for(&guild, channel.id);

                    let mut measurement = Measurement::new("voice_channels", Timestamp::Now)
                        .add_tag("channel_id", channel.id.to_string())
                        .add_tag("channel_name", channel.name.clone())
                        .add_tag("event", "state_update")
                        .add_field("count", i64::try_from(users.len()).unwrap_or(std::i64::MAX));
                    if ! users.is_empty() {
                        measurement = measurement.add_field("users", users.join_with(',').to_string());
                    }
                    measurements.push(measurement);
                }

                let influxdb = influxdb.clone();
                executor.block_on(async move { influxdb.write(&measurements).await })
                        .context("failed to write the user count to InfluxDB")?;
            }

            Ok(())
        })
    }

    fn message(&self, ctx: Context, new_message: Message) {
        Self::log_error(|| {
            let data = ctx.data.read();
            if let Some(influxdb) = data.get::<InfluxDB>() {
                let executor = data.extract::<Executor>()?;

                if let Some(channel) = new_message.channel(&ctx).and_then(Channel::guild) {
                    let channel = channel.read();

                    let measurement = Measurement::new("text_channels", Timestamp::Now)
                        .add_tag("channel_id", channel.id.to_string())
                        .add_tag("channel_name", channel.name.clone())
                        .add_tag("event", "message")
                        .add_tag("user_id", new_message.author.id.to_string())
                        .add_field("count", 1);

                    let influxdb = influxdb.clone();
                    executor.block_on(async move { influxdb.write(&[measurement]).await })
                        .context("failed to write the user count to InfluxDB")?;
                }
            }
            Ok(())
        })
    }
}
