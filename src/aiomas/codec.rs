use std::collections::HashMap;
use std::fmt::{Formatter, Result as FmtResult};

use anyhow::Error;
use bytes::{Bytes, BytesMut};
use futures::{Sink, SinkExt, Stream, TryStreamExt};
use serde::de::{Error as DeserializationError, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{self, Value};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::LengthDelimitedCodec;

#[derive(Copy, Clone, Debug)]
enum FrameType {
    Request = 0,
    Result = 1,
    Exception = 2,
}

impl Serialize for FrameType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(*self as u64)
    }
}

impl<'de> Deserialize<'de> for FrameType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<FrameType, D::Error> {
        struct FrameTypeVisitor;

        impl<'de> Visitor<'de> for FrameTypeVisitor {
            type Value = FrameType;

            fn expecting(&self, f: &mut Formatter) -> FmtResult {
                f.write_str("a positive integer")
            }

            fn visit_u64<E: DeserializationError>(self, value: u64) -> Result<FrameType, E> {
                match value {
                    0 => Ok(FrameType::Request),
                    1 => Ok(FrameType::Result),
                    2 => Ok(FrameType::Exception),
                    n => Err(DeserializationError::custom(format!("unknown frame type {}", n))),
                }
            }
        }

        deserializer.deserialize_u64(FrameTypeVisitor)
    }
}

type Frame<T> = (FrameType, u64, T);

pub type Request = (String, Vec<Value>, HashMap<String, Value>);
pub type Exception = String;

async fn encode_request((request_id, payload): (u64, Request)) -> Result<Bytes, Error> {
    Ok(serde_json::to_vec(&(FrameType::Request, request_id, payload))?.into())
}

async fn decode_response(buf: BytesMut) -> Result<(u64, Result<Value, Exception>), Error> {
    match serde_json::from_slice::<Frame<Value>>(&buf)? {
        (FrameType::Result, request_id, payload) => Ok((request_id, Ok(payload))),
        (FrameType::Exception, request_id, payload) => {
            Ok((request_id, Err(serde_json::from_value(payload)?)))
        }
        (ty, _, _) => anyhow::bail!("response type {:?} invalid", ty),
    }
}

async fn encode_response(
    (request_id, payload): (u64, Result<Value, Exception>),
) -> Result<Bytes, Error> {
    Ok(serde_json::to_vec(&(
        if payload.is_ok() { FrameType::Result } else { FrameType::Exception },
        request_id,
        payload.unwrap_or_else(Value::String),
    ))?
    .into())
}

async fn decode_request(buf: BytesMut) -> Result<(u64, Request), Error> {
    match serde_json::from_slice::<Frame<Request>>(&buf)? {
        (FrameType::Request, request_id, payload) => Ok((request_id, payload)),
        (ty, _, _) => anyhow::bail!("request type {:?} invalid", ty),
    }
}

pub fn client<T: AsyncRead + AsyncWrite>(
    io: T,
) -> impl Stream<Item = Result<(u64, Result<Value, Exception>), Error>>
       + Sink<(u64, Request), Error = Error> {
    LengthDelimitedCodec::builder()
        .big_endian()
        .length_field_length(4)
        .new_framed(io)
        .err_into()
        .and_then(decode_response)
        .with(encode_request)
}

pub fn server<T: AsyncRead + AsyncWrite>(
    io: T,
) -> impl Stream<Item = Result<(u64, Request), Error>>
       + Sink<(u64, Result<Value, Exception>), Error = Error> {
    LengthDelimitedCodec::builder()
        .big_endian()
        .length_field_length(4)
        .new_framed(io)
        .err_into()
        .and_then(decode_request)
        .with(encode_response)
}
