use tracing::error;
use twilight_cache_inmemory::InMemoryCache;
use twilight_gateway::Event;
use twilight_http::Client;
use twilight_model::gateway::payload::incoming::{GuildCreate, VoiceStateUpdate};
use twilight_model::id::marker::{GuildMarker, UserMarker};
use twilight_model::id::Id;

async fn disconnect(discord: &Client, guild: Id<GuildMarker>, user: Id<UserMarker>) {
    if let Err(error) = discord.update_guild_member(guild, user).channel_id(None).await {
        error!(?error, "failed to kick user from the AFK channel");
    }
}

pub async fn on_event(cache: &InMemoryCache, discord: &Client, event: &Event) {
    match event {
        Event::GuildCreate(event) => {
            let GuildCreate(ref guild) = **event;
            let Some(afk_channel) = guild.afk_channel_id else { return };
            for voice_state in &guild.voice_states {
                if voice_state.channel_id == Some(afk_channel) {
                    disconnect(discord, guild.id, voice_state.user_id).await;
                }
            }
        }
        Event::VoiceStateUpdate(event) => {
            let VoiceStateUpdate(ref voice_state) = **event;
            let Some(channel_id) = voice_state.channel_id else { return };
            let Some(guild_id) = voice_state.guild_id else { return };
            let Some(guild) = cache.guild(guild_id) else { return };
            if guild.afk_channel_id() == Some(channel_id) {
                disconnect(discord, guild_id, voice_state.user_id).await;
            }
        }
        _ => (),
    }
}
