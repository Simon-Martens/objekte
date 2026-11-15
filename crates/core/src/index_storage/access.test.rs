use std::collections::{BTreeMap, HashMap};

use roaring::RoaringBitmap;

use crate::bigram::BigramIndex;
use crate::execution_options::BigramAlphabet;
use crate::model::{LogicalType, Number};
use crate::posting::{PostingIndex, PostingStats, ScalarKey, StructuralFacts, StructuralKey};
use crate::query::Operator;

use super::access::{BigramAccess, PackedIndexCatalog, PostingAccess};
use super::backing::Backing;
use super::bigram_builder::{BigramBinding, BigramInput};
use super::format::{ObjectView, SectionKind};
use super::posting_builder::FieldInput;
use super::records::{FilePartInput, RecordInput};
use super::segment::FrozenSegment;
use super::segment_builder::{SegmentInput, build_segment};

const SLOTS: u32 = 9;

fn native_posting() -> PostingIndex {
    let mut values = BTreeMap::new();
    values.insert(ScalarKey::String("Alpha".into()), bitmap(&[0, 2]));
    values.insert(ScalarKey::String("Beta".into()), bitmap(&[1]));
    values.insert(ScalarKey::String(String::new()), bitmap(&[3]));
    values.insert(ScalarKey::String("sparse".into()), bitmap(&[4, 5, 6, 7, 8]));
    let mut lengths = BTreeMap::new();
    lengths.insert(Number::Integer(5), bitmap(&[0, 2]));
    lengths.insert(Number::Integer(4), bitmap(&[1]));
    let mut structural = BTreeMap::new();
    structural.insert(
        StructuralKey::String,
        bitmap(&(0..SLOTS).collect::<Vec<_>>()),
    );
    structural.insert(StructuralKey::Array, bitmap(&[3]));
    PostingIndex {
        path: vec!["title".into()],
        kind: LogicalType::String,
        multi: false,
        presence: bitmap(&(0..SLOTS).collect::<Vec<_>>()),
        empty_selections: bitmap(&[3]),
        nulls: bitmap(&[2]),
        structural: Some(StructuralFacts {
            postings: structural,
        }),
        values,
        lengths,
        values_indexed: true,
        value_complete: true,
        length_complete: true,
        empty_string_complete: true,
        covered_records: SLOTS,
        stats: PostingStats::default(),
    }
}

fn native_score() -> PostingIndex {
    let mut values = BTreeMap::new();
    for value in -2i128..=2 {
        let slots = (0..SLOTS)
            .filter(|slot| *slot as i128 % 5 == value + 2)
            .collect::<Vec<_>>();
        values.insert(ScalarKey::Number(Number::Integer(value)), bitmap(&slots));
    }
    PostingIndex {
        path: vec!["score".into()],
        kind: LogicalType::Number,
        multi: false,
        presence: bitmap(&(0..SLOTS).collect::<Vec<_>>()),
        empty_selections: RoaringBitmap::new(),
        nulls: bitmap(&[8]),
        structural: None,
        values,
        lengths: BTreeMap::new(),
        values_indexed: true,
        value_complete: true,
        length_complete: true,
        empty_string_complete: false,
        covered_records: SLOTS,
        stats: PostingStats::default(),
    }
}

fn native_bigram() -> BigramIndex {
    let mut normal = HashMap::new();
    normal.insert(1, bitmap(&[0, 1, 2]));
    normal.insert(2, bitmap(&[2, 3, 4]));
    normal.insert(3, bitmap(&[4, 5, 6, 7, 8]));
    normal.insert(4, bitmap(&[0, 5]));
    let mut skip = HashMap::new();
    skip.insert(1, bitmap(&[3, 4, 5]));
    BigramIndex {
        alphabet: BigramAlphabet::Ascii,
        record_count: SLOTS,
        normal,
        skip,
    }
}

