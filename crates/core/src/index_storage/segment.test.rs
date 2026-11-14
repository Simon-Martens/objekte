use std::collections::BTreeMap;

use crate::model::{LogicalType, Number};
use crate::posting::ScalarKey;
use crate::sorting::{Placement, SortKey, SortOptions};

use super::bigram_builder::{BigramBinding, BigramInput};
use super::posting_builder::FieldInput;
use super::records::{FilePartInput, RecordInput};
use super::relations::{RelationEdgeInput, RelationInput};
use super::segment::FrozenSegment;
use super::segment_builder::{build_segment, SegmentInput};
use super::sort::SortColumnInput;

fn fixture() -> SegmentInput {
    let mut title_values = BTreeMap::new();
    title_values.insert(ScalarKey::String("Alpha".into()), vec![0, 2]);
    title_values.insert(ScalarKey::String("Beta".into()), vec![1]);
    let mut lengths = BTreeMap::new();
    lengths.insert(Number::Integer(5), vec![0, 2]);
    lengths.insert(Number::Integer(4), vec![1]);
    let mut structural = BTreeMap::new();
    structural.insert(3, vec![0, 1, 2]);
    structural.insert(2, vec![0, 2]);
    let mut count_values = BTreeMap::new();
    count_values.insert(ScalarKey::Number(Number::Integer(1)), vec![0]);
    count_values.insert(ScalarKey::Number(Number::Integer(2)), vec![1]);
    count_values.insert(ScalarKey::Number(Number::Integer(3)), vec![2]);
    let mut flag_values = BTreeMap::new();
    flag_values.insert(ScalarKey::Bool(true), vec![0, 1]);
    flag_values.insert(ScalarKey::Bool(false), vec![2]);
    let fields = vec![
        FieldInput {
            ordinal: 0,
            path: vec!["title".into()],
            kind: LogicalType::String,
            multi: false,
            presence: vec![0, 1, 2],
            nulls: vec![2],
            empty_selections: vec![1],
            empty_strings: vec![1],
            structural,
            values: title_values,
            lengths,
            values_indexed: true,
            value_complete: true,
            length_complete: true,
            empty_string_complete: true,
            covered_records: 3,
        },
        FieldInput {
            ordinal: 1,
            path: vec!["count".into()],
            kind: LogicalType::Number,
            multi: false,
            presence: vec![0, 1, 2],
            nulls: Vec::new(),
            empty_selections: Vec::new(),
            empty_strings: Vec::new(),
            structural: BTreeMap::new(),
            values: count_values,
            lengths: BTreeMap::new(),
            values_indexed: true,
            value_complete: true,
            length_complete: true,
            empty_string_complete: false,
            covered_records: 3,
        },
        FieldInput {
            ordinal: 2,
            path: vec!["flag".into()],
            kind: LogicalType::Bool,
            multi: false,
            presence: vec![0, 1, 2],
            nulls: Vec::new(),
            empty_selections: Vec::new(),
            empty_strings: Vec::new(),
            structural: BTreeMap::new(),
            values: flag_values,
            lengths: BTreeMap::new(),
            values_indexed: true,
            value_complete: true,
            length_complete: true,
            empty_string_complete: false,
            covered_records: 3,
        },
    ];
    let sorts = vec![SortColumnInput {
        path: vec!["title".into()],
        kind: LogicalType::String,
        locale: "und".into(),
        options: SortOptions {
            nulls: Placement::Last,
            missing: Placement::Last,
            ..SortOptions::default()
        },
        collation_version: "test-collation".into(),
        keys: vec![
            SortKey::String(b"Alpha".to_vec()),
            SortKey::String(b"Beta".to_vec()),
            SortKey::Missing,
        ],
        ascending: vec![0, 2, 1],
        descending: vec![1, 2, 0],
    }];
    SegmentInput {
        collection: 1,
        source_id: "text".into(),
        source_version: 1,
        file_parts: vec![
            FilePartInput {
                file_id: 1,
                content_generation: 1,
                local_record_start: 0,
                record_count: 2,
                segment_slot_start: 0,
            },
            FilePartInput {
                file_id: 2,
                content_generation: 2,
                local_record_start: 0,
                record_count: 1,
                segment_slot_start: 2,
            },
        ],
        records: vec![
            RecordInput {
                part_ordinal: 0,
                local_record_id: 0,
                byte_start: 0,
                byte_end: 10,
                logical_id: "a".into(),
                diagnostic_ref: 0,
                record_class: 0,
            },
            RecordInput {
                part_ordinal: 0,
                local_record_id: 1,
                byte_start: 10,
                byte_end: 20,
                logical_id: "b".into(),
                diagnostic_ref: 0,
                record_class: 0,
            },
            RecordInput {
                part_ordinal: 1,
                local_record_id: 0,
                byte_start: 0,
                byte_end: 30,
                logical_id: "c".into(),
                diagnostic_ref: 0,
                record_class: 1,
            },
        ],
        identity_order: vec![0, 1, 2],
        fields,
        bigrams: vec![BigramInput {
            binding: BigramBinding::Raw,
            alphabet: 0,
            record_count: 3,
            normal: vec![(0x4142, vec![0, 2]), (0x4344, vec![1])],
            skip: vec![(0x4142, vec![1])],
        }],
        sorts,
        relations: vec![RelationInput {
            id_ordinal: 0,
            target_collection: 2,
            path: vec!["parent".into()],
            cardinality: 0,
            reverse_alias: Some("children".into()),
            offsets: vec![0, 1, 1, 1],
            edges: vec![RelationEdgeInput {
                source_slot: 0,
                target_slot: Some(1),
                target_id: "b".into(),
                positions: vec![0],
            }],
            present: vec![0],
            nulls: vec![1],
            empty_arrays: vec![2],
            unavailable: Vec::new(),
            unresolved_count: 0,
        }],
        logical_records: vec![0, 1, 2],
        empty_arrays: vec![2],
        estimated_index_bytes: 1024,
    }
}

