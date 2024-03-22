use std::fmt::{Display, Formatter, Result as FmtResult};

use chrono::TimeDelta;

pub struct HumanReadable(TimeDelta);

impl HumanReadable {
    pub fn new(duration: TimeDelta) -> HumanReadable {
        HumanReadable(duration)
    }
}

impl Display for HumanReadable {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        let mut d = self.0;
        let mut started = false;

        if d < TimeDelta::zero() {
            d = -d;
            f.write_str("-")?;
        }

        if d.num_days() > 0 {
            write!(f, "{}d", d.num_days())?;
            d = d - TimeDelta::try_days(d.num_days()).expect("invalid number of days");
            started = true;
        }

        if started || d.num_hours() > 0 {
            write!(f, "{}h", d.num_hours())?;
            d = d - TimeDelta::try_hours(d.num_hours()).expect("invalid number of hours");
            started = true;
        }

        if started || d.num_minutes() > 0 {
            write!(f, "{}m", d.num_minutes())?;
            d = d - TimeDelta::try_minutes(d.num_minutes()).expect("invalid number of minutes");
            started = true;
        }

        // skip seconds if longer than a minute
        if !started {
            write!(f, "{}s", d.num_seconds())?;
        }

        Ok(())
    }
}
