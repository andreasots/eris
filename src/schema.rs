table! {
    state (key) {
        key -> Text,
        value -> Jsonb,
    }
}

table! {
    users (id) {
        id -> Int4,
        name -> Text,
        display_name -> Nullable<Text>,
        twitch_oauth -> Nullable<Text>,
        is_sub -> Bool,
        is_mod -> Bool,
        autostatus -> Bool,
        patreon_user_id -> Nullable<Int4>,
        stream_delay -> Int4,
        chat_timestamps -> Int4,
        chat_timestamps_24hr -> Bool,
        chat_timestamps_secs -> Bool,
    }
}

allow_tables_to_appear_in_same_query!(state, users,);
