table! {
    game_per_show_data (game_id, show_id) {
        game_id -> Int4,
        show_id -> Int4,
        display_name -> Nullable<Text>,
        verified -> Nullable<Bool>,
    }
}

table! {
    games (id) {
        id -> Int4,
        name -> Text,
    }
}

table! {
    quotes (id) {
        id -> Int4,
        quote -> Text,
        attrib_name -> Nullable<Text>,
        attrib_date -> Nullable<Date>,
        deleted -> Bool,
        context -> Nullable<Text>,
        game_id -> Nullable<Int4>,
        show_id -> Nullable<Int4>,
    }
}

table! {
    shows (id) {
        id -> Int4,
        string_id -> Text,
        name -> Text,
    }
}

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

joinable!(game_per_show_data -> games (game_id));
joinable!(game_per_show_data -> shows (show_id));
joinable!(quotes -> games (game_id));
joinable!(quotes -> shows (show_id));

allow_tables_to_appear_in_same_query!(game_per_show_data, games, quotes, shows, state, users,);
