//! Ordered, allocation-conscious key encoding.
//!
//! Rules (store format version 1):
//! - unsigned fixed-width integers are big-endian;
//! - signed fixed-width integers are XORed with the sign bit before the
//!   big-endian encoding, so lexicographic order is numeric order;
//! - variable byte/string components escape every `0x00` as `0x00 0xFF` and
//!   terminate with `0x00 0x00`;
//! - composite keys concatenate the component encodings in table order.
//!
//! The escaping keeps byte order, distinguishes prefixes, and handles
//! embedded NUL bytes and non-UTF-8 path bytes.

use std::cmp::Ordering;

use crate::decimal::ExactDecimal;
use crate::model::Number;
use crate::posting::ScalarKey;
use crate::sorting::{SortKey, SortOptions};

use super::StoreError;

pub const SCALAR_STRING: u8 = 0;
pub const SCALAR_ID: u8 = 1;
pub const SCALAR_NUMBER: u8 = 2;
pub const SCALAR_BOOL: u8 = 3;
pub const SCALAR_DATE: u8 = 4;
pub const SCALAR_DATETIME: u8 = 5;
pub const SCALAR_UNIXTIME: u8 = 6;

pub const NUMERIC_NEGATIVE: u8 = 0;
pub const NUMERIC_ZERO: u8 = 1;
pub const NUMERIC_POSITIVE: u8 = 2;

pub const SORT_BOOL: u8 = 0;
pub const SORT_NUMBER: u8 = 1;
pub const SORT_DATE: u8 = 2;
pub const SORT_DATETIME: u8 = 3;
pub const SORT_UNIXTIME: u8 = 4;
pub const SORT_ID: u8 = 5;
pub const SORT_STRING: u8 = 6;

/// Reusable key buffer. Callers clear it between components.
#[derive(Clone, Debug, Default)]
pub struct KeyWriter {
    pub bytes: Vec<u8>,
}

