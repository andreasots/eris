use serenity::framework::standard::macros::help;
use serenity::framework::standard::help_commands;
use serenity::client::Context;
use serenity::model::channel::Message;
use std::collections::HashSet;
use serenity::model::id::UserId;
use std::hash::BuildHasher;
use serenity::framework::standard::{HelpOptions, CommandGroup, CommandResult, Args};

#[help]
#[individual_command_tip = "To get help with an individual command, pass its name as an argument to this command. Simple text response commands (like `!advice`) are  not listed here, for those see <https://lrrbot.com/help#help-section-text>."]
fn help(
    ctx: &mut Context,
    msg: &Message,
    args: Args,
    help_options: &'static HelpOptions,
    groups: &[&'static CommandGroup],
    owners: HashSet<UserId, impl BuildHasher>
) -> CommandResult {
    help_commands::with_embeds(ctx, msg, args, help_options, groups, owners)
}
