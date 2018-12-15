use crate::config::Config;
use crate::rpc::LRRbot;
use failure::{Error, ResultExt, SyncFailure};
use futures::prelude::*;
use rand::seq::SliceRandom;
use regex::Regex;
use serde::{Deserialize, Deserializer};
use serde_derive::Deserialize;
use serenity::model::channel::Message;
use serenity::model::guild::Emoji;
use serenity::prelude::*;
use serenity::utils::Colour;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use slog::{slog_error, slog_info};
use slog_scope::{error, info};

#[derive(Deserialize, Debug, PartialEq, Eq, Copy, Clone)]
#[serde(rename_all = "lowercase")]
enum Access {
    Any,
    Sub,
    Mod,
}

impl Access {
    fn user_has_access(self, msg: &Message) -> bool {
        match self {
            Access::Any => true,
            Access::Sub => {
                // A user is a "subscriber" if they have a coloured role
                msg.guild()
                    .and_then(|guild| {
                        let guild = guild.read();

                        guild.members.get(&msg.author.id).map(|member| {
                            member.roles.iter().any(|role_id| {
                                guild
                                    .roles
                                    .get(role_id)
                                    .map(|role| role.colour)
                                    .unwrap_or_else(Colour::default)
                                    != Colour::default()
                            })
                        })
                    })
                    .unwrap_or(false)
            }
            Access::Mod => msg
                .guild()
                .and_then(|guild| {
                    guild.read().members.get(&msg.author.id).map(|member| {
                        member
                            .permissions()
                            .map(|p| p.administrator())
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false),
        }
    }
}

#[derive(Deserialize, Debug, PartialEq, Eq)]
#[serde(untagged)]
enum Response {
    Some {
        access: Access,
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

fn replace_emojis<'a, S: Into<String>, I: Iterator<Item = &'a Emoji>>(
    msg: S,
    emojis: I,
) -> Result<String, Error> {
    let mut msg = msg.into();

    for emoji in emojis {
        let regex = Regex::new(&format!(r"\b{}\b", regex::escape(&emoji.name)))
            .with_context(|err| format!("invalid regex syntax with {:?}: {:?}", emoji.name, err))?;
        if let Cow::Owned(s) = regex.replace_all(&msg, &emoji.mention()[..]) {
            msg = s;
        }
    }

    Ok(msg)
}

fn static_response_impl(config: Arc<Config>, msg: &Message, command: &str) -> Result<(), Error> {
    let response = {
        let command = String::from(command);

        let mut runtime =
            tokio::runtime::Runtime::new().context("failed to construct a new Tokio runtime")?;
        runtime
            .block_on::<_, Response, Error>(
                async move {
                    let mut lrrbot = LRRbot::new(&config);

                    Ok(await!(lrrbot.get_data::<Response>(vec![
                        String::from("responses"),
                        command
                    ]))?)
                }
                    .boxed()
                    .compat(),
            )
            .context("failed to fetch the command")?
    };

    if let Response::Some { access, response } = response {
        info!("Static command received";
            "command_name" => ?command,
            "message" => ?&msg.content,
            "message.id" => ?msg.id.0,
            "from.id" => ?msg.author.id.0,
            "from.name" => ?&msg.author.name,
            "from.discriminator" => ?msg.author.discriminator,
        );

        if access.user_has_access(msg) {
            let response = response.choose(&mut rand::thread_rng());
            if let Some(response) = response {
                let mut vars = HashMap::new();
                vars.insert(
                    "user".into(),
                    msg.guild_id
                        .and_then(|guild| msg.author.nick_in(guild))
                        .unwrap_or_else(|| msg.author.name.clone()),
                );
                let response =
                    strfmt::strfmt(response, &vars).context("failed to format the reply")?;
                let response = if let Some(guild) = msg.guild() {
                    replace_emojis(response, guild.read().emojis.values())
                        .context("failed to replace emojis")?
                } else {
                    response
                };
                msg.reply(&response)
                    .map_err(SyncFailure::new)
                    .context("failed to send a reply")?;
            }
        } else {
            info!("Refusing to reply because user lacks access";
                "message.id" => ?msg.id.0,
                "access_required" => ?access,
            );
        }
    }

    Ok(())
}

pub fn static_response(
    config: Arc<Config>,
) -> impl Fn(&mut Context, &Message, &str) + Send + Sync + 'static {
    move |_, msg, command| {
        let config = config.clone();
        match static_response_impl(config.clone(), msg, command) {
            Ok(()) => (),
            Err(err) => {
                error!("Static command resulted in an unexpected error";
                    "message.id" => ?msg.id.0,
                    "error" => ?err,
                );

                let _ = msg.reply(&format!(
                    "Static command resulted in an unexpected error: {}.",
                    err
                ));
            }
        }
    }
}

#[test]
fn test_deserialize_single_response() {
    let res = serde_json::from_str::<Response>(
        r#"{"access": "any", "response": "Help: https://lrrbot.mrphlip.com/help"}"#,
    )
    .unwrap();
    assert_eq!(
        res,
        Response::Some {
            access: Access::Any,
            response: vec!["Help: https://lrrbot.mrphlip.com/help".into()]
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
            access: Access::Sub,
            response: vec!["peach".into(), "barf".into()]
        }
    );
}

#[test]
fn test_deserialize_missing() {
    let res = serde_json::from_str::<Response>("{}").unwrap();
    assert_eq!(res, Response::None {});
}

#[test]
fn test_replace_emojis() {
    use serenity::model::id::EmojiId;

    let emoji = [
        Emoji {
            animated: false,
            id: EmojiId(1),
            name: String::from("lrrDOTS"),
            managed: true,
            require_colons: true,
            roles: vec![],
        },
        Emoji {
            animated: false,
            id: EmojiId(2),
            name: String::from("lrrCIRCLE"),
            managed: true,
            require_colons: true,
            roles: vec![],
        },
        Emoji {
            animated: false,
            id: EmojiId(3),
            name: String::from("lrrARROW"),
            managed: true,
            require_colons: true,
            roles: vec![],
        },
    ];

    assert_eq!(
        replace_emojis("lrrDOTS lrrCIRCLE lrrARROW Visit LoadingReadyRun: http://loadingreadyrun.com/", emoji.iter()).unwrap(),
        "<:lrrDOTS:1> <:lrrCIRCLE:2> <:lrrARROW:3> Visit LoadingReadyRun: http://loadingreadyrun.com/"
    );
}
