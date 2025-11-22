use std::fmt::Write;

use anyhow::{Context, Error};
use bytes::BufMut;
use chrono::{DateTime, Utc};
use influxdb_line_protocol::LineProtocolBuilder;
use tracing::{error, warn};
use twilight_gateway::Event;
use twilight_model::channel::{Channel, ChannelType};
use twilight_model::gateway::payload::incoming::{
    ChannelCreate, ChannelDelete, ChannelUpdate, GuildCreate, MessageCreate, ThreadCreate,
    ThreadDelete, ThreadUpdate, VoiceStateUpdate,
};
use twilight_model::id::Id;
use twilight_model::id::marker::UserMarker;

use crate::cache::Cache;
use crate::influxdb::InfluxDb;

const TEXT_CHANNELS_MEASUREMENT: &str = "text_channels";
const VOICE_CHANNELS_MEASUREMENT: &str = "voice_channels";

struct Measurement<'a> {
    time: DateTime<Utc>,

    event: &'a str,
    channel_id: Option<u64>,
    channel_name: Option<&'a str>,
    category_id: Option<u64>,
    thread_id: Option<u64>,
    thread_name: Option<&'a str>,
    user_id: Option<u64>,

    count: f64,
    users: Option<String>,
}

impl<'a> Measurement<'a> {
    fn new(
        time: DateTime<Utc>,
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
            #[allow(clippy::cast_precision_loss)]
            count: count as f64,
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
        if serialized.is_empty() { self } else { Self { users: Some(serialized), ..self } }
    }

    fn user_id(self, user_id: Id<UserMarker>) -> Self {
        Self { user_id: Some(user_id.get()), ..self }
    }
}

trait LineProtocolBuilderExt {
    fn append(&mut self, name: &str, measurement: Measurement);
}

impl<B: BufMut + Default> LineProtocolBuilderExt for LineProtocolBuilder<B> {
    fn append(&mut self, name: &str, measurement: Measurement) {
        let builder = std::mem::take(self).measurement(name).tag("event", measurement.event);
        let builder = if let Some(channel_id) = measurement.channel_id {
            builder.tag("channel_id", &channel_id.to_string())
        } else {
            builder
        };
        let builder = if let Some(channel_name) = measurement.channel_name {
            builder.tag("channel_name", channel_name)
        } else {
            builder
        };
        let builder = if let Some(category_id) = measurement.category_id {
            builder.tag("category_id", &category_id.to_string())
        } else {
            builder
        };
        let builder = if let Some(thread_id) = measurement.thread_id {
            builder.tag("thread_id", &thread_id.to_string())
        } else {
            builder
        };
        let builder = if let Some(thread_name) = measurement.thread_name {
            builder.tag("thread_name", thread_name)
        } else {
            builder
        };
        let builder = if let Some(user_id) = measurement.user_id {
            builder.tag("user_id", &user_id.to_string())
        } else {
            builder
        };
        let builder = builder.field("count", measurement.count);
        let builder = if let Some(users) = measurement.users.as_deref() {
            builder.field("users", users)
        } else {
            builder
        };
        *self = if let Some(ts) = measurement.time.timestamp_nanos_opt() {
            builder.timestamp(ts).close_line()
        } else {
            warn!(timestamp = measurement.time.to_rfc3339(), "timestamp out of i64 range");
            builder.close_line()
        };
    }
}

fn is_guild_text_channel(kind: ChannelType) -> bool {
    match kind {
        ChannelType::GuildText
        | ChannelType::GuildVoice
        | ChannelType::GuildAnnouncement
        | ChannelType::AnnouncementThread
        | ChannelType::PublicThread
        | ChannelType::PrivateThread
        | ChannelType::GuildForum => true,

        ChannelType::Private
        | ChannelType::Group
        | ChannelType::GuildCategory
        | ChannelType::GuildStageVoice
        | ChannelType::GuildDirectory => false,

        kind => {
            error!(?kind, "unknown channel type");
            false
        }
    }
}

fn is_guild_voice_channel(kind: ChannelType) -> bool {
    match kind {
        ChannelType::GuildVoice | ChannelType::GuildStageVoice => true,

        ChannelType::GuildText
        | ChannelType::Private
        | ChannelType::Group
        | ChannelType::GuildCategory
        | ChannelType::GuildAnnouncement
        | ChannelType::AnnouncementThread
        | ChannelType::PublicThread
        | ChannelType::PrivateThread
        | ChannelType::GuildDirectory
        | ChannelType::GuildForum => false,

        kind => {
            error!(?kind, "unknown channel type");
            false
        }
    }
}

