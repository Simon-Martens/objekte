//! Per-record reverse-contribution envelope with reusable LZ4 scratch.

use bincode::Options;
use serde::Serialize;

use super::value_codec::{
    ContributionPayload, StoredContributions, validate_state_masks,
};
use super::{StoreError, StoreResult};

pub const FORMAT_TAG: u8 = 3;
pub const CODEC_RAW: u8 = 0;
pub const CODEC_LZ4: u8 = 1;
const HEADER_BYTES: usize = 10;
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

#[derive(Default)]
pub struct ContributionScratch {
    raw: Vec<u8>,
    compressed: Vec<u8>,
    output: Vec<u8>,
}

impl ContributionScratch {
    pub fn encode<'a>(&'a mut self, contributions: &StoredContributions) -> StoreResult<&'a [u8]> {
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
        self.raw.clear();
        options()
            .serialize_into(&mut self.raw, &payload)
            .map_err(|error| {
                StoreError::CorruptValue(format!("contribution encode failed: {error}"))
            })?;
        if self.raw.len() > MAX_PAYLOAD_BYTES {
            return Err(StoreError::CorruptValue(
                "contribution exceeds the supported payload bound".into(),
            ));
        }

        let mut codec = CODEC_RAW;
        let mut stored = self.raw.as_slice();
        if self.raw.len() >= COMPRESSION_MIN_BYTES {
            let maximum = lz4_flex::block::get_maximum_output_size(self.raw.len());
            if maximum <= SCRATCH_BYTES {
                self.compressed.clear();
                self.compressed.resize(maximum, 0);
                let written = lz4_flex::block::compress_into(&self.raw, &mut self.compressed)
                    .map_err(|error| {
                        StoreError::CorruptValue(format!("LZ4 encode failed: {error}"))
                    })?;
                self.compressed.truncate(written);
                let required_saving = self.raw.len().div_ceil(8).max(32);
                if self.compressed.len() + required_saving <= self.raw.len() {
                    codec = CODEC_LZ4;
                    stored = &self.compressed;
                }
            }
        }
        let raw_len = u32::try_from(self.raw.len())
            .map_err(|_| StoreError::CorruptValue("contribution length overflow".into()))?;
        let stored_len = u32::try_from(stored.len())
            .map_err(|_| StoreError::CorruptValue("contribution length overflow".into()))?;
        self.output.clear();
        self.output.reserve(HEADER_BYTES.saturating_add(stored.len()));
        self.output.push(FORMAT_TAG);
        self.output.push(codec);
        self.output.extend_from_slice(&raw_len.to_le_bytes());
        self.output.extend_from_slice(&stored_len.to_le_bytes());
        self.output.extend_from_slice(stored);
        Ok(&self.output)
    }

    pub fn retained_capacity_bytes(&self) -> usize {
        self.raw.capacity() + self.compressed.capacity() + self.output.capacity()
    }
}

pub fn encode(contributions: &StoredContributions) -> StoreResult<Vec<u8>> {
    let mut scratch = ContributionScratch::default();
    Ok(scratch.encode(contributions)?.to_vec())
}