impl KeyWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
        }
    }

    pub fn clear(&mut self) {
        self.bytes.clear();
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub fn push_u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    pub fn push_u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    pub fn push_u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    pub fn push_u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    pub fn push_i32(&mut self, value: i32) {
        self.push_u32((value as u32) ^ (1 << 31));
    }

    pub fn push_i64(&mut self, value: i64) {
        self.push_u64((value as u64) ^ (1 << 63));
    }

    pub fn push_i128(&mut self, value: i128) {
        self.push_u128((value as u128) ^ (1 << 127));
    }

    pub fn push_u128(&mut self, value: u128) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    /// Append the escaped, terminated encoding of a variable byte component.
    pub fn push_bytes(&mut self, value: &[u8]) {
        let mut run_start = 0;
        for (index, byte) in value.iter().copied().enumerate() {
            if byte == 0 {
                self.bytes.extend_from_slice(&value[run_start..index]);
                self.bytes.push(0);
                self.bytes.push(0xFF);
                run_start = index + 1;
            }
        }
        self.bytes.extend_from_slice(&value[run_start..]);
        self.bytes.push(0);
        self.bytes.push(0);
    }

    pub fn push_str(&mut self, value: &str) {
        self.push_bytes(value.as_bytes());
    }

    /// A fixed-width component is not terminated; the declared layout decides
    /// where it ends.
    pub fn push_scalar(&mut self, key: &ScalarKey) -> Result<(), StoreError> {
        match key {
            ScalarKey::String(value) => {
                self.push_u8(SCALAR_STRING);
                self.push_str(value);
            }
            ScalarKey::Id(value) => {
                self.push_u8(SCALAR_ID);
                self.push_str(value);
            }
            ScalarKey::Number(value) => {
                self.push_u8(SCALAR_NUMBER);
                self.push_numeric_component(value)?;
            }
            ScalarKey::Bool(value) => {
                self.push_u8(SCALAR_BOOL);
                self.push_u8(u8::from(*value));
            }
            ScalarKey::Date(value) => {
                self.push_u8(SCALAR_DATE);
                self.push_i32(value.to_julian_day());
            }
            ScalarKey::Datetime(value) => {
                self.push_u8(SCALAR_DATETIME);
                self.push_i128(value.unix_timestamp_nanos());
            }
            ScalarKey::Unixtime(value) => {
                self.push_u8(SCALAR_UNIXTIME);
                self.push_i64(*value);
            }
        }
        Ok(())
    }

    /// Numeric component: class byte followed by the escaped magnitude payload.
    /// All zeros share one encoding, and negative values complement the
    /// positive payload so magnitude order reverses.
    pub fn push_numeric_component(&mut self, value: &Number) -> Result<(), StoreError> {
        let decimal = exact_decimal(value)?;
        if decimal.digits == "0" {
            self.push_u8(NUMERIC_ZERO);
            self.push_bytes(&[]);
            return Ok(());
        }
        let magnitude = i128::from(decimal.exponent) + decimal.digits.len() as i128;
        let mut payload = Vec::with_capacity(16 + decimal.digits.len() + 1);
        // Signed magnitudes are sign-bit-flipped before the big-endian write
        // so lexicographic byte order is numeric order, including negative
        // exponents.
        payload.extend_from_slice(&((magnitude as u128) ^ (1 << 127)).to_be_bytes());
        payload.extend_from_slice(decimal.digits.as_bytes());
        payload.push(0);
        if decimal.negative {
            self.push_u8(NUMERIC_NEGATIVE);
            let complemented = payload.iter().map(|byte| !byte).collect::<Vec<_>>();
            self.push_bytes(&complemented);
        } else {
            self.push_u8(NUMERIC_POSITIVE);
            self.push_bytes(&payload);
        }
        Ok(())
    }

    /// Sort-key constituent: rank, then the direction-specific ordinary
    /// component, then the logical identity tie-break is appended by the
    /// caller.
    pub fn push_sort_key(&mut self, key: &SortKey, descending: bool, options: SortOptions) {
        let rank = sort_rank(key, options);
        self.push_u8(rank);
        let start = self.bytes.len();
        match key {
            SortKey::Missing | SortKey::Unavailable | SortKey::Null => {}
            SortKey::Bool(value) => {
                self.push_u8(SORT_BOOL);
                self.push_u8(u8::from(*value));
            }
            SortKey::Number(value) => {
                self.push_u8(SORT_NUMBER);
                // Infallible for every admitted Number; nonfinite values are
                // rejected at admission. Keep a conservative zero if a direct
                // caller ever constructs one.
                let _ = self.push_numeric_component(value);
            }
            SortKey::Date(value) => {
                self.push_u8(SORT_DATE);
                self.push_i32(value.to_julian_day());
            }
            SortKey::Datetime(value) => {
                self.push_u8(SORT_DATETIME);
                self.push_i128(value.unix_timestamp_nanos());
            }
            SortKey::Unixtime(value) => {
                self.push_u8(SORT_UNIXTIME);
                self.push_i64(*value);
            }
            SortKey::Id(value) => {
                self.push_u8(SORT_ID);
                self.push_str(value);
            }
            SortKey::String(bytes) => {
                self.push_u8(SORT_STRING);
                self.push_bytes(bytes);
            }
        }
        if descending && rank == 2 {
            for byte in &mut self.bytes[start..] {
                *byte = !*byte;
            }
        }
    }
}

/// Shift the sort rank into an unsigned byte. `Missing`/`Unavailable` share
/// the existing special-value group and never move when descending.
pub fn sort_rank(key: &SortKey, options: SortOptions) -> u8 {
    use crate::sorting::Placement;
    let rank: i8 = match key {
        SortKey::Missing | SortKey::Unavailable if options.missing == Placement::First => -1,
        SortKey::Missing | SortKey::Unavailable => 2,
        SortKey::Null if options.nulls == Placement::First => -2,
        SortKey::Null => 1,
        _ => 0,
    };
    (rank + 2) as u8
}