fn packed_segment_bytes() -> Vec<u8> {
    let mut values = BTreeMap::new();
    values.insert(ScalarKey::String("Alpha".into()), vec![0, 2]);
    values.insert(ScalarKey::String("Beta".into()), vec![1]);
    values.insert(ScalarKey::String(String::new()), vec![3]);
    values.insert(ScalarKey::String("sparse".into()), vec![4, 5, 6, 7, 8]);
    let mut lengths = BTreeMap::new();
    lengths.insert(Number::Integer(5), vec![0, 2]);
    lengths.insert(Number::Integer(4), vec![1]);
    let mut structural = BTreeMap::new();
    structural.insert(2, (0..SLOTS).collect());
    structural.insert(4, vec![3]);
    let input = SegmentInput {
        collection: 0,
        source_id: "fixture".into(),
        source_version: 1,
        file_parts: vec![FilePartInput {
            file_id: 0,
            content_generation: 1,
            local_record_start: 0,
            record_count: SLOTS,
            segment_slot_start: 0,
        }],
        records: (0..SLOTS)
            .map(|slot| RecordInput {
                part_ordinal: 0,
                local_record_id: slot,
                byte_start: slot as u64,
                byte_end: slot as u64 + 1,
                logical_id: format!("id{slot}"),
                diagnostic_ref: 0,
                record_class: 0,
            })
            .collect(),
        identity_order: (0..SLOTS).collect(),
        fields: vec![
            FieldInput {
                ordinal: 0,
                path: vec!["title".into()],
                source: Default::default(),
                complete: true,
                kind: LogicalType::String,
                multi: false,
                presence: (0..SLOTS).collect(),
                nulls: vec![2],
                empty_selections: vec![3],
                empty_strings: vec![3],
                structural,
                values,
                lengths,
                values_indexed: true,
                value_complete: true,
                length_complete: true,
                empty_string_complete: true,
                covered_records: SLOTS,
            },
            {
                let mut values = BTreeMap::new();
                for value in -2i128..=2 {
                    let slots = (0..SLOTS)
                        .filter(|slot| *slot as i128 % 5 == value + 2)
                        .collect::<Vec<_>>();
                    values.insert(ScalarKey::Number(Number::Integer(value)), slots);
                }
                FieldInput {
                    ordinal: 1,
                    path: vec!["score".into()],
                    source: Default::default(),
                    complete: true,
                    kind: LogicalType::Number,
                    multi: false,
                    presence: (0..SLOTS).collect(),
                    nulls: vec![8],
                    empty_selections: Vec::new(),
                    empty_strings: Vec::new(),
                    structural: BTreeMap::new(),
                    values,
                    lengths: BTreeMap::new(),
                    values_indexed: true,
                    value_complete: true,
                    length_complete: true,
                    empty_string_complete: false,
                    covered_records: SLOTS,
                }
            },
        ],
        bigrams: vec![BigramInput {
            binding: BigramBinding::Raw,
            alphabet: 0,
            record_count: SLOTS,
            normal: vec![
                (1, vec![0, 1, 2]),
                (2, vec![2, 3, 4]),
                (3, vec![4, 5, 6, 7, 8]),
                (4, vec![0, 5]),
            ],
            skip: vec![(1, vec![3, 4, 5])],
        }],
        sorts: Vec::new(),
        relations: Vec::new(),
        logical_records: (0..SLOTS).collect(),
        empty_arrays: Vec::new(),
        active_records: (0..SLOTS).collect(),
        metadata: None,
        estimated_index_bytes: 0,
    };
    build_segment(input).unwrap().object.into_bytes()
}

fn bitmap(values: &[u32]) -> RoaringBitmap {
    values.iter().copied().collect()
}

