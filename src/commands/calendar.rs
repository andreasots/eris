use crate::executor_ext::ExecutorExt;
use crate::google::calendar::{Calendar as GoogleCalendar, Event, FANSTREAMS, LRR};
use crate::time::HumanReadable;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use failure::{Compat, Error};
use serenity::framework::standard::{ArgError, Args, CommandResult};
use serenity::framework::standard::macros::{command, group};
use serenity::model::prelude::*;
use serenity::prelude::*;
use serenity::utils::MessageBuilder;
use std::fmt::Display;
use std::str::FromStr;
use url::Url;
use crate::config::Config;
use crate::extract::Extract;
use crate::typemap_keys::Executor;

group!({
    name: "Calendar",
    options: {
        description: "Commands to query the streaming calendars.",
    },
    commands: [
        next,
        nextfan,
    ],
});

#[command]
#[help_available]
#[description = "Gets the next scheduled stream from the LoadingReadyLive calendar. Can specify a timezone, to show stream in your local time. If no time zone is specified, times will be shown in Moonbase time. Unlike on Twitch the timezone is case-sensitive."]
#[usage = "[TIMEZONE]"]
#[example = "America/New_York"]
#[min_args("0")]
#[max_args("1")]
pub fn next(ctx: &mut Context, msg: &Message, args: Args) -> CommandResult {
    Calendar::lrr().execute(ctx, msg, args)
}

#[command]
#[help_available]
#[description = "Gets the next scheduled stream from the fan-streaming calendar. Can specify a timezone, to show stream in your local time. If no time zone is specified, times will be shown in Moonbase time. Unlike on Twitch the timezone is case-sensitive."]
#[usage = "[TIMEZONE]"]
#[example = "America/New_York"]
#[min_args("0")]
#[max_args("1")]
pub fn nextfan(ctx: &mut Context, msg: &Message, args: Args) -> CommandResult {
    Calendar::fan().execute(ctx, msg, args)
}

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
    fn push_safer<S: Display>(&mut self, text: S) -> &mut Self;
    fn push_event(&mut self, event: &Event, now: DateTime<Utc>, timezone: Tz) -> &mut Self;
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
    fn push_safer<S: Display>(&mut self, text: S) -> &mut Self {
        let text = text.to_string();
        let mut last_index = 0;
        for entity in egg_mode_text::url_entities(&text) {
            self
                .push_safe(&text[last_index..entity.range.0])
                .push("<")
                .push_safe(url_normalise(&entity.substr(&text)))
                .push(">");
            last_index = entity.range.1;
        }
        self.push_safe(&text[last_index..])
    }

    fn push_event(&mut self, event: &Event, now: DateTime<Utc>, tz: Tz) -> &mut Self {
        self.push_safer(&event.summary);

        if let Some(ref location) = event.location {
            self.push(" (").push_safer(location).push(")");
        }

        if let Some(ref desc) = event.description {
            // TODO: shorten to 200 characters.
            self
                .push(" (")
                .push_safer(GoogleCalendar::format_description(desc))
                .push(")");
        }
        self.push(" on ").push(
            event
                .start
                .with_timezone(&tz)
                .format("%a %e %b at %I:%M %p %Z"),
        );

        let start = event.start.with_timezone(&Utc);
        self.push(" (");
        if start > now {
            self
                .push(HumanReadable::new(start - now))
                .push(" from now)");
        } else {
            self.push(HumanReadable::new(now - start)).push(" ago)");
        }

        self
    }
}

struct Calendar {
    calendar: &'static str,
    tag: &'static str,
    include_current: bool,
}

impl Calendar {
    pub const fn lrr() -> Calendar {
        Calendar {
            calendar: LRR,
            tag: "Next scheduled stream",
            include_current: false,
        }
    }

    pub const fn fan() -> Calendar {
        Calendar {
            calendar: FANSTREAMS,
            tag: "Next scheduled fan stream",
            include_current: true,
        }
    }

    pub fn execute(&self, ctx: &mut Context, msg: &Message, mut args: Args) -> CommandResult {
        let data = ctx.data.read();
        let config = data.extract::<Config>()?;
        let google_calendar = data.extract::<GoogleCalendar>()?;
        let executor = data.extract::<Executor>()?;
        let tz = match args.single::<Timezone>() {
            Ok(tz) => tz.0,
            Err(ArgError::Eos) => config.timezone,
            Err(ArgError::Parse(err)) => {
                msg.reply(ctx, &format!("Failed to parse the timezone: {}", err))?;
                return Ok(());
            },
            Err(err) => return Err(err.into()),
        };

        let now = Utc::now();

        let events = {
            let google_calendar = google_calendar.clone();
            let calendar = self.calendar;
            executor.block_on(
                async move { google_calendar.get_upcoming_events(calendar, now).await },
            )?
        };

        let events = GoogleCalendar::get_next_event(&events, now, self.include_current);

        let mut builder = MessageBuilder::new();
        builder.push_safer(self.tag).push(": ");

        for (i, event) in events.iter().enumerate() {
            if i != 0 {
                builder.push(", ");
            }
            builder.push_event(event, now, tz);
        }

        msg.reply(ctx, &builder.build())?;

        Ok(())
    }
}
