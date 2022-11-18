use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

use anyhow::{Context as _, Error};
use time::macros::format_description;
use time::OffsetDateTime;
use time_tz::{OffsetDateTimeExt, TimeZone};
use twilight_cache_inmemory::InMemoryCache;
use twilight_http::Client as DiscordClient;
use twilight_model::channel::message::MessageFlags;
use twilight_model::channel::Message;

use crate::calendar::{CalendarHub, FANSTREAMS, LRR};
use crate::command_parser::{Args, CommandHandler, Commands, Help};
use crate::config::Config;
use crate::time::HumanReadable;

#[derive(Clone, Copy)]
enum Mode {
    Lrr,
    Fan,
}

impl Mode {
    fn pattern(self) -> &'static str {
        match self {
            Mode::Lrr => r"next(?: (.+))?",
            Mode::Fan => r"nextfan(?: (.+))?",
        }
    }

    fn calendar_id(self) -> &'static str {
        match self {
            Mode::Lrr => LRR,
            Mode::Fan => FANSTREAMS,
        }
    }

    fn include_current(self) -> bool {
        match self {
            Mode::Lrr => false,
            Mode::Fan => true,
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Mode::Lrr => "Next scheduled stream",
            Mode::Fan => "Next scheduled fan stream",
        }
    }

    fn help(self) -> Help {
        match self {
            Mode::Lrr => Help {
                name: "next".into(),
                usage: "next [TIMEZONE]".into(),
                summary: "Get the next scheduled stream from the streaming calendar".into(),
                description: concat!(
                    "Get the next scheduled stream from the ",
                    "[LoadingReadyRun Streams calendar](http://lrr.cc/schedule).\n\n",
                    "Can specify a timezone, to show stream in your local time. If no time zone ",
                    "is specified, times will be shown in Moonbase time.",
                )
                .into(),
                examples: Cow::Borrowed(&[Cow::Borrowed("next America/New_York")]),
            },
            Mode::Fan => Help {
                name: "nextfan".into(),
                usage: "nextfan [TIMEZONE]".into(),
                summary: "Get the next scheduled stream from the fan-streaming calendar".into(),
                description: concat!(
                    "Get the next scheduled stream from the ",
                    "[fan-streaming calendar](http://bit.ly/LRRFanStreamSched).\n\n",
                    "Can specify a timezone, to show stream in your local time. If no time zone ",
                    "is specified, times will be shown in Moonbase time.",
                )
                .into(),
                examples: Cow::Borrowed(&[Cow::Borrowed("nextfan America/New_York")]),
            },
        }
    }
}

pub struct Next {
    mode: Mode,
    calendar: CalendarHub,
}

impl Next {
    pub const fn lrr(calendar: CalendarHub) -> Next {
        Next { mode: Mode::Lrr, calendar }
    }

    pub const fn fan(calendar: CalendarHub) -> Next {
        Next { mode: Mode::Fan, calendar }
    }

    pub async fn get_response(&self, config: &Config, args: &Args) -> Result<String, Error> {
        let tz = match args.get(0) {
            Some(name) => {
                match time_tz::timezones::iter().find(|tz| tz.name().eq_ignore_ascii_case(name)) {
                    Some(tz) => tz,
                    None => {
                        return Ok(format!("Unknown time zone: {}", crate::markdown::escape(name)))
                    }
                }
            }
            None => config.timezone,
        };

        let now = OffsetDateTime::now_utc();

        let events = crate::calendar::get_next_event(
            &self.calendar,
            self.mode.calendar_id(),
            now,
            self.mode.include_current(),
        )
        .await
        .context("failed to get the upcoming events")?;

        let mut result = String::from(self.mode.tag());
        result.push_str(": ");

        for (i, event) in events.iter().enumerate() {
            if i != 0 {
                result.push_str(", ");
            }

            result.push_str(&crate::markdown::escape(&event.summary));

            if let Some(ref location) = event.location {
                result.push_str(" (");
                result.push_str(&crate::markdown::escape(location));
                result.push_str(")");
            }

            if let Some(ref desc) = event.description {
                // TODO: shorten to 200 characters.
                result.push_str(" (");
                result.push_str(&crate::markdown::escape(&crate::shorten::shorten(
                    &crate::calendar::format_description(desc),
                    200,
                )));
                result.push_str(")");
            }
            result.push_str(" on ");
            result.push_str(&
            event
                .start
                .to_timezone(tz)
                .format(format_description!(
                    // TODO: timezone short name
                    "[weekday repr:short] [day padding:space] [month repr:short] at [hour repr:12]:[minute] [period]"
                ))
                .context("failed to format the event start")?
            );

            result.push_str(" (");
            if event.start > now {
                result.push_str(&HumanReadable::new(event.start - now).to_string());
                result.push_str(" from now)");
            } else {
                result.push_str(&HumanReadable::new(now - event.start).to_string());
                result.push_str(" ago)");
            }
        }

        Ok(result)
    }
}

impl CommandHandler for Next {
    fn pattern(&self) -> &str {
        self.mode.pattern()
    }

    fn help(&self) -> Option<Help> {
        Some(self.mode.help())
    }

    fn handle<'a>(
        &'a self,
        _: &'a InMemoryCache,
        config: &'a Config,
        discord: &'a DiscordClient,
        _: Commands<'a>,
        message: &'a Message,
        args: &'a Args,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            discord
                .create_message(message.channel_id)
                .reply(message.id)
                .flags(MessageFlags::SUPPRESS_EMBEDS)
                .content(&self.get_response(config, &args).await?)
                .context("reply message invalid")?
                .await
                .context("failed to reply to command")?;
            Ok(())
        })
    }
}
