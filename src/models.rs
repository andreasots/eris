use serde_json::Value;

#[derive(Queryable)]
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

#[derive(Queryable)]
pub struct State {
    pub key: String,
    pub value: Value,
}