pub fn decode(bytes: &[u8]) -> StoreResult<ContributionPayload> {
    let tag = *bytes
        .first()
        .ok_or_else(|| StoreError::CorruptValue("empty contribution record".into()))?;
    if tag != FORMAT_TAG {
        return Err(StoreError::CorruptValue(format!(
            "unknown contribution format tag {tag}"
        )));
    }
    let codec = *bytes
        .get(1)
        .ok_or_else(|| StoreError::CorruptValue("truncated contribution header".into()))?;
    let raw_len = read_u32(bytes, 2)? as usize;
    let stored_len = read_u32(bytes, 6)? as usize;
    if raw_len > MAX_PAYLOAD_BYTES || stored_len > MAX_PAYLOAD_BYTES {
        return Err(StoreError::CorruptValue(
            "contribution payload exceeds the supported bound".into(),
        ));
    }
    let payload = bytes
        .get(HEADER_BYTES..)
        .ok_or_else(|| StoreError::CorruptValue("truncated contribution payload".into()))?;
    if payload.len() != stored_len {
        return Err(StoreError::CorruptValue(
            "contribution stored length mismatch".into(),
        ));
    }
    let decoded = match codec {
        CODEC_RAW => {
            if raw_len != stored_len {
                return Err(StoreError::CorruptValue(
                    "raw contribution length mismatch".into(),
                ));
            }
            payload.to_vec()
        }
        CODEC_LZ4 => {
            if raw_len > SCRATCH_BYTES {
                return Err(StoreError::CorruptValue(
                    "compressed contribution exceeds the supported bound".into(),
                ));
            }
            let mut decoded = vec![0; raw_len];
            let written = lz4_flex::block::decompress_into(payload, &mut decoded)
                .map_err(|error| {
                    StoreError::CorruptValue(format!("LZ4 decode failed: {error}"))
                })?;
            if written != raw_len {
                return Err(StoreError::CorruptValue(
                    "decompressed contribution length mismatch".into(),
                ));
            }
            decoded
        }
        other => {
            return Err(StoreError::CorruptValue(format!(
                "unknown contribution codec {other}"
            )));
        }
    };
    let value: ContributionPayload = options().deserialize(&decoded).map_err(|error| {
        StoreError::CorruptValue(format!("malformed contribution: {error}"))
    })?;
    validate_state_masks(&value.state_masks)?;
    Ok(value)
}

fn options() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .reject_trailing_bytes()
}

fn read_u32(bytes: &[u8], offset: usize) -> StoreResult<u32> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| StoreError::CorruptValue("truncated contribution header".into()))?;
    Ok(u32::from_le_bytes(slice.try_into().expect("four bytes")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::{ByteRange, DiagnosticRef, RecordClass, RecordEntry, SourceRevision};
    use crate::model::{FileId, RecordId};

    fn contribution(text: String) -> StoredContributions {
        let entry = RecordEntry {
            slot: crate::model::RecordSlot(0),
            id: RecordId("record".into()),
            file: FileId(0),
            range: ByteRange { start: 0, end: 8 },
            revision: SourceRevision {
                modified_ns: 1,
                size: 8,
            },
            class: RecordClass::Logical,
            diagnostic_ref: DiagnosticRef(0),
        };
        StoredContributions {
            record: super::super::value_codec::RecordMetadata::from_entry(&entry, 1),
            values: vec![(0, crate::posting::ScalarKey::String(text))],
            lengths: Vec::new(),
            state_masks: vec![(0, 1)],
            coverage: Vec::new(),
            domains: Vec::new(),
            sorts: Vec::new(),
            relations: Vec::new(),
            grams: Vec::new(),
        }
    }

    #[test]
    fn raw_and_compressed_contributions_round_trip() {
        for text in ["small".into(), "repeat ".repeat(100)] {
            let expected = contribution(text);
            let encoded = encode(&expected).unwrap();
            let decoded = decode(&encoded).unwrap();
            assert_eq!(decoded, ContributionPayload::from_contributions(&expected));
        }
    }

    #[test]
    fn reusable_scratch_does_not_grow_unnecessarily_after_large_then_small() {
        let mut scratch = ContributionScratch::default();
        scratch.encode(&contribution("x".repeat(10_000))).unwrap();
        let large = scratch.retained_capacity_bytes();
        scratch.encode(&contribution("x".repeat(10_000))).unwrap();
        assert_eq!(scratch.retained_capacity_bytes(), large);
        scratch.encode(&contribution("small".into())).unwrap();
        let small = scratch.retained_capacity_bytes();
        for _ in 0..20 {
            scratch.encode(&contribution("small".into())).unwrap();
        }
        assert_eq!(scratch.retained_capacity_bytes(), small);
    }

    #[test]
    fn malformed_envelopes_and_headers_are_rejected() {
        assert!(decode(&[]).is_err());
        assert!(decode(&[9, 0, 0, 0, 0, 0, 0, 0, 0, 0]).is_err());
        assert!(decode(&[FORMAT_TAG]).is_err());
        let encoded = encode(&contribution("value".into())).unwrap();
        assert!(decode(&encoded[..encoded.len() - 1]).is_err());
    }
}
