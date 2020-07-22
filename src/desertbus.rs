use anyhow::{Context, Error};
use chrono::{DateTime, TimeZone};
use chrono_tz::America::Vancouver as TIMEZONE;
use chrono_tz::Tz;
use reqwest::Client;
use serde::Deserialize;

#[derive(Deserialize)]
struct Init {
    total: f64,
}

#[derive(Clone)]
pub struct DesertBus {
    client: Client,
}

impl DesertBus {
    pub const FIRST_HOUR: f64 = 1.00;
    pub const MULTIPLIER: f64 = 1.07;

    pub fn new(client: Client) -> DesertBus {
        DesertBus { client }
    }

    pub fn start_time() -> DateTime<Tz> {
        TIMEZONE.ymd(2020, 11, 13).and_hms(10, 0, 0)
    }

    pub fn hours_raised(money_raised: f64) -> f64 {
        // money_raised = FIRST_HOUR + FIRST_HOUR * MULTIPLIER + FIRST_HOUR * MULTIPLIER.pow(2.0) + ... + FIRST_HOUR * MULTIPLIER.pow(hours)
        // money_raised = FIRST_HOUR * (1.0 - MULTIPLIER.pow(hours)) / (1.0 - MULTIPLIER)
        // money_raised / FIRST_HOUR = (MULTIPLIER.pow(hours) - 1.0) / (MULTIPLIER - 1.0)
        // money_raised / FIRST_HOUR * (MULTIPLIER - 1.0) = MULTIPLIER.pow(hours) - 1.0
        // MULTIPLIER.pow(hours) = money_raised / FIRST_HOUR * (MULTIPLIER - 1.0) + 1.0
        // hours = (money_raised / FIRST_HOUR * (MULTIPLIER - 1.0) + 1.0).log(MULTIPLIER)

        (money_raised / DesertBus::FIRST_HOUR * (DesertBus::MULTIPLIER - 1.0) + 1.0)
            .log(DesertBus::MULTIPLIER)
            .floor()
    }

    pub async fn money_raised(&self) -> Result<f64, Error> {
        Ok(self
            .client
            .get("https://desertbus.org/wapi/init")
            .send()
            .await
            .context("failed to get the current Desert Bus total")?
            .json::<Init>()
            .await
            .context("failed to parse the current Desert Bus total")?
            .total)
    }
}
