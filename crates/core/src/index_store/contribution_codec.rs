//! Per-record reverse-contribution envelope with optional LZ4.
//!
//! Format tag 3:
//!
//! ```text
//! tag:            u8 = 3
//! codec:          u8 (0 raw, 1 LZ4 block)
//! raw length:     u32 little-endian
//! stored length:  u32 little-endian
//! payload:        exactly stored-length bytes
//! ```
//!
//! The payload is a checked fixint little-endian bincode serialization of
//! `ContributionPayload`. Gram lists are not part of this value.

use bincode::Options;
use serde::Serialize;

use super::value_codec::{
    ContributionPayload, StoredContributions, validate_state_masks,
};
use super::{StoreError, StoreResult};

pub const FORMAT_TAG: u8 = 3;
pub const CODEC_RAW: u8 = 0;
pub const CODEC_LZ4: u8 = 1;
const HEADER_BYTES: usize = 1 + 1 + 4 + 4;
#[cfg(feature = "store-reverse-lz4")]
const COMPRESSION_MIN_BYTES: usize = 256;
const MAX_PAYLOAD_BYTES: usize = 256 * 1024 * 1024;
const SCRATCH_BYTES: usize = 64 * 1024 * 1024;

#[derive(Serialize)]
struct ContributionPayloadRef<'a> {
    record: &'a super::value_codec::RecordMetadata,
    values: &'a [(u32, crate::posting::ScalarKey)],
    lengths: &'a [(u32, crate::model::Number)],
    state_masks: &'a [(u32, u16)],
    coverage: &'a [(u32, super::value_codec::CoverageKind)],
    domains: &'a [super::value_codec::DomainKind],
    sorts: &'a [super::value_codec::SortProjection],
    relations: &'a [super::value_codec::RelationOccurrence],
}

fn options() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .reject_trailing_bytes()
}

fn serialized_payload(contributions: &StoredContributions) -> StoreResult<Vec<u8>> {
    validate_state_masks(&contributions.state_masks)?;
    let payload = ContributionPayloadRef {
        record: &contributions.record,
        values: &contributions.values,
        lengths: &contributions.lengths,
        state_masks: &contributions.state_masks,
        coverage: &contributions.coverage,
        domains: &contributions.domains,
        sorts: &contributions.sorts,
        relations: &contributions.relations,
    };
    options()
        .serialize(&payload)
        .map_err(|error| StoreError::CorruptIndex(format!("contribution encode failed: {error}")))
}

/// Encode one record contribution. Raw is chosen below 256 bytes or when the
/// compression saving is less than `max(32, ceil(raw/8))`.
#[allow(unused_mut)]
pub fn encode(contributions: &StoredContributions) -> StoreResult<Vec<u8>> {
    let raw = serialized_payload(contributions)?;
    if raw.len() > MAX_PAYLOAD_BYTES {
        return Err(StoreError::CorruptIndex(
            "contribution exceeds the supported payload bound".into(),
        ));
    }
    let mut codec = CODEC_RAW;
    let mut stored = raw.as_slice();
    #[cfg(feature = "store-reverse-lz4")]
    let mut compressed: Vec<u8> = Vec::new();
    #[cfg(feature = "store-reverse-lz4")]
    if raw.len() >= COMPRESSION_MIN_BYTES {
        let maximum = lz4_flex::block::get_maximum_output_size(raw.len());
        if maximum <= SCRATCH_BYTES {
            compressed.resize(maximum, 0);
            let written = lz4_flex::block::compress_into(&raw, &mut compressed)
                .map_err(|error| StoreError::CorruptIndex(format!("LZ4 encode failed: {error}")))?;
            compressed.truncate(written);
            let required_saving = raw.len().div_ceil(8).max(32);
            if compressed.len() + required_saving <= raw.len() {
                codec = CODEC_LZ4;
                stored = &compressed;
            }
        }
    }
    let raw_len = u32::try_from(raw.len())
        .map_err(|_| StoreError::CorruptIndex("contribution length overflow".into()))?;
    let stored_len = u32::try_from(stored.len())
        .map_err(|_| StoreError::CorruptIndex("contribution length overflow".into()))?;
    let mut bytes = Vec::with_capacity(HEADER_BYTES + stored.len());
    bytes.push(FORMAT_TAG);
    bytes.push(codec);
    bytes.extend_from_slice(&raw_len.to_le_bytes());
    bytes.extend_from_slice(&stored_len.to_le_bytes());
    bytes.extend_from_slice(stored);
    Ok(bytes)
}

pub fn decode(bytes: &[u8]) -> StoreResult<ContributionPayload> {
    let tag = *bytes
        .first()
        .ok_or_else(|| StoreError::CorruptIndex("empty contribution record".into()))?;
    if tag != FORMAT_TAG {
        return Err(StoreError::CorruptIndex(format!(
            "obsolete or unknown contribution format tag {tag}"
        )));
    }
    let codec = *bytes
        .get(1)
        .ok_or_else(|| StoreError::CorruptIndex("truncated contribution header".into()))?;
    let raw_len = read_u32(bytes, 2)? as usize;
    let stored_len = read_u32(bytes, 6)? as usize;
    if raw_len > MAX_PAYLOAD_BYTES {
        return Err(StoreError::CorruptIndex(
            "contribution payload exceeds the supported bound".into(),
        ));
    }
    let payload = bytes
        .get(HEADER_BYTES..)
        .ok_or_else(|| StoreError::CorruptIndex("truncated contribution payload".into()))?;
    if payload.len() != stored_len {
        return Err(StoreError::CorruptIndex(
            "contribution stored length mismatch".into(),
        ));
    }
    let decoded = match codec {
        CODEC_RAW => {
            if stored_len != raw_len {
                return Err(StoreError::CorruptIndex(
                    "raw contribution length mismatch".into(),
                ));
            }
            payload.to_vec()
        }
        CODEC_LZ4 => {
            if raw_len > SCRATCH_BYTES {
                return Err(StoreError::CorruptIndex(
                    "compressed contribution exceeds the supported bound".into(),
                ));
            }
            let mut decoded = vec![0u8; raw_len];
            let written = lz4_flex::block::decompress_into(payload, &mut decoded)
                .map_err(|error| StoreError::CorruptIndex(format!("LZ4 decode failed: {error}")))?;
            if written != raw_len {
                return Err(StoreError::CorruptIndex(
                    "decompressed contribution length mismatch".into(),
                ));
            }
            decoded
        }
        other => {
            return Err(StoreError::CorruptIndex(format!(
                "unknown contribution codec {other}"
            )));
        }
    };
    let value: ContributionPayload = options()
        .deserialize(&decoded)
        .map_err(|error| StoreError::CorruptIndex(format!("malformed contribution: {error}")))?;
    validate_state_masks(&value.state_masks)?;
    Ok(value)
}

fn read_u32(bytes: &[u8], offset: usize) -> StoreResult<u32> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| StoreError::CorruptIndex("truncated contribution header".into()))?;
    Ok(u32::from_le_bytes(
        slice.try_into().expect("four-byte length"),
    ))
}

#[cfg(test)]
#[path = "contribution_codec.test.rs"]
mod tests;
