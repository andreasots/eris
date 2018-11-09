use chrono::{DateTime, FixedOffset, Utc};
use chrono_tz::Tz;
use crate::config::Config;
use crate::desertbus::DesertBus;
use crate::google_calendar::{Calendar, Event, LRR};
use crate::models::{Game, GameEntry, Show};
use crate::rpc::LRRbot;
use crate::time::HumanReadable;
use crate::twitch::helix::User;
use crate::twitch::Helix;
use crate::PgPool;
use diesel::OptionalExtension;
use failure::{Error, ResultExt, SyncFailure};
use futures::compat::Stream01CompatExt;
use futures::prelude::*;
use separator::FixedPlaceSeparatable;
use slog::slog_error;
use slog_scope::error;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::timer::Interval;

struct ShortDisplay<'a> {
    event: &'a Event,
    now: DateTime<Utc>,
    tz: Tz,
}

impl<'a> fmt::Display for ShortDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let start = self.event.start.with_timezone(&Utc);
        if start > self.now {
            write!(f, "In {}: ", HumanReadable::new(start - self.now))?;
        } else {
            write!(f, "{} ago: ", HumanReadable::new(self.now - start))?;
        }

        f.write_str(&self.event.summary)?;

        if let Some(ref location) = self.event.location {
            write!(f, " ({})", location);
        }

        if let Some(ref desc) = self.event.description {
            // TODO: shorten to 200 characters.
            write!(f, " ({})", Calendar::format_description(desc))?;
        }
        write!(
            f,
            " on {}.",
            self.event
                .start
                .with_timezone(&self.tz)
                .format("%a %e %b at %I:%M %p %Z")
        )?;

        Ok(())
    }
}

pub async fn autotopic(
    config: Arc<Config>,
    helix: Helix,
    calendar: Calendar,
    desertbus: DesertBus,
    pg_pool: PgPool,
) {
    let lrrbot = LRRbot::new(&config);
    let mut autotopic = Autotopic {
        config,
        lrrbot,
        pg_pool,
        helix,
        calendar,
        desertbus,
    };

    let mut timer = Interval::new(Instant::now(), Duration::from_secs(60)).compat();

    loop {
        match await!(timer.try_next()) {
            Ok(Some(_)) => match await!(autotopic.update_topic()) {
                Ok(()) => (),
                Err(err) => error!("Failed to update the topic"; "error" => ?err),
            },
            Ok(None) => break,
            Err(err) => {
                error!("Timer error"; "error" => ?err);
            }
        }
    }
}

struct Autotopic {
    config: Arc<Config>,
    lrrbot: LRRbot,
    pg_pool: PgPool,
    helix: Helix,
    calendar: Calendar,
    desertbus: DesertBus,
}

impl Autotopic {
    async fn update_topic<'a>(&mut self) -> Result<(), Error> {
        let header =
            await!(self.lrrbot.get_header_info()).context("failed to fetch header info")?;
        // FIXME: This is what discord.py was doing. This should probably be a config option instead.
        #[allow(deprecated)]
        let general = self.config.guild.as_channel_id();

        let mut messages = vec![];