fn on_guild_create(
    measurements: &mut LineProtocolBuilder<Vec<u8>>,
    time: DateTime<Utc>,
    event: &GuildCreate,
) -> Result<(), Error> {
    let guild = match event {
        GuildCreate::Available(guild) => guild,
        GuildCreate::Unavailable(guild) => {
            anyhow::bail!("guild {} is unavailable", guild.id)
        }
    };

    for channel in &guild.channels {
        if is_guild_text_channel(channel.kind) {
            measurements.append(
                TEXT_CHANNELS_MEASUREMENT,
                Measurement::new(time, "guild_create", Some(channel), None, 0),
            );
        }

        if is_guild_voice_channel(channel.kind) {
            measurements.append(
                VOICE_CHANNELS_MEASUREMENT,
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
                ),
            );
        }
    }

    for thread in &guild.threads {
        if thread.thread_metadata.as_ref().is_some_and(|meta| meta.archived) {
            continue;
        }

        let channel = guild.channels.iter().find(|c| Some(c.id) == thread.parent_id);

        measurements.append(
            TEXT_CHANNELS_MEASUREMENT,
            Measurement::new(time, "guild_create", channel, Some(thread), 0),
        );
    }

    Ok(())
}

fn on_channel_create(
    measurements: &mut LineProtocolBuilder<Vec<u8>>,
    time: DateTime<Utc>,
    ChannelCreate(channel): &ChannelCreate,
) {
    if is_guild_text_channel(channel.kind) {
        measurements.append(
            TEXT_CHANNELS_MEASUREMENT,
            Measurement::new(time, "channel_create", Some(channel), None, 0),
        );
    }

    if is_guild_voice_channel(channel.kind) {
        measurements.append(
            VOICE_CHANNELS_MEASUREMENT,
            Measurement::new(time, "channel_create", Some(channel), None, 0),
        );
    }
}

fn on_channel_update(
    measurements: &mut LineProtocolBuilder<Vec<u8>>,
    cache: &Cache,
    time: DateTime<Utc>,
    ChannelUpdate(channel): &ChannelUpdate,
) {
    cache.with(|cache| {
        if is_guild_text_channel(channel.kind) {
            measurements.append(
                TEXT_CHANNELS_MEASUREMENT,
                Measurement::new(time, "channel_update", Some(channel), None, 0),
            );
        }

        if is_guild_voice_channel(channel.kind) {
            measurements.append(
                VOICE_CHANNELS_MEASUREMENT,
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
                ),
            );
        }
    });
}

fn on_channel_delete(
    measurements: &mut LineProtocolBuilder<Vec<u8>>,
    time: DateTime<Utc>,
    ChannelDelete(channel): &ChannelDelete,
) {
    if is_guild_text_channel(channel.kind) {
        measurements.append(
            TEXT_CHANNELS_MEASUREMENT,
            Measurement::new(time, "channel_delete", Some(channel), None, 0),
        );
    }

    if is_guild_voice_channel(channel.kind) {
        measurements.append(
            VOICE_CHANNELS_MEASUREMENT,
            Measurement::new(time, "channel_delete", Some(channel), None, 0),
        );
    }
}

fn on_thread_create(
    measurements: &mut LineProtocolBuilder<Vec<u8>>,
    cache: &Cache,
    time: DateTime<Utc>,
    ThreadCreate(thread): &ThreadCreate,
) {
    cache.with(|cache| {
        let channel = thread.parent_id.and_then(|id| cache.channel(id));

        measurements.append(
            TEXT_CHANNELS_MEASUREMENT,
            Measurement::new(time, "thread_create", channel.as_deref(), Some(thread), 0),
        );
    });
}

fn on_thread_update(
    measurements: &mut LineProtocolBuilder<Vec<u8>>,
    cache: &Cache,
    time: DateTime<Utc>,
    ThreadUpdate(thread): &ThreadUpdate,
) {
    cache.with(|cache| {
        let channel = thread.parent_id.and_then(|id| cache.channel(id));

        measurements.append(
            TEXT_CHANNELS_MEASUREMENT,
            Measurement::new(time, "thread_update", channel.as_deref(), Some(thread), 0),
        );
    });
}

