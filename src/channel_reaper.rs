use std::sync::Arc;
use std::time::Duration;

use time::OffsetDateTime;
use tracing::{error, info};
use twilight_cache_inmemory::InMemoryCache;
use twilight_http::Client;
use twilight_model::channel::ChannelType;
use twilight_util::snowflake::Snowflake;

use crate::config::Config;

const REAP_INTERVAL: Duration = Duration::from_secs(60);
const MIN_CHANNEL_AGE: Duration = Duration::from_secs(15 * 60);

pub async fn channel_reaper(cache: Arc<InMemoryCache>, config: Arc<Config>, discord: Arc<Client>) {
    let mut interval = tokio::time::interval(REAP_INTERVAL);

    loop {
        interval.tick().await;

        let now = OffsetDateTime::now_utc();

        let Some(channels) = cache
            .guild_channels(config.guild) else { continue };

        for channel_id in channels.iter().copied() {
            let Some(channel) = cache.channel(channel_id) else {
                info!(channel.id = channel_id.get(), "Channel not in cache");
                continue
            };

            let created_at = channel_id.timestamp();
            let created_at_fraction = Duration::from_millis((created_at % 1000) as u64);
            let created_at = match OffsetDateTime::from_unix_timestamp(created_at / 1000) {
                Ok(created_at) => created_at + created_at_fraction,
                Err(error) => {
                    info!(channel.id = channel_id.get(), ?error, "timestamp out of range");
                    continue;
                }
            };

            if channel.kind != ChannelType::GuildVoice {
                continue;
            }

            if !channel.name.as_deref().unwrap_or("").starts_with(&config.temp_channel_prefix) {
                continue;
            }

            if (now - created_at) < MIN_CHANNEL_AGE {
                continue;
            }

            let member_count =
                cache.voice_channel_states(channel_id).map(|states| states.count()).unwrap_or(0);
            if member_count > 0 {
                continue;
            }

            if let Err(error) = discord.delete_channel(channel_id).await {
                error!(
                    ?error,
                    channel.id = channel_id.get(),
                    "failed to delete a temporary channel"
                );
            }
        }
    }
}
