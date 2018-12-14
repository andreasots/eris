use crate::config::Config;
use chrono::{DateTime, Utc};
use failure::{self, Error, ResultExt, SyncFailure};
use serenity::model::prelude::*;
use serenity::CACHE;
use slog::{slog_error, slog_info};
use slog_scope::{error, info};
use std::collections::HashMap;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

fn reap_channels(config: &Config) -> Result<(), Error> {
    let guild = CACHE
        .read()
        .guilds
        .get(&config.guild)
        .cloned()
        .ok_or_else(|| failure::err_msg("failed to get the guild"))?;
    let channels = config
        .guild
        .channels()
        .map_err(SyncFailure::new)
        .context("failed to fetch channels")?;

    let mut voice_users = HashMap::new();
    for voice_state in guild.read().voice_states.values() {
        if let Some(channel) = voice_state.channel_id {
            *voice_users.entry(channel).or_insert(0) += 1;
        }
    }

    let now = Utc::now();

    for channel in channels.values() {
        if channel.kind != ChannelType::Voice
            || !channel.name.starts_with(&config.temp_channel_prefix)
        {
            continue;
        }

        let created_at = DateTime::from_utc(channel.id.created_at(), Utc);

        if (now - created_at).to_std()? > Duration::from_secs(15 * 60)
            && voice_users.get(&channel.id).cloned().unwrap_or(0) == 0
        {
            info!("Deleting a channel"; "channel.name" => ?channel.name);
            channel
                .delete()
                .map_err(SyncFailure::new)
                .with_context(|_| format!("failed to delete {:?}", channel.name))?;
        }
    }

    Ok(())
}

pub fn channel_reaper(config: Arc<Config>) -> impl FnOnce() {
    move || loop {
        match reap_channels(&config) {
            Ok(()) => (),
            Err(err) => error!("Failed to reap channels"; "error" => ?err),
        }
        thread::sleep(Duration::from_secs(60));
    }
}