fn compare_native_and_packed(native: &PostingIndex, packed: &FrozenSegment) {
    let native_access = PostingAccess::Native(native);
    let packed_access = PostingAccess::Packed(
        packed
            .postings()
            .field(&["title".to_owned()])
            .expect("title"),
    );
    assert_eq!(native_access.path(), packed_access.path());
    assert_eq!(native_access.kind(), packed_access.kind());
    assert_eq!(native_access.multi(), packed_access.multi());
    assert_eq!(
        native_access.covered_records(),
        packed_access.covered_records()
    );
    assert_eq!(
        native_access.presence().unwrap(),
        packed_access.presence().unwrap()
    );
    assert_eq!(
        native_access.nulls().unwrap(),
        packed_access.nulls().unwrap()
    );
    assert_eq!(
        native_access.empty_selections().unwrap(),
        packed_access.empty_selections().unwrap()
    );
    assert_eq!(
        native_access.empty_string().unwrap(),
        packed_access.empty_string().unwrap()
    );
    assert_eq!(
        native_access.structural(StructuralKey::String).unwrap(),
        packed_access.structural(StructuralKey::String).unwrap()
    );
    assert_eq!(
        native_access.structural(StructuralKey::Array).unwrap(),
        packed_access.structural(StructuralKey::Array).unwrap()
    );
    assert_eq!(
        native_access.distinct_values(),
        packed_access.distinct_values()
    );
    for key in [
        ScalarKey::String("Alpha".into()),
        ScalarKey::String("Beta".into()),
        ScalarKey::String("sparse".into()),
        ScalarKey::String(String::new()),
        ScalarKey::String("missing".into()),
    ] {
        assert_eq!(
            native_access.exact(&key).unwrap(),
            packed_access.exact(&key).unwrap(),
            "exact {key:?}"
        );
        assert_eq!(
            native_access.not_equal(&key).unwrap(),
            packed_access.not_equal(&key).unwrap(),
            "not equal {key:?}"
        );
        assert_eq!(
            native_access.value_cardinality(&key).unwrap(),
            packed_access.value_cardinality(&key).unwrap(),
            "cardinality {key:?}"
        );
        for operator in [
            Operator::Eq,
            Operator::Ne,
            Operator::Gt,
            Operator::Ge,
            Operator::Lt,
            Operator::Le,
        ] {
            assert_eq!(
                native_access.range(&key, operator).unwrap(),
                packed_access.range(&key, operator).unwrap(),
                "range {operator:?} {key:?}"
            );
        }
    }
    for length in [Number::Integer(4), Number::Integer(5), Number::Integer(9)] {
        assert_eq!(
            native_access.length(&length).unwrap(),
            packed_access.length(&length).unwrap()
        );
    }
    let mut native_values = Vec::new();
    native_access
        .for_each_value(|key, value| {
            native_values.push((key.clone(), value.clone()));
            Ok(())
        })
        .unwrap();
    let mut packed_values = Vec::new();
    packed_access
        .for_each_value(|key, value| {
            packed_values.push((key.clone(), value.clone()));
            Ok(())
        })
        .unwrap();
    assert_eq!(native_values, packed_values);
    let mut native_lengths = Vec::new();
    native_access
        .for_each_length(|length, value| {
            native_lengths.push((length.clone(), value.clone()));
            Ok(())
        })
        .unwrap();
    let mut packed_lengths = Vec::new();
    packed_access
        .for_each_length(|length, value| {
            packed_lengths.push((length.clone(), value.clone()));
            Ok(())
        })
        .unwrap();
    assert_eq!(native_lengths, packed_lengths);
}

fn compare_bigrams(native: &BigramIndex, packed: &FrozenSegment) {
    let native_access = BigramAccess::Native {
        index: native,
        binding: BigramBinding::Raw,
    };
    let packed_access = BigramAccess::Packed {
        section: packed.bigrams(),
        binding: BigramBinding::Raw,
    };
    assert_eq!(native_access.binding(), packed_access.binding());
    assert_eq!(native_access.alphabet(), packed_access.alphabet());
    assert_eq!(native_access.record_count(), packed_access.record_count());
    assert_eq!(
        native_access.normal_gram_count(),
        packed_access.normal_gram_count()
    );
    assert_eq!(
        native_access.skip_gram_count(),
        packed_access.skip_gram_count()
    );
    for gram in [1, 2, 3, 4, 99] {
        assert_eq!(
            native_access.normal(gram).unwrap(),
            packed_access.normal(gram).unwrap(),
            "normal {gram}"
        );
        assert_eq!(
            native_access.skip(gram).unwrap(),
            packed_access.skip(gram).unwrap(),
            "skip {gram}"
        );
    }
    for grams in [vec![1], vec![1, 2], vec![1, 2, 3], vec![1, 99], vec![4]] {
        assert_eq!(
            native_access.union_normal(&grams).unwrap(),
            packed_access.union_normal(&grams).unwrap(),
            "union {grams:?}"
        );
        assert_eq!(
            native_access.intersect_normal(&grams).unwrap(),
            packed_access.intersect_normal(&grams).unwrap(),
            "intersection {grams:?}"
        );
        assert_eq!(
            native_access.intersection_count_normal(&grams).unwrap(),
            packed_access.intersection_count_normal(&grams).unwrap(),
            "count {grams:?}"
        );
        assert_eq!(
            native_access.union_skip(&grams).unwrap(),
            packed_access.union_skip(&grams).unwrap(),
            "skip union {grams:?}"
        );
    }
}

