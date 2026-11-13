use super::*;
use crate::model::LogicalType;
use crate::model::Number;
use crate::posting::{PostingIndex, ScalarKey};
use crate::query::parse_filter;
use crate::reference::{BoundExpr, FieldDefinition, Schema, bind_filter};
use crate::segment_format::SegmentWriter;
use std::io::Cursor;

fn seal<K: Ord + Clone + Serialize + DeserializeOwned>(
    map: &BTreeMap<K, RoaringBitmap>,
    allowed: RoaringBitmap,
) -> SealedPostingDictionary<K> {
    let mut pack = PostingPack::new();
    let mut dictionaries = Vec::new();
    let span = append_dictionary(&mut dictionaries, &mut pack, map).unwrap();
    let mut writer = SegmentWriter::new(Cursor::new(Vec::new()), 2).unwrap();
    writer
        .write_section(SectionKind::ScalarDictionaries, |target| {
            target
                .write_all(&dictionaries)
                .map_err(|error| error.to_string())
        })
        .unwrap();
    writer
        .write_section(SectionKind::PostingPayloads, |target| {
            target
                .write_all(pack.bytes())
                .map_err(|error| error.to_string())
        })
        .unwrap();
    let reader = Arc::new(
        SegmentReader::from_owned(Arc::from(writer.finish().unwrap().into_inner())).unwrap(),
    );
    SealedPostingDictionary::open(
        reader,
        SectionKind::ScalarDictionaries,
        span,
        Arc::new(allowed),
    )
    .unwrap()
}

