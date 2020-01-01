use crate::config::Config;
use chrono::Duration;
use chrono::{DateTime, FixedOffset, TimeZone};
use failure::{Error, ResultExt};
use reqwest::Client;
use reqwest::Url;
use serde::{Deserialize, Deserializer, Serialize};
use std::cmp;

pub const LRR: &str = "loadingreadyrun.com_72jmf1fn564cbbr84l048pv1go@group.calendar.google.com";
pub const FANSTREAMS: &str = "caffeinatedlemur@gmail.com";

#[derive(Debug, Deserialize)]
pub struct Event {
    #[serde(deserialize_with = "deserialize_nested_datetime")]
    pub start: DateTime<FixedOffset>,
    pub summary: String,
    #[serde(deserialize_with = "deserialize_nested_datetime")]
    pub end: DateTime<FixedOffset>,
    pub location: Option<String>,
    pub description: Option<String>,
}

#[allow(non_snake_case)]
#[derive(Serialize)]
#[serde(bound = "")]
struct ListEventsRequest<'a, Tz: TimeZone> {
    maxResults: usize,
    orderBy: &'a str,
    singleEvents: bool,
    timeMin: DateTime<Tz>,
    key: &'a str,
}

#[derive(Deserialize)]
struct ListEventsResponse {
    items: Vec<Event>,
}

fn deserialize_nested_datetime<'de, D>(deserializer: D) -> Result<DateTime<FixedOffset>, D::Error>
where
    D: Deserializer<'de>,
{
    #[allow(non_snake_case)]
    #[derive(Deserialize)]
    struct Nested {
        dateTime: DateTime<FixedOffset>,
    }

    Ok(Nested::deserialize(deserializer)?.dateTime)
}

#[derive(Clone)]
pub struct Calendar {
    client: Client,
    key: String,
}

impl Calendar {
    pub fn new(client: Client, config: &Config) -> Calendar {
        Calendar {
            client,
            key: config.google_key.clone(),
        }
    }

    pub async fn get_upcoming_events<'a, Tz: TimeZone + 'a>(
        &'a self,
        calendar: &'a str,
        after: DateTime<Tz>,
    ) -> Result<Vec<Event>, Error> {
        let url = {
            let mut url = Url::parse("https://www.googleapis.com/calendar/v3/calendars")
                .context("failed to parse the base URL")?;
            {
                let mut path_segments = url
                    .path_segments_mut()
                    .map_err(|()| failure::err_msg("https URL is cannot-be-a-base?"))?;
                path_segments.push(calendar);
                path_segments.push("events");
            }

            url
        };

        Ok(self
            .client
            .get(url)
            .query(&ListEventsRequest {
                maxResults: 10,
                orderBy: "startTime",
                singleEvents: true,
                timeMin: after,
                key: &self.key,
            })
            .send()
            .await
            .context("failed to get calendar events")?
            .error_for_status()
            .context("request failed")?
            .json::<ListEventsResponse>()
            .await
            .context("failed to parse calendar events")?
            .items)
    }

    pub fn get_next_event<Tz: TimeZone>(
        events: &[Event],
        at: DateTime<Tz>,
        include_current: bool,
    ) -> &[Event] {
        let at = at.with_timezone(&FixedOffset::west(0));
        let mut first_future_event = None;

        for (i, event) in events.iter().enumerate() {
            let relevant_duration = cmp::min(event.end - event.start, Duration::hours(1));
            let relevant_until = event.start + relevant_duration;
            if relevant_until >= at {
                first_future_event = Some(i);
                break;
            }
        }

        let first_future_event = match first_future_event {
            Some(i) => i,
            None => return &[],
        };

        let current_events_end = events[first_future_event].start + Duration::hours(1);

        let mut start = events.len();
        let mut end = 0;
        for (i, event) in events.iter().enumerate() {
            if (i >= first_future_event || include_current) && event.start < current_events_end {
                start = cmp::min(start, i);
                end = cmp::max(end, i);
            }
        }

        &events[start..=end]
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
                format!("{} Game: {}", description, game)
            } else {
                format!("{}. Game: {}", description, game)
            }
        } else {
            lines.join("; ")
        }
    }
}
