use crate::config::Config;
use crate::extract::Extract;
use crate::models::User;
use crate::twitch::helix::{Game, GameDescriptor, Stream, User as TwitchUser};
use crate::twitch::Helix;
use crate::typemap_keys::{Executor, PgPool};
use anyhow::{Context as _, Error};
use serenity::framework::standard::macros::{command, group};
use serenity::framework::standard::{Args, CommandResult};
use serenity::model::prelude::*;
use serenity::prelude::*;
use serenity::utils::MessageBuilder;
use std::collections::HashMap;

#[group("Fanstreams")]
#[description = "Fanstream commands"]
#[commands(live)]
struct Fanstreams;

fn push_stream(builder: &mut MessageBuilder, games: &HashMap<&str, &Game>, stream: &Stream) {
    // FIXME: the MessageBuilder doesn't escape spoilers
    builder.push_safe(&stream.user_name.replace('|', "\\|"));
    builder.push(" (<https://twitch.tv/");
    builder.push_safe(&stream.user_name.replace('|', "\\|"));
    builder.push(">)");
    builder.push(" is playing ");
    builder.push_safe(&games[stream.game_id.as_str()].name.replace('|', "\\|"));
    builder.push(" (");
    builder.push_safe(&stream.title.replace('|', "\\|"));
    builder.push(")");
}

#[command]
#[help_available]
#[description = "Post the currently live fanstreamers."]
#[num_args(0)]
fn live(ctx: &mut Context, msg: &Message, _: Args) -> CommandResult {
    let data = ctx.data.read();
    let user = {
        let conn = data.extract::<PgPool>()?.get()?;

        User::by_name(&data.extract::<Config>()?.username, &conn)
            .context("failed to load the bot user")?
    };

    let (mut streams, games) = {
        let helix = data.extract::<Helix>()?.clone();
        data.extract::<Executor>()?.block_on(async move {
            let token = user.twitch_oauth.as_ref().map(String::as_str).context("token missing")?;

            let follows = helix
                .get_user_follows(token, Some(&user.id.to_string()), None)
                .await
                .context("failed to get the follows")?;

            let users =
                follows.iter().map(|follow| TwitchUser::Id(&follow.to_id)).collect::<Vec<_>>();

            let streams =
                helix.get_streams(token, &users).await.context("failed to get the streams")?;

            let games = streams
                .iter()
                .map(|stream| GameDescriptor::Id(&stream.game_id))
                .collect::<Vec<_>>();

            let games = helix.get_games(token, &games).await.context("failed to get the games")?;

            Ok::<_, Error>((streams, games))
        })?
    };

    let games = games.iter().map(|game| (game.id.as_str(), game)).collect::<HashMap<&str, &Game>>();

    if streams.is_empty() {
        msg.reply(&ctx, "No fanstreamers currently live.")?;
    } else {
        streams.sort_by(|a, b| a.user_name.cmp(&b.user_name));
        let mut builder = MessageBuilder::new();
        builder.push("Currently live fanstreamers: ");

        for (i, stream) in streams.iter().enumerate() {
            if i != 0 {
                builder.push(", ");
            }
            push_stream(&mut builder, &games, &stream);
        }
        msg.reply(&ctx, builder.build())?;
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use super::push_stream;
    use crate::twitch::helix::{Game, Stream, StreamType};
    use chrono::DateTime;
    use serenity::utils::MessageBuilder;
    use std::collections::HashMap;

    #[test]
    fn formatting() {
        let minesweeper = Game {
            id: "3681".to_string(),
            name: "Minesweeper".to_string(),
            box_art_url: "https://".to_string(),
        };

        let mut games = HashMap::new();
        games.insert(minesweeper.id.as_str(), &minesweeper);

        let mut builder = MessageBuilder::new();
        push_stream(
            &mut builder,
            &games,
            &Stream {
                game_id: "3681".to_string(),
                id: "123456789".to_string(),
                language: "en".to_string(),
                started_at: DateTime::parse_from_rfc3339("2020-04-07T11:45:20Z").unwrap(),
                tag_ids: vec![],
                thumbnail_url: "https://".to_string(),
                title: "Let's explode || Minesweeper".to_string(),
                stream_type: StreamType::Live,
                user_id: "29801300".to_string(),
                user_name: "qrpth".to_string(),
                viewer_count: 1,
            },
        );
        assert_eq!(builder.build(), "qrpth (<https://twitch.tv/qrpth>) is playing Minesweeper (Let\'s explode \\|\\| Minesweeper)");
    }
}
