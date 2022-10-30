pub mod game_entry {
    use std::convert::TryInto;

    use sea_orm::entity::prelude::*;

    #[derive(Debug, Clone, DeriveEntityModel)]
    #[sea_orm(table_name = "game_per_show_data")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub game_id: i32,
        #[sea_orm(primary_key)]
        pub show_id: i32,
        pub display_name: Option<String>,
        pub verified: Option<bool>,
    }

    #[derive(Debug, EnumIter, DeriveRelation)]
    pub enum Relation {
        #[sea_orm(
            belongs_to = "super::game::Entity",
            from = "Column::GameId",
            to = "super::game::Column::Id"
        )]
        Game,
        #[sea_orm(
            belongs_to = "super::show::Entity",
            from = "Column::ShowId",
            to = "super::show::Column::Id"
        )]
        Show,
    }

    impl Related<super::game::Entity> for Entity {
        fn to() -> RelationDef {
            Relation::Game.def()
        }
    }

    impl Related<super::show::Entity> for Entity {
        fn to() -> RelationDef {
            Relation::Show.def()
        }
    }

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod game {
    use std::convert::TryInto;

    use sea_orm::entity::prelude::*;

    #[derive(Debug, Clone, DeriveEntityModel)]
    #[sea_orm(table_name = "games")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub name: String,
    }

    #[derive(Debug, EnumIter, DeriveRelation)]
    pub enum Relation {
        #[sea_orm(has_many = "super::game_entry::Entity")]
        GameEntry,
    }

    impl Related<super::game_entry::Entity> for Entity {
        fn to() -> RelationDef {
            Relation::GameEntry.def()
        }
    }

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod quote {
    use std::convert::TryInto;
    use std::fmt::{Display, Formatter};

    use sea_orm::entity::prelude::*;
    use time::Date;

    #[derive(Debug, Clone, DeriveEntityModel)]
    #[sea_orm(table_name = "quotes")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub quote: String,
        pub attrib_name: Option<String>,
        pub attrib_date: Option<Date>,
        pub deleted: bool,
        pub context: Option<String>,
        pub game_id: Option<i32>,
        pub show_id: Option<i32>,
    }

    impl Display for Model {
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

    #[derive(DeriveRelation, Debug, EnumIter)]
    pub enum Relation {
        #[sea_orm(
            belongs_to = "super::game::Entity",
            from = "Column::GameId",
            to = "super::game::Column::Id"
        )]
        Game,
        #[sea_orm(
            belongs_to = "super::show::Entity",
            from = "Column::ShowId",
            to = "super::show::Column::Id"
        )]
        Show,
        #[sea_orm(
            belongs_to = "super::game_entry::Entity",
            from = "(Column::GameId, Column::ShowId)",
            to = "(super::game_entry::Column::GameId, super::game_entry::Column::ShowId)"
        )]
        GameEntry,
    }

    impl Related<super::game::Entity> for Entity {
        fn to() -> RelationDef {
            Relation::Game.def()
        }
    }

    impl Related<super::show::Entity> for Entity {
        fn to() -> RelationDef {
            Relation::Show.def()
        }
    }

    impl Related<super::game_entry::Entity> for Entity {
        fn to() -> RelationDef {
            Relation::GameEntry.def()
        }
    }

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod show {
    use std::convert::TryInto;

    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(table_name = "shows")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        #[sea_orm(column_name = "string_id")]
        pub key: String,
        pub name: String,
    }

    #[derive(Debug, EnumIter, DeriveRelation)]
    pub enum Relation {
        #[sea_orm(has_many = "super::game_entry::Entity")]
        GameEntry,
        #[sea_orm(has_many = "super::quote::Entity")]
        Quote,
    }

    impl Related<super::game_entry::Entity> for Entity {
        fn to() -> RelationDef {
            Relation::GameEntry.def()
        }
    }

    impl Related<super::quote::Entity> for Entity {
        fn to() -> RelationDef {
            Relation::Quote.def()
        }
    }

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod state {
    use std::convert::TryInto;

    use anyhow::{Context, Error};
    use sea_orm::entity::prelude::*;
    use sea_orm::sea_query::OnConflict;
    use sea_orm::Insert;
    use serde::de::DeserializeOwned;
    use serde::Serialize;

    #[derive(Debug, Clone, DeriveEntityModel)]
    #[sea_orm(table_name = "state")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub key: String,
        pub value: serde_json::Value,
    }

    #[derive(Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    pub async fn get<T: DeserializeOwned>(
        key: &str,
        conn: &DatabaseConnection,
    ) -> Result<Option<T>, Error> {
        let state = Entity::find()
            .filter(Column::Key.eq(key))
            .one(conn)
            .await
            .with_context(|| format!("failed to load state key {key:?}"))?;

        match state {
            Some(state) => {
                Ok(Some(serde_json::from_value(state.value).context("failed to parse the value")?))
            }
            None => Ok(None),
        }
    }

    pub async fn set<T: Serialize>(
        key: String,
        value: T,
        conn: &DatabaseConnection,
    ) -> Result<(), Error> {
        Insert::one(Model {
            key,
            value: serde_json::to_value(value).context("failed to serialize value")?,
        })
        .on_conflict(OnConflict::column(Column::Key).update_columns([Column::Value]).to_owned())
        .exec(conn)
        .await
        .context("failed to update the state")?;

        Ok(())
    }
}

pub mod user {
    use std::convert::TryInto;

    use sea_orm::entity::prelude::*;

    #[derive(Debug, Clone, DeriveEntityModel)]
    #[sea_orm(table_name = "users")]
    pub struct Model {
        #[sea_orm(primary_key)]
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

    #[derive(Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}
