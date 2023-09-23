use std::fmt::{Display, Formatter, Result as FmtResult};

use anyhow::{Context, Error};
use google_calendar3::chrono::{DateTime, TimeZone};
use time::{Duration, OffsetDateTime};

pub struct HumanReadable(Duration);

impl HumanReadable {
    pub fn new(duration: Duration) -> HumanReadable {
        HumanReadable(duration)
    }
}

impl Display for HumanReadable {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        let mut d = self.0;
        let mut started = false;

        if d < Duration::ZERO {
            d = -d;
            f.write_str("-")?;
        }

        if d.whole_days() > 0 {
            write!(f, "{}d", d.whole_days())?;
            d = d - Duration::days(d.whole_days());
            started = true;
        }

        if started || d.whole_hours() > 0 {
            write!(f, "{}h", d.whole_hours())?;
            d = d - Duration::hours(d.whole_hours());
            started = true;
        }

        if started || d.whole_minutes() > 0 {
            write!(f, "{}m", d.whole_minutes())?;
            d = d - Duration::minutes(d.whole_minutes());
            started = true;
        }

        // skip seconds if longer than a minute
        if !started {
            write!(f, "{}s", d.whole_seconds())?;
        }

        Ok(())
    }
}

pub fn chrono_to_time(timestamp: DateTime<impl TimeZone>) -> Result<OffsetDateTime, Error> {
    let ts = OffsetDateTime::from_unix_timestamp(timestamp.timestamp())
        .context("failed to convert timestamp to `OffsetDateTime`")?;
    Ok(ts + Duration::nanoseconds(timestamp.timestamp_subsec_nanos() as i64))
}
