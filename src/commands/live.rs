use crate::config::Config;
use crate::executor_ext::ExecutorExt;
use crate::models::User;
use crate::twitch::Kraken;
use diesel::prelude::*;
use serenity::framework::standard::{Args, CommandResult};
use serenity::framework::standard::macros::{command, group};
use serenity::model::prelude::*;
use serenity::prelude::*;
use crate::typemap_keys::{PgPool, Executor};
use crate::extract::Extract;

group!({
    name: "Fanstreams",
    commands: [
        live,
    ],
});


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

    if streams.len() == 0 {
        msg.reply(ctx, "No fanstreamers currently live.")?;
    } else {
        streams.sort_by(|a, b| {
            a.channel
                .display_name
                .as_ref()
                .unwrap_or(&a.channel.name)
                .cmp(b.channel.display_name.as_ref().unwrap_or(&b.channel.name))
        });
        let streams = streams
            .into_iter()
            .map(|stream| {
                let display_name = stream
                    .channel
                    .display_name
                    .as_ref()
                    .unwrap_or(&stream.channel.name);
                let mut output = format!(
                    "{} (<{}>)",
                    markdown_escape(display_name),
                    stream.channel.url
                );
                if let Some(game) = stream.game {
                    output += &format!(" is playing {}", markdown_escape(&game));
                }
                if let Some(status) = stream.channel.status {
                    output += &format!(" ({})", markdown_escape(&status));
                }

                output
            })
            .collect::<Vec<String>>();
        msg.reply(ctx, &format!(
            "Currently live fanstreamers: {}",
            streams.join(", ")
        ))?;
    }

    Ok(())
}

fn markdown_escape(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '_' | '*' | '<' | '`' => vec!['\\', c],
            '#' | '@' => vec![c, '\u{200B}'],
            c => vec![c],
        })
        .collect()
}
