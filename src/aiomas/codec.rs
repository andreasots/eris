use bytes::{Buf, BufMut, BytesMut};
use serde::de::{Error as DeserializationError, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{self, Value};
use std::collections::HashMap;
use std::fmt::{Formatter, Result as FmtResult};
use std::io::{Error, ErrorKind};
use std::u32;
use tokio_util::codec::{Decoder, Encoder};

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

fn encode_frame<T: Serialize>(frame: &Frame<T>, dst: &mut BytesMut) -> Result<(), Error> {
    let request = serde_json::to_vec(frame)?;
    if request.len() > u32::MAX as usize {
        return Err(Error::new(ErrorKind::Other, "request larger than u32::MAX"));
    }
    dst.put_u32(request.len() as u32);
    dst.put_slice(&request);
    Ok(())
}

fn decode_frame<T: for<'de> Deserialize<'de>>(
    src: &mut BytesMut,
) -> Result<Option<Frame<T>>, Error> {
    if src.len() < 4 {
        return Ok(None);
    }
    let len = u32::from_be_bytes([src[0], src[1], src[2], src[3]]) as usize;
    if src.len() < 4 + len {
        return Ok(None);
    }
    src.advance(4);
    let data = src.split_to(len);
    Ok(Some(serde_json::from_slice::<Frame<T>>(&data)?))
}

pub type Request = (String, Vec<Value>, HashMap<String, Value>);
pub type Exception = String;

pub struct ClientCodec;

impl Encoder for ClientCodec {
    type Item = (u64, Request);
    type Error = Error;
    fn encode(
        &mut self,
        (request_id, payload): Self::Item,
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        encode_frame(&(FrameType::Request, request_id, payload), dst)
    }
}

impl Decoder for ClientCodec {
    type Item = (u64, Result<Value, Exception>);
    type Error = Error;
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match decode_frame(src)? {
            Some((FrameType::Result, request_id, payload)) => Ok(Some((request_id, Ok(payload)))),
            Some((FrameType::Exception, request_id, payload)) => {
                Ok(Some((request_id, Err(serde_json::from_value(payload)?))))
            }
            Some((ty, _, _)) => {
                Err(Error::new(ErrorKind::Other, format!("response type {:?} invalid", ty)))
            }
            None => Ok(None),
        }
    }
}

pub struct ServerCodec;

impl Encoder for ServerCodec {
    type Item = (u64, Result<Value, Exception>);
    type Error = Error;
    fn encode(
        &mut self,
        (request_id, payload): Self::Item,
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        let ty = if payload.is_ok() { FrameType::Result } else { FrameType::Exception };
        encode_frame(&(ty, request_id, payload.unwrap_or_else(Value::String)), dst)
    }
}

impl Decoder for ServerCodec {
    type Item = (u64, Request);
    type Error = Error;
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match decode_frame(src)? {
            Some((FrameType::Request, request_id, payload)) => Ok(Some((request_id, payload))),
            Some((ty, _, _)) => {
                Err(Error::new(ErrorKind::Other, format!("request type {:?} invalid", ty)))
            }
            None => Ok(None),
        }
    }
}
