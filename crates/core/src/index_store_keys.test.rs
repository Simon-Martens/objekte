use super::*;
use crate::inventory::{
    ByteRange, DiagnosticRef, FileEntry, FileStatus, RecordClass, RecordEntry, SourceRevision,
    SharedPath,
};
use crate::model::{Number, RecordId, RecordSlot};
use crate::posting::ScalarKey;
use crate::relation::PendingReference;
use crate::sorting::SortKey;
use time::{Date, Month, OffsetDateTime};

fn round_trip<T>(value: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug + PartialEq,
{
    let bytes = bincode::serialize(value).unwrap();
    let decoded: T = bincode::deserialize(&bytes).unwrap();
    assert_eq!(&decoded, value);
}

fn file_entry() -> FileEntry {
    FileEntry {
        id: crate::model::FileId(7),
        relative_path: SharedPath::Plain("data/a.json".into()),
        revision: SourceRevision {
            modified_ns: 1_700_000_000_123_456_789,
            size: 42,
        },
        created_ns: Some(1_600_000_000_000_000_000),
        collections: vec![crate::model::CollectionId(0), crate::model::CollectionId(2)],
        status: FileStatus::Broken,
        diagnostics: vec![crate::inventory::SourceDiagnostic {
            file: crate::model::FileId(7),
            collection: crate::model::CollectionId(0),
            source_id: "src".into(),
            source_version: 1,
            record: Some(RecordSlot(1)),
            range: Some(ByteRange { start: 0, end: 4 }),
            category: crate::source::DiagnosticCategory::Syntax,
            code: "bad-json".into(),
            field_path: Some(vec!["a".into()]),
            byte_offset: Some(1),
            line: Some(3),
            column: Some(5),
            message: "expected value".into(),
        }],
        omitted_diagnostics: 2,
    }
}

#[test]
fn store_values_round_trip_through_bincode() {
    for key in [
        ScalarKey::String("text".into()),
        ScalarKey::Id("id".into()),
        ScalarKey::Number(Number::integer(i128::MIN)),
        ScalarKey::Number(Number::integer(i128::MAX)),
        ScalarKey::Number(Number::Float(0.0)),
        ScalarKey::Number(Number::Float(-0.0)),
        ScalarKey::Number(Number::Float(f64::MIN_POSITIVE / 2.0)),
        ScalarKey::Number(Number::Float(f64::INFINITY)),
        ScalarKey::Number(Number::ExactDecimal(
            crate::decimal::ExactDecimal::parse("1234567890.123456789012345678901234567890")
                .unwrap(),
        )),
        ScalarKey::Bool(true),
        ScalarKey::Date(Date::from_calendar_date(2024, Month::February, 29).unwrap()),
        ScalarKey::Datetime(OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()),
        ScalarKey::Unixtime(-5),
    ] {
        let bytes = encode_scalar_key(&key);
        assert_eq!(decode_scalar_key(&bytes).as_ref(), Some(&key));
    }
    for key in [
        SortKey::Missing,
        SortKey::Unavailable,
        SortKey::Null,
        SortKey::String(vec![0, 1, 255]),
        SortKey::Id("id".into()),
        SortKey::Number(Number::integer(-1)),
        SortKey::Bool(false),
        SortKey::Date(Date::from_calendar_date(2024, Month::January, 1).unwrap()),
        SortKey::Datetime(OffsetDateTime::from_unix_timestamp(0).unwrap()),
        SortKey::Unixtime(9),
    ] {
        let bytes = encode_sort_key(&key);
        assert_eq!(decode_sort_key(&bytes).as_ref(), Some(&key));
    }
    let record = RecordEntry {
        slot: RecordSlot(3),
        id: RecordId("record".into()),
        file: crate::model::FileId(7),
        range: ByteRange { start: 10, end: 20 },
        revision: SourceRevision {
            modified_ns: -1,
            size: 30,
        },
        class: RecordClass::RawFallback,
        diagnostic_ref: DiagnosticRef(4),
    };
    round_trip(&record);
    let file = file_entry();
    round_trip(&file);
    round_trip(&crate::build::SegmentFile {
        entry: file.clone(),
        records: vec![RecordSlot(0)],
    });
    round_trip(&PendingReference {
        source: RecordSlot(2),
        target_id: "target".into(),
        positions: vec![1, 4, 9],
    });
    round_trip(&ShardHeader {
        ordinal: 3,
        alphabet: crate::workload::BigramAlphabet::Ascii,
        role: 1,
        shared: true,
        source_id: "source".into(),
        source_version: 2,
        record_count: 10,
        file_count: 1,
        record_parses: 11,
        failed_parses: 0,
        has_inference: true,
    });
    round_trip(&PostingHeader {
        kind: LogicalType::String,
        multi: false,
        values_indexed: true,
        value_complete: true,
        length_complete: false,
        empty_string_complete: true,
        covered_records: 10,
        has_structural: true,
        custom_complete: Some(true),
    });
    round_trip(&SortColumnHeader {
        path: FieldPath::parse("name").unwrap(),
        kind: LogicalType::String,
        locale: "de".into(),
        options: SortOptions::default(),
        collation_version: "v1".into(),
    });
    round_trip(&RelationHeader {
        declaration: RelationDefinition {
            path: vec!["ref".into()],
            target_collection: "target".into(),
            cardinality: crate::relation::RelationCardinality::One,
            reverse_alias: None,
        },
        record_count: 10,
        reference_count: 20,
    });
    let report = crate::inference::InferenceReport {
        mode: crate::inference::InferenceMode::Disabled,
        policy: crate::inference::InferencePolicy::default(),
        eligible_records: 10,
        unavailable_records: 0,
        omitted_paths: 1,
        complete: true,
        observed_paths: 2,
        admitted_paths: 1,
        dropped_paths: 1,
        summary_bytes_estimate: 3,
        index_bytes_estimate: 4,
        retained_bytes_estimate: 7,
        fields: Vec::new(),
    };
    let bytes = bincode::serialize(&report).unwrap();
    let decoded: crate::inference::InferenceReport = bincode::deserialize(&bytes).unwrap();
    assert_eq!(format!("{decoded:?}"), format!("{report:?}"));
}

#[test]
fn shard_keys_sort_by_position() {
    for (collection, store, slot) in [(0u32, 0u64, 0u32), (1, 2, 3), (7, u64::MAX, u32::MAX - 1)] {
        let prefix = prefix(collection, store);
        let low = records_key(&prefix, slot);
        let high = records_key(&prefix, slot + 1);
        assert!(low < high);
        assert!(files_key(&prefix, slot) < files_key(&prefix, slot + 1));
        assert!(sort_key_key(&prefix, b"a", slot) < sort_key_key(&prefix, b"a", slot + 1));
    }
    assert!(prefix(0, 1) < prefix(1, 0));
    assert!(prefix(1, 1) < prefix(1, 2));
}

#[test]
fn posting_keys_group_chunks_of_one_key() {
    let prefix = prefix(0, 1);
    let mut keys = Vec::new();
    for key in [b"a".as_slice(), b"ab".as_slice(), b"b".as_slice()] {
        for chunk in [0u32, 1, 300] {
            keys.push(posting_key(&prefix, FAMILY_DECLARED_VALUES, key, key, chunk));
        }
    }
    keys.sort();
    let collected = keys
        .iter()
        .map(|key| {
            let (_, field, posting_key, chunk) = posting_parts(key).unwrap();
            (field.to_vec(), posting_key.to_vec(), chunk)
        })
        .collect::<Vec<_>>();
    assert_eq!(collected.len(), 9);
    for window in collected.windows(2) {
        if window[0].0 == window[1].0 {
            assert_eq!(window[0].1, window[1].1);
            assert!(window[0].2 < window[1].2);
        }
    }
    assert_eq!(
        collected
            .iter()
            .filter(|(field, _, _)| field == b"a")
            .count(),
        3
    );
}

#[test]
fn prefix_end_bounds_exactly_the_prefix() {
    assert_eq!(prefix_end(&[1, 2, 3]), Some(vec![1, 2, 4]));
    assert_eq!(prefix_end(&[0xFF]), None);
    assert_eq!(prefix_end(&[1, 0xFF]), Some(vec![2]));
    let prefix = prefix(3, 9);
    let end = prefix_end(&prefix).unwrap();
    assert!(prefix < end);
    assert!(shard_key(&prefix) < end);
    assert!(records_key(&prefix, u32::MAX) < end);
}
