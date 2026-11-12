use super::*;

#[test]
fn primitive_classes_and_temporal_limit() {
    let values = [
        (Value::Null, NULL),
        (Value::Bool(false), FALSE),
        (Value::Bool(true), TRUE),
        (Value::Number(Number::integer(0)), ZERO),
        (Value::Number(Number::Float(-0.0)), ZERO),
        (
            Value::Number(Number::parse_json_literal("0.0").unwrap()),
            ZERO,
        ),
        (Value::Number(Number::integer(-3)), OTHER_NUMBER),
        (Value::String(String::new()), EMPTY_STRING),
        (Value::String(" ".into()), OTHER_STRING),
        (Value::Id(String::new()), EMPTY_STRING),
        (Value::Id("id".into()), OTHER_STRING),
        (
            Value::from_json(serde_json::json!([])).unwrap(),
            EMPTY_ARRAY,
        ),
        (
            Value::from_json(serde_json::json!([null])).unwrap(),
            OTHER_ARRAY,
        ),
        (
            Value::from_json(serde_json::json!({})).unwrap(),
            EMPTY_OBJECT,
        ),
        (
            Value::from_json(serde_json::json!({"x":null})).unwrap(),
            OTHER_OBJECT,
        ),
    ];
    for (value, expected) in &values {
        assert_eq!(classify_value(value), Some(*expected), "{value:?}");
    }
    let temporal =
        Value::Date(time::Date::from_calendar_date(2026, time::Month::January, 1).unwrap());
    assert_eq!(classify_value(&temporal), None);
    assert!(value_matches(&temporal, StatePredicate::NonEmpty));
    assert!(value_matches(&temporal, StatePredicate::Exists));
    assert!(!value_matches(&temporal, StatePredicate::String));
}

#[test]
fn mixed_occurrences_keep_every_type_and_nonempty_dominates() {
    let values = [
        Value::Null,
        Value::String("".into()),
        Value::String("hello".into()),
    ];
    let selected: Vec<_> = values.iter().collect();
    let mask = classify_selected(Some(&selected)).unwrap();
    assert_eq!(mask, NULL | EMPTY_STRING | OTHER_STRING);
    for predicate in [
        StatePredicate::Null,
        StatePredicate::String,
        StatePredicate::NonEmpty,
    ] {
        assert!(predicate_matches(mask, predicate));
        assert!(selected_matches(Some(&selected), predicate));
    }
    let empty = [Value::Null, Value::String("".into())];
    let selected: Vec<_> = empty.iter().collect();
    assert!(!predicate_matches(
        classify_selected(Some(&selected)).unwrap(),
        StatePredicate::NonEmpty
    ));
    assert_eq!(classify_selected(None), Some(0));
    assert_eq!(classify_selected(Some(&[])), Some(0));
}

#[test]
fn coverage_sparse_packed_and_dense_round_trip() {
    let mut builder = FieldStateColumnBuilder::new();
    builder.cover_slots(&RoaringBitmap::from_iter([0, 1, 2, 255, 256, 999]));
    builder.insert_mask(0, NULL);
    builder.insert_mask(0, OTHER_STRING);
    builder.insert_mask(255, OTHER_ARRAY);
    for slot in 256..=383 {
        builder.insert_mask(slot, NULL | OTHER_STRING);
    }
    let column = builder.finish();
    assert_eq!(column.mask_at(0), Some(NULL | OTHER_STRING));
    assert_eq!(column.mask_at(1), Some(0));
    assert_eq!(column.mask_at(255), Some(OTHER_ARRAY));
    assert_eq!(column.mask_at(500), None);
    assert_eq!(column.mask_at(999), Some(0));
    assert_eq!(column.mixed_entries(), 129);
    assert!(matches!(column.blocks.get(&1), Some(StateBlock::Dense(_))));
    let candidates = RoaringBitmap::from_iter([0, 1, 255, 256, 350, 500, 999]);
    assert_eq!(
        column.matching_slots(StatePredicate::Null, true, &candidates),
        RoaringBitmap::from_iter([0, 256, 350])
    );
    assert_eq!(
        column.matching_slots(StatePredicate::Exists, false, &candidates),
        RoaringBitmap::from_iter([1, 999])
    );
    assert_eq!(column.covered_slots().len(), 133);
    let encoded = bincode::serialize(&column).unwrap();
    assert_eq!(column.encoded_bytes(), encoded.len());
    assert!(column.estimated_retained_bytes() >= encoded.len() / 2);
    let restored: FieldStateColumn = bincode::deserialize(&encoded).unwrap();
    for slot in 0..=1000 {
        assert_eq!(restored.mask_at(slot), column.mask_at(slot));
    }
}

#[test]
fn random_mixed_selections_match_source_and_column_at_partial_coverage() {
    let pool = [
        Value::Null,
        Value::Bool(false),
        Value::Number(Number::integer(0)),
        Value::Number(Number::integer(7)),
        Value::String("".into()),
        Value::String("text".into()),
        Value::from_json(serde_json::json!([])).unwrap(),
        Value::from_json(serde_json::json!([null])).unwrap(),
        Value::from_json(serde_json::json!({})).unwrap(),
        Value::from_json(serde_json::json!({"x":1})).unwrap(),
    ];
    let mut seed = 0x1234_5678_u32;
    let mut builder = FieldStateColumnBuilder::new();
    let mut expected = Vec::new();
    for slot in 0..1000u32 {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let count = ((seed >> 24) % 4) as usize;
        let mut indices = Vec::new();
        for offset in 0..count {
            let index = ((seed as usize >> (offset * 5)) + offset) % pool.len();
            indices.push(index);
        }
        let selected: Vec<&Value> = indices.iter().map(|&index| &pool[index]).collect();
        if slot % 3 != 0 {
            builder.cover_slots(&RoaringBitmap::from_iter([slot]));
            if let Some(mask) = classify_selected(Some(&selected))
                && mask != 0
            {
                builder.insert_mask(slot, mask);
            }
        }
        expected.push(indices);
    }
    let column = builder.finish();
    for slot in 0..1000u32 {
        let refs: Vec<&Value> = expected[slot as usize]
            .iter()
            .map(|&index| &pool[index])
            .collect();
        for predicate in [
            StatePredicate::NonEmpty,
            StatePredicate::String,
            StatePredicate::Number,
            StatePredicate::Array,
            StatePredicate::Null,
            StatePredicate::Object,
            StatePredicate::Exists,
        ] {
            let source = selected_matches(Some(&refs), predicate);
            if let Some(mask) = column.mask_at(slot) {
                assert_eq!(
                    predicate_matches(mask, predicate),
                    source,
                    "slot {slot} {predicate:?}"
                );
            }
        }
    }
}
