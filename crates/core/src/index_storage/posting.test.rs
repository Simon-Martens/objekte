use std::collections::BTreeMap;
use std::ops::Bound::{Excluded, Included, Unbounded};

use roaring::RoaringBitmap;

use crate::model::{LogicalType, Number};
use crate::posting::ScalarKey;
use crate::query::Operator;

use super::bigram_builder::{BigramBinding, BigramInput};
use super::posting_builder::FieldInput;
use super::records::FilePartInput;
use super::segment::FrozenSegment;
use super::segment_builder::{SegmentInput, build_segment};

fn scalar_segment() -> (FrozenSegment, BTreeMap<ScalarKey, Vec<u32>>) {
    let slot_count = 80u32;
    let mut values: BTreeMap<ScalarKey, Vec<u32>> = BTreeMap::new();
    for value in -10i128..=10 {
        let slots = (0..slot_count)
            .filter(|slot| *slot as i128 % 21 == value + 10)
            .collect::<Vec<_>>();
        values.insert(ScalarKey::Number(Number::Integer(value)), slots);
    }
    let mut lengths = BTreeMap::new();
    for length in 0i128..=5 {
        let slots = (0..slot_count)
            .filter(|slot| *slot as i128 % 6 == length)
            .collect::<Vec<_>>();
        lengths.insert(Number::Integer(length), slots);
    }
    let presence = (0..slot_count).collect::<Vec<_>>();
    let nulls = (0..slot_count)
        .filter(|slot| slot % 7 == 0)
        .collect::<Vec<_>>();
    let mut sparse_values = BTreeMap::new();
    sparse_values.insert(
        ScalarKey::String("sparse".into()),
        vec![0, 5, 10, 15, 20, 25],
    );
    let sparse = FieldInput {
        ordinal: 1,
        path: vec!["sparse".into()],
        kind: LogicalType::String,
        multi: false,
        presence: vec![0, 5, 10, 15, 20, 25],
        nulls: Vec::new(),
        empty_selections: Vec::new(),
        empty_strings: Vec::new(),
        structural: BTreeMap::new(),
        values: sparse_values,
        lengths: BTreeMap::new(),
        values_indexed: true,
        value_complete: true,
        length_complete: true,
        empty_string_complete: false,
        covered_records: slot_count,
    };
    let field = FieldInput {
        ordinal: 0,
        path: vec!["value".into()],
        kind: LogicalType::Number,
        multi: false,
        presence: presence.clone(),
        nulls: nulls.clone(),
        empty_selections: Vec::new(),
        empty_strings: Vec::new(),
        structural: BTreeMap::new(),
        values: values.clone(),
        lengths: lengths.clone(),
        values_indexed: true,
        value_complete: true,
        length_complete: true,
        empty_string_complete: false,
        covered_records: slot_count,
    };
    let normal = vec![
        (1u64, vec![0, 1, 2, 3, 4]),
        (2u64, vec![2, 3, 4, 5, 6]),
        (3u64, vec![4, 5]),
        (4u64, vec![0, 5, 10, 15, 20, 25]),
    ];
    let skip = vec![(1u64, vec![3, 4, 5])];
    let input = SegmentInput {
        collection: 0,
        source_id: "fixture".into(),
        source_version: 1,
        file_parts: vec![FilePartInput {
            file_id: 1,
            content_generation: 1,
            local_record_start: 0,
            record_count: slot_count,
            segment_slot_start: 0,
        }],
        records: (0..slot_count)
            .map(|slot| super::records::RecordInput {
                part_ordinal: 0,
                local_record_id: slot,
                byte_start: slot as u64 * 10,
                byte_end: slot as u64 * 10 + 10,
                logical_id: format!("id{slot:03}"),
                diagnostic_ref: 0,
                record_class: 0,
            })
            .collect(),
        identity_order: (0..slot_count).collect(),
        fields: vec![field, sparse],
        bigrams: vec![BigramInput {
            binding: BigramBinding::Raw,
            alphabet: 0,
            record_count: slot_count,
            normal,
            skip,
        }],
        sorts: Vec::new(),
        relations: Vec::new(),
        logical_records: presence.clone(),
        empty_arrays: Vec::new(),
        active_records: presence,
        estimated_index_bytes: 0,
    };
    let built = build_segment(input).unwrap();
    (
        FrozenSegment::from_bytes(built.object.into_bytes()).unwrap(),
        values,
    )
}

fn reference_posting(
    values: &BTreeMap<ScalarKey, Vec<u32>>,
    key: &ScalarKey,
    operator: Operator,
) -> RoaringBitmap {
    let same_type =
        |candidate: &ScalarKey| std::mem::discriminant(candidate) == std::mem::discriminant(key);
    let union = |iterator: &mut dyn Iterator<Item = (&ScalarKey, &Vec<u32>)>| {
        let mut result = RoaringBitmap::new();
        for (_, slots) in iterator.take_while(|(candidate, _)| same_type(candidate)) {
            result.extend(slots.iter().copied());
        }
        result
    };
    match operator {
        Operator::Eq => values
            .get(key)
            .map_or_else(RoaringBitmap::new, |slots| slots.iter().copied().collect()),
        Operator::Ne => {
            let mut result = RoaringBitmap::new();
            for slots in values.values() {
                result.extend(slots.iter().copied());
            }
            let nulls = (0u32..80)
                .filter(|slot| slot % 7 == 0)
                .collect::<RoaringBitmap>();
            result -= nulls;
            if let Some(equal) = values.get(key) {
                result -= equal.iter().copied().collect::<RoaringBitmap>();
            }
            result
        }
        Operator::Gt => union(&mut values.range((Excluded(key.clone()), Unbounded))),
        Operator::Ge => union(&mut values.range((Included(key.clone()), Unbounded))),
        Operator::Lt => union(&mut values.range((Unbounded, Excluded(key.clone()))).rev()),
        Operator::Le => union(&mut values.range((Unbounded, Included(key.clone()))).rev()),
        _ => RoaringBitmap::new(),
    }
}

