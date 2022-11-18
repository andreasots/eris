use std::fmt::Write;

use anyhow::{Context, Error};
use influxdb::{Client as InfluxDb, InfluxDbWriteable, Timestamp};
use time::OffsetDateTime;
use tracing::error;
use twilight_cache_inmemory::InMemoryCache;
use twilight_gateway::Event;
use twilight_model::channel::{Channel, ChannelType};
use twilight_model::gateway::payload::incoming::{
    ChannelCreate, ChannelDelete, ChannelUpdate, GuildCreate, MessageCreate, ThreadCreate,
    ThreadDelete, ThreadUpdate, VoiceStateUpdate,
};
use twilight_model::id::marker::UserMarker;
use twilight_model::id::Id;

const TEXT_CHANNELS_MEASUREMENT: &str = "text_channels";
const VOICE_CHANNELS_MEASUREMENT: &str = "voice_channels";

#[derive(InfluxDbWriteable)]
struct Measurement<'a> {
    time: Timestamp,

    #[influxdb(tag)]
    event: &'a str,
    #[influxdb(tag)]
    channel_id: Option<u64>,
    #[influxdb(tag)]
    channel_name: Option<&'a str>,
    #[influxdb(tag)]
    category_id: Option<u64>,
    #[influxdb(tag)]
    thread_id: Option<u64>,
    #[influxdb(tag)]
    thread_name: Option<&'a str>,
    #[influxdb(tag)]
    user_id: Option<u64>,

    count: u64,
    users: Option<String>,
}

impl<'a> Measurement<'a> {
    fn new(
        time: Timestamp,
        event: &'a str,
        channel: Option<&'a Channel>,
        thread: Option<&'a Channel>,
        count: usize,
    ) -> Self {
        Self {
            time,
            event,
            channel_id: channel.map(|c| c.id.get()),
            channel_name: channel.and_then(|c| c.name.as_deref()),
            category_id: channel.and_then(|c| c.parent_id.map(Id::get)),
            thread_id: thread.map(|t| t.id.get()),
            thread_name: thread.and_then(|t| t.name.as_deref()),
            count: count as u64,
            users: None,
            user_id: None,
        }
    }

    fn users(self, users: impl IntoIterator<Item = Id<UserMarker>>) -> Self {
        let mut serialized = String::new();
        for (i, user) in users.into_iter().enumerate() {
            if i != 0 {
                serialized.push(',');
            }
            write!(serialized, "{}", user.get()).unwrap();
        }
        if !serialized.is_empty() {
            Self { users: Some(serialized), ..self }
        } else {
            self
        }
    }

    fn user_id(self, user_id: Id<UserMarker>) -> Self {
        Self { user_id: Some(user_id.get()), ..self }
    }
}

fn now() -> Timestamp {
    Timestamp::Nanoseconds(OffsetDateTime::now_utc().unix_timestamp_nanos() as u128)
}

fn is_guild_text_channel(kind: ChannelType) -> bool {
    match kind {
        ChannelType::GuildText => true,
        ChannelType::Private => false,
        ChannelType::GuildVoice => true,
        ChannelType::Group => false,
        ChannelType::GuildCategory => false,
        ChannelType::GuildAnnouncement => true,
        ChannelType::AnnouncementThread => true,
        ChannelType::PublicThread => true,
        ChannelType::PrivateThread => true,
        ChannelType::GuildStageVoice => false,
        ChannelType::GuildDirectory => false,
        ChannelType::GuildForum => true,
        kind => {
            error!(?kind, "unknown channel type");
            false
        }
    }
}

fn is_guild_voice_channel(kind: ChannelType) -> bool {
    match kind {
        ChannelType::GuildText => false,
        ChannelType::Private => false,
        ChannelType::GuildVoice => true,
        ChannelType::Group => false,
        ChannelType::GuildCategory => false,
        ChannelType::GuildAnnouncement => false,
        ChannelType::AnnouncementThread => false,
        ChannelType::PublicThread => false,
        ChannelType::PrivateThread => false,
        ChannelType::GuildStageVoice => true,
        ChannelType::GuildDirectory => false,
        ChannelType::GuildForum => false,
        kind => {
            error!(?kind, "unknown channel type");
            false
        }
    }
}

