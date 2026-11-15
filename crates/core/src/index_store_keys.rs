//! redb table definitions and key/value codecs for the index store.

use redb::TableDefinition;
use serde::{Deserialize, Serialize};

use crate::model::{FieldPath, LogicalType, Number};
use crate::posting::ScalarKey;
use crate::sorting::{SortKey, SortOptions};
use crate::relation::RelationDefinition;

pub const STORE_FORMAT_VERSION: u32 = 1;
pub const SHARD_PREFIX_LEN: usize = 12;

pub const META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");
pub const COLLECTIONS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("collections");
pub const CATALOG: TableDefinition<&[u8], &[u8]> = TableDefinition::new("catalog");
pub const SHARDS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("shards");
pub const ACTIVE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("active");
pub const FILES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("files");
pub const RECORDS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("records");
pub const POSTING_FIELDS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("posting_fields");
pub const POSTINGS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("postings");
pub const BITMAPS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("bitmaps");
pub const SORT_COLUMNS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("sort_columns");
pub const SORT_KEYS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("sort_keys");
pub const COMPOSITES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("composites");
pub const RELATIONS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("relations");
pub const REFERENCES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("references");
pub const INFERENCE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("inference");

pub const FAMILY_DECLARED_VALUES: u8 = 0;
pub const FAMILY_DECLARED_LENGTHS: u8 = 1;
pub const FAMILY_DECLARED_STRUCTURAL: u8 = 2;
pub const FAMILY_CUSTOM_VALUES: u8 = 3;
pub const FAMILY_CUSTOM_LENGTHS: u8 = 4;
pub const FAMILY_CUSTOM_STRUCTURAL: u8 = 5;
pub const FAMILY_INFERRED_VALUES: u8 = 6;
pub const FAMILY_INFERRED_LENGTHS: u8 = 7;
pub const FAMILY_INFERRED_STRUCTURAL: u8 = 8;
pub const FAMILY_RAW_NORMAL: u8 = 9;
pub const FAMILY_RAW_SKIP: u8 = 10;
pub const FAMILY_FALLBACK_NORMAL: u8 = 11;
pub const FAMILY_FALLBACK_SKIP: u8 = 12;
pub const FAMILY_CUSTOM_TEXT_NORMAL: u8 = 13;
pub const FAMILY_CUSTOM_TEXT_SKIP: u8 = 14;

