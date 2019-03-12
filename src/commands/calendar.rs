use crate::config::Config;
use crate::executor_ext::ExecutorExt;
use crate::google::calendar::{Calendar as GoogleCalendar, Event, FANSTREAMS, LRR};
use crate::time::HumanReadable;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use failure::{Compat, Error};
use serenity::framework::standard::{ArgError, Args, Command, CommandError};
use serenity::model::prelude::*;
use serenity::prelude::*;
use serenity::utils::MessageBuilder;
use std::fmt::Display;
use std::str::FromStr;
use std::sync::Arc;
use tokio::runtime::TaskExecutor;
use url::Url;

struct Timezone(Tz);

impl FromStr for Timezone {
    type Err = Compat<Error>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Timezone(
            Tz::from_str(s).map_err(|err| failure::err_msg(err).compat())?,
        ))
    }
}

trait PushEvent {
    fn push_safer<S: Display>(self, text: S) -> Self;
    fn push_event(self, event: &Event, now: DateTime<Utc>, timezone: Tz) -> Self;
}

fn url_normalise(url: &str) -> String {
    match Url::parse(url) {
        Ok(url) => url.into_string(),
        Err(url::ParseError::RelativeUrlWithoutBase) => url_normalise(&format!("http://{}", url)),
        Err(_) => String::from(url),
    }
}

impl PushEvent for MessageBuilder {
    /// Like `push_safe` but also extract URLs from `text` and suppress previews.
    fn push_safer<S: Display>(mut self, text: S) -> Self {
        let text = text.to_string();
        let mut last_index = 0;
        for entity in egg_mode_text::url_entities(&text) {
            self = self
                .push_safe(&text[last_index..entity.range.0])
                .push("<")
                .push_safe(url_normalise(&entity.substr(&text)))
                .push(">");
            last_index = entity.range.1;
        }
        self.push_safe(&text[last_index..])
    }

    fn push_event(mut self, event: &Event, now: DateTime<Utc>, tz: Tz) -> Self {
        self = self.push_safer(&event.summary);

        if let Some(ref location) = event.location {
            self = self.push(" (").push_safer(location).push(")");
        }

        if let Some(ref desc) = event.description {
            // TODO: shorten to 200 characters.
            self = self
                .push(" (")
                .push_safer(GoogleCalendar::format_description(desc))
                .push(")");
        }
        self = self.push(" on ").push(
            event
                .start
                .with_timezone(&tz)
                .format("%a %e %b at %I:%M %p %Z"),
        );

        let start = event.start.with_timezone(&Utc);
        self = self.push(" (");
        if start > now {
            self = self
                .push(HumanReadable::new(start - now))
                .push(" from now)");
        } else {
            self = self.push(HumanReadable::new(now - start)).push(" ago)");
        }

        self
    }
}

pub struct Calendar {
    config: Arc<Config>,
    google_calendar: GoogleCalendar,
    executor: TaskExecutor,
    calendar: &'static str,
    tag: &'static str,
    include_current: bool,
}

impl Calendar {
    pub fn lrr(
        config: Arc<Config>,
        google_calendar: GoogleCalendar,
        executor: TaskExecutor,
    ) -> Calendar {
        Calendar {
            config,
            google_calendar,
            executor,
            calendar: LRR,
            tag: "Next scheduled stream",
            include_current: false,
        }
    }

    pub fn fan(
        config: Arc<Config>,
        google_calendar: GoogleCalendar,
        executor: TaskExecutor,
    ) -> Calendar {
        Calendar {
            config,
            google_calendar,
            executor,
            calendar: FANSTREAMS,
            tag: "Next scheduled fan stream",
            include_current: true,
        }
    }
}

impl Command for Calendar {
    fn execute(&self, _: &mut Context, msg: &Message, mut args: Args) -> Result<(), CommandError> {
        let tz = match args.single::<Timezone>() {
            Ok(tz) => tz.0,
            Err(ArgError::Eos) => self.config.timezone,
            Err(ArgError::Parse(err)) => {
                msg.reply(&format!("Failed to parse the timezone: {}", err))?;
                return Ok(());
            }
        };

        let now = Utc::now();

        let events = {
            let google_calendar = self.google_calendar.clone();
            let calendar = self.calendar;
            self.executor.block_on(
                async move { await!(google_calendar.get_upcoming_events(calendar, now)) },
            )?
        };

        let events = GoogleCalendar::get_next_event(&events, now, self.include_current);

        let mut builder = MessageBuilder::new().push_safer(self.tag).push(": ");

        for (i, event) in events.iter().enumerate() {
            if i != 0 {
                builder = builder.push(", ");
            }
            builder = builder.push_event(event, now, tz);
        }

        msg.reply(&builder.build())?;

        Ok(())
    }
}
