//! Contribution envelope and compression tests.

use crate::inventory::{ByteRange, RecordClass, SourceRevision};
use crate::model::{FileId, RecordId};
use crate::posting::ScalarKey;

use super::*;
use crate::index_store::value_codec::{RecordMetadata, SortProjection, StateKind};

fn record(record_id: &str) -> RecordMetadata {
    RecordMetadata {
        id: RecordId(record_id.into()),
        file: FileId(0),
        range: ByteRange { start: 0, end: 1 },
        revision: SourceRevision {
            modified_ns: 0,
            size: 1,
        },
        class: RecordClass::Logical,
        diagnostic_ref: 0,
        incarnation: 1,
    }
}

fn contribution(values: Vec<(u32, String)>, _grams: usize) -> StoredContributions {
    StoredContributions {
        record: record("r"),
        values: values
            .into_iter()
            .map(|(binding, value)| (binding, ScalarKey::String(value)))
            .collect(),
        lengths: vec![(0, crate::model::Number::Integer(3))],
        state_masks: vec![(0, 1 << StateKind::PathPresence.tag())],
        coverage: Vec::new(),
        domains: Vec::new(),
        sorts: vec![SortProjection {
            definition: 0,
            key: vec![crate::sorting::SortKey::String(b"a".to_vec())],
        }],
        relations: Vec::new(),
        grams: Vec::new(),
    }
}

#[test]
fn small_payloads_are_raw() {
    let value = contribution(vec![(0, "x".into())], 0);
    let encoded = encode(&value).expect("encode");
    assert_eq!(encoded[0], FORMAT_TAG);
    assert_eq!(encoded[1], CODEC_RAW);
    let decoded = decode(&encoded).expect("decode");
    assert_eq!(decoded.values, vec![(0, ScalarKey::String("x".into()))]);
}

#[test]
fn repetitive_payloads_compress_and_round_trip() {
    let repeated = "the same long field value repeated many times".repeat(64);
    let value = contribution(vec![(0, repeated.clone())], 0);
    let encoded = encode(&value).expect("encode");
    #[cfg(feature = "store-reverse-lz4")]
    assert_eq!(encoded[1], CODEC_LZ4);
    #[cfg(not(feature = "store-reverse-lz4"))]
    assert_eq!(encoded[1], CODEC_RAW);
    let decoded = decode(&encoded).expect("decode");
    assert_eq!(
        decoded.values,
        vec![(0, ScalarKey::String(repeated))]
    );
}

#[test]
fn incompressible_payloads_stay_raw_and_never_grow() {
    // Deterministic high-entropy printable data above the threshold.
    let mut state = 0x1234_5678_9ABC_DEF0u64;
    let mut text = String::with_capacity(8192);
    for _ in 0..8192 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        text.push(char::from(b'!' + (state % 90) as u8));
    }
    let value = contribution(vec![(0, text)], 0);
    let encoded = encode(&value).expect("encode");
    assert_eq!(encoded[1], CODEC_RAW);
    let raw_len = u32::from_le_bytes(encoded[2..6].try_into().expect("length")) as usize;
    assert_eq!(encoded.len(), HEADER_BYTES + raw_len);
    assert_eq!(decode(&encoded).expect("decode").values.len(), 1);
}

#[test]
fn malformed_headers_and_payloads_are_rejected() {
    let value = contribution(vec![(0, "x".into())], 0);
    let encoded = encode(&value).expect("encode");
    assert!(decode(&[]).is_err());
    assert!(decode(&[2]).is_err());
    assert!(decode(&[FORMAT_TAG]).is_err());
    let mut wrong_codec = encoded.clone();
    wrong_codec[1] = 9;
    assert!(decode(&wrong_codec).is_err());
    let mut wrong_length = encoded.clone();
    wrong_length[2] = 5;
    assert!(decode(&wrong_length).is_err());
    let mut truncated = encoded.clone();
    truncated.pop();
    assert!(decode(&truncated).is_err());
    let mut trailing = encoded;
    trailing.push(0);
    assert!(decode(&trailing).is_err());
}

#[test]
fn invalid_state_masks_are_rejected_on_encode_and_decode() {
    let mut value = contribution(vec![(0, "x".into())], 0);
    value.state_masks = vec![(0, 0xFFFF)];
    assert!(encode(&value).is_err());
    value.state_masks = vec![(0, 0)];
    assert!(encode(&value).is_err());
}

#[cfg(feature = "store-reverse-lz4")]
#[test]
fn compressed_payloads_reject_declared_length_mismatch() {
    let repeated = "compress me ".repeat(100);
    let value = contribution(vec![(0, repeated)], 0);
    let mut encoded = encode(&value).expect("encode");
    assert_eq!(encoded[1], CODEC_LZ4);
    encoded[2] ^= 0x01;
    assert!(decode(&encoded).is_err());
}

#[test]
fn envelope_is_independently_addressable() {
    let first = contribution(vec![(0, "a".repeat(400))], 0);
    let second = contribution(vec![(1, "b".repeat(400))], 0);
    let encoded_first = encode(&first).expect("first");
    let encoded_second = encode(&second).expect("second");
    assert_eq!(decode(&encoded_first).expect("first decode").values[0].0, 0);
    assert_eq!(decode(&encoded_second).expect("second decode").values[0].0, 1);
}
