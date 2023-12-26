use anyhow::{Context, Error};
use chrono::{DateTime, Duration, Utc};
use google_calendar3::api::EventDateTime;
use google_calendar3::hyper::client::HttpConnector;
use google_calendar3::hyper_rustls::HttpsConnector;
use tracing::info;

use crate::tz::Tz;

pub const LRR: &str = "loadingreadyrun.com_72jmf1fn564cbbr84l048pv1go@group.calendar.google.com";
pub const FANSTREAMS: &str = "caffeinatedlemur@gmail.com";

pub type CalendarHub = google_calendar3::CalendarHub<HttpsConnector<HttpConnector>>;

pub struct Event {
    pub start: DateTime<Utc>,
    pub summary: String,
    pub end: DateTime<Utc>,
    pub location: Option<String>,
    pub description: Option<String>,
}

impl Event {
    fn from_api_event(event: google_calendar3::api::Event, timezone: &Tz) -> Result<Self, Error> {
        Ok(Self {
            start: parse_timestamp(&event.start.context("no event start time")?, timezone)
                .context("failed to parse the event start time")?,
            summary: event.summary.context("event summary missing")?,
            end: parse_timestamp(&event.end.context("no event end time")?, timezone)
                .context("failed to parse the event end time")?,
            location: event.location,
            description: event.description,
        })
    }
}

fn parse_timestamp(timestamp: &EventDateTime, timezone: &Tz) -> Result<DateTime<Utc>, Error> {
    if let Some(timestamp) = timestamp.date_time {
        Ok(timestamp)
    } else if let Some(date) = timestamp.date {
        Ok(date
            .and_hms_opt(0, 0, 0)
            .context("midnight is invalid?")?
            .and_local_timezone(timezone)
            .earliest()
            .ok_or_else(|| Error::msg("invalid timestamp: midnight doesn't exist in time zone"))?
            .with_timezone(&Utc))
    } else {
        Err(Error::msg("timestamp missing"))
    }
}

pub fn format_description(description: &str) -> String {
    let lines = description.lines().collect::<Vec<_>>();
    if lines.len() == 2 {
        // Show info: first line is the game, the second line is the description.
        let game = lines[0].trim();
        let description = lines[1].trim();

        if game == "-" {
            lines[1].into()
        } else if description.ends_with(|c: char| c.is_ascii_punctuation()) {
            format!("{description} Game: {game}")
        } else {
            format!("{description}. Game: {game}")
        }
    } else {
        lines.join(" / ")
    }
}

pub async fn get_next_event(
    client: &CalendarHub,
    calendar_id: &str,
    at: DateTime<Utc>,
    include_current: bool,
) -> Result<Vec<Event>, Error> {
    let (_, res) = client
        .events()
        .list(calendar_id)
        .max_results(10)
        .order_by("startTime")
        .single_events(true)
        .time_min(at)
        .doit()
        .await
        .context("failed to get the calendar events")?;

    let timezone = Tz::from_name(res.time_zone.as_deref().unwrap_or("America/Vancouver"))
        .context("calendar in an unknown timezone")?;

    let Some(events) = res.items else { return Ok(vec![]) };
    let events = events
        .into_iter()
        .filter_map(|event| match Event::from_api_event(event, &timezone) {
            Ok(event) => Some(event),
            Err(error) => {
                info!(?error, "failed to normalize the event");
                None
            }
        })
        .collect::<Vec<_>>();

    let mut first_future_event = None;

    for (i, event) in events.iter().enumerate() {
        let relevant_duration = std::cmp::min(event.end - event.start, Duration::hours(1));
        let relevant_until = event.start + relevant_duration;
        if relevant_until >= at {
            first_future_event = Some(i);
            break;
        }
    }

    let Some(first_future_event) = first_future_event else { return Ok(vec![]) };

    let current_events_end = events[first_future_event].start + Duration::hours(1);

    Ok(events
        .into_iter()
        .enumerate()
        .filter(|&(i, ref event)| {
            (i >= first_future_event || include_current) && event.start < current_events_end
        })
        .map(|(_, event)| event)
        .collect())
}
