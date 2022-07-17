use crate::config::Config;
use crate::extract::Extract;
use serenity::framework::standard::macros::{command, group};
use serenity::framework::standard::{Args, CommandResult};
use serenity::model::prelude::*;
use serenity::prelude::*;
use time::macros::format_description;
use time::OffsetDateTime;
use time_tz::OffsetDateTimeExt;

#[group("Time")]
#[description = "Time commands"]
#[commands(time)]
struct Time;

#[command]
#[description = "Post the current moonbase time, optionally in the 24-hour format."]
#[usage = "[24]"]
#[example = "24"]
#[min_args(0)]
#[max_args(1)]
async fn time(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    let format = match args.current() {
        Some("24") => format_description!("[hour repr:24]:[minute]"),
        None => format_description!("[hour repr:12]:[minute] [period]"),
        _ => return Ok(()),
    };

    let now =
        OffsetDateTime::now_utc().to_timezone(ctx.data.read().await.extract::<Config>()?.timezone);
    msg.reply(ctx, &format!("Current moonbase time: {}", now.format(format)?)).await?;

    Ok(())
}
