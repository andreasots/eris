use crate::config::Config;
use crate::extract::Extract;
use crate::google::calendar::{Calendar as GoogleCalendar, Event, FANSTREAMS, LRR};
use crate::time::HumanReadable;
use anyhow::Error;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serenity::framework::standard::macros::{command, group};
use serenity::framework::standard::{ArgError, Args, CommandResult};
use serenity::model::prelude::*;
use serenity::prelude::*;
use serenity::utils::MessageBuilder;
use std::fmt::Display;
use std::str::FromStr;
use url::Url;

#[group("Calendar")]
#[description("Connands to query the streaming calendars.")]
#[commands(next, nextfan)]
struct Calendar;

#[command]
#[help_available]
#[description = "Gets the next scheduled stream from the LoadingReadyLive calendar. Can specify a timezone, to show stream in your local time. If no time zone is specified, times will be shown in Moonbase time. Unlike on Twitch the timezone is case-sensitive."]
#[usage = "[TIMEZONE]"]
#[example = "America/New_York"]
#[min_args("0")]
#[max_args("1")]
pub async fn next(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    Next::lrr().execute(ctx, msg, args).await
}

#[command]
#[help_available]
#[description = "Gets the next scheduled stream from the fan-streaming calendar. Can specify a timezone, to show stream in your local time. If no time zone is specified, times will be shown in Moonbase time. Unlike on Twitch the timezone is case-sensitive."]
#[usage = "[TIMEZONE]"]
#[example = "America/New_York"]
#[min_args("0")]
#[max_args("1")]
pub async fn nextfan(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    Next::fan().execute(ctx, msg, args).await
}

struct Timezone(Tz);

impl FromStr for Timezone {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Timezone(Tz::from_str(s).map_err(Error::msg)?))
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
            self.push_safe(&text[last_index..entity.range.0])
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
            self.push(" (").push_safer(GoogleCalendar::format_description(desc)).push(")");
        }
        self.push(" on ").push(event.start.with_timezone(&tz).format("%a %e %b at %I:%M %p %Z"));

        let start = event.start.with_timezone(&Utc);
        self.push(" (");
        if start > now {
            self.push(HumanReadable::new(start - now)).push(" from now)");
        } else {
            self.push(HumanReadable::new(now - start)).push(" ago)");
        }

        self
    }
}

struct Next {
    calendar: &'static str,
    tag: &'static str,
    include_current: bool,
}

impl Next {
    pub const fn lrr() -> Next {
        Next { calendar: LRR, tag: "Next scheduled stream", include_current: false }
    }

    pub const fn fan() -> Next {
        Next { calendar: FANSTREAMS, tag: "Next scheduled fan stream", include_current: true }
    }

    pub async fn execute(&self, ctx: &Context, msg: &Message, mut args: Args) -> CommandResult {
        let data = ctx.data.read().await;
        let config = data.extract::<Config>()?;
        let google_calendar = data.extract::<GoogleCalendar>()?;
        let tz = match args.single::<Timezone>() {
            Ok(tz) => tz.0,
            Err(ArgError::Eos) => config.timezone,
            Err(ArgError::Parse(err)) => {
                msg.reply(&ctx, &format!("Failed to parse the timezone: {}", err)).await?;
                return Ok(());
            }
            Err(err) => return Err(err.into()),
        };

        let now = Utc::now();

        let events = google_calendar.get_upcoming_events(self.calendar, now).await?;
        let events = GoogleCalendar::get_next_event(&events, now, self.include_current);

        let mut builder = MessageBuilder::new();
        builder.push_safer(self.tag).push(": ");

        for (i, event) in events.iter().enumerate() {
            if i != 0 {
                builder.push(", ");
            }
            builder.push_event(event, now, tz);
        }

        msg.reply(&ctx, &builder.build()).await?;

        Ok(())
    }
}