        if header.is_live {
            let conn = self
                .pg_pool
                .get()
                .context("failed to get a database connection from the pool")?;

            let game = header
                .current_game
                .map(|game| Game::find(game.id, &conn))
                .transpose()
                .context("failed to load the game")?;
            let show = header
                .current_show
                .map(|show| Show::find(show.id, &conn))
                .transpose()
                .context("failed to load the show")?;
            let game_entry =
                if let (Some(game), Some(show)) = (header.current_game, header.current_show) {
                    GameEntry::find(game.id, show.id, &conn)
                        .optional()
                        .context("failed to load the game entry")?
                } else {
                    None
                };

            match (game, show) {
                (Some(game), Some(show)) => {
                    messages.push(format!(
                        "Now live: {} on {}.",
                        game_entry
                            .and_then(|entry| entry.display_name)
                            .unwrap_or(game.name),
                        show.name
                    ));
                }
                (Some(game), None) => {
                    messages.push(format!(
                        "Now live: {}.",
                        game_entry
                            .and_then(|entry| entry.display_name)
                            .unwrap_or(game.name)
                    ));
                }
                (None, Some(show)) => {
                    messages.push(format!("Now live: {}.", show.name));
                }
                (None, None) => messages.push(String::from("Now live: something?")),
            }

            messages.push(await!(self.uptime_msg(&header.channel))?);
        } else {
            let now = Utc::now();

            let events = await!(self.calendar.get_upcoming_events(LRR, now))
                .context("failed to get the next scheduled stream")?;
            let events = Calendar::get_next_event(&events, now, false);

            let desertbus = await!(self.desertbus(now, &events));
            if !desertbus.is_empty() {
                messages.extend(desertbus);
            } else {
                messages.extend(events.iter().map(|event| {
                    format!(
                        "{}",
                        ShortDisplay {
                            event,
                            now,
                            tz: self.config.timezone
                        }
                    )
                }));
            }
        }

        if let Some(advice) = header.advice {
            messages.push(advice);
        }

        // TODO: shorten to a max of 1024 characters, whatever that means.
        general
            .edit(|c| c.topic(&messages.join(" ")))
            .map_err(SyncFailure::new)
            .context("failed to update the topic")?;

        Ok(())
    }

    async fn uptime_msg<'a>(&'a mut self, channel: &'a str) -> Result<String, Error> {
        Ok(await!(self.helix.get_stream(User::Login(channel)))
            .context("failed to get the stream")?
            .map(|stream| {
                format!(
                    "The stream has been live for {}",
                    HumanReadable::new(Utc::now() - stream.started_at.with_timezone(&Utc))
                )
            })
            .unwrap_or_else(|| String::from("The stream is not live.")))
    }

    async fn desertbus<'a>(&'a mut self, now: DateTime<Utc>, events: &'a [Event]) -> Vec<String> {
        let start = DesertBus::start_time().with_timezone(&Utc);
        let announce_start = start - chrono::Duration::days(2);
        let announce_end = start + chrono::Duration::days(7);
        let mut messages = vec![];

        if announce_start <= now && now <= announce_end {
            if let Some(next_event_start) = events.get(0).map(|event| event.start) {
                if next_event_start.with_timezone(&Utc) < start {
                    return messages;
                }
            }

            let money_raised = match await!(self.desertbus.money_raised()) {
                Ok(money_raised) => money_raised,
                Err(err) => {
                    error!("Failed to fetch the current Desert Bus total"; "error" => ?err);
                    messages.push(String::from("DESERT BUS?"));
                    return messages;
                }
            };
            let total_hours = DesertBus::hours_raised(money_raised) as i64;
            if now < start {
                messages.push(format!(
                    "{}",
                    ShortDisplay {
                        event: &Event {
                            start: start.with_timezone(&FixedOffset::east(0)),
                            summary: String::from("Desert Bus for Hope"),
                            end: start.with_timezone(&FixedOffset::east(0))
                                + chrono::Duration::hours(total_hours),
                            location: Some(String::from(
                                "https://desertbus.org/ or https://twitch.tv/desertbus"
                            )),
                            description: None,
                        },
                        now,
                        tz: self.config.timezone,
                    }
                ));
                messages.push(format!(
                    "${} raised.",
                    money_raised.separated_string_with_fixed_place(2)
                ));
            } else if now <= start + chrono::Duration::hours(total_hours) {
                messages.push(String::from("DESERT BUS!"));
                messages.push(format!(
                    "${} raised.",
                    money_raised.separated_string_with_fixed_place(2)
                ));
                messages.push(format!(
                    "{} of {}+ hours bussed.",
                    (now - start).num_hours(),
                    total_hours
                ));
            }
        }

        messages
    }
}
