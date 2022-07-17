use crate::google::ServiceAccount;
use anyhow::{Context, Error};
use reqwest::header::AUTHORIZATION;
use reqwest::Client;
use reqwest::Url;
use serde::de::Unexpected;
use serde::de::Visitor;
use serde::Deserializer;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cmp;
use std::path::PathBuf;
use std::sync::Arc;
use time::macros::format_description;
use time::Date;
use time::Duration;
use time::OffsetDateTime;
use time_tz::PrimitiveDateTimeExt;
use time_tz::Tz;

pub const LRR: &str = "loadingreadyrun.com_72jmf1fn564cbbr84l048pv1go@group.calendar.google.com";
pub const FANSTREAMS: &str = "caffeinatedlemur@gmail.com";
const SCOPES: &[&str] = &["https://www.googleapis.com/auth/calendar.events.readonly"];

#[derive(Debug)]
pub struct Event {
    pub start: OffsetDateTime,
    pub summary: String,
    pub end: OffsetDateTime,
    pub location: Option<String>,
    pub description: Option<String>,
}

impl Event {
    fn from_api_event(event: ApiEvent, timezone: &Tz) -> Option<Self> {
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
struct ListEventsRequest<'a> {
    #[serde(rename = "maxResults")]
    max_results: usize,
    #[serde(rename = "orderBy")]
    order_by: &'a str,
    #[serde(rename = "singleEvents")]
    single_events: bool,
    #[serde(rename = "timeMin", with = "time::serde::rfc3339")]
    time_min: OffsetDateTime,
}

#[derive(Deserialize)]
struct ListEventsResponse {
    items: Vec<ApiEvent>,
    #[serde(rename = "timeZone", deserialize_with = "deserialize_timezone")]
    timezone: &'static Tz,
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
    #[serde(default, rename = "dateTime", with = "time::serde::rfc3339::option")]
    date_time: Option<OffsetDateTime>,
    #[serde(default, deserialize_with = "deserialize_option_date")]
    date: Option<Date>,
    #[serde(rename = "timeZone", deserialize_with = "deserialize_optional_timezone")]
    timezone: Option<&'static Tz>,
}

struct Timezone(&'static Tz);

impl<'de> Deserialize<'de> for Timezone {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct TzVisitor;

        impl<'de> Visitor<'de> for TzVisitor {
            type Value = &'static Tz;

            fn expecting(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(fmt, "an IANA timezone name")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                time_tz::timezones::get_by_name(value).ok_or_else(|| {
                    E::invalid_value(Unexpected::Str(value), &"an IANA timezone name")
                })
            }
        }

        deserializer.deserialize_any(TzVisitor).map(Self)
    }
}

fn deserialize_timezone<'de, D>(deserializer: D) -> Result<&'static Tz, D::Error>
where
    D: Deserializer<'de>,
{
    Timezone::deserialize(deserializer).map(|Timezone(tz)| tz)
}

fn deserialize_optional_timezone<'de, D>(deserializer: D) -> Result<Option<&'static Tz>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<Timezone>::deserialize(deserializer).map(|opt| opt.map(|Timezone(tz)| tz))
}

fn deserialize_option_date<'de, D>(deserializer: D) -> Result<Option<Date>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;

    match Option::<Cow<'de, str>>::deserialize(deserializer)? {
        Some(date) => match Date::parse(&date, format_description!("[year]-[month]-[day]")) {
            Ok(date) => Ok(Some(date)),
            Err(err) => Err(D::Error::custom(err)),
        },
        None => Ok(None),
    }
}

impl Time {
    fn resolve_datetime(&self, calendar_tz: &Tz) -> Option<OffsetDateTime> {
        if let Some(datetime) = self.date_time {
            Some(datetime)
        } else if let Some(date) = self.date {
            date.with_hms(0, 0, 0)
                .unwrap()
                .assume_timezone(self.timezone.unwrap_or(calendar_tz))
                .take_first()
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

    pub async fn get_upcoming_events<'a>(
        &'a self,
        calendar: &'a str,
        after: OffsetDateTime,
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

    pub fn get_next_event(events: &[Event], at: OffsetDateTime, include_current: bool) -> &[Event] {
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
