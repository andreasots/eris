use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use tokio::sync::watch::Receiver;
use tracing::{error, info};
use twilight_http::Client;
use twilight_model::channel::ChannelType;
use twilight_util::snowflake::Snowflake;

use crate::cache::Cache;
use crate::config::Config;

const REAP_INTERVAL: Duration = Duration::from_secs(60);
const MIN_CHANNEL_AGE: Duration = Duration::from_secs(15 * 60);

pub async fn channel_reaper(
    mut running: Receiver<bool>,
    cache: Arc<Cache>,
    config: Arc<Config>,
    discord: Arc<Client>,
) {
    let mut interval = tokio::time::interval(REAP_INTERVAL);

    loop {
        tokio::select! {
            _ = running.changed() => break,
            _ = interval.tick() => {
                cache.wait_until_ready().await;

                let now = Utc::now();

                let channels_to_delete = cache.with(|cache| {
                    let Some(guild_channels) = cache.guild_channels(config.guild) else { return vec![] };
                    guild_channels.iter()
                        .copied()
                        .flat_map(|channel_id| cache.channel(channel_id))
                        .filter(|channel| channel.kind == ChannelType::GuildVoice)
                        .filter(|channel| channel.name.as_deref().unwrap_or("").starts_with(&config.temp_channel_prefix))
                        .filter(|channel| {
                            let Some(created_at) = Utc.timestamp_millis_opt(channel.id.timestamp()).latest() else {
                                info!(channel.id = channel.id.get(), "timestamp out of range");
                                return false;
                            };

                            created_at + MIN_CHANNEL_AGE < now
                        })
                        .filter(|channel| cache.voice_channel_states(channel.id).map_or(0, Iterator::count) == 0)
                        .map(|channel| channel.id)
                        .collect()
                });

                for channel_id in channels_to_delete {
                    if let Err(error) = discord.delete_channel(channel_id).await {
                        error!(
                            ?error,
                            channel.id = channel_id.get(),
                            "failed to delete a temporary channel"
                        );
                    }
                }
            },
        }
    }
}