pub const BITMAP_PRESENCE: u8 = 0;
pub const BITMAP_NULLS: u8 = 1;
pub const BITMAP_EMPTY_SELECTIONS: u8 = 2;
pub const BITMAP_LOGICAL_RECORDS: u8 = 30;
pub const BITMAP_EMPTY_ARRAY_RECORDS: u8 = 31;
pub const BITMAP_INFERRED_EMPTY_SELECTIONS: u8 = 32;
pub const BITMAP_INFERRED_SHORT_STRING_COVERAGE: u8 = 33;
pub const BITMAP_RELATION_PRESENT: u8 = 40;
pub const BITMAP_RELATION_NULLS: u8 = 41;
pub const BITMAP_RELATION_EMPTY_ARRAYS: u8 = 42;
pub const BITMAP_RELATION_UNAVAILABLE: u8 = 43;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct ShardHeader {
    pub ordinal: u64,
    pub alphabet: crate::workload::BigramAlphabet,
    pub role: u8,
    pub shared: bool,
    pub source_id: String,
    pub source_version: u32,
    pub record_count: u32,
    pub file_count: u32,
    pub record_parses: u64,
    pub failed_parses: u64,
    pub has_inference: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct PostingHeader {
    pub kind: LogicalType,
    pub multi: bool,
    pub values_indexed: bool,
    pub value_complete: bool,
    pub length_complete: bool,
    pub empty_string_complete: bool,
    pub covered_records: u32,
    pub has_structural: bool,
    pub custom_complete: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct SortColumnHeader {
    pub path: FieldPath,
    pub kind: LogicalType,
    pub locale: String,
    pub options: SortOptions,
    pub collation_version: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct RelationHeader {
    pub declaration: RelationDefinition,
    pub record_count: u32,
    pub reference_count: u32,
}

pub(crate) fn prefix(collection: u32, store_id: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(SHARD_PREFIX_LEN);
    key.extend_from_slice(&collection.to_be_bytes());
    key.extend_from_slice(&store_id.to_be_bytes());
    key
}

pub(crate) fn push_u32(key: &mut Vec<u8>, value: u32) {
    key.extend_from_slice(&value.to_be_bytes());
}

pub(crate) fn push_u64(key: &mut Vec<u8>, value: u64) {
    key.extend_from_slice(&value.to_be_bytes());
}

pub(crate) fn push_bytes(key: &mut Vec<u8>, value: &[u8]) {
    push_u32(key, u32::try_from(value.len()).expect("store key component is too long"));
    key.extend_from_slice(value);
}

fn with_prefix(prefix: &[u8], build: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut key = Vec::with_capacity(prefix.len() + 16);
    key.extend_from_slice(prefix);
    build(&mut key);
    key
}

pub(crate) fn shard_key(prefix: &[u8]) -> Vec<u8> {
    prefix.to_vec()
}

pub(crate) fn files_key(prefix: &[u8], position: u32) -> Vec<u8> {
    with_prefix(prefix, |key| push_u32(key, position))
}

pub(crate) fn records_key(prefix: &[u8], slot: u32) -> Vec<u8> {
    with_prefix(prefix, |key| push_u32(key, slot))
}

pub(crate) fn posting_field_key(prefix: &[u8], family: u8, field: &[u8]) -> Vec<u8> {
    with_prefix(prefix, |key| {
        key.push(family);
        push_bytes(key, field);
    })
}

pub(crate) fn posting_key(
    prefix: &[u8],
    family: u8,
    field: &[u8],
    posting_key: &[u8],
    chunk: u32,
) -> Vec<u8> {
    with_prefix(prefix, |key| {
        key.push(family);
        push_bytes(key, field);
        push_bytes(key, posting_key);
        push_u32(key, chunk);
    })
}

pub(crate) fn bitmap_key(prefix: &[u8], kind: u8, field: &[u8]) -> Vec<u8> {
    with_prefix(prefix, |key| {
        key.push(kind);
        push_bytes(key, field);
    })
}

pub(crate) fn sort_column_key(prefix: &[u8], column: &[u8]) -> Vec<u8> {
    with_prefix(prefix, |key| push_bytes(key, column))
}

pub(crate) fn sort_key_key(prefix: &[u8], column: &[u8], slot: u32) -> Vec<u8> {
    with_prefix(prefix, |key| {
        push_bytes(key, column);
        push_u32(key, slot);
    })
}

pub(crate) fn composite_key(prefix: &[u8], index: u32) -> Vec<u8> {
    with_prefix(prefix, |key| push_u32(key, index))
}

pub(crate) fn relation_key(prefix: &[u8], ordinal: u32) -> Vec<u8> {
    with_prefix(prefix, |key| push_u32(key, ordinal))
}

pub(crate) fn reference_key(prefix: &[u8], ordinal: u32, source: u32, index: u32) -> Vec<u8> {
    with_prefix(prefix, |key| {
        push_u32(key, ordinal);
        push_u32(key, source);
        push_u32(key, index);
    })
}

pub(crate) fn inference_key(prefix: &[u8]) -> Vec<u8> {
    prefix.to_vec()
}

/// The smallest byte string greater than every string with `prefix`.
pub(crate) fn prefix_end(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut end = prefix.to_vec();
    while end.last() == Some(&0xFF) {
        end.pop();
    }
    let last = end.last_mut()?;
    *last += 1;
    Some(end)
}

pub(crate) fn read_u32(bytes: &[u8], offset: &mut usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let value = u32::from_be_bytes(bytes.get(*offset..end)?.try_into().ok()?);
    *offset = end;
    Some(value)
}

pub(crate) fn read_bytes<'a>(bytes: &'a [u8], offset: &mut usize) -> Option<&'a [u8]> {
    let length = read_u32(bytes, offset)? as usize;
    let end = offset.checked_add(length)?;
    let value = bytes.get(*offset..end)?;
    *offset = end;
    Some(value)
}

/// Decode a postings key after its shard prefix.
pub(crate) fn posting_parts(key: &[u8]) -> Option<(u8, &[u8], &[u8], u32)> {
    let mut offset = SHARD_PREFIX_LEN;
    let family = *key.get(offset)?;
    offset += 1;
    let field = read_bytes(key, &mut offset)?;
    let posting_key = read_bytes(key, &mut offset)?;
    let chunk = read_u32(key, &mut offset)?;
    Some((family, field, posting_key, chunk))
}

pub(crate) fn bitmap_parts(key: &[u8]) -> Option<(u8, &[u8])> {
    let mut offset = SHARD_PREFIX_LEN;
    let kind = *key.get(offset)?;
    offset += 1;
    let field = read_bytes(key, &mut offset)?;
    Some((kind, field))
}

#[cfg(test)]
#[path = "index_store_keys.test.rs"]
mod tests;

fn number_tag(number: &Number) -> u8 {
    match number {
        Number::Integer(_) => 0,
        Number::Float(_) => 1,
        Number::ExactDecimal(_) => 2,
    }
}

pub(crate) fn encode_number_value(number: &Number) -> Vec<u8> {
    let mut out = Vec::new();
    encode_number(number, &mut out);
    out
}

pub(crate) fn decode_number_value(bytes: &[u8]) -> Option<Number> {
    decode_number(bytes, &mut 0)
}

fn encode_number(number: &Number, out: &mut Vec<u8>) {
    match number {
        Number::Integer(value) => {
            out.push(0);
            out.extend_from_slice(&value.to_be_bytes());
        }
        Number::Float(value) => {
            out.push(1);
            out.extend_from_slice(&value.to_bits().to_be_bytes());
        }
        Number::ExactDecimal(value) => {
            out.push(2);
            out.push(u8::from(value.negative));
            push_bytes(out, value.digits.as_bytes());
            out.extend_from_slice(&value.exponent.to_be_bytes());
        }
    }
}

fn decode_number(bytes: &[u8], offset: &mut usize) -> Option<Number> {
    let tag = *bytes.get(*offset)?;
    *offset += 1;
    Some(match tag {
        0 => {
            let end = offset.checked_add(16)?;
            let value = i128::from_be_bytes(bytes.get(*offset..end)?.try_into().ok()?);
            *offset = end;
            Number::Integer(value)
        }
        1 => {
            let end = offset.checked_add(8)?;
            let value = f64::from_bits(u64::from_be_bytes(bytes.get(*offset..end)?.try_into().ok()?));
            *offset = end;
            Number::Float(value)
        }
        2 => {
            let negative = *bytes.get(*offset)? != 0;
            *offset += 1;
            let digits = String::from_utf8(read_bytes(bytes, offset)?.to_vec()).ok()?;
            let end = offset.checked_add(8)?;
            let exponent = i64::from_be_bytes(bytes.get(*offset..end)?.try_into().ok()?);
            *offset = end;
            Number::ExactDecimal(crate::decimal::ExactDecimal {
                negative,
                digits,
                exponent,
            })
        }
        _ => return None,
    })
}

pub(crate) fn encode_scalar_key(key: &ScalarKey) -> Vec<u8> {
    let mut out = Vec::new();
    match key {
        ScalarKey::String(value) => {
            out.push(0);
            push_bytes(&mut out, value.as_bytes());
        }
        ScalarKey::Id(value) => {
            out.push(1);
            push_bytes(&mut out, value.as_bytes());
        }
        ScalarKey::Number(value) => {
            out.push(2);
            encode_number(value, &mut out);
        }
        ScalarKey::Bool(value) => {
            out.push(3);
            out.push(u8::from(*value));
        }
        ScalarKey::Date(value) => {
            out.push(4);
            out.extend_from_slice(&value.to_julian_day().to_be_bytes());
        }
        ScalarKey::Datetime(value) => {
            out.push(5);
            out.extend_from_slice(&value.unix_timestamp().to_be_bytes());
            out.extend_from_slice(&value.nanosecond().to_be_bytes());
        }
        ScalarKey::Unixtime(value) => {
            out.push(6);
            out.extend_from_slice(&value.to_be_bytes());
        }
    }
    out
}

pub(crate) fn decode_scalar_key(bytes: &[u8]) -> Option<ScalarKey> {
    let mut offset = 0usize;
    let tag = *bytes.get(offset)?;
    offset += 1;
    Some(match tag {
        0 => ScalarKey::String(String::from_utf8(read_bytes(bytes, &mut offset)?.to_vec()).ok()?),
        1 => ScalarKey::Id(String::from_utf8(read_bytes(bytes, &mut offset)?.to_vec()).ok()?),
        2 => ScalarKey::Number(decode_number(bytes, &mut offset)?),
        3 => ScalarKey::Bool(*bytes.get(offset)? != 0),
        4 => {
            let end = offset.checked_add(4)?;
            let day = i32::from_be_bytes(bytes.get(offset..end)?.try_into().ok()?);
            ScalarKey::Date(time::Date::from_julian_day(day).ok()?)
        }
        5 => {
            let end = offset.checked_add(8)?;
            let seconds = i64::from_be_bytes(bytes.get(offset..end)?.try_into().ok()?);
            offset = end;
            let end = offset.checked_add(4)?;
            let nanos = u32::from_be_bytes(bytes.get(offset..end)?.try_into().ok()?);
            ScalarKey::Datetime(
                time::OffsetDateTime::from_unix_timestamp(seconds)
                    .ok()?
                    .replace_nanosecond(nanos)
                    .ok()?,
            )
        }
        6 => {
            let end = offset.checked_add(8)?;
            let value = i64::from_be_bytes(bytes.get(offset..end)?.try_into().ok()?);
            ScalarKey::Unixtime(value)
        }
        _ => return None,
    })
}

pub(crate) fn encode_sort_key(key: &SortKey) -> Vec<u8> {
    let mut out = Vec::new();
    match key {
        SortKey::Missing => out.push(0),
        SortKey::Unavailable => out.push(1),
        SortKey::Null => out.push(2),
        SortKey::String(value) => {
            out.push(3);
            push_bytes(&mut out, value);
        }
        SortKey::Id(value) => {
            out.push(4);
            push_bytes(&mut out, value.as_bytes());
        }
        SortKey::Number(value) => {
            out.push(5);
            encode_number(value, &mut out);
        }
        SortKey::Bool(value) => {
            out.push(6);
            out.push(u8::from(*value));
        }
        SortKey::Date(value) => {
            out.push(7);
            out.extend_from_slice(&value.to_julian_day().to_be_bytes());
        }
        SortKey::Datetime(value) => {
            out.push(8);
            out.extend_from_slice(&value.unix_timestamp().to_be_bytes());
            out.extend_from_slice(&value.nanosecond().to_be_bytes());
        }
        SortKey::Unixtime(value) => {
            out.push(9);
            out.extend_from_slice(&value.to_be_bytes());
        }
    }
    out
}

pub(crate) fn decode_sort_key(bytes: &[u8]) -> Option<SortKey> {
    let mut offset = 0usize;
    let tag = *bytes.get(offset)?;
    offset += 1;
    Some(match tag {
        0 => SortKey::Missing,
        1 => SortKey::Unavailable,
        2 => SortKey::Null,
        3 => SortKey::String(read_bytes(bytes, &mut offset)?.to_vec()),
        4 => SortKey::Id(String::from_utf8(read_bytes(bytes, &mut offset)?.to_vec()).ok()?),
        5 => SortKey::Number(decode_number(bytes, &mut offset)?),
        6 => SortKey::Bool(*bytes.get(offset)? != 0),
        7 => {
            let end = offset.checked_add(4)?;
            let day = i32::from_be_bytes(bytes.get(offset..end)?.try_into().ok()?);
            SortKey::Date(time::Date::from_julian_day(day).ok()?)
        }
        8 => {
            let end = offset.checked_add(8)?;
            let seconds = i64::from_be_bytes(bytes.get(offset..end)?.try_into().ok()?);
            offset = end;
            let end = offset.checked_add(4)?;
            let nanos = u32::from_be_bytes(bytes.get(offset..end)?.try_into().ok()?);
            SortKey::Datetime(
                time::OffsetDateTime::from_unix_timestamp(seconds)
                    .ok()?
                    .replace_nanosecond(nanos)
                    .ok()?,
            )
        }
        9 => {
            let end = offset.checked_add(8)?;
            let value = i64::from_be_bytes(bytes.get(offset..end)?.try_into().ok()?);
            SortKey::Unixtime(value)
        }
        _ => return None,
    })
}
