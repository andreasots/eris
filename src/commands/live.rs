use crate::config::Config;
use crate::extract::Extract;
use crate::models::User;
use crate::twitch::kraken::Stream;
use crate::twitch::Kraken;
use crate::typemap_keys::{Executor, PgPool};
use diesel::prelude::*;
use serenity::framework::standard::macros::{command, group};
use serenity::framework::standard::{Args, CommandResult};
use serenity::model::prelude::*;
use serenity::prelude::*;
use serenity::utils::MessageBuilder;

#[group("Fanstreams")]
#[description = "Fanstream commands"]
#[commands(live)]
struct Fanstreams;

fn push_stream(builder: &mut MessageBuilder, stream: &Stream) {
    // FIXME: the MessageBuilder doesn't escape spoilers
    builder.push_safe(
        stream.channel.display_name.as_ref().unwrap_or(&stream.channel.name).replace("||", "\\||"),
    );
    builder.push(" (<");
    builder.push_safe(stream.channel.url.replace("||", "\\||"));
    builder.push(">)");
    if let Some(game) = stream.game.as_ref() {
        builder.push(" is playing ");
        builder.push_safe(game.replace("||", "\\||"));
    }
    if let Some(status) = stream.channel.status.as_ref() {
        builder.push(" (");
        builder.push_safe(status.replace("||", "\\||"));
        builder.push(")");
    }
}

#[command]
#[help_available]
#[description = "Post the currently live fanstreamers."]
#[num_args(0)]
fn live(ctx: &mut Context, msg: &Message, _: Args) -> CommandResult {
    let data = ctx.data.read();
    let token = {
        use crate::schema::users::dsl::*;

        let conn = data.extract::<PgPool>()?.get()?;

        users
            .filter(name.eq(&data.extract::<Config>()?.username))
            .first::<User>(&conn)?
            .twitch_oauth
            .ok_or("Twitch token missing")?
    };

    let mut streams = {
        let kraken = data.extract::<Kraken>()?.clone();
        data.extract::<Executor>()?
            .block_on(async move { kraken.get_streams_followed(token).await })?
    };

    if streams.is_empty() {
        msg.reply(&ctx, "No fanstreamers currently live.")?;
    } else {
        streams.sort_by(|a, b| {
            a.channel
                .display_name
                .as_ref()
                .unwrap_or(&a.channel.name)
                .cmp(b.channel.display_name.as_ref().unwrap_or(&b.channel.name))
        });
        let mut builder = MessageBuilder::new();
        builder.push("Currently live fanstreamers: ");

        for (i, stream) in streams.iter().enumerate() {
            if i != 0 {
                builder.push(", ");
            }
            push_stream(&mut builder, &stream);
        }
        msg.reply(&ctx, builder.build())?;
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use super::push_stream;
    use crate::twitch::kraken::{Channel, Stream};
    use serenity::utils::MessageBuilder;

    #[test]
    fn formatting() {
        let mut builder = MessageBuilder::new();
        push_stream(
            &mut builder,
            &Stream {
                channel: Channel {
                    display_name: None,
                    name: "qrpth".to_string(),
                    status: Some("Let's explode || Minesweeper".to_string()),
                    url: "https://twitch.tv/qrpth".to_string(),
                },
                game: Some("Minesweeper".to_string()),
            },
        );
        assert_eq!(builder.build(), "qrpth (<https://twitch.tv/qrpth>) is playing Minesweeper (Let\'s explode \\|| Minesweeper)");
    }
}
