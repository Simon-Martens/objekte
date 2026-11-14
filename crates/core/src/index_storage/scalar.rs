//! Borrowed typed-key encoding and comparison.
//!
//! The dictionary stores encoded keys. Ordering is defined by decoding the
//! original logical value, never by comparing variant tags or raw numeric
//! bytes. This preserves the engine's existing `Number`, date, datetime, and
//! path semantics.

use std::cmp::Ordering;

use time::{Date, OffsetDateTime};

use crate::decimal::ExactDecimal;
use crate::model::Number;
use crate::posting::ScalarKey;

use super::error::{StorageError, StorageResult};


/// Key ordering family stored in a dictionary header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum KeyOrder {
    /// Raw byte strings compared lexically.
    Bytes = 0,
    /// Encoded [`ScalarKey`] values compared by their logical ordering.
    Scalar = 1,
    /// Little-endian `u64` values compared numerically.
    Unsigned = 2,
    /// Reversible query-path strings compared lexically as raw bytes.
    Paths = 3,
}


impl KeyOrder {
    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> StorageResult<Self> {
        Ok(match code {
            0 => Self::Bytes,
            1 => Self::Scalar,
            2 => Self::Unsigned,
            3 => Self::Paths,
            other => {
                return Err(StorageError::UnsupportedFormat(format!(
                    "unknown dictionary key order {other}"
                )));
            }
        })
    }

    /// Compare two encoded keys of this order.
    pub fn compare(self, left: &[u8], right: &[u8]) -> StorageResult<Ordering> {
        match self {
            Self::Bytes | Self::Paths => Ok(left.cmp(right)),
            Self::Scalar => compare_scalar_keys(left, right),
            Self::Unsigned => {
                if left.len() != 8 || right.len() != 8 {
                    return Err(StorageError::corrupt(
                        "unsigned dictionary key must be eight bytes",
                    ));
                }
                let left = u64::from_le_bytes(left.try_into().expect("eight-byte checked key"));
                let right = u64::from_le_bytes(right.try_into().expect("eight-byte checked key"));
                Ok(left.cmp(&right))
            }
        }
    }
}


const TAG_STRING: u8 = 0;
const TAG_ID: u8 = 1;
const TAG_NUMBER: u8 = 2;
const TAG_BOOL: u8 = 3;
const TAG_DATE: u8 = 4;
const TAG_DATETIME: u8 = 5;
const TAG_UNIXTIME: u8 = 6;

const NUMBER_INTEGER: u8 = 0;
const NUMBER_FLOAT: u8 = 1;
const NUMBER_DECIMAL: u8 = 2;

