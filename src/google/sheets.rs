use crate::google::ServiceAccount;
use anyhow::{Context, Error};
use reqwest::header::AUTHORIZATION;
use reqwest::Client;
use reqwest::Url;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

const SCOPES: &[&str] = &["https://www.googleapis.com/auth/spreadsheets"];

/// https://developers.google.com/sheets/api/reference/rest/v4/spreadsheets#Spreadsheet
#[derive(Deserialize, Debug)]
pub struct Spreadsheet {
    pub properties: Option<SpreadsheetProperties>,
    pub sheets: Option<Vec<Sheet>>,
}

/// https://developers.google.com/sheets/api/reference/rest/v4/spreadsheets/sheets#Sheet
#[derive(Deserialize, Debug)]
pub struct Sheet {
    pub properties: Option<SheetProperties>,
    pub data: Option<Vec<GridData>>,
}

/// https://developers.google.com/sheets/api/reference/rest/v4/spreadsheets/sheets#SheetProperties
#[derive(Deserialize, Debug)]
pub struct SheetProperties {
    #[serde(rename = "sheetId")]
    pub sheet_id: Option<u64>,
}

/// https://developers.google.com/sheets/api/reference/rest/v4/spreadsheets#SpreadsheetProperties
#[derive(Deserialize, Debug)]
pub struct SpreadsheetProperties {
    #[serde(rename = "timeZone")]
    pub timezone: Option<String>,
}

/// https://developers.google.com/sheets/api/reference/rest/v4/spreadsheets/sheets#GridData
#[derive(Deserialize, Debug)]
pub struct GridData {
    #[serde(rename = "startRow")]
    pub start_row: Option<u64>,
    #[serde(rename = "startColumn")]
    pub start_column: Option<u64>,
    #[serde(rename = "rowData")]
    pub row_data: Option<Vec<RowData>>,
    #[serde(rename = "rowMetadata")]
    pub row_metadata: Option<Vec<DimensionProperties>>,
}

/// https://developers.google.com/sheets/api/reference/rest/v4/spreadsheets/sheets#DimensionProperties
#[derive(Deserialize, Debug)]
pub struct DimensionProperties {
    #[serde(rename = "developerMetadata")]
    pub developer_metadata: Option<Vec<DeveloperMetadata>>,
}

/// https://developers.google.com/sheets/api/reference/rest/v4/spreadsheets.developerMetadata#DeveloperMetadata
#[derive(Deserialize, Debug)]
pub struct DeveloperMetadata {
    #[serde(rename = "metadataId")]
    pub id: Option<u64>,
    #[serde(rename = "metadataKey")]
    pub key: Option<String>,
    #[serde(rename = "metadataValue")]
    pub value: Option<String>,
    // FIXME: this also exists but the internal enum looks like a giant pain to add.
    //pub location: Option<DeveloperMetadataLocation>,
    pub visibility: Option<DeveloperMetadataVisibility>,
}

/// https://developers.google.com/sheets/api/reference/rest/v4/spreadsheets.developerMetadata#DeveloperMetadata.DeveloperMetadataVisibility
#[derive(Deserialize, Debug)]
pub enum DeveloperMetadataVisibility {
    #[serde(rename = "DEVELOPER_METADATA_VISIBILITY_UNSPECIFIED")]
    Unspecified,
    #[serde(rename = "DOCUMENT")]
    Document,
    #[serde(rename = "PROJECT")]
    Project,
}

/// https://developers.google.com/sheets/api/reference/rest/v4/spreadsheets/sheets#RowData
#[derive(Deserialize, Debug)]
pub struct RowData {
    pub values: Option<Vec<CellData>>,
}

/// https://developers.google.com/sheets/api/reference/rest/v4/spreadsheets/cells#CellData
#[derive(Deserialize, Debug)]
pub struct CellData {
    #[serde(rename = "effectiveValue")]
    pub effective_value: Option<ExtendedValue>,
}

