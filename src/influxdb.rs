use std::sync::Arc;

use anyhow::{Context, Error};
use influxdb_line_protocol::LineProtocolBuilder;
use reqwest::Client;
use url::Url;

#[derive(Clone)]
pub struct InfluxDb {
    http: Client,
    write_url: Arc<Url>,
}

impl InfluxDb {
    pub fn new(http: Client, url: &str, database: &str) -> Result<Self, Error> {
        let base_url = Url::parse(url).context("failed to parse the URL")?;
        let mut write_url =
            base_url.join("write").context("failed to construct the /write endpoint URL")?;
        write_url.query_pairs_mut().append_pair("db", database);
        Ok(Self { http, write_url: Arc::new(write_url) })
    }

    pub async fn write(&self, measurements: LineProtocolBuilder<Vec<u8>>) -> Result<(), Error> {
        let body = measurements.build();
        if !body.is_empty() {
            self.http
                .post((*self.write_url).clone())
                .body(body)
                .send()
                .await
                .context("failed to send the write request")?
                .error_for_status()
                .context("write request failed")?;
        }

        Ok(())
    }
}
