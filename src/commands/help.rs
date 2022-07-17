use serenity::client::Context;
use serenity::framework::standard::help_commands;
use serenity::framework::standard::macros::help;
use serenity::framework::standard::{Args, CommandGroup, CommandResult, HelpOptions};
use serenity::model::channel::Message;
use serenity::model::id::UserId;
use std::collections::HashSet;

#[help]
#[individual_command_tip = "To get help with an individual command, pass its name as an argument to this command. Simple text response commands (like `!advice`) are  not listed here, for those see <https://lrrbot.com/help#help-section-text>."]
async fn help(
    ctx: &Context,
    msg: &Message,
    args: Args,
    help_options: &'static HelpOptions,
    groups: &[&'static CommandGroup],
    owners: HashSet<UserId>,
) -> CommandResult {
    help_commands::with_embeds(ctx, msg, args, help_options, groups, owners).await?;
    Ok(())
}