/// Encode one typed scalar key. The encoding is canonical and stable for one
/// logical value; dictionary construction sorts by [`compare_scalar_keys`].
pub fn encode_scalar_key(key: &ScalarKey) -> StorageResult<Vec<u8>> {
    let mut bytes = Vec::with_capacity(32);
    match key {
        ScalarKey::String(value) => {
            bytes.push(TAG_STRING);
            push_bytes(&mut bytes, value.as_bytes())?;
        }
        ScalarKey::Id(value) => {
            bytes.push(TAG_ID);
            push_bytes(&mut bytes, value.as_bytes())?;
        }
        ScalarKey::Number(value) => {
            bytes.push(TAG_NUMBER);
            encode_number(&mut bytes, value);
        }
        ScalarKey::Bool(value) => {
            bytes.push(TAG_BOOL);
            bytes.push(u8::from(*value));
        }
        ScalarKey::Date(value) => {
            bytes.push(TAG_DATE);
            bytes.extend_from_slice(&value.to_julian_day().to_le_bytes());
        }
        ScalarKey::Datetime(value) => {
            bytes.push(TAG_DATETIME);
            bytes.extend_from_slice(&value.unix_timestamp_nanos().to_le_bytes());
        }
        ScalarKey::Unixtime(value) => {
            bytes.push(TAG_UNIXTIME);
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    Ok(bytes)
}

/// Decode one typed scalar key, rejecting truncated or invalid encodings.
pub fn decode_scalar_key(bytes: &[u8]) -> StorageResult<ScalarKey> {
    let (&tag, payload) = bytes
        .split_first()
        .ok_or_else(|| StorageError::corrupt("empty scalar key"))?;
    Ok(match tag {
        TAG_STRING => ScalarKey::String(decode_string(payload)?.into_owned()),
        TAG_ID => ScalarKey::Id(decode_string(payload)?.into_owned()),
        TAG_NUMBER => ScalarKey::Number(decode_number(payload)?),
        TAG_BOOL => match payload {
            [0] => ScalarKey::Bool(false),
            [1] => ScalarKey::Bool(true),
            _ => return Err(StorageError::corrupt("invalid boolean scalar key")),
        },
        TAG_DATE => {
            if payload.len() != 4 {
                return Err(StorageError::corrupt("invalid date scalar key length"));
            }
            let day = i32::from_le_bytes(payload.try_into().expect("four-byte checked date"));
            let date = Date::from_julian_day(day)
                .map_err(|_| StorageError::corrupt("date scalar key is out of range"))?;
            ScalarKey::Date(date)
        }
        TAG_DATETIME => {
            if payload.len() != 16 {
                return Err(StorageError::corrupt("invalid datetime scalar key length"));
            }
            let nanos =
                i128::from_le_bytes(payload.try_into().expect("sixteen-byte checked datetime"));
            let value = OffsetDateTime::from_unix_timestamp_nanos(nanos)
                .map_err(|_| StorageError::corrupt("datetime scalar key is out of range"))?;
            ScalarKey::Datetime(value)
        }
        TAG_UNIXTIME => {
            if payload.len() != 8 {
                return Err(StorageError::corrupt("invalid unixtime scalar key length"));
            }
            ScalarKey::Unixtime(i64::from_le_bytes(
                payload.try_into().expect("eight-byte checked unixtime"),
            ))
        }
        other => {
            return Err(StorageError::UnsupportedFormat(format!(
                "unknown scalar key tag {other}"
            )));
        }
    })
}

/// Compare two encoded scalar keys using the original logical ordering.
pub fn compare_scalar_keys(left: &[u8], right: &[u8]) -> StorageResult<Ordering> {
    let left = decode_scalar_key(left)?;
    let right = decode_scalar_key(right)?;
    Ok(match (&left, &right) {
        (ScalarKey::Number(a), ScalarKey::Number(b)) => a.cmp(b),
        (ScalarKey::Datetime(a), ScalarKey::Datetime(b)) => {
            a.unix_timestamp_nanos().cmp(&b.unix_timestamp_nanos())
        }
        _ => left.cmp(&right),
    })
}

/// Encode one scalar key into a caller-owned buffer.
pub fn append_scalar_key(destination: &mut Vec<u8>, key: &ScalarKey) -> StorageResult<()> {
    destination.extend_from_slice(&encode_scalar_key(key)?);
    Ok(())
}

/// Encode an unsigned dictionary key.
pub fn encode_unsigned(value: u64) -> [u8; 8] {
    value.to_le_bytes()
}

fn encode_number(bytes: &mut Vec<u8>, value: &Number) {
    match value {
        Number::Integer(integer) => {
            bytes.push(NUMBER_INTEGER);
            bytes.extend_from_slice(&integer.to_le_bytes());
        }
        Number::Float(float) => {
            bytes.push(NUMBER_FLOAT);
            bytes.extend_from_slice(&float.to_bits().to_le_bytes());
        }
        Number::ExactDecimal(decimal) => {
            bytes.push(NUMBER_DECIMAL);
            bytes.push(u8::from(decimal.negative));
            bytes.extend_from_slice(&decimal.exponent.to_le_bytes());
            push_bytes(bytes, decimal.digits.as_bytes()).expect("decimal digits fit u32");
        }
    }
}

fn decode_number(bytes: &[u8]) -> StorageResult<Number> {
    let (&form, payload) = bytes
        .split_first()
        .ok_or_else(|| StorageError::corrupt("empty number encoding"))?;
    match form {
        NUMBER_INTEGER => {
            if payload.len() != 16 {
                return Err(StorageError::corrupt("invalid integer length"));
            }
            Ok(Number::Integer(i128::from_le_bytes(
                payload.try_into().expect("sixteen-byte checked integer"),
            )))
        }
        NUMBER_FLOAT => {
            if payload.len() != 8 {
                return Err(StorageError::corrupt("invalid float length"));
            }
            let bits = u64::from_le_bytes(payload.try_into().expect("eight-byte checked float"));
            let float = f64::from_bits(bits);
            if !float.is_finite() {
                return Err(StorageError::corrupt("non-finite persisted number"));
            }
            Ok(Number::Float(float))
        }
        NUMBER_DECIMAL => {
            let (&negative, rest) = payload
                .split_first()
                .ok_or_else(|| StorageError::corrupt("truncated decimal sign"))?;
            if negative > 1 || rest.len() < 12 {
                return Err(StorageError::corrupt("invalid decimal header"));
            }
            let exponent = i64::from_le_bytes(rest[..8].try_into().expect("eight-byte exponent"));
            let digits_len =
                u32::from_le_bytes(rest[8..12].try_into().expect("four-byte digit length"))
                    as usize;
            let digits = rest
                .get(12..)
                .filter(|digits| digits.len() == digits_len)
                .ok_or_else(|| StorageError::corrupt("truncated decimal digits"))?;
            if digits.is_empty()
                || !digits.iter().all(u8::is_ascii_digit)
                || (digits[0] == b'0' && digits != b"0")
                || (digits.len() > 1 && digits.ends_with(b"0"))
            {
                return Err(StorageError::corrupt("invalid decimal digits"));
            }
            let digits = std::str::from_utf8(digits)
                .map_err(|_| StorageError::corrupt("decimal digits are not ASCII"))?;
            let token = format!(
                "{}{}e{}",
                if negative != 0 { "-" } else { "" },
                digits,
                exponent
            );
            let decimal = ExactDecimal::parse(&token)
                .map_err(|error| StorageError::corrupt(format!("invalid decimal: {error}")))?;
            Ok(Number::ExactDecimal(decimal))
        }
        other => Err(StorageError::UnsupportedFormat(format!(
            "unknown number encoding {other}"
        ))),
    }
}

fn push_bytes(bytes: &mut Vec<u8>, value: &[u8]) -> StorageResult<()> {
    let length = u32::try_from(value.len())
        .map_err(|_| StorageError::ResourceLimit("scalar key exceeds 4 GiB".into()))?;
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(value);
    Ok(())
}

fn decode_string(bytes: &[u8]) -> StorageResult<std::borrow::Cow<'_, str>> {
    if bytes.len() < 4 {
        return Err(StorageError::corrupt("truncated string key length"));
    }
    let length = u32::from_le_bytes(bytes[..4].try_into().expect("four-byte key length")) as usize;
    let value = bytes
        .get(4..)
        .filter(|value| value.len() == length)
        .ok_or_else(|| StorageError::corrupt("truncated string key"))?;
    std::str::from_utf8(value)
        .map(std::borrow::Cow::Borrowed)
        .map_err(|_| StorageError::corrupt("string key is not UTF-8"))
}