pub async fn on_event(
    cache: &InMemoryCache,
    influxdb: &InfluxDb,
    event: &Event,
) -> Result<(), Error> {
    match event {
        Event::GuildCreate(event) => {
            let GuildCreate(ref guild) = **event;
            let time = now();
            let mut measurements = vec![];

            for channel in &guild.channels {
                if is_guild_text_channel(channel.kind) {
                    measurements.push(
                        Measurement::new(time, "guild_create", Some(channel), None, 0)
                            .into_query(TEXT_CHANNELS_MEASUREMENT),
                    );
                }

                if is_guild_voice_channel(channel.kind) {
                    measurements.push(
                        Measurement::new(
                            time,
                            "guild_create",
                            Some(channel),
                            None,
                            guild
                                .voice_states
                                .iter()
                                .filter(|vs| vs.channel_id == Some(channel.id))
                                .count(),
                        )
                        .users(
                            guild
                                .voice_states
                                .iter()
                                .filter(|vs| vs.channel_id == Some(channel.id))
                                .map(|state| state.user_id),
                        )
                        .into_query(VOICE_CHANNELS_MEASUREMENT),
                    );
                }
            }

            for thread in &guild.threads {
                if thread.thread_metadata.as_ref().map(|meta| meta.archived).unwrap_or(false) {
                    continue;
                }

                let channel = guild.channels.iter().find(|c| Some(c.id) == thread.parent_id);

                measurements.push(
                    Measurement::new(time, "guild_create", channel, Some(thread), 0)
                        .into_query(TEXT_CHANNELS_MEASUREMENT),
                );
            }

            influxdb
                .query(measurements)
                .await
                .context("failed to write the user count to InfluxDB")?;
        }

        Event::ChannelCreate(event) => {
            let ChannelCreate(ref channel) = **event;
            let time = now();
            let mut measurements = vec![];

            if is_guild_text_channel(channel.kind) {
                measurements.push(
                    Measurement::new(time, "channel_create", Some(channel), None, 0)
                        .into_query(TEXT_CHANNELS_MEASUREMENT),
                );
            }

            if is_guild_voice_channel(channel.kind) {
                measurements.push(
                    Measurement::new(time, "channel_create", Some(channel), None, 0)
                        .into_query(VOICE_CHANNELS_MEASUREMENT),
                );
            }

            if !measurements.is_empty() {
                influxdb
                    .query(measurements)
                    .await
                    .context("failed to write the user count to InfluxDB")?;
            }
        }
        Event::ChannelUpdate(event) => {
            let ChannelUpdate(ref channel) = **event;
            let time = now();
            let mut measurements = vec![];

            if is_guild_text_channel(channel.kind) {
                measurements.push(
                    Measurement::new(time, "channel_update", Some(channel), None, 0)
                        .into_query(TEXT_CHANNELS_MEASUREMENT),
                );
            }

            if is_guild_voice_channel(channel.kind) {
                measurements.push(
                    Measurement::new(
                        time,
                        "channel_update",
                        Some(channel),
                        None,
                        cache.stats().channel_voice_states(channel.id).unwrap_or(0),
                    )
                    .users(
                        cache
                            .voice_channel_states(channel.id)
                            .into_iter()
                            .flatten()
                            .map(|state| state.user_id()),
                    )
                    .into_query(VOICE_CHANNELS_MEASUREMENT),
                );
            }

            if !measurements.is_empty() {
                influxdb
                    .query(measurements)
                    .await
                    .context("failed to write the user count to InfluxDB")?;
            }
        }
        Event::ChannelDelete(event) => {
            let ChannelDelete(ref channel) = **event;
            let time = now();
            let mut measurements = vec![];

            if is_guild_text_channel(channel.kind) {
                measurements.push(
                    Measurement::new(time, "channel_delete", Some(channel), None, 0)
                        .into_query(TEXT_CHANNELS_MEASUREMENT),
                );
            }

            if is_guild_voice_channel(channel.kind) {
                measurements.push(
                    Measurement::new(time, "channel_delete", Some(channel), None, 0)
                        .into_query(VOICE_CHANNELS_MEASUREMENT),
                );
            }

            if !measurements.is_empty() {
                influxdb
                    .query(measurements)
                    .await
                    .context("failed to write the user count to InfluxDB")?;
            }
        }

        Event::ThreadCreate(event) => {
            let ThreadCreate(ref thread) = **event;
            let channel = thread.parent_id.and_then(|id| cache.channel(id));
            let time = now();

            influxdb
                .query(
                    Measurement::new(time, "thread_create", channel.as_deref(), Some(thread), 0)
                        .into_query(TEXT_CHANNELS_MEASUREMENT),
                )
                .await
                .context("failed to write the thread creation to InfluxDB")?;
        }
        Event::ThreadUpdate(event) => {
            let ThreadUpdate(ref thread) = **event;
            let channel = thread.parent_id.and_then(|id| cache.channel(id));
            let time = now();

            influxdb
                .query(
                    Measurement::new(time, "thread_update", channel.as_deref(), Some(thread), 0)
                        .into_query(TEXT_CHANNELS_MEASUREMENT),
                )
                .await
                .context("failed to write the thread update to InfluxDB")?;
        }
        &Event::ThreadDelete(ThreadDelete { id, parent_id, .. }) => {
            let thread = cache.channel(id);
            let channel = cache.channel(parent_id);
            let time = now();

            influxdb
                .query(
                    Measurement::new(
                        time,
                        "thread_delete",
                        channel.as_deref(),
                        thread.as_deref(),
                        0,
                    )
                    .into_query(TEXT_CHANNELS_MEASUREMENT),
                )
                .await
                .context("failed to write the thread deletion to InfluxDB")?;
        }

        Event::VoiceStateUpdate(event) => {
            // NOTE: voice state counts are off by one because this event hasn't been processed yet
            let VoiceStateUpdate(ref new_state) = **event;
            let old_state = new_state
                .guild_id
                .and_then(|guild_id| cache.voice_state(new_state.user_id, guild_id));
            let time = now();
            let mut measurements = vec![];

            let old_channel_id = old_state.map(|state| state.channel_id());

            match (old_channel_id, new_state.channel_id) {
                // old and new are the same, ignore (someone muted themselves or something)
                (Some(old), Some(new)) if old == new => (),
                // user moved voice channels
                (Some(old_channel_id), Some(new_channel_id)) => {
                    measurements.push(
                        Measurement::new(
                            time,
                            "state_update",
                            cache.channel(old_channel_id).as_deref(),
                            None,
                            cache.stats().channel_voice_states(old_channel_id).unwrap_or(1) - 1,
                        )
                        .users(
                            cache
                                .voice_channel_states(old_channel_id)
                                .into_iter()
                                .flatten()
                                .map(|state| state.user_id())
                                .filter(|&id| id != new_state.user_id),
                        )
                        .into_query(VOICE_CHANNELS_MEASUREMENT),
                    );

                    measurements.push(
                        Measurement::new(
                            time,
                            "state_update",
                            cache.channel(new_channel_id).as_deref(),
                            None,
                            cache.stats().channel_voice_states(new_channel_id).unwrap_or(0) + 1,
                        )
                        .users(
                            cache
                                .voice_channel_states(new_channel_id)
                                .into_iter()
                                .flatten()
                                .map(|state| state.user_id())
                                .chain(Some(new_state.user_id)),
                        )
                        .into_query(VOICE_CHANNELS_MEASUREMENT),
                    );
                }
                // user joined a voice channel
                (None, Some(channel_id)) => measurements.push(
                    Measurement::new(
                        time,
                        "state_update",
                        cache.channel(channel_id).as_deref(),
                        None,
                        cache.stats().channel_voice_states(channel_id).unwrap_or(0) + 1,
                    )
                    .users(
                        cache
                            .voice_channel_states(channel_id)
                            .into_iter()
                            .flatten()
                            .map(|state| state.user_id())
                            .chain(Some(new_state.user_id)),
                    )
                    .into_query(VOICE_CHANNELS_MEASUREMENT),
                ),
                // user left a voice channel
                (Some(channel_id), None) => measurements.push(
                    Measurement::new(
                        time,
                        "state_update",
                        cache.channel(channel_id).as_deref(),
                        None,
                        cache.stats().channel_voice_states(channel_id).unwrap_or(1) - 1,
                    )
                    .users(
                        cache
                            .voice_channel_states(channel_id)
                            .into_iter()
                            .flatten()
                            .map(|state| state.user_id())
                            .filter(|&id| id != new_state.user_id),
                    )
                    .into_query(VOICE_CHANNELS_MEASUREMENT),
                ),
                // Nothing happened, probably unreachable
                (None, None) => (),
            }

            if !measurements.is_empty() {
                influxdb
                    .query(measurements)
                    .await
                    .context("failed to write the user count to InfluxDB")?;
            }
        }

        Event::MessageCreate(event) => {
            let MessageCreate(ref message) = **event;
            let time = now();

            let (channel, thread) = if let Some(channel) = cache.channel(message.channel_id) {
                if let ChannelType::Private | ChannelType::Group = channel.kind {
                    // don't collect stats on direct messages
                    return Ok(());
                }

                if channel.kind.is_guild() && channel.kind.is_thread() {
                    (channel.parent_id.and_then(|id| cache.channel(id)), Some(channel))
                } else {
                    (Some(channel), None)
                }
            } else {
                (None, None)
            };

            influxdb
                .query(
                    Measurement::new(time, "message", channel.as_deref(), thread.as_deref(), 1)
                        .user_id(message.author.id)
                        .into_query(TEXT_CHANNELS_MEASUREMENT),
                )
                .await
                .context("failed to write the message count to InfluxDB")?;
        }

        _ => (),
    }

    Ok(())
}
