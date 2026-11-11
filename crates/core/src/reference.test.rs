use super::*;

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
    let copied_before = crate::source::owned_collector_copied_bytes();
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
    assert_eq!(crate::source::owned_collector_copied_bytes(), copied_before);
    let owned = crate::source::text_segments(&value);
    assert_eq!(
        crate::source::owned_collector_copied_bytes() - copied_before,
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
fn prepared_contains_preserves_plain_wildcard_and_escaped_percent_semantics() {
    for (pattern, cases) in [
        ("abc", vec![("zabcx", true), ("abxc", false)]),
        ("", vec![("", true), ("anything", true)]),
        ("a%b", vec![("ab", true), ("axxxb", true), ("ba", false)]),
        ("aba%aba", vec![("abaaba", true), ("ababa", false)]),
        ("a\\%b", vec![("a%b", true), ("axxb", false)]),
        ("%%", vec![("", true), ("anything", true)]),
        ("a\\\\%b", vec![("a\\%b", true), ("a%b", false)]),
    ] {
        let prepared = PreparedContains::new(pattern);
        for (text, expected) in cases {
            assert_eq!(
                prepared.matches(text, pattern),
                expected,
                "{pattern:?} {text:?}"
            );
            assert_eq!(
                contains_pattern(text, pattern),
                expected,
                "dynamic {pattern:?} {text:?}"
            );
        }
    }
    assert!(matches!(
        PreparedContains::new("abc"),
        PreparedContains::Literal(_)
    ));
    assert!(matches!(
        PreparedContains::new("a\\%b"),
        PreparedContains::Unescaped(_)
    ));
    assert!(matches!(
        PreparedContains::new("a%b"),
        PreparedContains::Wildcards(_)
    ));
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
fn bound_contains_reuses_prepared_pattern_across_scalar_and_array_values() {
    for (filter, document, expected) in [
        ("name ~ 'a%b'", serde_json::json!({"name":"axxb"}), true),
        ("name !~ 'a%b'", serde_json::json!({"name":"axxb"}), false),
        ("name ~ 'a\\%b'", serde_json::json!({"name":"a%b"}), true),
        (
            "name:lower ~ 'str%se'",
            serde_json::json!({"name":"Straße"}),
            true,
        ),
        (
            "values ?~ 'a%b'",
            serde_json::json!({"values":["no", "axxb"]}),
            true,
        ),
        ("values ?~ 'a%b'", serde_json::json!({"values":[]}), false),
    ] {
        PREPARED_CONTAINS_BUILDS.with(|count| count.set(0));
        let bound = bind_filter(&parse_filter(filter).unwrap(), &schema()).unwrap();
        let BoundExpr::Term(term) = &bound else {
            panic!("one term")
        };
        assert!(term.prepared_contains.is_some());
        assert_eq!(PREPARED_CONTAINS_BUILDS.with(|count| count.get()), 1);
        let value = Value::from_json(document).unwrap();
        with_record(value, |ctx| {
            for _ in 0..32 {
                assert_eq!(
                    eval_filter(&bound, &ctx, &mut HashSet::new()).unwrap(),
                    expected
                );
            }
        });
        assert_eq!(PREPARED_CONTAINS_BUILDS.with(|count| count.get()), 1);
    }
}

#[test]
fn membership_filter_and_unavailable_truth_match_evidence_evaluation() {
    let value = Value::from_json(serde_json::json!({
        "name": "Ada", "values": ["x", "y"], "people": [{"name": "Bob"}]
    }))
    .unwrap();
    with_record(value, |ctx| {
        for filter in [
            "name = 'Ada'",
            "name ~ 'A%a' && values ?= 'x'",
            "name !~ 'A%a' || people.name ?= 'Bob'",
            "!(name = 'Ada' && values ?= 'no')",
            "values ?= 'missing'",
            "@file.id = 0 || name = 'no'",
            "@file.id != 0 && name = 'Ada'",
        ] {
            let bound = bind_filter(&parse_filter(filter).unwrap(), &schema()).unwrap();
            let unavailable = eval_unavailable_filter(&bound, &ctx, &mut HashSet::new()).unwrap();
            assert_eq!(
                eval_unavailable_filter_membership(&bound, &ctx).unwrap(),
                unavailable,
                "{filter}"
            );
        }
    });
}

#[test]
fn incremental_conversion_tree_matches_complete_rebuild_in_any_registration_order() {
    fn field(path: &[&str], kind: LogicalType, required: bool) -> FieldDefinition {
        FieldDefinition {
            path: path.iter().map(|part| (*part).into()).collect(),
            kind,
            required,
            nullable: !required,
            indexed: false,
            multi: false,
            element_nullable: Vec::new(),
        }
    }

    fn complete_tree(schema: &Schema) -> ValidationNode {
        let mut root = ValidationNode::default();
        for field in schema
            .fields
            .values()
            .filter(|field| converted_kind(&field.kind))
        {
            if field.path.is_empty() {
                root.field = Some(field.clone());
            }
            let mut node = &mut root;
            for depth in 0..field.path.len() {
                node = node.children.entry(field.path[depth].clone()).or_default();
                if let Some(ancestor) = schema.fields.get(&field.path[..=depth]) {
                    node.field = Some(ancestor.clone());
                }
            }
        }
        root
    }

    fn assert_same_nodes(actual: &ValidationNode, expected: &ValidationNode) {
        match (&actual.field, &expected.field) {
            (Some(actual), Some(expected)) => {
                assert_eq!(actual.path, expected.path);
                assert_eq!(actual.kind, expected.kind);
                assert_eq!(actual.required, expected.required);
                assert_eq!(actual.nullable, expected.nullable);
                assert_eq!(actual.multi, expected.multi);
                assert_eq!(actual.element_nullable, expected.element_nullable);
            }
            (None, None) => {}
            _ => panic!("conversion validation field differs"),
        }
        assert_eq!(actual.children.len(), expected.children.len());
        for (name, expected_child) in &expected.children {
            assert_same_nodes(&actual.children[name], expected_child);
        }
    }

    let fields = [
        field(&[], LogicalType::Object, false),
        field(&["details"], LogicalType::Object, false),
        field(&["details", "published"], LogicalType::Date, true),
        field(
            &["details", "history"],
            LogicalType::Array(Box::new(LogicalType::Object)),
            false,
        ),
        field(&["details", "history", "at"], LogicalType::Datetime, true),
        field(&["details", "history", "label"], LogicalType::String, false),
        field(&["other", "count"], LogicalType::Number, false),
        field(
            &["other", "days"],
            LogicalType::Array(Box::new(LogicalType::Date)),
            false,
        ),
    ];
    for order in [
        (0..fields.len()).collect::<Vec<_>>(),
        (0..fields.len()).rev().collect(),
        vec![2, 4, 7, 5, 3, 1, 6, 0],
    ] {
        let mut schema = Schema::default();
        for index in order {
            schema.register_field(fields[index].clone()).unwrap();
            assert_same_nodes(&schema.conversion_validation, &complete_tree(&schema));
        }
    }
}

#[test]
fn small_selection_preserves_scalar_and_array_path_states() {
    let value = Value::from_json(serde_json::json!({
        "name": "Ada", "empty": [], "null": null,
        "people": [{"name": "A"}, {"name": "B"}]
    }))
    .unwrap();
    let path = |parts: &[&str]| {
        parts
            .iter()
            .map(|part| (*part).to_string())
            .collect::<Vec<_>>()
    };
    let scalar = data_values_small(&value, &path(&["name"])).unwrap();
    assert!(!scalar.spilled());
    assert!(matches!(scalar.as_slice(), [Value::String(name)] if name == "Ada"));
    assert!(data_values_small(&value, &path(&["absent"])).is_none());
    assert!(matches!(
        data_values_small(&value, &path(&["null"]))
            .unwrap()
            .as_slice(),
        [Value::Null]
    ));
    assert_eq!(
        data_values_small(&value, &path(&["empty", "name"]))
            .unwrap()
            .len(),
        0
    );
    let many = data_values_small(&value, &path(&["people", "name"])).unwrap();
    assert_eq!(many.len(), 2);
    assert_eq!(
        many.as_slice(),
        data_values(&value, &path(&["people", "name"])).unwrap()
    );
}
use crate::inventory::{ByteRange, SourceRevision};
use crate::model::{FileId, RecordId, RecordSlot};
use crate::query::parse_filter;

fn schema() -> Schema {
    let mut schema = Schema::default();
    for (path, kind, multi) in [
        ("name", LogicalType::String, false),
        (
            "values",
            LogicalType::Array(Box::new(LogicalType::String)),
            true,
        ),
        ("people.name", LogicalType::String, true),
        (
            "people.aliases",
            LogicalType::Array(Box::new(LogicalType::String)),
            true,
        ),
    ] {
        schema
            .register_field(FieldDefinition {
                path: path.split('.').map(str::to_owned).collect(),
                kind,
                required: false,
                nullable: true,
                indexed: true,
                multi,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    schema
        .register_custom(
            vec!["name".into()],
            CustomBinding {
                name: "folded".into(),
                version: 1,
                transform: Transform::CaseFold,
            },
        )
        .unwrap();
    schema
}

#[test]
fn fts_exact_modes_preserve_original_and_folded_literals() {
    let schema = schema();
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
        &schema(),
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

#[test]
fn optional_ancestor_of_required_temporal_child_stays_optional_during_query_conversion() {
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["details".into()],
            kind: LogicalType::Object,
            required: false,
            nullable: true,
            indexed: false,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    schema
        .register_field(FieldDefinition {
            path: vec!["details".into(), "published".into()],
            kind: LogicalType::Date,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    schema
        .register_field(FieldDefinition {
            path: vec!["details".into(), "history".into(), "at".into()],
            kind: LogicalType::Datetime,
            required: true,
            nullable: false,
            indexed: false,
            multi: true,
            element_nullable: vec![],
        })
        .unwrap();
    schema
        .register_field(FieldDefinition {
            path: vec!["details".into(), "history".into()],
            kind: LogicalType::Array(Box::new(LogicalType::Object)),
            required: false,
            nullable: false,
            indexed: false,
            multi: true,
            element_nullable: vec![false],
        })
        .unwrap();
    for json in [
        serde_json::json!({}),
        serde_json::json!({"details":null}),
        serde_json::json!({"details":{"published":"2026-01-02", "history":[{"at":"2026-01-02T03:04:05Z"}]}}),
    ] {
        let mut built = Value::from_json(json.clone()).unwrap();
        schema.coerce_record(&mut built).unwrap();
        let mut scanned = Value::from_json(json).unwrap();
        schema.convert_indexed_record(&mut scanned, None).unwrap();
        assert_eq!(scanned, built);
    }
    let mut projected = Value::from_json(serde_json::json!({"note":"keep"})).unwrap();
    schema
        .convert_indexed_record(&mut projected, Some(&HashSet::from(["note".into()])))
        .unwrap();
    let mut invalid = Value::from_json(serde_json::json!({"details":{}})).unwrap();
    assert!(
        schema
            .coerce_record(&mut invalid)
            .unwrap_err()
            .contains("details.published")
    );
}

fn with_record<F: FnOnce(RecordContext<'_>)>(value: Value, f: F) {
    let record = RecordEntry {
        slot: RecordSlot(0),
        id: RecordId("x".into()),
        file: FileId(0),
        range: ByteRange { start: 0, end: 4 },
        revision: SourceRevision {
            modified_ns: 0,
            size: 4,
        },
        class: crate::inventory::RecordClass::Logical,
        diagnostic_ref: crate::inventory::DiagnosticRef(0),
    };
    let file = FileEntry {
        id: FileId(0),
        relative_path: "x.json".into(),
        revision: record.revision,
        created_ns: None,
        collections: vec![],
        status: crate::inventory::FileStatus::Valid,
        diagnostics: vec![],
        omitted_diagnostics: 0,
    };
    f(RecordContext {
        value: &value,
        source_bytes: None,
        record: &record,
        file: &file,
        collection_name: "test",
        repository_root: Path::new("/tmp"),
    });
}

#[test]
fn scalar_metadata_comparisons_preserve_null_and_timestamp_rules() {
    with_record(Value::Null, |ctx| {
        for (expression, expected) in [
            ("@file.modified >= '1970-01-01T00:00:00Z'", true),
            ("@file.modified > '1970-01-01T00:00:00Z'", false),
            ("@file.modified < '1971-01-01T00:00:00Z'", true),
            ("@file.created:null = true", true),
            ("@file.created = '1970-01-01T00:00:00Z'", false),
        ] {
            let bound = bind_filter(&parse_filter(expression).unwrap(), &schema()).unwrap();
            let BoundExpr::Term(term) = bound else {
                panic!("expected one metadata predicate");
            };
            assert_eq!(eval_term(&term, &ctx).unwrap(), expected, "{expression}");
        }
    });
}

#[test]
fn truth_table_arrays_null_and_case_folding() {
    let value = Value::from_json(serde_json::json!({"name":"Straße", "values":["a","b"], "people":[{"name":"Ada","aliases":["A","Ada"]},{"name":"Bob","aliases":["B"]}]})).unwrap();
    with_record(value, |ctx| {
        for (input, expected) in [
            ("values ?!= 'a'", true),
            ("!(values ?= 'a')", false),
            ("values:each = 'a'", false),
            ("values:length = 2", true),
            ("name:lower ~ 'STRASSE'", true),
            ("name:folded ~ 'strasse'", true),
            ("people.name ?= 'Ada'", true),
            ("people.aliases ?= 'Ada'", true),
            ("people.aliases ?= 'missing'", false),
            ("name:null = false", true),
            ("@file.created:null = true", true),
        ] {
            let bound = bind_filter(&parse_filter(input).unwrap(), &schema()).unwrap();
            assert_eq!(
                eval_filter(&bound, &ctx, &mut HashSet::new()).unwrap(),
                expected,
                "{input}"
            );
        }
    });
}

#[test]
fn invalid_branches_rejected_before_evaluation() {
    let schema = schema();
    assert!(
        bind_filter(
            &parse_filter("name = 'x' || absent:missing = 1").unwrap(),
            &schema
        )
        .is_err()
    );
    assert!(bind_filter(&parse_filter("name:missing = 'x'").unwrap(), &schema).is_err());
    assert!(bind_filter(&parse_filter("name ?= 'x'").unwrap(), &schema).is_err());
}

#[test]
fn evidence_excludes_false_boolean_branches() {
    let value = Value::from_json(
        serde_json::json!({"name":"Ada","values":["yes"],"people":[{"name":"Bob"}]}),
    )
    .unwrap();
    with_record(value, |ctx| {
        let bound = bind_filter(
            &parse_filter("(name = 'Ada' && values ?= 'no') || people.name ?= 'Bob'").unwrap(),
            &schema(),
        )
        .unwrap();
        let mut evidence = HashSet::new();
        assert!(eval_filter(&bound, &ctx, &mut evidence).unwrap());
        assert_eq!(evidence, HashSet::from(["people.name".to_string()]));
    });
}

#[test]
fn explicit_temporal_hints_coerce_values_and_reject_bad_inputs() {
    let mut schema = Schema::default();
    for (name, kind) in [
        ("day", LogicalType::Date),
        ("time", LogicalType::Datetime),
        ("seconds", LogicalType::Unixtime),
        ("days", LogicalType::Array(Box::new(LogicalType::Date))),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: true,
                nullable: false,
                indexed: true,
                multi: name == "days",
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    let mut value = Value::from_json(serde_json::json!({
        "day":"2026-01-02","time":"2026-01-02T03:04:05+02:00",
        "seconds":1767319445,"days":["2026-01-01","2026-01-02"]
    }))
    .unwrap();
    schema.coerce_record(&mut value).unwrap();
    let Value::Object(fields) = value else {
        panic!("object");
    };
    assert!(matches!(fields["day"], Value::Date(_)));
    assert!(matches!(fields["time"], Value::Datetime(_)));
    assert!(matches!(fields["seconds"], Value::Unixtime(_)));
    assert!(
        matches!(fields["days"],Value::Array(ref array) if array.element_type == Some(LogicalType::Date))
    );
    let mut invalid = Value::from_json(serde_json::json!({"day":"not a date"})).unwrap();
    assert!(schema.coerce_record(&mut invalid).is_err());
}

#[test]
fn missing_null_empty_truth_table() {
    for (input, expected) in [
        ("values:exists = true", true),
        ("values:null = false", true),
        ("values:each = 'x'", true),
        ("values ?= 'x'", false),
        ("values:length = 0", true),
        ("name:exists = false", true),
        ("name:null = false", true),
        ("name != 'x'", false),
    ] {
        let value = Value::from_json(serde_json::json!({"values": []})).unwrap();
        with_record(value, |ctx| {
            let bound = bind_filter(&parse_filter(input).unwrap(), &schema()).unwrap();
            assert_eq!(
                eval_filter(&bound, &ctx, &mut HashSet::new()).unwrap(),
                expected,
                "{input}"
            );
        });
    }
    let value = Value::from_json(serde_json::json!({"name":null,"values":[null,"x"]})).unwrap();
    with_record(value, |ctx| {
        for (input, expected) in [
            ("name:exists = true", true),
            ("name:null = true", true),
            ("name = null", true),
            ("name != 'x'", false),
            ("values ?!= 'x'", false),
            ("values ?= 'x'", true),
        ] {
            let bound = bind_filter(&parse_filter(input).unwrap(), &schema()).unwrap();
            assert_eq!(
                eval_filter(&bound, &ctx, &mut HashSet::new()).unwrap(),
                expected,
                "{input}"
            );
        }
    });
}

#[test]
fn declared_types_required_fields_and_array_nullability_are_enforced() {
    let mut schema = Schema::default();
    for (name, kind, required, nullable, element_nullable) in [
        ("count", LogicalType::Number, true, false, vec![]),
        ("optional", LogicalType::String, false, true, vec![]),
        (
            "tags",
            LogicalType::Array(Box::new(LogicalType::String)),
            true,
            false,
            vec![false],
        ),
        (
            "days",
            LogicalType::Array(Box::new(LogicalType::Date)),
            true,
            false,
            vec![true],
        ),
        (
            "times",
            LogicalType::Array(Box::new(LogicalType::Datetime)),
            true,
            false,
            vec![false],
        ),
        (
            "seconds",
            LogicalType::Array(Box::new(LogicalType::Unixtime)),
            true,
            false,
            vec![false],
        ),
        (
            "nested",
            LogicalType::Array(Box::new(LogicalType::Array(Box::new(LogicalType::Number)))),
            true,
            false,
            vec![false, true],
        ),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required,
                nullable,
                indexed: name == "count",
                multi: name != "count" && name != "optional",
                element_nullable,
            })
            .unwrap();
    }
    let valid = serde_json::json!({
        "count": 7, "optional": null, "tags": ["a"],
        "days": ["2026-01-01", null],
        "times": ["2026-01-01T12:30:00+02:00"],
        "seconds": [0, 1767319445],
        "nested": [[], [1, null]],
    });
    let mut value = Value::from_json(valid.clone()).unwrap();
    schema.coerce_record(&mut value).unwrap();
    let Value::Object(fields) = value else {
        panic!("object");
    };
    assert!(
        matches!(fields["days"], Value::Array(ref array) if array.element_type == Some(LogicalType::Date) && array.nullable_elements)
    );
    assert!(
        matches!(fields["times"], Value::Array(ref array) if matches!(array.values[0], Value::Datetime(_)))
    );
    assert!(
        matches!(fields["seconds"], Value::Array(ref array) if matches!(array.values[0], Value::Unixtime(0)))
    );
    assert!(
        matches!(fields["nested"], Value::Array(ref array) if array.element_type == Some(LogicalType::Array(Box::new(LogicalType::Number))))
    );
    for (change, expected) in [
        (serde_json::json!({"count": "7"}), "count: expected Number"),
        (
            serde_json::json!({"count": null}),
            "count: null is not allowed",
        ),
        (
            serde_json::json!({"tags": [null]}),
            "tags: null is not allowed",
        ),
        (serde_json::json!({"days": ["not-a-date"]}), "days:"),
        (
            serde_json::json!({"nested": [null]}),
            "nested: null is not allowed",
        ),
    ] {
        let mut json = valid.clone();
        for (key, value) in change.as_object().unwrap() {
            json[key] = value.clone();
        }
        let mut logical = Value::from_json(json).unwrap();
        assert!(
            schema
                .coerce_record(&mut logical)
                .unwrap_err()
                .contains(expected)
        );
    }
    let mut missing = valid.as_object().unwrap().clone();
    missing.remove("count");
    let mut logical = Value::from_json(serde_json::Value::Object(missing)).unwrap();
    assert!(
        schema
            .coerce_record(&mut logical)
            .unwrap_err()
            .contains("count: required field is missing")
    );
    let mut projected = Value::from_json(serde_json::json!({"count": 7})).unwrap();
    schema
        .coerce_projected_record(&mut projected, Some(&HashSet::from(["count".into()])))
        .unwrap();
    assert!(schema.coerce_record(&mut projected).is_err());
    let mut query_record = Value::from_json(serde_json::json!({
        "days": ["2026-01-01", null], "tags": ["a"]
    }))
    .unwrap();
    schema
        .convert_indexed_record(&mut query_record, Some(&HashSet::from(["days".into()])))
        .unwrap();
    let Value::Object(query_fields) = query_record else {
        panic!("object");
    };
    assert!(
        matches!(query_fields["days"], Value::Array(ref array) if matches!(array.values[0], Value::Date(_)))
    );
}

#[test]
fn numeric_string_is_rejected_before_indexed_or_scanned_inequality() {
    use crate::query::{Query, QueryOptions};
    use crate::repository::{
        CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
    };
    use crate::source::SourceFormat;
    use crate::workload::ReturnMode;

    let root = std::env::temp_dir().join(format!("objekte-type-index-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let definition = |indexed| {
        let mut schema = Schema::default();
        for (name, kind, indexed) in [
            ("count", LogicalType::Number, indexed),
            ("other", LogicalType::String, false),
        ] {
            schema
                .register_field(FieldDefinition {
                    path: vec![name.into()],
                    kind,
                    required: true,
                    nullable: false,
                    indexed,
                    multi: false,
                    element_nullable: vec![],
                })
                .unwrap();
        }
        CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }
    };
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::Disabled,
        ..Default::default()
    };
    std::fs::write(root.join("data/bad.json"), r#"{"count":"7","other":"ok"}"#).unwrap();
    for indexed in [false, true] {
        let repo =
            Repository::open_with_options(&root, vec![definition(indexed)], options).unwrap();
        assert_eq!(
            repo.files.files[0].status,
            crate::inventory::FileStatus::Broken
        );
        assert_eq!(
            repo.collections[0].generation.records[0].class,
            crate::inventory::RecordClass::RawFallback
        );
        assert!(
            repo.files.files[0].diagnostics[0]
                .message
                .contains("count: expected Number")
        );
    }
    std::fs::remove_file(root.join("data/bad.json")).unwrap();
    std::fs::write(root.join("data/seven.json"), r#"{"count":7,"other":"ok"}"#).unwrap();
    std::fs::write(root.join("data/eight.json"), r#"{"count":8,"other":"ok"}"#).unwrap();
    let query = Query {
        collections: vec![],
        fts: None,
        filter: Some(parse_filter("count != 7").unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    for indexed in [false, true] {
        let repo =
            Repository::open_with_options(&root, vec![definition(indexed)], options).unwrap();
        let optimized = repo.search(&query).unwrap();
        let reference = repo.search_reference(&query).unwrap();
        assert_eq!(optimized.count.unwrap(), 1);
        assert_eq!(optimized.hits[0].record_id, "eight");
        assert_eq!(optimized.hits, reference.hits);
        let mut logical_query = query.clone();
        logical_query.options.return_mode = ReturnMode::Logical;
        let logical = repo.search(&logical_query).unwrap();
        assert!(
            matches!(&logical.hits[0].payload, Some(crate::repository::Payload::Logical(Value::Object(fields))) if fields.get("count") == Some(&Value::Number(Number::integer(8))))
        );
    }
    let persisted = RepositoryOptions {
        snapshot_mode: SnapshotMode::IfValid,
        ..Default::default()
    };
    let built = Repository::open_with_options(&root, vec![definition(true)], persisted).unwrap();
    assert!(!built.startup().restored);
    built.save_default_snapshot_for_test();
    drop(built);
    let restored = Repository::open_with_options(&root, vec![definition(true)], persisted).unwrap();
    assert!(restored.startup().restored);
    assert_eq!(restored.search(&query).unwrap().hits[0].record_id, "eight");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unhinted_paths_use_actual_types_and_collection_specific_bindings() {
    use crate::query::{FtsField, FtsQuery, Query, QueryOptions};
    use crate::repository::{
        CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
    };
    use crate::source::SourceFormat;
    use crate::workload::ReturnMode;

    let root = std::env::temp_dir().join(format!("objekte-dynamic-fields-{}", std::process::id()));
    for (collection, id, json) in [
        (
            "alpha",
            "one",
            r#"{"count":7,"title":"Hello","when":"2026-01-01","tags":[1,2]}"#,
        ),
        ("alpha", "two", r#"{"count":8,"title":"Other","tags":[]}"#),
        (
            "beta",
            "three",
            r#"{"count":7,"title":"HELLO","when":"2026-01-01","tags":[1,2]}"#,
        ),
        ("beta", "four", r#"{"count":"7","title":"Else","tags":[]}"#),
    ] {
        std::fs::create_dir_all(root.join(collection)).unwrap();
        std::fs::write(root.join(collection).join(format!("{id}.json")), json).unwrap();
    }
    let mut declared = Schema::default();
    declared
        .register_field(FieldDefinition {
            path: vec!["count".into()],
            kind: LogicalType::Number,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let definitions = vec![
        CollectionDefinition {
            name: "alpha".into(),
            source_path: "alpha".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema: declared,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        },
        CollectionDefinition {
            name: "beta".into(),
            source_path: "beta".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema: Schema::default(),
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        },
    ];
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::IfValid,
        ..Default::default()
    };
    let check = |repo: &Repository| {
        for (filter, expected) in [
            ("count = 7", vec![("alpha", "one"), ("beta", "three")]),
            (
                "title:lower ~ 'hello'",
                vec![("alpha", "one"), ("beta", "three")],
            ),
            ("tags ?= 2", vec![("alpha", "one"), ("beta", "three")]),
            ("tags:length = 0", vec![("alpha", "two"), ("beta", "four")]),
            (
                "when = '2026-01-01'",
                vec![("alpha", "one"), ("beta", "three")],
            ),
            (
                "missing:exists = false",
                vec![
                    ("alpha", "one"),
                    ("alpha", "two"),
                    ("beta", "four"),
                    ("beta", "three"),
                ],
            ),
            ("missing = 'x'", vec![]),
        ] {
            let query = Query {
                collections: vec![],
                fts: None,
                filter: Some(parse_filter(filter).unwrap()),
                options: QueryOptions {
                    sort: vec![],
                    limit: 20,
                    offset: 0,
                    count: true,
                    return_mode: ReturnMode::Locator,
                    evidence: true,
                },
            };
            let result = repo.search(&query).unwrap();
            let reference = repo.search_reference(&query).unwrap();
            let actual = result
                .hits
                .iter()
                .map(|hit| (hit.collection.as_str(), hit.record_id.as_str()))
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "{filter}");
            assert_eq!(result.count.unwrap(), reference.count.unwrap());
            assert_eq!(result.hits, reference.hits);
        }
        let query = Query {
            collections: vec![],
            fts: Some(FtsQuery {
                pattern_parameter: None,
                mode: crate::query::FtsMode::Literal,
                case_sensitivity: crate::query::CaseSensitivity::Sensitive,
                literal: "hello".into(),
                fields: vec![FtsField {
                    query_path: None,
                    path: FieldPath::parse("title").unwrap(),
                    binding: Some("lower".into()),
                }],
            }),
            filter: None,
            options: QueryOptions {
                sort: vec![],
                limit: 20,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: true,
            },
        };
        let result = repo.search(&query).unwrap();
        assert_eq!(result.count.unwrap(), 2);
        assert_eq!(result.hits, repo.search_reference(&query).unwrap().hits);
        let mut selected = query.clone();
        selected.fts.as_mut().unwrap().literal = "ell".into();
        selected.fts.as_mut().unwrap().fields[0].binding = None;
        let plain = repo.search(&selected).unwrap();
        assert_eq!(plain.count.unwrap(), 1);
        assert_eq!(plain.hits[0].record_id, "one");
        assert_eq!(plain.hits, repo.search_reference(&selected).unwrap().hits);
        let mut invalid = query.clone();
        invalid.fts.as_mut().unwrap().fields[0].binding = Some("unknown".into());
        assert!(
            repo.search(&invalid)
                .unwrap_err()
                .contains("unknown FTS binding")
        );
        let invalid = Query {
            collections: vec!["alpha".into()],
            fts: None,
            filter: Some(parse_filter("count = '7'").unwrap()),
            options: query.options.clone(),
        };
        assert!(repo.search(&invalid).unwrap_err().contains("incompatible"));
        let invalid = Query {
            collections: vec!["beta".into()],
            fts: None,
            filter: Some(parse_filter("title:unknown = 'x'").unwrap()),
            options: query.options.clone(),
        };
        assert!(
            repo.search(&invalid)
                .unwrap_err()
                .contains("unknown binding")
        );
    };
    let built = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
    assert!(!built.startup().restored);
    check(&built);
    built.save_default_snapshot_for_test();
    drop(built);
    let restored = Repository::open_with_options(&root, definitions, options).unwrap();
    assert!(restored.startup().restored);
    check(&restored);
    std::fs::remove_dir_all(root).unwrap();
}