/// https://developers.google.com/sheets/api/reference/rest/v4/spreadsheets/other#ExtendedValue
#[derive(Deserialize, Debug)]
pub enum ExtendedValue {
    #[serde(rename = "numberValue")]
    Number(f64),
    #[serde(rename = "stringValue")]
    String(String),
    #[serde(rename = "boolValue")]
    Bool(bool),
    #[serde(rename = "formulaValue")]
    Formula(String),
    /// https://developers.google.com/sheets/api/reference/rest/v4/spreadsheets/other#ErrorValue
    #[serde(rename = "errorValue")]
    Error {
        #[serde(rename = "type")]
        type_: Option<ErrorType>,
        message: Option<String>,
    },
}

/// https://developers.google.com/sheets/api/reference/rest/v4/spreadsheets/other#ErrorType
#[derive(Deserialize, Debug)]
pub enum ErrorType {
    #[serde(rename = "ERROR_TYPE_UNSPECIFIED")]
    Unspecified,
    #[serde(rename = "ERROR")]
    Error,
    #[serde(rename = "NULL_VALUE")]
    NullValue,
    #[serde(rename = "DIVIDE_BY_ZERO")]
    DivideByZero,
    #[serde(rename = "VALUE")]
    Value,
    #[serde(rename = "REF")]
    Ref,
    #[serde(rename = "NAME")]
    Name,
    #[serde(rename = "NUM")]
    Num,
    #[serde(rename = "N_A")]
    NA,
    #[serde(rename = "LOADING")]
    Loading,
}

#[derive(Clone)]
pub struct Sheets {
    client: Client,
    oauth2: Arc<ServiceAccount>,
}

impl Sheets {
    pub fn new<P: Into<PathBuf>>(client: Client, key_file_path: P) -> Sheets {
        Sheets {
            oauth2: Arc::new(ServiceAccount::new(key_file_path.into(), client.clone(), SCOPES)),
            client,
        }
    }

    async fn get_token(&self) -> Result<String, Error> {
        let mut token = self.oauth2.get_token().await?;
        token.insert_str(0, "Bearer ");
        Ok(token)
    }

    pub async fn get_spreadsheet<'a>(
        &'a self,
        spreadsheet: &'a str,
        fields: &'a str,
    ) -> Result<Spreadsheet, Error> {
        let token =
            self.get_token().await.context("failed to get a service account OAuth2 token")?;

        let url = {
            let mut url = Url::parse("https://sheets.googleapis.com/v4/spreadsheets")
                .context("failed to parse the base URL")?;
            {
                let mut path_segments = url
                    .path_segments_mut()
                    .map_err(|()| Error::msg("https URL is cannot-be-a-base?"))?;
                path_segments.push(spreadsheet);
            }
            url
        };

        Ok(self
            .client
            .get(url)
            .header(AUTHORIZATION, token)
            .query(&[("fields", fields)])
            .send()
            .await
            .context("failed to send the request")?
            .error_for_status()
            .context("request failed")?
            .json::<Spreadsheet>()
            .await
            .context("failed to read the response")?)
    }

    pub async fn create_developer_metadata_for_row<'a>(
        &'a self,
        spreadsheet: &'a str,
        sheet_id: u64,
        row: u64,
        key: &'a str,
        value: &'a str,
    ) -> Result<(), Error> {
        let token =
            self.get_token().await.context("failed to get a service account OAuth2 token")?;

        let url = {
            let mut url = Url::parse("https://sheets.googleapis.com/v4/spreadsheets")
                .context("failed to parse the base URL")?;
            {
                let mut path_segments = url
                    .path_segments_mut()
                    .map_err(|()| Error::msg("https URL is cannot-be-a-base?"))?;
                let mut segment = String::from(spreadsheet);
                segment.push_str(":batchUpdate");
                path_segments.push(&segment);
            }
            url
        };

        self.client
            .post(url)
            .header(AUTHORIZATION, token)
            .json(&json!({
                "requests": [
                    {
                        "createDeveloperMetadata": {
                            "developerMetadata": {
                                "metadataKey": key,
                                "metadataValue": value,
                                "location": {
                                    "dimensionRange": {
                                        "sheetId": sheet_id,
                                        "dimension": "ROWS",
                                        "startIndex": row,
                                        "endIndex": row + 1,
                                    }
                                },
                                "visibility": "DOCUMENT"
                            }
                        }
                    }
                ],
                "includeSpreadsheetInResponse": false,
            }))
            .send()
            .await
            .context("failed to send the request")?
            .error_for_status()
            .context("request failed")?;

        Ok(())
    }
}
