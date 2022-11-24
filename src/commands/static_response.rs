use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context as _, Error};
use rand::seq::SliceRandom;
use serde::{Deserialize, Deserializer};
use tracing::info;
use twilight_cache_inmemory::model::CachedMember;
use twilight_cache_inmemory::InMemoryCache;
use twilight_http::Client as DiscordClient;
use twilight_model::channel::message::MessageFlags;
use twilight_model::channel::Message;

use crate::command_parser::{Access, Args, CommandHandler, Commands};
use crate::config::Config;
use crate::rpc::LRRbot;

pub struct Static {
    lrrbot: Arc<LRRbot>,
}

impl Static {
    pub fn new(lrrbot: Arc<LRRbot>) -> Self {
        Self { lrrbot }
    }
}

impl CommandHandler for Static {
    fn pattern(&self) -> &str {
        r"(.*)"
    }

    fn help(&self) -> Option<crate::command_parser::Help> {
        None
    }

    fn handle<'a>(
        &'a self,
        cache: &'a InMemoryCache,
        config: &'a Config,
        discord: &'a DiscordClient,
        _: Commands<'a>,
        message: &'a Message,
        args: &'a Args,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            let Some(command) = args.get(0) else {
                return Ok(())
            };
            let command = extract_command(command);

            let response = self
                .lrrbot
                .get_data::<Response>(vec![String::from("responses"), command])
                .await
                .context("failed to fetch the command")?;

            if let Response::Some { access, response } = response {
                let access = Access::from(access);
                let guild_id = message.guild_id.unwrap_or(config.guild);
                if access.user_has_access(message.author.id, guild_id, cache) {
                    let response = response.choose(&mut rand::thread_rng());
                    if let Some(response) = response {
                        let vars = HashMap::from([(
                            "user".to_string(),
                            message
                                .guild_id
                                .and_then(|guild_id| cache.member(guild_id, message.author.id))
                                .as_deref()
                                .and_then(CachedMember::nick)
                                .unwrap_or(&message.author.name)
                                .to_string(),
                        )]);

                        let response = strfmt::strfmt(response, &vars)
                            .context("failed to format the reply")?;

                        discord
                            .create_message(message.channel_id)
                            .reply(message.id)
                            .flags(MessageFlags::SUPPRESS_EMBEDS)
                            .content(&response)
                            .context("reply message invalid")?
                            .await
                            .context("failed to reply to command")?;
                    }
                } else {
                    info!(?access, "Refusing to reply because user lacks access");
                    crate::command_parser::refuse_access(
                        discord,
                        message.channel_id,
                        message.id,
                        access,
                    )
                    .await?;
                }
            }

            Ok(())
        })
    }
}

#[derive(Deserialize, Debug, PartialEq, Eq, Copy, Clone)]
#[serde(rename_all = "lowercase")]
enum StoredAccess {
    Any,
    Sub,
    Mod,
}

impl From<StoredAccess> for Access {
    fn from(access: StoredAccess) -> Self {
        match access {
            StoredAccess::Any => Access::All,
            StoredAccess::Sub => Access::SubOnly,
            StoredAccess::Mod => Access::ModOnly,
        }
    }
}

#[derive(Deserialize, Debug, PartialEq, Eq)]
#[serde(untagged)]
enum Response {
    Some {
        access: StoredAccess,
        #[serde(deserialize_with = "string_or_seq_string")]
        response: Vec<String>,
    },
    None {},
}

fn string_or_seq_string<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct StringOrVec;

    impl<'de> serde::de::Visitor<'de> for StringOrVec {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("string or list of strings")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(vec![value.to_owned()])
        }

        fn visit_seq<S>(self, visitor: S) -> Result<Self::Value, S::Error>
        where
            S: serde::de::SeqAccess<'de>,
        {
            Deserialize::deserialize(serde::de::value::SeqAccessDeserializer::new(visitor))
        }
    }

    deserializer.deserialize_any(StringOrVec)
}

fn extract_command(cmd: &str) -> String {
    let mut command = String::new();
    for (i, part) in cmd.split_whitespace().enumerate() {
        if i != 0 {
            command.push(' ');
        }
        command.push_str(part);
    }
    command
}

#[cfg(test)]
mod tests {
    use super::{Response, StoredAccess};

    #[test]
    fn test_deserialize_single_response() {
        let res = serde_json::from_str::<Response>(
            r#"{"access": "any", "response": "Help: https://lrrbot.com/help"}"#,
        )
        .unwrap();
        assert_eq!(
            res,
            Response::Some {
                access: StoredAccess::Any,
                response: vec!["Help: https://lrrbot.com/help".into()]
            }
        );
    }

    #[test]
    fn test_deserialize_multi_response() {
        let res =
            serde_json::from_str::<Response>(r#"{"access": "sub", "response": ["peach", "barf"]}"#)
                .unwrap();
        assert_eq!(
            res,
            Response::Some {
                access: StoredAccess::Sub,
                response: vec!["peach".into(), "barf".into()]
            }
        );
    }

    #[test]
    fn deserialize_missing() {
        let res = serde_json::from_str::<Response>("{}").unwrap();
        assert_eq!(res, Response::None {});
    }

    #[test]
    fn extract_command() {
        assert_eq!(super::extract_command(" \t  \t some \t command \t "), "some command");
        assert_eq!(super::extract_command("command"), "command");
        assert_eq!(super::extract_command("some command"), "some command");
    }
}