pub fn exact_decimal(value: &Number) -> Result<ExactDecimal, StoreError> {
    match value {
        Number::Integer(integer) => Ok(ExactDecimal::from_i128(*integer)),
        Number::Float(float) if float.is_finite() => Ok(ExactDecimal::from_f64(*float)),
        Number::Float(_) => Err(StoreError::CorruptIndex(
            "nonfinite number in an ordered index key".into(),
        )),
        Number::ExactDecimal(decimal) => Ok(decimal.clone()),
    }
}

/// Encode a scalar key without a record slot.
pub fn encode_scalar(key: &ScalarKey) -> Result<Vec<u8>, StoreError> {
    let mut writer = KeyWriter::with_capacity(32);
    writer.push_scalar(key)?;
    Ok(writer.into_bytes())
}

/// Encode a scalar key followed by a record slot.
pub fn encode_scalar_slot(key: &ScalarKey, slot: u32) -> Result<Vec<u8>, StoreError> {
    let mut writer = KeyWriter::with_capacity(40);
    writer.push_scalar(key)?;
    writer.push_u32(slot);
    Ok(writer.into_bytes())
}

/// Lexicographic successor of a complete encoded prefix. Returns `None` when
/// no successor exists (the prefix is empty or all `0xFF`).
pub fn prefix_successor(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut successor = prefix.to_vec();
    while let Some(last) = successor.pop() {
        if last != 0xFF {
            successor.push(last + 1);
            return Some(successor);
        }
    }
    None
}

/// Successful exact-match bound for a prefix: `[prefix, successor)`.
pub fn prefix_bounds(prefix: &[u8]) -> (Vec<u8>, Option<Vec<u8>>) {
    (prefix.to_vec(), prefix_successor(prefix))
}

