use chrono::Duration;
use std::fmt::{Display, Formatter, Result as FmtResult};

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

        if d < Duration::zero() {
            d = -d;
            f.write_str("-")?;
        }

        if d.num_days() > 0 {
            write!(f, "{}d", d.num_days())?;
            d = d - Duration::days(d.num_days());
            started = true;
        }

        if started || d.num_hours() > 0 {
            write!(f, "{}h", d.num_hours())?;
            d = d - Duration::hours(d.num_hours());
            started = true;
        }

        if started || d.num_minutes() > 0 {
            write!(f, "{}m", d.num_minutes())?;
            d = d - Duration::minutes(d.num_minutes());
            started = true;
        }

        // skip seconds if longer than a minute
        if !started {
            write!(f, "{}s", d.num_seconds())?;
        }

        Ok(())
    }
}