#[test]
fn native_packed_resident_and_mapped_access_agree() {
    let bytes = packed_segment_bytes();
    let resident = FrozenSegment::from_bytes(bytes.clone()).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("segment.obx");
    std::fs::write(&path, &bytes).unwrap();
    let mapped = FrozenSegment::open_path(&path).unwrap();
    let native_posting = native_posting();
    let native_bigram = native_bigram();
    compare_native_and_packed(&native_posting, &resident);
    compare_native_and_packed(&native_posting, &mapped);
    compare_bigrams(&native_bigram, &resident);
    compare_bigrams(&native_bigram, &mapped);
    let schema = crate::reference::Schema::default();
    let catalog = PackedIndexCatalog::new(&resident, &schema);
    let catalog_access = catalog.field_postings(&["title".to_owned()]).unwrap();
    assert_eq!(
        PostingAccess::Native(&native_posting)
            .exact(&ScalarKey::String("Alpha".into()))
            .unwrap(),
        catalog_access
            .exact(&ScalarKey::String("Alpha".into()))
            .unwrap()
    );
    assert_eq!(
        catalog.logical_records().unwrap(),
        bitmap(&(0..SLOTS).collect::<Vec<_>>())
    );
    let catalog_bigram = catalog.bigram(&BigramBinding::Raw).unwrap();
    assert_eq!(
        catalog_bigram.union_normal(&[1, 2]).unwrap(),
        BigramAccess::Native {
            index: &native_bigram,
            binding: BigramBinding::Raw
        }
        .union_normal(&[1, 2])
        .unwrap()
    );

    let native_score = native_score();
    let resident_score = PostingAccess::Packed(
        resident
            .postings()
            .field(&["score".to_owned()])
            .expect("score"),
    );
    let mapped_score = PostingAccess::Packed(
        mapped
            .postings()
            .field(&["score".to_owned()])
            .expect("score"),
    );
    let native_score_access = PostingAccess::Native(&native_score);
    for value in -3i128..=3 {
        let key = ScalarKey::Number(Number::Integer(value));
        assert_eq!(
            native_score_access.exact(&key).unwrap(),
            resident_score.exact(&key).unwrap()
        );
        assert_eq!(
            resident_score.exact(&key).unwrap(),
            mapped_score.exact(&key).unwrap()
        );
        for operator in [
            Operator::Eq,
            Operator::Ne,
            Operator::Gt,
            Operator::Ge,
            Operator::Lt,
            Operator::Le,
        ] {
            assert_eq!(
                native_score_access.range(&key, operator).unwrap(),
                resident_score.range(&key, operator).unwrap(),
                "numeric range {operator:?} {value}"
            );
            assert_eq!(
                resident_score.range(&key, operator).unwrap(),
                mapped_score.range(&key, operator).unwrap()
            );
        }
    }
}

#[test]
fn corrupt_unused_arena_is_reported_only_when_read() {
    let mut bytes = packed_segment_bytes();
    let backing = Backing::resident(bytes.clone());
    let view = ObjectView::new(&backing).unwrap();
    let arena = view.required_section(SectionKind::StringArena).unwrap();
    drop(view);
    let last = (arena.offset + arena.length - 1) as usize;
    bytes[last] = 0xff;
    let packed = FrozenSegment::from_bytes(bytes).expect("admission must not validate payloads");
    // An unrelated posting lookup remains valid.
    let title = packed.postings().field(&["title".to_owned()]).unwrap();
    assert_eq!(
        title
            .value(&ScalarKey::String("Alpha".into()))
            .unwrap()
            .unwrap()
            .cardinality(),
        2
    );
    // Reading every record row eventually touches the corrupt string arena.
    let mut saw_error = false;
    for slot in 0..SLOTS {
        if packed.records().record(slot).is_err() {
            saw_error = true;
            break;
        }
    }
    assert!(saw_error, "corrupt string arena was not reported");
}
