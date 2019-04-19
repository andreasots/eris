use serenity::prelude::TypeMapKey;
use crate::config::Config;
use crate::rpc::LRRbot;
use tokio::runtime::TaskExecutor;
use crate::google::{Calendar, Sheets};
use crate::twitch::{Kraken, Helix};
use std::sync::Arc;
use crate::desertbus::DesertBus;

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

impl TypeMapKey for Kraken {
    type Value = Self;
}

impl TypeMapKey for LRRbot {
    type Value = Arc<Self>;
}

impl TypeMapKey for Sheets {
    type Value = Self;
}

// Foreign types

pub enum Executor {}

impl TypeMapKey for Executor {
    type Value = TaskExecutor;
}

pub enum PgPool {}

impl TypeMapKey for PgPool {
    type Value = diesel::r2d2::Pool<diesel::r2d2::ConnectionManager<diesel::pg::PgConnection>>;
}
