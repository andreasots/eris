use crate::config::Config;
use crate::context::ErisContext;
use crate::extract::Extract;
use chrono::Utc;
use failure::{self, Error};
use serenity::model::prelude::*;
use slog_scope::{error, info};
use std::collections::HashMap;
use std::thread;
use std::time::Duration;

const STARTUP_DELAY: Duration = Duration::from_secs(5);
const REAP_INTERVAL: Duration = Duration::from_secs(60);
const MIN_CHANNEL_AGE: Duration = Duration::from_secs(15 * 60);

fn reap_channels(ctx: &ErisContext) -> Result<(), Error> {
    let data = ctx.data.read();
    let config = data.extract::<Config>()?;
    let guild = ctx
        .cache_and_http
        .cache
        .read()
        .guild(config.guild)
        .ok_or_else(|| failure::err_msg("failed to get the guild"))?;

    let mut voice_users = HashMap::new();
    for voice_state in guild.read().voice_states.values() {
        if let Some(channel) = voice_state.channel_id {
            *voice_users.entry(channel).or_insert(0) += 1;
        }
    }

    let now = Utc::now();

    let mut unused_channels = vec![];

    for channel in guild.read().channels.values() {
        let channel = channel.read();
        if channel.kind != ChannelType::Voice
            || !channel.name.starts_with(&config.temp_channel_prefix)
        {
            continue;
        }

        let created_at = channel.id.created_at().with_timezone(&Utc);

        if (now - created_at).to_std()? > MIN_CHANNEL_AGE
            && voice_users.get(&channel.id).cloned().unwrap_or(0) == 0
        {
            info!("Scheduling a temporary channel for deletion"; "channel.id" => ?channel.id, "channel.name" => ?channel.name);
            unused_channels.push(channel.id);
        }
    }

    for channel_id in unused_channels {
        if let Err(err) = channel_id.delete(&ctx) {
            error!("Failed to delete a temporary channel"; "error" => ?err, "channel.id" => ?channel_id);
        }
    }

    Ok(())
}

pub fn channel_reaper(ctx: ErisContext) -> impl FnOnce() {
    move || {
        // Delay the first reap so that it doesn't happen before the Discord connection is ready.
        thread::sleep(STARTUP_DELAY);

        loop {
            match reap_channels(&ctx) {
                Ok(()) => (),
                Err(err) => error!("Failed to reap channels"; "error" => ?err),
            }
            thread::sleep(REAP_INTERVAL);
        }
    }
}
