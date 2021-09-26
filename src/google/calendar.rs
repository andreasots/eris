use crate::google::ServiceAccount;
use anyhow::{Context, Error};
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, TimeZone};
use chrono_tz::Tz;
use reqwest::header::AUTHORIZATION;
use reqwest::Client;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::cmp;
use std::path::PathBuf;
use std::sync::Arc;

pub const LRR: &str = "loadingreadyrun.com_72jmf1fn564cbbr84l048pv1go@group.calendar.google.com";
pub const FANSTREAMS: &str = "caffeinatedlemur@gmail.com";
const SCOPES: &[&str] = &["https://www.googleapis.com/auth/calendar.events.readonly"];

#[derive(Debug)]
pub struct Event {
    pub start: DateTime<FixedOffset>,
    pub summary: String,
    pub end: DateTime<FixedOffset>,
    pub location: Option<String>,
    pub description: Option<String>,
}

impl Event {
    fn from_api_event(event: ApiEvent, timezone: Tz) -> Option<Self> {
        Some(Self {
            start: event.start.resolve_datetime(timezone)?,
            summary: event.summary,
            end: event.end.resolve_datetime(timezone)?,
            location: event.location,
            description: event.description,
        })
    }
}

#[derive(Serialize)]
#[serde(bound = "")]
struct ListEventsRequest<'a, Tz: TimeZone> {
    #[serde(rename = "maxResults")]
    max_results: usize,
    #[serde(rename = "orderBy")]
    order_by: &'a str,
    #[serde(rename = "singleEvents")]
    single_events: bool,
    #[serde(rename = "timeMin")]
    time_min: DateTime<Tz>,
}

#[derive(Deserialize)]
struct ListEventsResponse {
    items: Vec<ApiEvent>,
    #[serde(rename = "timeZone")]
    timezone: Tz,
}

#[derive(Deserialize)]
struct ApiEvent {
    pub start: Time,
    pub summary: String,
    pub end: Time,
    pub location: Option<String>,
    pub description: Option<String>,
}

#[derive(Deserialize)]
pub struct Time {
    #[serde(rename = "dateTime")]
    date_time: Option<DateTime<FixedOffset>>,
    date: Option<NaiveDate>,
    #[serde(rename = "timeZone")]
    timezone: Option<Tz>,
}

impl Time {
    fn resolve_datetime(&self, calendar_tz: Tz) -> Option<DateTime<FixedOffset>> {
        if let Some(datetime) = self.date_time {
            return Some(datetime);
        } else if let Some(date) = self.date {
            self.timezone
                .unwrap_or(calendar_tz)
                .from_local_date(&date)
                .and_hms_opt(0, 0, 0)
                .earliest()
                .map(|datetime| datetime.with_timezone(&FixedOffset::east(0)))
        } else {
            None
        }
    }
}

#[derive(Clone)]
pub struct Calendar {
    client: Client,
    oauth2: Arc<ServiceAccount>,
}

impl Calendar {
    pub fn new<P: Into<PathBuf>>(client: Client, key_file_path: P) -> Calendar {
        Calendar {
            oauth2: Arc::new(ServiceAccount::new(key_file_path.into(), client.clone(), SCOPES)),
            client,
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
                    .map_err(|()| Error::msg("https URL is cannot-be-a-base?"))?;
                path_segments.push(calendar);
                path_segments.push("events");
            }

            url
        };

        let token = self
            .oauth2
            .get_token()
            .await
            .context("failed to get a service account OAuth2 token")?;

        let res = self
            .client
            .get(url)
            .header(AUTHORIZATION, token)
            .query(&ListEventsRequest {
                max_results: 10,
                order_by: "startTime",
                single_events: true,
                time_min: after,
            })
            .send()
            .await
            .context("failed to get calendar events")?
            .error_for_status()
            .context("request failed")?
            .json::<ListEventsResponse>()
            .await
            .context("failed to parse calendar events")?;
        let timezone = res.timezone;
        Ok(res.items.into_iter().flat_map(|event| Event::from_api_event(event, timezone)).collect())
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
