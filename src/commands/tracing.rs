use crate::extract::Extract;
use crate::typemap_keys::ReloadHandle;
use serenity::framework::standard::macros::{command, group};
use serenity::framework::standard::{Args, CommandResult};
use serenity::model::prelude::*;
use serenity::prelude::*;
use tracing_subscriber::EnvFilter;

#[group("Tracing")]
#[description = "Tracing commands"]
#[commands(tracing_filter)]
struct Tracing;

#[command]
#[owners_only]
#[help_available(false)]
async fn tracing_filter(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    let directives = args.rest().trim();
    let directives = if directives != "" { directives } else { crate::DEFAULT_TRACING_FILTER };

    let filter = match EnvFilter::try_new(&directives) {
        Ok(filter) => filter,
        Err(err) => {
            msg.reply(ctx, format!("Failed to construct the new filter: {:?}", err)).await?;

            return Ok(());
        }
    };

    let mut old_filter = String::new();

    ctx.data.read().await.extract::<ReloadHandle>()?.modify(|layer| {
        old_filter = format!("{}", layer);
        *layer = filter;
    })?;

    msg.reply(ctx, format!("Replaced `{}` with `{}`.", old_filter, directives)).await?;

    Ok(())
}
