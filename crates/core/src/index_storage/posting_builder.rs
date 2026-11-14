//! Packed typed-posting construction.
//!
//! Each field owns one posting arena containing presence, null, structural,
//! typed-value, and length record sets. A field directory in the segment
//! carries paths and ordinals; builder maps are released when sealing ends.

use std::collections::BTreeMap;

use crate::model::{LogicalType, Number};
use crate::posting::ScalarKey;

use super::dictionary::KeyOrder;
use super::dictionary_builder::build_dictionary;
use super::error::{StorageError, StorageResult};
use super::record_set::{RecordSetRef, encode_record_set};
use super::scalar::encode_scalar_key;

/// Fixed width of one packed posting field descriptor.
pub const POSTING_FIELD_DESCRIPTOR_LEN: usize = 208;
/// Posting payload magic.
pub const POSTING_PAYLOAD_MAGIC: [u8; 4] = *b"OBPT";
/// Posting payload version.
pub const POSTING_PAYLOAD_VERSION: u16 = 1;
/// Posting field flags.
pub const FIELD_VALUES_INDEXED: u32 = 1 << 0;
pub const FIELD_VALUE_COMPLETE: u32 = 1 << 1;
pub const FIELD_LENGTH_COMPLETE: u32 = 1 << 2;
pub const FIELD_EMPTY_STRING_COMPLETE: u32 = 1 << 3;
pub const FIELD_HAS_LENGTHS: u32 = 1 << 4;
pub const FIELD_HAS_STRUCTURAL: u32 = 1 << 5;
pub const FIELD_HAS_EMPTY_STRING: u32 = 1 << 6;
/// Field directory version.
pub const FIELD_DIRECTORY_VERSION: u16 = 1;


/// One field contribution supplied to the packed posting builder.
#[derive(Clone, Debug)]
pub struct FieldInput {
    pub ordinal: u32,
    pub path: Vec<String>,
    pub kind: LogicalType,
    pub multi: bool,
    pub presence: Vec<u32>,
    pub nulls: Vec<u32>,
    pub empty_selections: Vec<u32>,
    pub empty_strings: Vec<u32>,
    pub structural: BTreeMap<u8, Vec<u32>>,
    pub values: BTreeMap<ScalarKey, Vec<u32>>,
    pub lengths: BTreeMap<Number, Vec<u32>>,
    pub values_indexed: bool,
    pub value_complete: bool,
    pub length_complete: bool,
    pub empty_string_complete: bool,
    pub covered_records: u32,
}


/// Packed field-index sections.
pub struct PostingSections {
    pub payloads: Vec<u8>,
    pub field_directory: Vec<u8>,
}