fn segment() -> FrozenSegment {
    let built = build_segment(fixture()).unwrap();
    FrozenSegment::from_bytes(built.object.into_bytes()).unwrap()
}

#[test]
fn record_table_reads_slots_parts_and_logical_ids() {
    let segment = segment();
    assert_eq!(segment.stats().record_count, 3);
    assert_eq!(segment.stats().file_part_count, 2);
    assert_eq!(segment.stats().source_id, "text");
    assert_eq!(segment.records().file_part(0).unwrap().unwrap().file_id, 1);
    assert_eq!(segment.records().file_part(1).unwrap().unwrap().file_id, 2);
    assert_eq!(segment.records().logical_id(0).unwrap().unwrap(), "a");
    assert_eq!(segment.records().slot_for_logical_id("b").unwrap(), Some(1));
    assert_eq!(segment.records().slot_for_logical_id("missing").unwrap(), None);
    assert_eq!(segment.records().identity_order().unwrap(), vec![0, 1, 2]);
    let record = segment.records().record(2).unwrap().unwrap();
    assert_eq!(record.byte_end, 30);
    assert_eq!(record.record_class, 1);
    assert_eq!(record.part_ordinal, 1);
}

#[test]
fn typed_postings_query_entirely_through_readers() {
    let segment = segment();
    let title = segment
        .postings()
        .field(&["title".to_owned()])
        .expect("title field");
    assert_eq!(title.presence().unwrap().cardinality(), 3);
    assert_eq!(title.nulls().unwrap().ids().collect::<Result<Vec<_>, _>>().unwrap(), vec![2]);
    assert_eq!(
        title
            .value(&ScalarKey::String("Alpha".into()))
            .unwrap()
            .unwrap()
            .ids()
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        vec![0, 2]
    );
    assert!(title.contains_value(1, &ScalarKey::String("Beta".into())).unwrap());
    assert!(!title.contains_value(2, &ScalarKey::String("Beta".into())).unwrap());
    assert_eq!(title.value_cardinality(&ScalarKey::String("Alpha".into())).unwrap(), 2);
    assert_eq!(
        title.structural(3).unwrap().unwrap().ids().collect::<Result<Vec<_>, _>>().unwrap(),
        vec![0, 1, 2]
    );
    assert_eq!(
        title.length(&Number::Integer(5)).unwrap().unwrap().cardinality(),
        2
    );
    assert_eq!(title.values().entry_count(), 2);
    let count = segment.postings().field_by_ordinal(1).unwrap();
    assert!(count.contains_value(1, &ScalarKey::Number(Number::Integer(2))).unwrap());
    let flag = segment.postings().field_by_ordinal(2).unwrap();
    assert_eq!(
        flag.value(&ScalarKey::Bool(true)).unwrap().unwrap().cardinality(),
        2
    );
}