#[test]
fn packed_scalar_queries_match_btree_reference() {
    let (segment, values) = scalar_segment();
    let field = segment
        .postings()
        .field(&["value".to_owned()])
        .expect("field");
    for value in -12i128..=12 {
        let key = ScalarKey::Number(Number::Integer(value));
        for operator in [
            Operator::Eq,
            Operator::Ne,
            Operator::Gt,
            Operator::Ge,
            Operator::Lt,
            Operator::Le,
        ] {
            let packed = field.range(&key, operator).unwrap();
            let reference = reference_posting(&values, &key, operator);
            assert_eq!(
                packed, reference,
                "operator {operator:?} at numeric key {value}"
            );
        }
    }
}

#[test]
fn packed_ranges_report_visited_keys_and_respect_restriction() {
    let (segment, _values) = scalar_segment();
    let field = segment.postings().field(&["value".to_owned()]).unwrap();
    let key = ScalarKey::Number(Number::Integer(0));
    let mut restriction = RoaringBitmap::new();
    restriction.insert_range(0..20);
    let (restricted, visited) = field
        .range_restricted(&key, Operator::Ge, Some(&restriction))
        .unwrap();
    assert!(visited > 0);
    let all = field.range(&key, Operator::Ge).unwrap();
    assert_eq!(restricted, &all & &restriction);
}

#[test]
fn packed_value_and_length_keys_enumerate_order_and_counts() {
    let (segment, values) = scalar_segment();
    let field = segment.postings().field(&["value".to_owned()]).unwrap();
    let keys = field.value_keys(Unbounded, Unbounded, false).unwrap();
    assert_eq!(keys.len(), 21);
    assert_eq!(
        keys.first().unwrap().0,
        ScalarKey::Number(Number::Integer(-10))
    );
    assert_eq!(
        keys.last().unwrap().0,
        ScalarKey::Number(Number::Integer(10))
    );
    assert!(keys.iter().all(|(_, count)| *count > 0));
    let reverse = field.value_keys(Unbounded, Unbounded, true).unwrap();
    assert_eq!(
        reverse.first().unwrap().0,
        ScalarKey::Number(Number::Integer(10))
    );
    let lengths = field.length_keys(Unbounded, Unbounded).unwrap();
    assert_eq!(lengths.len(), 6);
    assert_eq!(lengths.first().unwrap().0, Number::Integer(0));
}

#[test]
fn packed_bigram_candidates_match_reference_sets() {
    let (segment, values) = scalar_segment();
    let raw = segment
        .bigrams()
        .binding(super::bigram_builder::BigramBindingKind::Raw)
        .unwrap();
    let normal = [
        (1u64, vec![0u32, 1, 2, 3, 4]),
        (2, vec![2, 3, 4, 5, 6]),
        (3, vec![4, 5]),
    ];
    let mut expected_union = RoaringBitmap::new();
    for (_, slots) in &normal {
        expected_union.extend(slots.iter().copied());
    }
    assert_eq!(raw.union_normal(&[1, 2, 3]).unwrap(), expected_union);
    let expected_intersection = normal
        .iter()
        .filter(|(gram, _)| *gram != 3)
        .map(|(_, slots)| slots.iter().copied().collect::<RoaringBitmap>())
        .reduce(|left, right| &left & &right)
        .unwrap();
    assert_eq!(
        raw.intersect_normal(&[1, 2]).unwrap().unwrap(),
        expected_intersection
    );
    assert_eq!(
        raw.intersection_count_normal(&[1, 2]).unwrap(),
        expected_intersection.len()
    );
    assert_eq!(raw.intersection_count_normal(&[1, 2, 4]).unwrap(), 0);
    assert_eq!(
        raw.union_skip(&[1]).unwrap(),
        [3u32, 4, 5].into_iter().collect::<RoaringBitmap>()
    );
    assert_eq!(
        raw.normal(4)
            .unwrap()
            .unwrap()
            .ids()
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        vec![0, 5, 10, 15, 20, 25]
    );
    let sparse = segment.postings().field(&["sparse".to_owned()]).unwrap();
    assert_eq!(
        sparse
            .value(&ScalarKey::String("sparse".into()))
            .unwrap()
            .unwrap()
            .ids()
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        vec![0, 5, 10, 15, 20, 25]
    );
    sparse.verify_full().unwrap();
}

#[test]
fn missing_gram_is_an_empty_candidate_domain() {
    let (segment, values) = scalar_segment();
    let raw = segment
        .bigrams()
        .binding(super::bigram_builder::BigramBindingKind::Raw)
        .unwrap();
    assert!(raw.normal(999).unwrap().is_none());
    assert_eq!(
        raw.intersect_normal(&[1, 999]).unwrap().unwrap(),
        RoaringBitmap::new()
    );
    assert_eq!(raw.intersection_count_normal(&[1, 999]).unwrap(), 0);
}
