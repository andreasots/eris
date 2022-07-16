use crate::config::Config;
use crate::desertbus::DesertBus;
use crate::google::{Calendar, Sheets};
use crate::influxdb::InfluxDB;
use crate::rpc::LRRbot;
use crate::twitch::Helix;
use crate::twitter::Twitter;
use serenity::prelude::TypeMapKey;
use std::sync::Arc;

// Local types

impl TypeMapKey for Calendar {
    type Value = Self;
}

impl TypeMapKey for Config {
    type Value = Self;
}

impl TypeMapKey for DesertBus {
    type Value = Self;
}

impl TypeMapKey for Helix {
    type Value = Self;
}

impl TypeMapKey for InfluxDB {
    type Value = Self;
}

impl TypeMapKey for LRRbot {
    type Value = Arc<Self>;
}

impl TypeMapKey for Sheets {
    type Value = Self;
}

impl TypeMapKey for Twitter {
    type Value = Self;
}

// Foreign types

pub enum PgPool {}

impl TypeMapKey for PgPool {
    type Value = diesel::r2d2::Pool<diesel::r2d2::ConnectionManager<diesel::pg::PgConnection>>;
}

pub enum ReloadHandle {}

impl TypeMapKey for ReloadHandle {
    type Value = tracing_subscriber::reload::Handle<
        tracing_subscriber::EnvFilter,
        tracing_subscriber::layer::Layered<
            tracing_subscriber::fmt::Layer<
                tracing_subscriber::Registry,
                tracing_subscriber::fmt::format::JsonFields,
                tracing_subscriber::fmt::format::Format<
                    tracing_subscriber::fmt::format::Json,
                    tracing_subscriber::fmt::time::UtcTime<
                        time::format_description::well_known::Rfc3339,
                    >,
                >,
            >,
            tracing_subscriber::Registry,
        >,
    >;
}
