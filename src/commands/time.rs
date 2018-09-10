use chrono::Utc;
use crate::config::Config;
use serenity::framework::standard::{Args, Command, CommandError};
use serenity::model::prelude::*;
use serenity::prelude::*;
use std::sync::Arc;

pub struct Time {
    config: Arc<Config>,
}

impl Time {
    pub fn new(config: Arc<Config>) -> Time {
        Time { config }
    }
}

impl Command for Time {
    fn execute(&self, _: &mut Context, msg: &Message, args: Args) -> Result<(), CommandError> {
        let format = match args.current() {
            Some("24") => "%H:%M",
            None => "%l:%M %p",
            _ => return Ok(()),
        };

        let now = Utc::now().with_timezone(&self.config.timezone);
        msg.reply(&format!("Current moonbase time: {}", now.format(format)))?;

        Ok(())
    }
}
