use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Error};
use google_sheets4::api::{
    BatchUpdateSpreadsheetRequest, CellData, CreateDeveloperMetadataRequest, DeveloperMetadata,
    DeveloperMetadataLocation, DimensionRange, Request, Spreadsheet,
};
use google_sheets4::hyper::client::HttpConnector;
use google_sheets4::hyper_rustls::HttpsConnector;
use google_sheets4::Sheets;
use time::PrimitiveDateTime;
use time_tz::{PrimitiveDateTimeExt, Tz};
use tokio::sync::watch::Receiver;
use tracing::{error, info};
use twilight_http::Client as DiscordClient;
use twilight_model::util::Timestamp;
use twilight_util::builder::embed::{EmbedAuthorBuilder, EmbedBuilder, EmbedFooterBuilder};
use twilight_validate::embed::{AUTHOR_NAME_LENGTH, DESCRIPTION_LENGTH};

use crate::config::Config;
use crate::shorten::{shorten, split_to_parts};

const SENT_KEY: &str = "lrrbot.sent";
const EPOCH: PrimitiveDateTime = time::macros::datetime!(1899-12-30 00:00:00);

pub async fn post_messages(
    mut running: Receiver<bool>,
    config: Arc<Config>,
    discord: Arc<DiscordClient>,
    sheets: Sheets<HttpsConnector<HttpConnector>>,
) {
    if config.contact_spreadsheet.is_none() {
        info!("Contact spreadsheet not set");
        return;
    };

    let mut timer = tokio::time::interval(Duration::from_secs(60));

    loop {
        tokio::select! {
            _ = running.changed() => break,
            _ = timer.tick() => {
                if let Err(error) = inner(&config, &discord, &sheets).await {
                    error!(?error, "Failed to post new messages");
                }
            },
        }
    }
}

#[derive(Debug)]
struct Entry<'a> {
    timestamp: Option<Timestamp>,
    message: &'a str,
    username: Option<&'a str>,
    row: i32,
}

fn extract_timestamp(cell: &CellData, tz: &Tz) -> Option<Timestamp> {
    // The timestamp is in days since 1899-12-30. Apparently for compatibility with Lotus 1-2-3.
    let offset = Duration::from_secs_f64(cell.effective_value.as_ref()?.number_value? * 86400.0);
    let timestamp = EPOCH + offset;
    let timestamp = timestamp.assume_timezone(tz).take_first().unwrap_or_else(|| timestamp.assume_utc());
    Timestamp::from_micros((timestamp.unix_timestamp_nanos() / 1_000) as i64).ok()
}

fn extract_string(cell: &CellData) -> Option<&str> {
    cell.effective_value.as_ref()?.string_value.as_deref()
}

fn find_unsent_rows(spreadsheet: &Spreadsheet) -> Option<(i32, Vec<Entry>)> {
    let tz = spreadsheet
        .properties
        .as_ref()
        .and_then(|prop| prop.time_zone.as_deref())
        .and_then(time_tz::timezones::get_by_name)
        .unwrap_or(time_tz::timezones::db::UTC);
    let sheets = spreadsheet.sheets.as_ref()?;
    let sheet = sheets.get(0)?;
    let sheet_id = sheet.properties.as_ref()?.sheet_id?;

    let mut rows = vec![];

    for grid in sheet.data.as_ref()? {
        let start_row = grid.start_row.unwrap_or(0);

        let row_data = grid.row_data.as_ref()?.iter();
        let metadata = grid.row_metadata.as_ref()?.iter();
        'row: for (i, (row, meta)) in row_data.zip(metadata).enumerate() {
            let row_idx = start_row + i as i32;
            if row_idx == 0 {
                continue;
            }

            if let Some(meta) = meta.developer_metadata.as_ref() {
                for entry in meta {
                    if entry.metadata_key.as_ref().map(|s| s == SENT_KEY).unwrap_or(false) {
                        continue 'row;
                    }
                }
            }

            let values = row.values.as_ref();

            let timestamp =
                values.and_then(|row| row.get(0)).and_then(|cell| extract_timestamp(cell, tz));
            let message = values.and_then(|row| row.get(1)).and_then(extract_string);
            let username = values.and_then(|row| row.get(2)).and_then(extract_string);

            if let Some(message) = message {
                rows.push(Entry { timestamp, message, username, row: row_idx });
            }
        }
    }

    Some((sheet_id, rows))
}

async fn inner(
    config: &Config,
    discord: &DiscordClient,
    sheets: &Sheets<HttpsConnector<HttpConnector>>,
) -> Result<(), Error> {
    let spreadsheet_id = config
        .contact_spreadsheet
        .as_deref()
        .ok_or_else(|| Error::msg("Contact spreadsheet is not set"))?;

    let (_, spreadsheet) = sheets
        .spreadsheets()
        .get(spreadsheet_id)
        .param("fields", "properties.timeZone,sheets(properties.sheetId,data(startRow,startColumn,rowData.values.effectiveValue,rowMetadata.developerMetadata))")
        .doit()
        .await
        .context("failed to fetch the spreadsheet")?;

    let (sheet_id, unsent) = find_unsent_rows(&spreadsheet)
        .ok_or_else(|| Error::msg("no sheets or required information missing"))?;

    for message in unsent {
        let parts = split_to_parts(message.message, DESCRIPTION_LENGTH);
        let num_parts = parts.len();
        for (i, part) in parts.into_iter().enumerate() {
            let mut req = discord.create_message(config.mods_channel);
            if i == 0 {
                req =
                    req.content("New message from the contact form:").context("invalid message")?;
            }
            let mut embed = EmbedBuilder::new()
                .description(part)
                .footer(EmbedFooterBuilder::new(format!("{}/{}", i + 1, num_parts)));
            if let Some(username) = message.username {
                embed =
                    embed.author(EmbedAuthorBuilder::new(shorten(username, AUTHOR_NAME_LENGTH)));
            }
            if let Some(timestamp) = message.timestamp {
                embed = embed.timestamp(timestamp);
            }
            req.embeds(&[embed.build()])
                .context("invalid embed")?
                .await
                .context("failed to forward the message")?;
        }

        let req = BatchUpdateSpreadsheetRequest {
            include_spreadsheet_in_response: Some(false),
            requests: Some(vec![Request {
                create_developer_metadata: Some(CreateDeveloperMetadataRequest {
                    developer_metadata: Some(DeveloperMetadata {
                        location: Some(DeveloperMetadataLocation {
                            dimension_range: Some(DimensionRange {
                                sheet_id: Some(sheet_id),
                                dimension: Some("ROWS".to_string()),
                                start_index: Some(message.row),
                                end_index: Some(message.row + 1),
                            }),
                            ..DeveloperMetadataLocation::default()
                        }),
                        metadata_key: Some(SENT_KEY.to_string()),
                        metadata_value: Some("1".to_string()),
                        visibility: Some("DOCUMENT".to_string()),
                        ..DeveloperMetadata::default()
                    }),
                }),
                ..Request::default()
            }]),
            ..BatchUpdateSpreadsheetRequest::default()
        };
        sheets
            .spreadsheets()
            .batch_update(req, spreadsheet_id)
            .doit()
            .await
            .context("failed to set the message as sent")?;
    }

    Ok(())
}
