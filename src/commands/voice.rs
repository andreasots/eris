use crate::config::Config;
use crate::extract::Extract;
use serenity::framework::standard::macros::{command, group};
use serenity::framework::standard::{Args, CommandResult};
use serenity::model::prelude::*;
use serenity::prelude::*;

#[group("Voice")]
#[description = "Voice channel commands"]
#[commands(voice)]
struct Voice;

#[command]
#[description = "Create a temporary voice channel. Unused temporary voice channels will be automatically deleted if they're older than 15 minutes."]
#[usage = "CHANNEL NAME"]
#[example = "PUBG #15"]
pub async fn voice(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    let data = ctx.data.read().await;
    let config = data.extract::<Config>()?;

    let name = format!("{} {}", config.temp_channel_prefix, args.rest().trim());
    let reply = match config
        .guild
        .create_channel(&ctx, |c| {
            c.name(name).category(config.voice_category).kind(ChannelType::Voice)
        })
        .await
    {
        Ok(channel) => format!("Created a temporary voice channel {:?}", channel.name),
        Err(err) => format!("Failed to create a temporary voice channel: {}", err),
    };
    msg.reply(&ctx, &reply).await?;
    Ok(())
}
