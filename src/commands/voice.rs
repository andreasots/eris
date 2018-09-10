use crate::config::Config;
use serenity::framework::standard::{Args, Command, CommandError};
use serenity::model::prelude::*;
use serenity::prelude::*;
use std::sync::Arc;

pub struct Voice {
    config: Arc<Config>,
}

impl Voice {
    pub fn new(config: Arc<Config>) -> Voice {
        Voice { config }
    }
}

impl Command for Voice {
    fn execute(&self, _: &mut Context, msg: &Message, args: Args) -> Result<(), CommandError> {
        let name = format!("{} {}", self.config.temp_channel_prefix, args.full());
        let reply = match self.config.guild.create_channel(
            &name,
            ChannelType::Voice,
            self.config.voice_category,
        ) {
            Ok(channel) => format!("Created a temporary voice channel {:?}", channel.name),
            Err(err) => format!("Failed to create a temporary voice channel: {}", err),
        };
        msg.reply(&reply)?;
        Ok(())
    }
}
