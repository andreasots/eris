use crate::config::Config;
use crate::extract::Extract;
use chrono::{NaiveDate, TimeZone, Utc};
use ordinal::Ordinal;
use serenity::framework::standard::macros::{command, group};
use serenity::framework::standard::{Args, CommandResult};
use serenity::model::prelude::*;
use serenity::prelude::*;

#[group("Date")]
#[description = "Date commands"]
#[commands(march, september, november)]
struct Date;

#[command]
#[description = "Post the current date.\n\n(https://en.wikipedia.org/wiki/COVID-19)"]
#[num_args(0)]
fn march(ctx: &mut Context, msg: &Message, _args: Args) -> CommandResult {
    eternal(ctx, msg, NaiveDate::from_ymd(2020, 3, 1))
}

#[command]
#[description = "Post the current date.\n\n(https://en.wikipedia.org/wiki/Eternal_September)"]
#[num_args(0)]
fn september(ctx: &mut Context, msg: &Message, _args: Args) -> CommandResult {
    eternal(ctx, msg, NaiveDate::from_ymd(1993, 9, 1))
}

#[command]
#[description = "Post the current date.\n\n(https://desertbus.org/)"]
#[num_args(0)]
fn november(ctx: &mut Context, msg: &Message, _args: Args) -> CommandResult {
    eternal(ctx, msg, NaiveDate::from_ymd(2007, 11, 1))
}

fn eternal(ctx: &mut Context, msg: &Message, epoch: NaiveDate) -> CommandResult {
    let timezone = ctx.data.read().extract::<Config>()?.timezone;
    let epoch = timezone.from_local_date(&epoch).unwrap();
    let today = Utc::now().with_timezone(&timezone).date();
    let day = (today - epoch).num_days() + 1;

    msg.reply(
        ctx,
        &format!("Today is {}, {} of {}", today.format("%A"), Ordinal(day), epoch.format("%B, %Y")),
    )?;

    Ok(())
}
