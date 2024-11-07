use std::sync::LazyLock;

use anyhow::{Context, Error};
use chrono::{DateTime, TimeZone, Utc};
use reqwest::Client;
use scraper::{Html, Selector};
use serde::Deserialize;

use crate::tz::Tz;

#[derive(Deserialize)]
struct HeaderProps {
    #[serde(rename = "currentEvent")]
    current_event: (f64, Event),
}

#[derive(Deserialize)]
struct Event {
    total: (f64, f64),
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

    pub fn start_time() -> DateTime<Utc> {
        static START_TIME: LazyLock<DateTime<Utc>> = LazyLock::new(|| {
            let tz =
                &Tz::from_name("America/Vancouver").expect("no timezone named `America/Vancouver`");
            tz.with_ymd_and_hms(2024, 11, 8, 15, 0, 0)
                .single()
                .expect("ambiguous timestamp")
                .with_timezone(&Utc)
        });

        *START_TIME
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
        static HEADER_SELECTOR: LazyLock<Selector> =
            LazyLock::new(|| Selector::parse("astro-island[component-export='Header']").unwrap());

        let html = self
            .client
            .get("https://desertbus.org/")
            .send()
            .await
            .context("failed to request the Desert Bus homepage")?
            .text()
            .await
            .context("failed to read the Desert Bus homepage")?;

        let html = Html::parse_document(&html);

        for element in html.select(&HEADER_SELECTOR) {
            let Some(props) = element.attr("props") else { continue };
            let props = serde_json::from_str::<HeaderProps>(props)
                .context("failed to parse header props")?;
            return Ok(props.current_event.1.total.1);
        }

        anyhow::bail!("failed to find the header component")
    }
}
