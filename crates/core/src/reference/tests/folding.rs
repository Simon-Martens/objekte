use super::super::*;
use super::{fixture_schema, with_record};
use crate::query::parse_filter;
#[test]
fn borrowed_value_visitation_uses_original_strings_and_key_storage() {
    let value = Value::from_json(serde_json::json!({
        "outer": {"items": ["first", "second"]},
        "other": "last"
    }))
    .unwrap();
    let Value::Object(root) = &value else {
        panic!("object")
    };
    let (outer_key, outer) = root.get_key_value("outer").unwrap();
    let Value::Object(outer) = outer else {
        panic!("nested object")
    };
    let (items_key, items) = outer.get_key_value("items").unwrap();
    let Value::Array(items) = items else {
        panic!("array")
    };
    let Value::String(first) = &items.values[0] else {
        panic!("string")
    };
    let mut path = Vec::with_capacity(4);
    let mut seen = Vec::new();
    let copied_before = crate::source::text::owned_collector_copied_bytes();
    visit_borrowed_value_strings(&value, &mut path, &mut |keys, text| {
        if text == "first" {
            assert_eq!(keys, ["outer", "items"]);
            assert_eq!(keys[0].as_ptr(), outer_key.as_ptr());
            assert_eq!(keys[1].as_ptr(), items_key.as_ptr());
            assert_eq!(text.as_ptr(), first.as_ptr());
        }
        seen.push((keys.join("."), text.to_owned()));
        Ok(false)
    })
    .unwrap();
    assert_eq!(path.capacity(), 4);
    assert_eq!(
        crate::source::text::owned_collector_copied_bytes(),
        copied_before
    );
    let owned = crate::source::text::text_segments(&value);
    assert_eq!(
        crate::source::text::owned_collector_copied_bytes() - copied_before,
        owned
            .iter()
            .map(|segment| segment.text.len())
            .sum::<usize>()
    );
    assert_eq!(
        seen,
        [
            ("other".into(), "last".into()),
            ("outer.items".into(), "first".into()),
            ("outer.items".into(), "second".into()),
        ]
    );
}

#[test]
#[ignore = "release-only in-memory production value visitor timing; run serially with --nocapture"]
fn borrowed_value_consumer_microbench() {
    use std::hint::black_box;
    use std::time::Instant;

    let fts = bind_fts(
        &FtsQuery {
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Literal,
            literal: "needle".into(),
            pattern_parameter: None,
            fields: Vec::new(),
        },
        &Schema::default(),
    )
    .unwrap();
    for (name, count) in [("one-string", 1), ("many-strings", 512)] {
        let mut strings = vec![Value::String("ordinary text".into()); count];
        strings.push(Value::String("needle".into()));
        let value = Value::Array(crate::model::ArrayValue::from_unrestricted(strings));
        with_record(value, |ctx| {
            assert!(eval_borrowed_value_fts(&fts, &ctx, None).unwrap());
            let iterations = (4096 / count).max(128);
            let mut samples = Vec::new();
            for round in 0..6 {
                let started = Instant::now();
                for _ in 0..iterations {
                    black_box(eval_borrowed_value_fts(&fts, black_box(&ctx), None).unwrap());
                }
                if round > 0 {
                    samples.push(started.elapsed().as_nanos() / iterations as u128);
                }
            }
            samples.sort_unstable();
            println!(
                "consumer {name} existing-value: {} ns/record median of 5; min {}; max {}; strings {}",
                samples[2],
                samples[0],
                samples[4],
                count + 1
            );
        });
    }
}

#[test]
fn folded_candidate_scratch_matches_full_unicode_fold_and_remains_bounded() {
    for text in ["", "ASCII I", "Straße Σ ς İ I ı", "a\u{0308}😀"] {
        let expected = fold(text);
        with_folded_candidate(text, |actual| assert_eq!(actual, expected.as_bytes()));
    }
    let large = "A".repeat(MAX_FOLD_SCRATCH_CAPACITY + 1);
    with_folded_candidate(&large, |_| {});
    FOLD_SCRATCH.with(|scratch| assert!(scratch.borrow().capacity() <= MAX_FOLD_SCRATCH_CAPACITY));
}

#[test]
fn folded_candidate_scratch_reuses_capacity_across_segments_and_records() {
    with_folded_candidate(&"Straße".repeat(64), |_| {});
    let (initial_ptr, initial_capacity) = FOLD_SCRATCH.with(|scratch| {
        let scratch = scratch.borrow();
        (scratch.as_ptr(), scratch.capacity())
    });
    assert!(initial_capacity >= 64 * 7);
    for text in ["short", "İ", "STRAßE", "", "plain", "Σ ς"] {
        let expected = fold(text);
        with_folded_candidate_str(text, |actual| assert_eq!(actual, expected));
        FOLD_SCRATCH.with(|scratch| {
            let scratch = scratch.borrow();
            assert_eq!(scratch.as_ptr(), initial_ptr);
            assert_eq!(scratch.capacity(), initial_capacity);
        });
    }
}

#[test]
fn shared_fold_reuses_one_logical_occurrence_and_resets_at_record_boundary() {
    let bound = bind_filter(
        &parse_filter("name:lower ~ 'stra' && name:lower ~ 'sse'").unwrap(),
        &fixture_schema(),
    )
    .unwrap();
    let before = fold_value_cache_stats();
    for _ in 0..2 {
        let value = Value::from_json(serde_json::json!({"name":"Straße"})).unwrap();
        with_record(value, |ctx| {
            let _guard = FoldRecordGuard::new_with_sharing(true);
            assert!(eval_filter(&bound, &ctx, &mut HashSet::new()).unwrap());
        });
    }
    let after = fold_value_cache_stats();
    assert_eq!(after.0 - before.0, 2);
    assert_eq!(after.1 - before.1, 2);

    let independent = bind_filter(
        &parse_filter("values:lower ?~ 'stra' && values:lower ?~ 'masse'").unwrap(),
        &fixture_schema(),
    )
    .unwrap();
    let value = Value::from_json(serde_json::json!({"values":["Straße","Masse"]})).unwrap();
    with_record(value, |ctx| {
        let _guard = FoldRecordGuard::new_with_sharing(true);
        assert!(eval_filter(&independent, &ctx, &mut HashSet::new()).unwrap());
    });
}
