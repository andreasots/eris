use crate::config::Config;
use crate::extract::Extract;
use crate::influxdb::{InfluxDB, Measurement, New, Timestamp};
use anyhow::{bail, Context as _, Error};
use joinery::Joinable;
use serenity::async_trait;
use serenity::http::client::Http;
use serenity::model::prelude::*;
use serenity::prelude::*;
use tracing::error;
use std::convert::TryFrom;
use std::future::Future;

pub struct DiscordEvents;

impl DiscordEvents {
    pub fn new() -> Self {
        Self
    }
}

impl DiscordEvents {
    fn users_for(guild: &Guild, channel: ChannelId) -> Vec<UserId> {
        guild
            .voice_states
            .values()
            .filter(|state| state.channel_id == Some(channel))
            .map(|state| state.user_id)
            .collect()
    }

    async fn log_error<F: FnOnce() -> T, T: Future<Output = Result<(), Error>>>(f: F) {
        match f().await {
            Ok(()) => (),
            Err(error) => error!(?error, "Error in event handler"),
        }
    }

    async fn set_activity(&self, ctx: Context) {
        let data = ctx.data.read().await;
        let config = data.extract::<Config>().unwrap();
        let activity = if let Some(build_number) = option_env!("TRAVIS_BUILD_NUMBER") {
            format!(
                "{}help || v{}+{}",
                config.command_prefix,
                env!("CARGO_PKG_VERSION"),
                build_number
            )
        } else {
            format!("{}help || v{}", config.command_prefix, env!("CARGO_PKG_VERSION"))
        };
        ctx.set_activity(Activity::listening(&activity)).await;
    }

    fn create_measurement_for_channel<'a>(
        &self,
        channel: &'a GuildChannel,
        event: &'a str,
    ) -> Measurement<'a, New> {
        let kind = match channel.kind {
            ChannelType::Voice => "voice_channels",
            ChannelType::Text => "text_channels",
            kind => unimplemented!("channel type: {:?}", kind),
        };

        let mut measurement = Measurement::new(kind, Timestamp::Now)
            .add_tag("channel_id", channel.id.to_string())
            .add_tag("channel_name", channel.name.as_str())
            .add_tag("event", event);
        if let Some(category_id) = channel.category_id {
            measurement = measurement.add_tag("category_id", category_id.to_string());
        }

        measurement
    }

    async fn kick_from_voice(
        &self,
        http: &Http,
        guild: GuildId,
        user: UserId,
    ) -> Result<(), Error> {
        guild.disconnect_member(http, user).await?;
        Ok(())
    }
}

#[async_trait]
impl EventHandler for DiscordEvents {
    async fn ready(&self, ctx: Context, _data_about_bot: Ready) {
        self.set_activity(ctx).await
    }

    async fn resume(&self, ctx: Context, _event: ResumedEvent) {
        self.set_activity(ctx).await
    }

    async fn channel_create(&self, ctx: Context, channel: &GuildChannel) {
        Self::log_error(|| async {
            let data = ctx.data.read().await;
            if let Some(influxdb) = data.get::<InfluxDB>() {
                let measurement = match channel.kind {
                    ChannelType::Voice => {
                        let guild = match channel.guild(&ctx).await {
                            Some(guild) => guild,
                            None => {
                                bail!("failed to get the guild for the channel {:?}", channel.name)
                            }
                        };

                        let users = Self::users_for(&guild, channel.id);

                        let mut measurement =
                            self.create_measurement_for_channel(&channel, "create").add_field(
                                "count",
                                i64::try_from(users.len()).unwrap_or(std::i64::MAX),
                            );
                        if !users.is_empty() {
                            measurement =
                                measurement.add_field("users", users.join_with(',').to_string());
                        }
                        measurement
                    }
                    ChannelType::Text => self
                        .create_measurement_for_channel(&channel, "create")
                        .add_field("count", 0),
                    _ => return Ok(()),
                };

                influxdb
                    .write(&[measurement])
                    .await
                    .context("failed to write the user count to InfluxDB")?;
            }
            Ok(())
        })
        .await
    }

    async fn channel_delete(&self, ctx: Context, channel: &GuildChannel) {
        Self::log_error(|| async {
            let data = ctx.data.read().await;
            if let Some(influxdb) = data.get::<InfluxDB>() {
                let measurement = match channel.kind {
                    ChannelType::Voice => self
                        .create_measurement_for_channel(&channel, "delete")
                        .add_field("count", 0),
                    ChannelType::Text => self
                        .create_measurement_for_channel(&channel, "delete")
                        .add_field("count", 0),
                    _ => return Ok(()),
                };

                influxdb
                    .write(&[measurement])
                    .await
                    .context("failed to write the user count to InfluxDB")?;
            }
            Ok(())
        })
        .await
    }

