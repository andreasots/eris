use crate::config::Config;
use crate::context::ErisContext;
use crate::extract::Extract;
use anyhow::Error;
use chrono::Utc;
use serenity::model::prelude::*;
use std::collections::HashMap;
use std::time::Duration;
use tracing::{error, info};

const STARTUP_DELAY: Duration = Duration::from_secs(5);
const REAP_INTERVAL: Duration = Duration::from_secs(60);
const MIN_CHANNEL_AGE: Duration = Duration::from_secs(15 * 60);

async fn reap_channels(ctx: &ErisContext) -> Result<(), Error> {
    let data = ctx.data.read().await;
    let config = data.extract::<Config>()?;
    let guild = config
        .guild
        .to_guild_cached(&ctx)
        .await
        .ok_or_else(|| Error::msg("failed to get the guild"))?;

    let mut voice_users = HashMap::<ChannelId, u64>::new();
    for voice_state in guild.voice_states.values() {
        if let Some(channel) = voice_state.channel_id {
            *voice_users.entry(channel).or_insert(0) += 1;
        }
    }

    let now = Utc::now();

    let mut unused_channels = vec![];

    for channel in guild.channels.values() {
        if channel.kind != ChannelType::Voice
            || !channel.name.starts_with(&config.temp_channel_prefix)
        {
            continue;
        }

        let created_at = channel.id.created_at().with_timezone(&Utc);

        if (now - created_at).to_std()? > MIN_CHANNEL_AGE
            && voice_users.get(&channel.id).copied().unwrap_or(0) == 0
        {
            info!(
                channel.id = channel.id.0,
                channel.name = channel.name.as_str(),
                "Scheduling a temporary channel for deletion"
            );
            unused_channels.push(channel.id);
        }
    }

    for channel_id in unused_channels {
        if let Err(error) = channel_id.delete(&ctx).await {
            error!(?error, channel.id = channel_id.0, "Failed to delete a temporary channel");
        }
    }

    Ok(())
}

pub async fn channel_reaper(ctx: ErisContext) {
    // Delay the first reap so that it doesn't happen before the Discord connection is ready.
    tokio::time::sleep(STARTUP_DELAY).await;

    loop {
        if let Err(error) = reap_channels(&ctx).await {
            error!(?error, "Failed to reap channels");
        }
        tokio::time::sleep(REAP_INTERVAL).await;
    }
}
