use failure::{Error, ResultExt};
use reqwest::{Body, Client};
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Write;
use std::marker::PhantomData;
use std::sync::Arc;
use url::Url;

pub enum New {}
pub enum Complete {}

pub enum Timestamp {
    Now,
}

pub enum Value<'a> {
    Float(f64),
    Integer(i64),
    String(Cow<'a, str>),
    Boolean(bool),
}

impl From<f64> for Value<'_> {
    fn from(x: f64) -> Self {
        Value::Float(x)
    }
}

impl From<i64> for Value<'_> {
    fn from(x: i64) -> Self {
        Value::Integer(x)
    }
}

// Can't implement `From<impl Into<Cow<'a, str>>` because that overlaps with the other impls.
impl<'a> From<&'a str> for Value<'a> {
    fn from(x: &'a str) -> Self {
        Value::String(x.into())
    }
}

impl From<String> for Value<'_> {
    fn from(x: String) -> Self {
        Value::String(x.into())
    }
}

impl From<bool> for Value<'_> {
    fn from(x: bool) -> Self {
        Value::Boolean(x)
    }
}

pub struct Measurement<'a, State> {
    measurement: Cow<'a, str>,
    tags: HashMap<Cow<'a, str>, Cow<'a, str>>,
    fields: HashMap<Cow<'a, str>, Value<'a>>,
    timestamp: Timestamp,
    _marker: PhantomData<State>,
}

impl<'a> Measurement<'a, New> {
    pub fn new<T: Into<Cow<'a, str>>>(measurement: T, timestamp: Timestamp) -> Self {
        Measurement {
            measurement: measurement.into(),
            tags: HashMap::new(),
            fields: HashMap::new(),
            timestamp,
            _marker: PhantomData,
        }
    }
}

impl<'a, State> Measurement<'a, State> {
    pub fn add_tag<K: Into<Cow<'a, str>>, V: Into<Cow<'a, str>>>(
        mut self,
        tag: K,
        value: V,
    ) -> Self {
        let value = value.into();
        if !value.is_empty() {
            self.tags.insert(tag.into(), value);
        }
        self
    }

    pub fn add_field<K: Into<Cow<'a, str>>, V: Into<Value<'a>>>(
        mut self,
        tag: K,
        value: V,
    ) -> Measurement<'a, Complete> {
        self.fields.insert(tag.into(), value.into());

        Measurement {
            measurement: self.measurement,
            tags: self.tags,
            fields: self.fields,
            timestamp: self.timestamp,
            _marker: PhantomData,
        }
    }
}

impl Measurement<'_, Complete> {
    fn append_escaped<F: Fn(u8) -> bool>(dst: &mut Vec<u8>, s: &str, is_special: F) {
        for b in s.as_bytes() {
            if is_special(*b) {
                dst.push(b'\\');
            }
            dst.push(*b);
        }
    }

    fn is_special_for_measurement(b: u8) -> bool {
        b == b',' || b == b' ' || b == b'\n'
    }

    fn is_special_for_tag_keys_tag_values_and_field_keys(b: u8) -> bool {
        b == b',' || b == b'=' || b == b' ' || b == b'\n'
    }

    fn is_special_for_string_field_value(b: u8) -> bool {
        b == b'"' || b == b'\\'
    }

    fn serialize(&self, buf: &mut Vec<u8>) {
        Self::append_escaped(buf, &self.measurement, Self::is_special_for_measurement);
        if !self.tags.is_empty() {
            for (key, value) in self.tags.iter() {
                buf.push(b',');
                Self::append_escaped(
                    buf,
                    key,
                    Self::is_special_for_tag_keys_tag_values_and_field_keys,
                );
                buf.push(b'=');
                Self::append_escaped(
                    buf,
                    value.trim_end_matches('\\'),
                    Self::is_special_for_tag_keys_tag_values_and_field_keys,
                );
            }
        }
        buf.push(b' ');
        for (i, (key, value)) in self.fields.iter().enumerate() {
            if i != 0 {
                buf.push(b',');
            }
            Self::append_escaped(
                buf,
                key,
                Self::is_special_for_tag_keys_tag_values_and_field_keys,
            );
            buf.push(b'=');
            match value {
                Value::Float(x) => {
                    write!(buf, "{}", x).expect("failed to write a `f64` to the buffer")
                }
                Value::Integer(x) => {
                    write!(buf, "{}", x).expect("failed to write a `i64` to the buffer")
                }
                Value::String(x) => {
                    buf.push(b'"');
                    Self::append_escaped(buf, x, Self::is_special_for_string_field_value);
                    buf.push(b'"');
                }
                Value::Boolean(x) => buf.push(if *x { b't' } else { b'f' }),
            }
        }
        match self.timestamp {
            Timestamp::Now => (),
        }
    }
}

struct WriteRequest<'a>(&'a [Measurement<'a, Complete>]);

impl From<WriteRequest<'_>> for Body {
    fn from(measurements: WriteRequest<'_>) -> Self {
        let mut buffer = vec![];
        for measurement in measurements.0 {
            measurement.serialize(&mut buffer);
            buffer.push(b'\n');
        }
        buffer.into()
    }
}

#[derive(Deserialize)]
struct InfluxError {
    error: String,
}

#[derive(Clone)]
pub struct InfluxDB {
    client: Client,
    url: Arc<Url>,
}

impl InfluxDB {
    pub fn new(client: Client, url: Url) -> InfluxDB {
        InfluxDB {
            client,
            url: Arc::new(url),
        }
    }

    pub async fn write(&self, req: &[Measurement<'_, Complete>]) -> Result<(), Error> {
        if req.is_empty() {
            return Ok(());
        }

        let res = self
            .client
            .post(self.url.as_str())
            .body(WriteRequest(req))
            .send()
            .await
            .context("failed to send the request")?;
        // For some reason if you `match` on `res.status()` a `&Response` gets saved in the generator, making it not `Sync`.
        let status = res.status();
        match status {
            status if status.is_success() => Ok(()),
            status if status.is_client_error() || status.is_server_error() => {
                let error = res
                    .json::<InfluxError>()
                    .await
                    .context("failed to read the response")?;
                Err(failure::err_msg(error.error)
                    .context("server returned an error")
                    .into())
            }
            status => {
                let body = res
                    .bytes()
                    .await
                    .context("failed to read the response")?;
                unimplemented!(
                    "status code {} {}, response: {:?}",
                    status.as_str(),
                    status.canonical_reason().unwrap_or(""),
                    &body[..]
                )
            }
        }
    }
}