#[test]
fn bigram_sort_relation_and_coverage_readers_work() {
    let segment = segment();
    let raw = segment.bigrams().binding(super::bigram_builder::BigramBindingKind::Raw).unwrap();
    assert_eq!(raw.alphabet(), 0);
    assert_eq!(
        raw.normal(0x4142).unwrap().unwrap().ids().collect::<Result<Vec<_>, _>>().unwrap(),
        vec![0, 2]
    );
    assert_eq!(
        raw.skip(0x4142).unwrap().unwrap().ids().collect::<Result<Vec<_>, _>>().unwrap(),
        vec![1]
    );
    assert_eq!(raw.normal_gram_count(), 2);

    let sorting = segment.sorting();
    let column = sorting.column(&["title".to_owned()]).unwrap();
    assert_eq!(column.locale(), "und");
    assert_eq!(column.ascending_slots().unwrap(), vec![0, 2, 1]);
    assert_eq!(column.descending_slots().unwrap(), vec![1, 2, 0]);
    assert_eq!(column.key_at(0).unwrap().unwrap(), SortKey::String(b"Alpha".to_vec()));
    assert_eq!(column.key_at(2).unwrap().unwrap(), SortKey::Missing);

    let relation = segment.relations().relation(0).unwrap();
    assert_eq!(relation.path, vec!["parent".to_owned()]);
    assert_eq!(relation.reverse_alias.as_deref(), Some("children"));
    let edges = relation.edges(0).unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].target_id, "b");
    assert_eq!(edges[0].target_slot, Some(1));
    assert_eq!(edges[0].positions, vec![0]);
    assert!(relation.edges(1).unwrap().is_empty());
    assert_eq!(relation.present().unwrap().cardinality(), 1);
    assert_eq!(relation.nulls().unwrap().cardinality(), 1);

    let coverage = segment.coverage();
    assert_eq!(coverage.logical_records().unwrap().cardinality(), 3);
    assert_eq!(
        coverage.empty_arrays().unwrap().ids().collect::<Result<Vec<_>, _>>().unwrap(),
        vec![2]
    );
}

#[test]
fn full_verification_and_mapped_parity() {
    let built = build_segment(fixture()).unwrap();
    let bytes = built.object.into_bytes();
    let resident = FrozenSegment::from_bytes(bytes.clone()).unwrap();
    resident.verify_full().unwrap();

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("segment.obx");
    std::fs::write(&path, &bytes).unwrap();
    let mapped = FrozenSegment::open_path(&path).unwrap();
    mapped.verify_full().unwrap();
    assert_eq!(
        mapped
            .postings()
            .field(&["title".to_owned()])
            .unwrap()
            .value(&ScalarKey::String("Alpha".into()))
            .unwrap()
            .unwrap()
            .ids()
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        vec![0, 2]
    );
    assert_eq!(
        mapped
            .bigrams()
            .binding(super::bigram_builder::BigramBindingKind::Raw)
            .unwrap()
            .normal(0x4142)
            .unwrap()
            .unwrap()
            .cardinality(),
        2
    );
}

#[test]
fn corrupt_or_truncated_segment_fails_to_open() {
    let built = build_segment(fixture()).unwrap();
    let mut bytes = built.object.into_bytes();
    bytes.truncate(200);
    assert!(FrozenSegment::from_bytes(bytes).is_err());

    let built = build_segment(fixture()).unwrap();
    let mut bytes = built.object.into_bytes();
    let at = bytes.len() - 20;
    bytes[at] ^= 0xff;
    let segment = FrozenSegment::from_bytes(bytes).unwrap();
    assert!(segment.verify_full().is_err());
}