    async fn channel_update(&self, ctx: Context, _old: Option<Channel>, new: Channel) {
        Self::log_error(|| async {
            let data = ctx.data.read().await;
            if let Some(influxdb) = data.get::<InfluxDB>() {
                let channel = if let Some(channel) = new.guild() {
                    channel
                } else {
                    return Ok(());
                };
                let measurement = match channel.kind {
                    ChannelType::Voice => {
                        let guild = match channel.guild(&ctx).await {
                            Some(guild) => guild,
                            None => {
                                bail!("failed to get the guild for the channel {:?}", channel.name)
                            }
                        };

                        let users = Self::users_for(&guild, channel.id);

                        let mut measurement =
                            self.create_measurement_for_channel(&channel, "update").add_field(
                                "count",
                                i64::try_from(users.len()).unwrap_or(std::i64::MAX),
                            );
                        if !users.is_empty() {
                            measurement =
                                measurement.add_field("users", users.join_with(',').to_string());
                        }
                        measurement
                    }
                    ChannelType::Text => self
                        .create_measurement_for_channel(&channel, "update")
                        .add_field("count", 0),
                    _ => return Ok(()),
                };

                influxdb
                    .write(&[measurement])
                    .await
                    .context("failed to write the user count to InfluxDB")?;
            }
            Ok(())
        })
        .await
    }

    async fn guild_create(&self, ctx: Context, guild: Guild, _is_new: bool) {
        if let Some(afk_channel) = guild.afk_channel_id {
            for (&user, voice_state) in &guild.voice_states {
                if voice_state.channel_id == Some(afk_channel) {
                    if let Err(error) = self.kick_from_voice(&ctx.http, guild.id, user).await {
                        error!(?error, "failed to kick user from the AFK channel");
                    }
                }
            }
        }

        Self::log_error(|| async {
            let data = ctx.data.read().await;
            if let Some(influxdb) = data.get::<InfluxDB>() {
                let measurements = guild
                    .channels
                    .values()
                    .filter_map(|channel| match channel.kind {
                        ChannelType::Text => {
                            let measurement = self
                                .create_measurement_for_channel(&channel, "guild_create")
                                .add_field("count", 0);
                            Some(measurement)
                        }
                        ChannelType::Voice => {
                            let users = Self::users_for(&guild, channel.id);

                            let mut measurement = self
                                .create_measurement_for_channel(&channel, "guild_create")
                                .add_field(
                                    "count",
                                    i64::try_from(users.len()).unwrap_or(std::i64::MAX),
                                );
                            if !users.is_empty() {
                                measurement = measurement
                                    .add_field("users", users.join_with(',').to_string());
                            }
                            Some(measurement)
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();

                influxdb
                    .write(&measurements)
                    .await
                    .context("failed to write the user count to InfluxDB")?;
            }
            Ok(())
        })
        .await
    }

    async fn voice_state_update(
        &self,
        ctx: Context,
        guild: Option<GuildId>,
        old: Option<VoiceState>,
        new: VoiceState,
    ) {
        Self::log_error(|| async {
            let data = ctx.data.read().await;

            if let Some(guild) = guild {
                if let Some(guild) = guild.to_guild_cached(&ctx).await {
                    if let Some(afk_channel) = guild.afk_channel_id {
                        if new.channel_id == Some(afk_channel) {
                            if let Err(error) =
                                self.kick_from_voice(&ctx.http, guild.id, new.user_id).await
                            {
                                error!(?error, "failed to kick user from the AFK channel");
                            }
                        }
                    }
                }
            }

            if let Some(influxdb) = data.get::<InfluxDB>() {
                let guild = match guild {
                    Some(guild) => guild
                        .to_guild_cached(&ctx)
                        .await
                        .ok_or_else(|| Error::msg("failed to get the guild"))?,
                    None => return Ok(()),
                };

                let channels = old
                    .and_then(|state| state.channel_id)
                    .into_iter()
                    .filter(|channel_id| Some(*channel_id) != new.channel_id)
                    .chain(new.channel_id);

                let mut measurements = Vec::with_capacity(2);
                for channel_id in channels {
                    let channel = match guild.channels.get(&channel_id) {
                        Some(channel) => channel,
                        None => continue,
                    };

                    let users = Self::users_for(&guild, channel.id);

                    let mut measurement = self
                        .create_measurement_for_channel(&channel, "state_update")
                        .add_field("count", i64::try_from(users.len()).unwrap_or(std::i64::MAX));
                    if !users.is_empty() {
                        measurement =
                            measurement.add_field("users", users.join_with(',').to_string());
                    }
                    measurements.push(measurement);
                }

                influxdb
                    .write(&measurements)
                    .await
                    .context("failed to write the user count to InfluxDB")?;
            }

            Ok(())
        })
        .await
    }

    async fn message(&self, ctx: Context, new_message: Message) {
        Self::log_error(|| async {
            let data = ctx.data.read().await;
            if let Some(influxdb) = data.get::<InfluxDB>() {
                if let Some(channel) = new_message.channel(&ctx).await.and_then(Channel::guild) {
                    let measurement = self
                        .create_measurement_for_channel(&channel, "message")
                        .add_tag("user_id", new_message.author.id.to_string())
                        .add_field("count", 1);

                    influxdb
                        .write(&[measurement])
                        .await
                        .context("failed to write the user count to InfluxDB")?;
                }
            }
            Ok(())
        })
        .await
    }
}
