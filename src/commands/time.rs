use crate::config::Config;
use chrono::Utc;
use serenity::framework::standard::{Args, CommandResult};
use serenity::framework::standard::macros::{command, group};
use serenity::model::prelude::*;
use serenity::prelude::*;
use crate::extract::Extract;

group!({
    name: "Time",
    commands: [
        time,
    ],
});

#[command]
#[description = "Post the current moonbase time, optionally in the 24-hour format."]
#[usage = "[24]"]
#[example = "24"]
#[min_args(0)]
#[max_args(1)]
fn time(ctx: &mut Context, msg: &Message, args: Args) -> CommandResult {
    let format = match args.current() {
        Some("24") => "%H:%M",
        None => "%l:%M %p",
        _ => return Ok(()),
    };

    let now = Utc::now().with_timezone(&ctx.data.read().extract::<Config>()?.timezone);
    msg.reply(ctx, &format!("Current moonbase time: {}", now.format(format)))?;

    Ok(())
}
