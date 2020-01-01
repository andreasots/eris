use crate::config::Config;
use crate::desertbus::DesertBus;
use crate::google::{Calendar, Sheets};
use crate::influxdb::InfluxDB;
use crate::rpc::LRRbot;
use crate::twitch::{Helix, Kraken};
use crate::twitter::Twitter;
use serenity::prelude::TypeMapKey;
use std::sync::Arc;
use tokio::runtime::Handle;

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

impl TypeMapKey for Kraken {
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

pub enum Executor {}

impl TypeMapKey for Executor {
    type Value = Handle;
}

pub enum PgPool {}

impl TypeMapKey for PgPool {
    type Value = diesel::r2d2::Pool<diesel::r2d2::ConnectionManager<diesel::pg::PgConnection>>;
}
