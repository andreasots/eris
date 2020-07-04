use crate::schema::*;
use anyhow::Error;
use chrono::NaiveDate;
use diesel::pg::upsert::excluded;
use diesel::pg::Pg;
use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::{Display, Formatter};

#[derive(Identifiable, Debug, Queryable)]
#[primary_key(game_id, show_id)]
#[table_name = "game_per_show_data"]
pub struct GameEntry {
    pub game_id: i32,
    pub show_id: i32,
    pub display_name: Option<String>,
    pub verified: Option<bool>,
}

impl GameEntry {
    pub fn find<C: Connection<Backend = Pg>>(
        game_id: i32,
        show_id: i32,
        conn: &C,
    ) -> QueryResult<GameEntry> {
        game_per_show_data::table.find((game_id, show_id)).first::<GameEntry>(conn)
    }
}

#[derive(Identifiable, Debug, Queryable)]
pub struct Game {
    pub id: i32,
    pub name: String,
}

impl Game {
    pub fn find<C: Connection<Backend = Pg>>(id: i32, conn: &C) -> QueryResult<Game> {
        games::table.find(id).first::<Game>(conn)
    }
}

#[derive(Identifiable, Debug, Queryable)]
pub struct Quote {
    pub id: i32,
    pub quote: String,
    pub attrib_name: Option<String>,
    pub attrib_date: Option<NaiveDate>,
    pub deleted: bool,
    pub context: Option<String>,
    pub game_id: Option<i32>,
    pub show_id: Option<i32>,
}

impl Display for Quote {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}: \"{}\"", self.id, self.quote)?;
        if let Some(ref name) = self.attrib_name {
            write!(f, " —{}", name)?;
        }
        if let Some(ref context) = self.context {
            write!(f, ", {}", context)?;
        }
        if let Some(ref date) = self.attrib_date {
            write!(f, " [{}]", date)?;
        }
        Ok(())
    }
}

#[derive(Identifiable, Debug, Queryable)]
pub struct Show {
    pub id: i32,
    pub key: String,
    pub name: String,
}

impl Show {
    pub fn find<C: Connection<Backend = Pg>>(id: i32, conn: &C) -> QueryResult<Show> {
        shows::table.find(id).first::<Show>(conn)
    }
}

#[derive(Insertable, Debug)]
#[table_name = "state"]
pub struct NewState<'a> {
    pub key: &'a str,
    pub value: Value,
}

#[derive(Identifiable, Debug, Queryable)]
#[primary_key(key)]
#[table_name = "state"]
pub struct State {
    pub key: String,
    pub value: Value,
}

impl State {
    pub fn get<T: for<'de> Deserialize<'de>, C: Connection<Backend = Pg>>(
        key: &str,
        conn: &C,
    ) -> Result<Option<T>, Error> {
        let value = state::table.find(key).select(state::value).first::<Value>(conn).optional()?;

        match value {
            Some(value) => Ok(Some(serde_json::from_value(value)?)),
            None => Ok(None),
        }
    }

    pub fn set<T: Serialize, C: Connection<Backend = Pg>>(
        key: &str,
        value: T,
        conn: &C,
    ) -> Result<(), Error> {
        diesel::insert_into(state::table)
            .values(NewState { key, value: serde_json::to_value(value)? })
            .on_conflict(state::key)
            .do_update()
            .set(state::value.eq(excluded(state::value)))
            .execute(conn)?;
        Ok(())
    }
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

impl User {
    pub fn by_name<C: Connection<Backend = Pg>>(name: &str, conn: &C) -> QueryResult<Self> {
        users::table.filter(users::name.eq(name)).first(conn)
    }
}
