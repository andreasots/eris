use crate::config::Config;
use crate::google::sheets::{CellData, ExtendedValue, Sheets, Spreadsheet};
use chrono::TimeZone;
use chrono::{DateTime, Utc};
use failure::{Error, ResultExt, SyncFailure};
use futures::compat::Stream01CompatExt;
use futures::TryStreamExt;
use slog::{slog_error, slog_info};
use slog_scope::{error, info};
use std::time::{Duration, Instant};
use tokio::timer::Interval;
use crate::context::ErisContext;
use crate::extract::Extract;

const SENT_KEY: &str = "lrrbot.sent";

pub async fn post_messages(ctx: ErisContext) {
    let spreadsheet_set = ctx.data.read()
        .extract::<Config>()
        .map(|config| config.contact_spreadsheet.is_some())
        .unwrap_or(false);
    if !spreadsheet_set {
        info!("Contact spreadsheet not set");
        return;
    };

    let mut timer = Interval::new(Instant::now(), Duration::from_secs(60)).compat();

    loop {
        match timer.try_next().await {
            Ok(Some(_)) => if let Err(err) = inner(&ctx).await {
                error!("Failed to post new messages"; "error" => ?err);
            },
            Ok(None) => break,
            Err(err) => error!("Timer error"; "error" => ?err),
        }
    }
}

#[derive(Debug)]
struct Entry<'a> {
    timestamp: DateTime<Utc>,
    message: &'a str,
    username: Option<&'a str>,
    row: u64,
}

fn extract_timestamp(cell: &CellData) -> Option<f64> {
    if let ExtendedValue::Number(days) = cell.effective_value.as_ref()? {
        // The timestamp is days since 1899-12-30. Apparently for compatibility with Lotus 1-2-3.
        Some((days - 25569.0) * 86400.0)
    } else {
        None
    }
}

fn extract_string(cell: &CellData) -> Option<&str> {
    if let ExtendedValue::String(string) = cell.effective_value.as_ref()? {
        Some(string)
    } else {
        None
    }
}

fn find_unsent_rows(spreadsheet: &Spreadsheet) -> Option<(u64, Vec<Entry>)> {
    let sheets = spreadsheet.sheets.as_ref()?;
    let sheet = sheets.get(0)?;
    let sheet_id = sheet.properties.as_ref()?.sheet_id?;

    let mut rows = vec![];

    for grid in sheet.data.as_ref()? {
        let start_row = grid.start_row.unwrap_or(0);

        let row_data = grid.row_data.as_ref()?.iter();
        let metadata = grid.row_metadata.as_ref()?.iter();
        'row: for (i, (row, meta)) in row_data.zip(metadata).enumerate() {
            let row_idx = start_row + i as u64;
            if row_idx == 0 {
                continue;
            }

            if let Some(meta) = meta.developer_metadata.as_ref() {
                for entry in meta {
                    if entry.key.as_ref().map(|s| s == SENT_KEY).unwrap_or(false) {
                        continue 'row;
                    }
                }
            }

            let values = row.values.as_ref();

            let timestamp = values
                .and_then(|row| row.get(0))
                .and_then(extract_timestamp);
            let message = values.and_then(|row| row.get(1)).and_then(extract_string);
            let username = values.and_then(|row| row.get(2)).and_then(extract_string);

            if let (Some(timestamp), Some(message)) = (timestamp, message) {
                rows.push(Entry {
                    timestamp: Utc.timestamp(timestamp as i64, (timestamp.fract() * 1e9) as u32),
                    message,
                    username,
                    row: row_idx,
                });
            }
        }
    }

    Some((sheet_id, rows))
}

async fn inner(ctx: &ErisContext) -> Result<(), Error> {
    let (spreadsheet_key, sheets, mods_channel) = {
        let data = ctx.data.read();

        let config = data.extract::<Config>()?;

        let spreadsheet_key = config.contact_spreadsheet.clone().ok_or(failure::err_msg("Contact spreadsheet is not set"))?;

        let sheets = data.extract::<Sheets>()?.clone();

        (spreadsheet_key, sheets, config.mods_channel)
    };

    let spreadsheet = sheets.get_spreadsheet(&spreadsheet_key, "properties.timeZone,sheets(properties.sheetId,data(startRow,startColumn,rowData.values.effectiveValue,rowMetadata.developerMetadata))")
        .await
        .context("failed to fetch the spreadsheet")?;

    let (sheet_id, unsent) = find_unsent_rows(&spreadsheet)
        .ok_or_else(|| failure::err_msg("no sheets or required information missing"))?;

    for message in unsent {
        mods_channel
            .send_message(ctx, |m| {
                m.content(format!("New message from the contact form:"))
                    .embed(|mut embed| {
                        embed = embed
                            .description(message.message)
                            .timestamp(message.timestamp.to_rfc3339());
                        if let Some(user) = message.username {
                            embed = embed.author(|e| e.name(user))
                        }
                        embed
                    })
            })
            .map_err(SyncFailure::new)
            .context("failed to forward the message")?;

        sheets.create_developer_metadata_for_row(
            &spreadsheet_key,
            sheet_id,
            message.row,
            SENT_KEY,
            "1"
        )
            .await
            .context("failed to set the message as sent")?;
    }

    Ok(())
}
