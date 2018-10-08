use crate::schema::*;
use serde_json::Value;

#[derive(Identifiable, Debug, Queryable)]
#[primary_key(game_id, show_id)]
#[table_name = "game_per_show_data"]
pub struct GameEntry {
    pub game_id: i32,
    pub show_id: i32,
    pub display_name: Option<String>,
    pub verified: Option<bool>,
}

#[derive(Identifiable, Debug, Queryable)]
pub struct Game {
    pub id: i32,
    pub name: String,
}

#[derive(Identifiable, Debug, Queryable)]
pub struct Show {
    pub id: i32,
    pub key: String,
    pub name: String,
}

#[derive(Identifiable, Debug, Queryable)]
#[primary_key(key)]
#[table_name = "state"]
pub struct State {
    pub key: String,
    pub value: Value,
}

#[derive(Identifiable, Debug, Queryable)]
pub struct User {
    pub id: i32,
    pub name: String,
    pub display_name: Option<String>,
    pub twitch_oauth: Option<String>,
    pub is_sub: bool,
    pub is_mod: bool,
    pub autostatus: bool,
    pub patreon_user_id: Option<i32>,
    pub stream_delay: i32,
    pub chat_timestamps: i32,
    pub chat_timestamps_24hr: bool,
    pub chat_timestamps_secs: bool,
}