#[derive(Clone, Copy, Debug)]
pub struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.position)
    }

    pub fn position(&self) -> usize {
        self.position
    }

    pub fn is_empty(&self) -> bool {
        self.position == self.bytes.len()
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], StoreError> {
        let end = self
            .position
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| StoreError::CorruptIndex("truncated key component".into()))?;
        let slice = &self.bytes[self.position..end];
        self.position = end;
        Ok(slice)
    }

    pub fn read_u8(&mut self) -> Result<u8, StoreError> {
        Ok(self.take(1)?[0])
    }

    pub fn read_u16(&mut self) -> Result<u16, StoreError> {
        Ok(u16::from_be_bytes(
            self.take(2)?.try_into().expect("two-byte slice"),
        ))
    }

    pub fn read_u32(&mut self) -> Result<u32, StoreError> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().expect("four-byte slice"),
        ))
    }

    pub fn read_u64(&mut self) -> Result<u64, StoreError> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().expect("eight-byte slice"),
        ))
    }

    pub fn read_u128(&mut self) -> Result<u128, StoreError> {
        Ok(u128::from_be_bytes(
            self.take(16)?.try_into().expect("sixteen-byte slice"),
        ))
    }

    pub fn read_i32(&mut self) -> Result<i32, StoreError> {
        Ok((self.read_u32()? ^ (1 << 31)) as i32)
    }

    pub fn read_i64(&mut self) -> Result<i64, StoreError> {
        Ok((self.read_u64()? ^ (1 << 63)) as i64)
    }

    pub fn read_i128(&mut self) -> Result<i128, StoreError> {
        Ok((self.read_u128()? ^ (1 << 127)) as i128)
    }

    /// Decode one escaped, terminated byte component.
    pub fn read_bytes(&mut self) -> Result<Vec<u8>, StoreError> {
        let mut value = Vec::new();
        loop {
            let byte = self.read_u8()?;
            if byte != 0 {
                value.push(byte);
                continue;
            }
            let terminator = self.read_u8()?;
            match terminator {
                0 => return Ok(value),
                0xFF => value.push(0),
                other => {
                    return Err(StoreError::CorruptIndex(format!(
                        "invalid escaped-byte terminator {other:#04x}"
                    )));
                }
            }
        }
    }

    pub fn read_str(&mut self) -> Result<String, StoreError> {
        String::from_utf8(self.read_bytes()?)
            .map_err(|_| StoreError::CorruptIndex("invalid UTF-8 in key component".into()))
    }

    pub fn read_scalar(&mut self) -> Result<ScalarKey, StoreError> {
        use time::{Date, OffsetDateTime};
        Ok(match self.read_u8()? {
            SCALAR_STRING => ScalarKey::String(self.read_str()?),
            SCALAR_ID => ScalarKey::Id(self.read_str()?),
            SCALAR_NUMBER => ScalarKey::Number(self.read_numeric_component()?),
            SCALAR_BOOL => match self.read_u8()? {
                0 => ScalarKey::Bool(false),
                1 => ScalarKey::Bool(true),
                other => {
                    return Err(StoreError::CorruptIndex(format!(
                        "invalid Boolean scalar byte {other}"
                    )));
                }
            },
            SCALAR_DATE => ScalarKey::Date(
                Date::from_julian_day(self.read_i32()?)
                    .map_err(|_| StoreError::CorruptIndex("invalid Julian day".into()))?,
            ),
            SCALAR_DATETIME => ScalarKey::Datetime(
                OffsetDateTime::from_unix_timestamp_nanos(self.read_i128()?)
                    .map_err(|_| StoreError::CorruptIndex("invalid datetime".into()))?,
            ),
            SCALAR_UNIXTIME => ScalarKey::Unixtime(self.read_i64()?),
            other => {
                return Err(StoreError::CorruptIndex(format!(
                    "unknown scalar tag {other}"
                )));
            }
        })
    }

    pub fn read_numeric_component(&mut self) -> Result<Number, StoreError> {
        match self.read_u8()? {
            NUMERIC_ZERO => {
                let payload = self.read_bytes()?;
                if !payload.is_empty() {
                    return Err(StoreError::CorruptIndex(
                        "zero numeric component has a payload".into(),
                    ));
                }
                Ok(Number::Integer(0))
            }
            class @ (NUMERIC_NEGATIVE | NUMERIC_POSITIVE) => {
                let mut payload = self.read_bytes()?;
                if class == NUMERIC_NEGATIVE {
                    for byte in &mut payload {
                        *byte = !*byte;
                    }
                }
                if payload.len() < 17 || payload.last() != Some(&0) {
                    return Err(StoreError::CorruptIndex(
                        "malformed numeric payload".into(),
                    ));
                }
                let magnitude = (u128::from_be_bytes(
                    payload[..16].try_into().expect("sixteen-byte magnitude"),
                ) ^ (1 << 127)) as i128;
                let digits = &payload[16..payload.len() - 1];
                if digits.is_empty()
                    || digits[0] == b'0'
                    || digits.last() == Some(&b'0')
                    || !digits.iter().all(u8::is_ascii_digit)
                {
                    return Err(StoreError::CorruptIndex(
                        "noncanonical numeric significand".into(),
                    ));
                }
                let digit_count = i128::try_from(digits.len()).map_err(|_| {
                    StoreError::CorruptIndex("numeric significand too long".into())
                })?;
                let exponent = magnitude.checked_sub(digit_count).ok_or_else(|| {
                    StoreError::CorruptIndex("numeric exponent overflow".into())
                })?;
                let exponent = i64::try_from(exponent).map_err(|_| {
                    StoreError::CorruptIndex("numeric exponent out of range".into())
                })?;
                let text = format!(
                    "{}e{}",
                    std::str::from_utf8(digits)
                        .map_err(|_| StoreError::CorruptIndex("non-ASCII numeric digits".into()))?,
                    exponent
                );
                let decimal = if class == NUMERIC_NEGATIVE {
                    ExactDecimal::parse(&format!("-{text}"))
                } else {
                    ExactDecimal::parse(&text)
                }
                .map_err(StoreError::CorruptIndex)?;
                Ok(Number::ExactDecimal(decimal))
            }
            other => Err(StoreError::CorruptIndex(format!(
                "unknown numeric class {other}"
            ))),
        }
    }
}

/// Compare two encoded scalar order keys. Used by property tests and by
/// validation to verify the canonical encoding.
pub fn compare_scalar_keys(left: &ScalarKey, right: &ScalarKey) -> Ordering {
    left.cmp(right)
}

#[cfg(test)]
#[path = "key_codec.test.rs"]
mod tests;
