use super::prelude::*;
use super::{fixture_schema, with_record};
#[test]
fn fts_exact_modes_preserve_original_and_folded_literals() {
    let schema = fixture_schema();
    for (text, literal, sensitive, insensitive) in [
        ("Der", "der", false, true),
        ("Straße", "STRASSE", false, true),
        ("Σ", "ς", false, true),
        ("İ", "i", false, true),
        ("İ", "i\u{307}", false, true),
        ("ı", "I", false, false),
        ("e\u{301}", "é", false, false),
    ] {
        for selected in [false, true] {
            for (case_sensitivity, expected) in [
                (CaseSensitivity::Sensitive, sensitive),
                (CaseSensitivity::Insensitive, insensitive),
            ] {
                let fts = FtsQuery {
                    pattern_parameter: None,
                    mode: crate::query::FtsMode::Literal,
                    case_sensitivity,
                    literal: literal.into(),
                    fields: selected
                        .then(|| crate::query::FtsField {
                            query_path: None,
                            path: FieldPath::parse("name").unwrap(),
                            binding: None,
                        })
                        .into_iter()
                        .collect(),
                };
                let bound = bind_fts(&fts, &schema).unwrap();
                let segments = vec![TextSegment {
                    path: vec!["name".into()],
                    text: text.into(),
                }];
                with_record(
                    Value::from_json(serde_json::json!({"name": text})).unwrap(),
                    |ctx| {
                        assert_eq!(
                            eval_bound_fts(&bound, &segments, &ctx, &mut HashSet::new()).unwrap(),
                            expected,
                            "{text:?} {literal:?} {case_sensitivity:?} selected={selected}"
                        );
                    },
                );
                if !selected {
                    assert_eq!(raw_fallback_contains(&bound, text), expected);
                }
            }
        }
    }
}

#[test]
fn fts_membership_stops_after_first_match_but_evidence_checks_every_segment() {
    #[derive(Debug)]
    struct CountingMatcher(std::sync::Arc<std::sync::atomic::AtomicUsize>);

    impl PreparedTextMatcher for CountingMatcher {
        fn matches(&self, text: &str) -> Result<bool, String> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(text.contains("hit"))
        }
    }

    let mut bound = bind_fts(
        &FtsQuery {
            pattern_parameter: None,
            mode: crate::query::FtsMode::Literal,
            case_sensitivity: CaseSensitivity::Sensitive,
            literal: "hit".into(),
            fields: vec![crate::query::FtsField {
                query_path: None,
                path: FieldPath::parse("name").unwrap(),
                binding: None,
            }],
        },
        &fixture_schema(),
    )
    .unwrap();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    bound.fields[0].matcher = Some(std::sync::Arc::new(CountingMatcher(calls.clone())));
    let segments = (0..3)
        .map(|_| TextSegment {
            path: vec!["name".into()],
            text: "hit".into(),
        })
        .collect::<Vec<_>>();
    with_record(
        Value::from_json(serde_json::json!({"name": "hit"})).unwrap(),
        |ctx| {
            assert!(eval_bound_fts_membership(&bound, &segments, &ctx).unwrap());
            assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
            let mut evidence = HashSet::new();
            assert!(eval_bound_fts(&bound, &segments, &ctx, &mut evidence).unwrap());
            assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 4);
            assert_eq!(evidence, HashSet::from(["name".to_owned()]));
        },
    );
}