#[test]
fn equality_bounds_and_restriction_match_builder_map_across_leaves() {
    let mut map = BTreeMap::new();
    for number in 0..4000 {
        map.insert(
            ScalarKey::String(format!("key-{number:05}")),
            RoaringBitmap::from_iter([number % 1000, (number + 17) % 1000]),
        );
    }
    let sealed = seal(&map, RoaringBitmap::from_iter(0..1000));
    assert_eq!(sealed.len(), map.len());
    for number in [0, 1, 1500, 3999] {
        let key = ScalarKey::String(format!("key-{number:05}"));
        assert_eq!(sealed.get(&key).unwrap().as_ref(), map.get(&key));
        assert_eq!(sealed.estimated_cardinality(&key).unwrap(), Some(2));
    }
    assert_eq!(
        sealed.get(&ScalarKey::String("key-04000".into())).unwrap(),
        None
    );
    let lower = ScalarKey::String("key-01995".into());
    let upper = ScalarKey::String("key-02005".into());
    let restricted = RoaringBitmap::from_iter([12, 13, 14, 15, 16, 17, 18]);
    let mut expected = RoaringBitmap::new();
    for (_, posting) in map.range((Bound::Excluded(&lower), Bound::Included(&upper))) {
        expected |= posting & &restricted;
    }
    assert_eq!(
        sealed
            .union_bounds(
                Bound::Excluded(&lower),
                Bound::Included(&upper),
                Some(&restricted)
            )
            .unwrap(),
        expected
    );
    assert_eq!(
        sealed.get_restricted(&lower, &restricted).unwrap().unwrap(),
        map.get(&lower).unwrap() & &restricted
    );
    let lower = ScalarKey::String("key-00991".into());
    let upper = ScalarKey::String("key-03107".into());
    for descending in [false, true] {
        let mut cursor = sealed
            .cursor_range(
                Bound::Excluded(lower.clone()),
                Bound::Included(upper.clone()),
                descending,
            )
            .unwrap();
        let mut actual = Vec::new();
        while let Some(entry) = cursor.next().unwrap() {
            actual.push(entry);
        }
        let expected = map.range((Bound::Excluded(&lower), Bound::Included(&upper)));
        let expected = if descending {
            expected
                .rev()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Vec<_>>()
        } else {
            expected
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(actual, expected);
    }
    let mut empty = sealed
        .cursor_range(
            Bound::Unbounded,
            Bound::Excluded(ScalarKey::String("key-00000".into())),
            true,
        )
        .unwrap();
    assert!(empty.next().unwrap().is_none());
}

#[test]
fn exact_numbers_empty_posting_and_negative_lookup_remain_distinct() {
    let values = [
        Number::parse_json_literal("-100000000000000000000000000000000000000").unwrap(),
        Number::integer(-1),
        Number::parse_json_literal("0.00000000000000000000000000001").unwrap(),
        Number::integer(1),
        Number::parse_json_literal("100000000000000000000000000000000000000").unwrap(),
    ];
    let mut map = BTreeMap::new();
    for (position, value) in values.iter().enumerate() {
        map.insert(
            ScalarKey::Number(value.clone()),
            RoaringBitmap::from_iter([position as u32]),
        );
    }
    let empty_key = ScalarKey::String("present but empty".into());
    map.insert(empty_key.clone(), RoaringBitmap::new());
    let sealed = seal(&map, RoaringBitmap::from_iter(0..5));
    assert_eq!(sealed.get(&empty_key).unwrap(), Some(RoaringBitmap::new()));
    assert_eq!(sealed.estimated_cardinality(&empty_key).unwrap(), Some(0));
    assert_eq!(
        sealed.get(&ScalarKey::Number(Number::integer(0))).unwrap(),
        None
    );
    let lower = ScalarKey::Number(Number::integer(-1));
    let upper = ScalarKey::Number(Number::integer(1));
    assert_eq!(
        sealed
            .union_bounds(Bound::Included(&lower), Bound::Excluded(&upper), None)
            .unwrap(),
        RoaringBitmap::from_iter([1, 2])
    );
}

#[test]
fn malformed_posting_is_an_error_not_an_empty_result() {
    let mut map = BTreeMap::new();
    map.insert(
        ScalarKey::String("exists".into()),
        RoaringBitmap::from_iter([1, 4]),
    );
    let mut pack = PostingPack::new();
    let mut dictionary = Vec::new();
    let span = append_dictionary(&mut dictionary, &mut pack, &map).unwrap();
    let mut writer = SegmentWriter::new(Cursor::new(Vec::new()), 2).unwrap();
    writer
        .write_section(SectionKind::ScalarDictionaries, |out| {
            out.write_all(&dictionary).map_err(|e| e.to_string())
        })
        .unwrap();
    writer
        .write_section(SectionKind::PostingPayloads, |out| {
            out.write_all(pack.bytes()).map_err(|e| e.to_string())
        })
        .unwrap();
    let mut bytes = writer.finish().unwrap().into_inner();
    *bytes.last_mut().unwrap() ^= 1;
    let reader = Arc::new(SegmentReader::from_owned(Arc::from(bytes)).unwrap());
    let sealed = SealedPostingDictionary::open(
        Arc::clone(&reader),
        SectionKind::ScalarDictionaries,
        span,
        Arc::new(RoaringBitmap::from_iter(0..10)),
    )
    .unwrap();
    assert!(sealed.get(&ScalarKey::String("exists".into())).is_err());
    assert!(sealed.get(&ScalarKey::String("exists".into())).is_err());
    assert_eq!(
        sealed.get(&ScalarKey::String("absent".into())).unwrap(),
        None
    );

    let field = FieldDefinition {
        path: vec!["title".into()],
        kind: LogicalType::String,
        required: false,
        nullable: false,
        indexed: true,
        multi: false,
        element_nullable: Vec::new(),
    };
    let mut schema = Schema::default();
    schema.register_field(field.clone()).unwrap();
    let mut index = PostingIndex::new(&field);
    index.presence = RoaringBitmap::from_iter([1, 4]);
    index.values = map;
    index.covered_records = 10;
    index.finish_stats();
    let lengths = seal(&index.lengths, RoaringBitmap::from_iter(0..10));
    let sealed = SealedPostingDictionary::open(
        reader,
        SectionKind::ScalarDictionaries,
        span,
        Arc::new(RoaringBitmap::from_iter(0..10)),
    )
    .unwrap();
    index.install_sealed(sealed, lengths).unwrap();
    let BoundExpr::Term(term) =
        bind_filter(&parse_filter("title = 'exists'").unwrap(), &schema).unwrap()
    else {
        panic!("expected term");
    };
    assert!(
        index
            .exact_set_checked(&term, &RoaringBitmap::from_iter(0..10))
            .is_err()
    );
}

#[test]
fn posting_index_releases_builder_maps_and_materializes_for_compaction() {
    let field = FieldDefinition {
        path: vec!["title".into()],
        kind: LogicalType::String,
        required: false,
        nullable: false,
        indexed: true,
        multi: false,
        element_nullable: Vec::new(),
    };
    let mut index = PostingIndex::new(&field);
    let key = ScalarKey::String("alpha".into());
    let slots = RoaringBitmap::from_iter([0]);
    index.presence = slots.clone();
    index.values.insert(key.clone(), slots.clone());
    index.covered_records = 1;
    index.finish_stats();
    let values = seal(&index.values, slots.clone());
    let lengths = seal(&index.lengths, slots.clone());
    index.install_sealed(values, lengths).unwrap();
    assert!(index.values.is_empty());
    assert!(index.lengths.is_empty());
    assert_eq!(index.distinct_values(), 1);
    index.validate_eligible(1, 1).unwrap();
    let pinned = index.clone();
    index.materialize_mutable().unwrap();
    assert_eq!(index.values[&key], slots);
    assert!(index.sealed_values().is_none());
    assert!(pinned.values.is_empty());
    assert_eq!(
        pinned.sealed_values().unwrap().get(&key).unwrap(),
        Some(slots)
    );
}

#[test]
fn sealed_exact_queries_match_mutable_postings_and_estimates() {
    let field = FieldDefinition {
        path: vec!["score".into()],
        kind: LogicalType::Number,
        required: false,
        nullable: true,
        indexed: true,
        multi: false,
        element_nullable: Vec::new(),
    };
    let mut schema = Schema::default();
    schema.register_field(field.clone()).unwrap();
    let mut builder = PostingIndex::new(&field);
    for slot in 0..100 {
        let value = crate::model::Value::Number(Number::integer((slot % 11) as i128));
        builder
            .add_values(crate::model::RecordSlot(slot), Some(&[&value]))
            .unwrap();
    }
    builder.covered_records = 100;
    builder.finish_stats();
    let domain = RoaringBitmap::from_iter(0..100);
    let values = seal(&builder.values, domain.clone());
    let lengths = seal(&builder.lengths, domain.clone());
    let mut sealed = builder.clone();
    sealed.install_sealed(values, lengths).unwrap();
    let restricted = RoaringBitmap::from_iter((0..100).filter(|slot| slot % 3 == 0));
    for expression in [
        "score = 3",
        "score != 3",
        "score < 3",
        "score <= 3",
        "score > 3",
        "score >= 3",
    ] {
        let BoundExpr::Term(term) =
            bind_filter(&parse_filter(expression).unwrap(), &schema).unwrap()
        else {
            panic!("expected term");
        };
        let expected = builder.exact_set(&term, &domain).unwrap();
        assert_eq!(
            sealed.exact_set_checked(&term, &domain).unwrap(),
            Some(expected.clone()),
            "{expression}"
        );
        assert_eq!(
            sealed
                .exact_set_restricted_checked(&term, &domain, &restricted)
                .unwrap(),
            Some(&expected & &restricted),
            "{expression}"
        );
        assert!(
            sealed
                .estimate_exact_with_visits_checked(&term, domain.len(), &mut 0)
                .unwrap()
                .is_some()
        );
    }
    assert!(sealed.values.is_empty());
    assert_eq!(sealed.values_view().unwrap().len(), builder.values.len());
}

#[test]
fn sealed_temporal_and_length_ranges_match_builder() {
    let cases = [
        (
            FieldDefinition {
                path: vec!["issued".into()],
                kind: LogicalType::Date,
                required: true,
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: Vec::new(),
            },
            vec![
                "issued = 2024-02",
                "issued != 2024-02",
                "issued < 2024-02",
                "issued <= 2024-02",
                "issued > 2024-02",
                "issued >= 2024-02",
            ],
        ),
        (
            FieldDefinition {
                path: vec!["items".into()],
                kind: LogicalType::Array(Box::new(LogicalType::Number)),
                required: true,
                nullable: false,
                indexed: true,
                multi: true,
                element_nullable: vec![false],
            },
            vec![
                "items:length = 2",
                "items:length != 2",
                "items:length < 2",
                "items:length <= 2",
                "items:length > 2",
                "items:length >= 2",
            ],
        ),
    ];
    for (field, expressions) in cases {
        let mut schema = Schema::default();
        schema.register_field(field.clone()).unwrap();
        let mut builder = PostingIndex::new(&field);
        for slot in 0..100 {
            let record = if field.path[0] == "issued" {
                let date = time::Date::from_ordinal_date(2024, slot + 1).unwrap();
                crate::model::Value::Date(date)
            } else {
                crate::model::Value::from_json(
                    serde_json::json!({"items": vec![slot; (slot % 5) as usize]}),
                )
                .unwrap()
            };
            if field.path[0] == "issued" {
                builder
                    .add_values(crate::model::RecordSlot(slot as u32), Some(&[&record]))
                    .unwrap();
            } else {
                builder
                    .add(crate::model::RecordSlot(slot as u32), &record)
                    .unwrap();
            }
        }
        builder.finish_stats();
        let domain = RoaringBitmap::from_iter(0..100);
        let mut sealed = builder.clone();
        sealed
            .install_sealed(
                seal(&builder.values, domain.clone()),
                seal(&builder.lengths, domain.clone()),
            )
            .unwrap();
        for expression in expressions {
            let BoundExpr::Term(term) =
                bind_filter(&parse_filter(expression).unwrap(), &schema).unwrap()
            else {
                panic!("expected term");
            };
            assert_eq!(
                sealed.exact_set_checked(&term, &domain).unwrap(),
                builder.exact_set(&term, &domain),
                "{expression}"
            );
            assert!(
                sealed
                    .estimate_exact_with_visits_checked(&term, domain.len(), &mut 0)
                    .unwrap()
                    .is_some(),
                "{expression}"
            );
        }
    }
}