/// Encode every typed field contribution into one payload section and one
/// field directory.
pub fn build_postings(fields: &[FieldInput], slot_count: u32) -> StorageResult<PostingSections> {
    let mut ordinals = std::collections::HashSet::new();
    let mut fields = fields.iter().collect::<Vec<_>>();
    fields.sort_by_key(|field| field.ordinal);
    for field in &fields {
        if !ordinals.insert(field.ordinal) {
            return Err(StorageError::InvalidArgument(
                "duplicate posting field ordinal".into(),
            ));
        }
    }
    let mut directory = vec![0u8; 16 + fields.len() * POSTING_FIELD_DESCRIPTOR_LEN];
    directory[..4].copy_from_slice(&POSTING_PAYLOAD_MAGIC);
    directory[4..6].copy_from_slice(&POSTING_PAYLOAD_VERSION.to_le_bytes());
    directory[6..8].copy_from_slice(&(fields.len() as u16).to_le_bytes());
    directory[8..16].fill(0);
    let mut payload = Vec::new();
    for (index, field) in fields.iter().enumerate() {
        let mut arena = Vec::new();
        let mut embed = |slots: &[u32]| -> StorageResult<RecordSetRef> {
            let mut encoded = encode_record_set(slots, slot_count)?;
            if encoded.reference.tag == super::record_set::RecordSetTag::Blocks {
                encoded.reference.a = encoded
                    .reference
                    .a
                    .checked_add(arena.len() as u64)
                    .ok_or_else(|| {
                        StorageError::ResourceLimit("posting arena offset overflow".into())
                    })?;
            }
            arena.extend_from_slice(&encoded.arena);
            Ok(encoded.reference)
        };
        let presence = embed(&field.presence)?;
        let nulls = embed(&field.nulls)?;
        let empty_selections = embed(&field.empty_selections)?;
        let empty_string = embed(&field.empty_strings)?;
        let mut structural = Vec::new();
        let structural_count = u32::try_from(field.structural.len())
            .map_err(|_| StorageError::ResourceLimit("structural fact count overflow".into()))?;
        structural.extend_from_slice(&structural_count.to_le_bytes());
        for (key, slots) in &field.structural {
            let reference = embed(slots)?;
            structural.push(*key);
            structural.extend_from_slice(&[0u8; 7]);
            structural.extend_from_slice(&reference.encode());
        }
        let mut value_entries = Vec::with_capacity(field.values.len());
        for (value, slots) in &field.values {
            let reference = embed(slots)?;
            value_entries.push((encode_scalar_key(value)?, reference.encode().to_vec()));
        }
        let values = build_dictionary(value_entries, KeyOrder::Scalar, 8192)?;
        let mut length_entries = Vec::with_capacity(field.lengths.len());
        for (length, slots) in &field.lengths {
            let reference = embed(slots)?;
            let key = encode_scalar_key(&ScalarKey::Number(length.clone()))?;
            length_entries.push((key, reference.encode().to_vec()));
        }
        let lengths = if field.lengths.is_empty() {
            Vec::new()
        } else {
            build_dictionary(length_entries, KeyOrder::Scalar, 8192)?
        };
        let base = 16 + fields.len() * POSTING_FIELD_DESCRIPTOR_LEN + payload.len();
        let values_offset = base;
        let values_len = values.len();
        payload.extend_from_slice(&values);
        let lengths_offset = base + values_len;
        payload.extend_from_slice(&lengths);
        let structural_offset = base + values_len + lengths.len();
        payload.extend_from_slice(&structural);
        let arena_offset = base + values_len + lengths.len() + structural.len();
        payload.extend_from_slice(&arena);
        let at = 16 + index * POSTING_FIELD_DESCRIPTOR_LEN;
        let mut flags = 0u32;
        if field.values_indexed {
            flags |= FIELD_VALUES_INDEXED;
        }
        if field.value_complete {
            flags |= FIELD_VALUE_COMPLETE;
        }
        if field.length_complete {
            flags |= FIELD_LENGTH_COMPLETE;
        }
        if field.empty_string_complete {
            flags |= FIELD_EMPTY_STRING_COMPLETE;
        }
        if !field.lengths.is_empty() {
            flags |= FIELD_HAS_LENGTHS;
        }
        if !field.structural.is_empty() {
            flags |= FIELD_HAS_STRUCTURAL;
        }
        if !field.empty_strings.is_empty() {
            flags |= FIELD_HAS_EMPTY_STRING;
        }
        let slot = &mut directory[at..at + POSTING_FIELD_DESCRIPTOR_LEN];
        slot[..4].copy_from_slice(&field.ordinal.to_le_bytes());
        slot[4..8].copy_from_slice(&flags.to_le_bytes());
        slot[8..40].copy_from_slice(&presence.encode());
        slot[40..72].copy_from_slice(&nulls.encode());
        slot[72..104].copy_from_slice(&empty_selections.encode());
        slot[104..136].copy_from_slice(&empty_string.encode());
        slot[136..144].copy_from_slice(&(values_offset as u64).to_le_bytes());
        slot[144..152].copy_from_slice(&(values_len as u64).to_le_bytes());
        slot[152..160].copy_from_slice(&(lengths_offset as u64).to_le_bytes());
        slot[160..168].copy_from_slice(&(lengths.len() as u64).to_le_bytes());
        slot[168..176].copy_from_slice(&(structural_offset as u64).to_le_bytes());
        slot[176..184].copy_from_slice(&(structural.len() as u64).to_le_bytes());
        slot[184..192].copy_from_slice(&(arena_offset as u64).to_le_bytes());
        slot[192..200].copy_from_slice(&(arena.len() as u64).to_le_bytes());
        slot[200..204].copy_from_slice(&field.covered_records.to_le_bytes());
        slot[204..208].fill(0);
    }
    directory.extend_from_slice(&payload);
    let field_directory = build_field_directory(&fields)?;
    Ok(PostingSections {
        payloads: directory,
        field_directory,
    })
}

/// Encode the field-name-to-ordinal directory.
pub fn build_field_directory(fields: &[&FieldInput]) -> StorageResult<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&FIELD_DIRECTORY_VERSION.to_le_bytes());
    bytes.extend_from_slice(&[0u8; 2]);
    bytes.extend_from_slice(&(fields.len() as u32).to_le_bytes());
    for field in fields {
        let (kind, element) = super::posting::logical_type_codes(&field.kind);
        bytes.extend_from_slice(&field.ordinal.to_le_bytes());
        bytes.push(kind);
        bytes.push(element);
        bytes.push(u8::from(field.multi));
        bytes.push(0);
        let path_segments = u32::try_from(field.path.len())
            .map_err(|_| StorageError::ResourceLimit("field path segment count overflow".into()))?;
        bytes.extend_from_slice(&path_segments.to_le_bytes());
        for segment in &field.path {
            let length = u32::try_from(segment.len())
                .map_err(|_| StorageError::ResourceLimit("field path segment too large".into()))?;
            bytes.extend_from_slice(&length.to_le_bytes());
            bytes.extend_from_slice(segment.as_bytes());
        }
    }
    Ok(bytes)
}