fn on_thread_delete(
    measurements: &mut LineProtocolBuilder<Vec<u8>>,
    cache: &Cache,
    time: DateTime<Utc>,
    &ThreadDelete { id, parent_id, .. }: &ThreadDelete,
) {
    cache.with(|cache| {
        let thread = cache.channel(id);
        let channel = cache.channel(parent_id);

        measurements.append(
            TEXT_CHANNELS_MEASUREMENT,
            Measurement::new(time, "thread_delete", channel.as_deref(), thread.as_deref(), 0),
        );
    });
}

fn on_voice_state_update(
    measurements: &mut LineProtocolBuilder<Vec<u8>>,
    cache: &Cache,
    time: DateTime<Utc>,
    // NOTE: voice state counts are off by one because this event hasn't been processed yet
    VoiceStateUpdate(new_state): &VoiceStateUpdate,
) {
    cache.with(|cache| {
        let old_state =
            new_state.guild_id.and_then(|guild_id| cache.voice_state(new_state.user_id, guild_id));

        let old_channel_id = old_state.map(|state| state.channel_id());

        match (old_channel_id, new_state.channel_id) {
            // old and new are the same, ignore (someone muted themselves or something)
            (Some(old), Some(new)) if old == new => (),
            // user moved voice channels
            (Some(old_channel_id), Some(new_channel_id)) => {
                measurements.append(
                    VOICE_CHANNELS_MEASUREMENT,
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
                    ),
                );

                measurements.append(
                    VOICE_CHANNELS_MEASUREMENT,
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
                    ),
                );
            }
            // user joined a voice channel
            (None, Some(channel_id)) => measurements.append(
                VOICE_CHANNELS_MEASUREMENT,
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
                ),
            ),
            // user left a voice channel
            (Some(channel_id), None) => measurements.append(
                VOICE_CHANNELS_MEASUREMENT,
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
                ),
            ),
            // Nothing happened, probably unreachable
            (None, None) => (),
        }
    });
}

fn on_message_create(
    measurements: &mut LineProtocolBuilder<Vec<u8>>,
    cache: &Cache,
    time: DateTime<Utc>,
    MessageCreate(message): &MessageCreate,
) {
    cache.with(|cache| {
        let (channel, thread) = match cache.channel(message.channel_id) {
            Some(channel) => {
                if let ChannelType::Private | ChannelType::Group = channel.kind {
                    // don't collect stats on direct messages
                    return;
                }

                if channel.kind.is_guild() && channel.kind.is_thread() {
                    (channel.parent_id.and_then(|id| cache.channel(id)), Some(channel))
                } else {
                    (Some(channel), None)
                }
            }
            _ => (None, None),
        };

        measurements.append(
            TEXT_CHANNELS_MEASUREMENT,
            Measurement::new(time, "message", channel.as_deref(), thread.as_deref(), 1)
                .user_id(message.author.id),
        );
    });
}

pub async fn on_event(cache: &Cache, influxdb: &InfluxDb, event: &Event) -> Result<(), Error> {
    let mut measurements = LineProtocolBuilder::new();
    let time = Utc::now();

    match event {
        Event::GuildCreate(event) => on_guild_create(&mut measurements, time, event)?,

        Event::ChannelCreate(event) => on_channel_create(&mut measurements, time, event),
        Event::ChannelUpdate(event) => on_channel_update(&mut measurements, cache, time, event),
        Event::ChannelDelete(event) => on_channel_delete(&mut measurements, time, event),

        Event::ThreadCreate(event) => on_thread_create(&mut measurements, cache, time, event),
        Event::ThreadUpdate(event) => on_thread_update(&mut measurements, cache, time, event),
        Event::ThreadDelete(event) => on_thread_delete(&mut measurements, cache, time, event),

        Event::VoiceStateUpdate(event) => {
            on_voice_state_update(&mut measurements, cache, time, event);
        }

        Event::MessageCreate(event) => on_message_create(&mut measurements, cache, time, event),

        _ => (),
    }

    influxdb.write(measurements).await.context("failed to write the user count to InfluxDB")?;

    Ok(())
}
