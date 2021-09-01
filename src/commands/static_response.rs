use crate::extract::Extract;
use crate::rpc::LRRbot;
use anyhow::{Context as _, Error};
use rand::seq::SliceRandom;
use regex::Regex;
use serde::{Deserialize, Deserializer};
use serenity::framework::standard::macros::hook;
use serenity::framework::standard::{Args, Delimiter};
use serenity::model::channel::Message;
use serenity::model::guild::Emoji;
use serenity::prelude::*;
use serenity::utils::Colour;
use std::borrow::Cow;
use std::collections::HashMap;
use tracing::{error, info};

#[derive(Deserialize, Debug, PartialEq, Eq, Copy, Clone)]
#[serde(rename_all = "lowercase")]
enum Access {
    Any,
    Sub,
    Mod,
}

impl Access {
    async fn user_has_access(self, ctx: &Context, msg: &Message) -> bool {
        match self {
            Access::Any => true,
            Access::Sub => {
                // A user is a "subscriber" if they have a coloured role
                if let Some(guild) = msg.guild(ctx).await {
                    if let Ok(member) = guild.member(ctx, msg.author.id).await {
                        for role_id in &member.roles {
                            if let Some(role) = guild.roles.get(role_id) {
                                if role.colour != Colour::default() {
                                    return true;
                                }
                            }
                        }
                    }
                }

                false
            }
            Access::Mod => {
                if let Some(guild) = msg.guild(ctx).await {
                    if let Ok(member) = guild.member(&ctx, &msg.author.id).await {
                        if let Ok(permissions) = member.permissions(ctx).await {
                            return permissions.administrator();
                        }
                    }
                }

                false
            }
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
            .with_context(|| format!("invalid regex syntax with {:?}", emoji.name))?;
        if let Cow::Owned(s) = regex.replace_all(&msg, emoji.mention().to_string().as_str()) {
            msg = s;
        }
    }

    Ok(msg)
}

async fn static_response_impl(ctx: &Context, msg: &Message, command: &str) -> Result<(), Error> {
    info!(
        command_name = command,
        message = msg.content.as_str(),
        message.id = msg.id.0,
        from.id = msg.author.id.0,
        from.name = msg.author.name.as_str(),
        from.discriminator = msg.author.discriminator,
        "Static command received"
    );

    let response = ctx
        .data
        .read()
        .await
        .extract::<LRRbot>()?
        .get_data::<Response>(vec![String::from("responses"), String::from(command)])
        .await
        .context("failed to fetch the command")?;

    if let Response::Some { access, response } = response {
        if access.user_has_access(ctx, msg).await {
            let response = response.choose(&mut rand::thread_rng());
            if let Some(response) = response {
                let mut vars = HashMap::<String, Cow<str>>::new();
                vars.insert(
                    "user".into(),
                    if let Some(guild_id) = msg.guild_id {
                        msg.author
                            .nick_in(&ctx, guild_id)
                            .await
                            .map(Cow::Owned)
                            .unwrap_or_else(|| msg.author.name.as_str().into())
                    } else {
                        msg.author.name.as_str().into()
                    },
                );
                let response =
                    strfmt::strfmt(response, &vars).context("failed to format the reply")?;
                let response = if let Some(guild) = msg.guild(&ctx).await {
                    replace_emojis(response, guild.emojis.values())
                        .context("failed to replace emojis")?
                } else {
                    response
                };
                msg.reply(ctx, &response).await.context("failed to send a reply")?;
            }
        } else {
            info!(
                message.id = msg.id.0,
                access_required = ?access,
                "Refusing to reply because user lacks access"
            );
        }
    }

    Ok(())
}

#[hook]
pub async fn static_response(ctx: &Context, msg: &Message, command: &str) {
    match static_response_impl(ctx, msg, &extract_command(&msg.content, command)).await {
        Ok(()) => (),
        Err(error) => {
            error!(message.id = msg.id.0, ?error, "Static command resulted in an unexpected error");

            let _ = msg
                .reply(
                    ctx,
                    &format!(
                        "Simple text response command resulted in an unexpected error: {}.",
                        error
                    ),
                )
                .await;
        }
    }
}

fn extract_command(msg: &str, command: &str) -> String {
    let index = msg.find(command).unwrap();
    // FIXME: extract the delimiter from the framework configuration
    let mut args = Args::new(&msg[index + command.len()..], &[Delimiter::Single(' ')]);

    let mut command = String::from(command);
    while let Some(arg) = args.trimmed().current() {
        if !arg.is_empty() {
            command.push(' ');
            command.push_str(arg);
        }
        args.advance();
    }

    command
}

#[test]
fn test_deserialize_single_response() {
    let res = serde_json::from_str::<Response>(
        r#"{"access": "any", "response": "Help: https://lrrbot.com/help"}"#,
    )
    .unwrap();
    assert_eq!(
        res,
        Response::Some {
            access: Access::Any,
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
        Response::Some { access: Access::Sub, response: vec!["peach".into(), "barf".into()] }
    );
}

#[cfg(test)]
mod tests {
    use super::Response;
    use serenity::model::guild::Emoji;

    #[test]
    fn deserialize_missing() {
        let res = serde_json::from_str::<Response>("{}").unwrap();
        assert_eq!(res, Response::None {});
    }

    #[test]
    fn replace_emojis() {
        let emoji = serde_json::from_str::<Vec<Emoji>>(
            r#"
            [
                {
                    "animated": false,
                    "id": "1",
                    "name": "lrrDOTS",
                    "managed": true,
                    "require_colons": true,
                    "roles": []
                },
                {
                    "animated": false,
                    "id": "2",
                    "name": "lrrCIRCLE",
                    "managed": true,
                    "require_colons": true,
                    "roles": []
                },
                {
                    "animated": false,
                    "id": "3",
                    "name": "lrrARROW",
                    "managed": true,
                    "require_colons": true,
                    "roles": []
                }
            ]
        "#,
        )
        .unwrap();

        assert_eq!(
            super::replace_emojis(
                "lrrDOTS lrrCIRCLE lrrARROW Visit LoadingReadyRun: http://loadingreadyrun.com/",
                emoji.iter()
            )
            .unwrap(),
            "<:_:1> <:_:2> <:_:3> Visit LoadingReadyRun: http://loadingreadyrun.com/"
        );
    }

    #[test]
    fn extract_command() {
        assert_eq!(super::extract_command(" \t ! \t some \t command \t ", "some"), "some command");
        assert_eq!(super::extract_command("!command", "command"), "command");
        assert_eq!(super::extract_command("<@!1234> some command", "some"), "some command");
    }
}
