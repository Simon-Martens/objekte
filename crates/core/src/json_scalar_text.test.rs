use super::*;
use crate::model::FieldPath;
use crate::query::{CaseSensitivity, FtsField, FtsMode, FtsQuery, parse_filter};
use crate::reference::{BoundExpr, Schema, bind_filter, bind_fts};

fn borrowed_scan(
    bytes: &[u8],
    literal: Option<&str>,
    field: Option<&str>,
    filter: Option<&str>,
    decoded_scalars: bool,
    scratch: &mut BorrowedScanScratch,
) -> BorrowedScanResult {
    let schema = Schema::default();
    let fts = literal.map(|literal| {
        bind_fts(
            &FtsQuery {
                case_sensitivity: CaseSensitivity::Sensitive,
                mode: FtsMode::Literal,
                literal: literal.into(),
                pattern_parameter: None,
                fields: field
                    .map(|field| FtsField {
                        path: FieldPath::parse(field).unwrap(),
                        query_path: None,
                        binding: None,
                    })
                    .into_iter()
                    .collect(),
            },
            &schema,
        )
        .unwrap()
    });
    let filter = filter.map(|filter| bind_filter(&parse_filter(filter).unwrap(), &schema).unwrap());
    let filter = filter.as_ref().map(|bound| {
        let BoundExpr::Term(term) = bound else {
            panic!("one filter term")
        };
        term
    });
    scan_borrowed_segments(
        bytes,
        &BorrowedScanQuery {
            fts: fts.as_ref(),
            filter,
            decoded_scalars,
            retain_evidence: true,
        },
        scratch,
    )
    .unwrap()
}

#[test]
fn borrowed_source_segments_preserve_boundaries_and_duplicate_visibility() {
    let mut scratch = BorrowedScanScratch::default();
    let cases: &[(&[u8], &str, Option<&str>, bool)] = &[
        (
            br#"{"text":"before\u2139after"}"#,
            "beforeℹafter",
            None,
            true,
        ),
        (br#"{"a":"New","b":"York"}"#, "New York", None, false),
        (
            br#"{"a":["local history","archives"]}"#,
            "history archives",
            None,
            false,
        ),
        (br#"{"a":"old","a":"new"}"#, "old", None, true),
        (br#"{"a":"old","a":"new"}"#, "old", Some("a"), false),
        (br#"{"a":"old","a":null}"#, "old", Some("a"), false),
        (br#"{"a":{"b":"old"},"a":{}}"#, "old", Some("a.b"), false),
        (br#"{"a":"new","a":"old"}"#, "old", Some("a"), true),
        (br#"{"\u0061":"old","a":"new"}"#, "old", Some("a"), false),
        (br#"{"n":1e0}"#, "1e0", None, true),
        (
            br#"{"a":["local history","archives"]}"#,
            "history",
            Some("a"),
            true,
        ),
    ];
    for (bytes, literal, field, expected) in cases {
        let original = bytes.to_vec();
        let result = borrowed_scan(bytes, Some(literal), *field, None, true, &mut scratch);
        assert_eq!(result.fts_match, *expected, "{bytes:?} {literal} {field:?}");
        assert_eq!(*bytes, original);
    }
    let long = format!(
        "{{\"text\":\"{}needle{}\"}}",
        "a".repeat(8192),
        "b".repeat(8192)
    );
    assert!(
        borrowed_scan(
            long.as_bytes(),
            Some("aneedleb"),
            None,
            None,
            true,
            &mut scratch
        )
        .fts_match
    );
}

#[test]
fn borrowed_root_field_selects_visible_nested_scalars() {
    let mut scratch = BorrowedScanScratch::default();
    for (source, expected) in [
        (r#"{"title":"needle"}"#, true),
        (r#"{"nested":{"title":"needle"}}"#, true),
        (r#"["other",{"title":"needle"}]"#, true),
        (r#""needle""#, true),
        (r#"{"title":"needle","title":"other"}"#, false),
        (r#"{"title":"needle","title":null}"#, false),
        (r#"{"title":{"name":"needle"},"title":{}}"#, false),
    ] {
        for decoded_scalars in [false, true] {
            let result = borrowed_scan(
                source.as_bytes(),
                Some("needle"),
                Some("@root"),
                None,
                decoded_scalars,
                &mut scratch,
            );
            assert_eq!(
                result.fts_match, expected,
                "{source} decoded={decoded_scalars}"
            );
        }
    }
}

#[test]
fn borrowed_source_filter_preserves_string_runtime_rules_and_wildcards() {
    let mut scratch = BorrowedScanScratch::default();
    for (bytes, filter, expected) in [
        (br#"{"name":"axxb"}"#.as_slice(), "name ~ 'a%b'", true),
        (br#"{"name":"a%b"}"#, "name ~ 'a\\%b'", true),
        (br#"{"name":"axxb"}"#, "name ~ 'a\\%b'", false),
        (br#"{"name":"a\u0025b"}"#, "name ~ 'a\\%b'", true),
        (br#"{"name":"a\nb"}"#, "name ~ 'a%b'", true),
        (br#"{"name":"old","name":"new"}"#, "name ~ 'old'", false),
        (br#"{"name":"new","name":"old"}"#, "name ~ 'old'", true),
        (br#"{"name":null}"#, "name !~ 'x'", false),
        (br#"{"name":7}"#, "name !~ 'x'", false),
        (br#"{"other":"x"}"#, "name !~ 'x'", false),
        (br#"{"name":"ab"}"#, "name ~ ''", true),
        (br#"{"name":["Ada","Ada"]}"#, "name ~ 'A%a'", true),
        (br#"{"name":["Ada","Bob"]}"#, "name ~ 'A%a'", false),
        (br#"{"name":[]}"#, "name ~ 'A%a'", true),
        (br#"{"name":[]}"#, "name ?~ 'A%a'", false),
        (br#"{"name":["Bob","Ada"]}"#, "name ?~ 'A%a'", true),
    ] {
        let result = borrowed_scan(bytes, None, None, Some(filter), true, &mut scratch);
        assert_eq!(result.filter_match, expected, "{bytes:?} {filter}");
    }
}

#[test]
fn filter_only_scan_skips_irrelevant_nested_values() {
    let nested = format!(
        "[{}]",
        (0..2000)
            .map(|index| format!(r#"{{"nested":{{"value":"{index}"}}}}"#))
            .collect::<Vec<_>>()
            .join(",")
    );
    let source = format!(r#"{{"wanted":"needle","irrelevant":{nested}}}"#);
    let mut scratch = BorrowedScanScratch::default();
    let before = borrowed_node_visits();
    let result = borrowed_scan(
        source.as_bytes(),
        None,
        None,
        Some("wanted ~ 'needle'"),
        true,
        &mut scratch,
    );
    assert!(result.filter_match);
    assert!(borrowed_node_visits() - before < 10);
}

#[test]
fn combined_value_and_source_matching_avoids_owned_segments() {
    let source = br#"{"text":"old","text":"new","n":1e0,"other":"\u006e eedle"}"#;
    let schema = Schema::default();
    let projection = ScanProjection::full(true);
    for (literal, field, expected) in [
        ("old", None, true),
        ("old", Some("text"), false),
        ("new", Some("text"), true),
        ("1e0", None, true),
    ] {
        let fts = bind_fts(
            &FtsQuery {
                case_sensitivity: CaseSensitivity::Sensitive,
                mode: FtsMode::Literal,
                literal: literal.into(),
                pattern_parameter: None,
                fields: field
                    .map(|field| FtsField {
                        path: FieldPath::parse(field).unwrap(),
                        query_path: None,
                        binding: None,
                    })
                    .into_iter()
                    .collect(),
            },
            &schema,
        )
        .unwrap();
        let before = owned_segment_text_bytes();
        let (value, result) =
            extract_query_record_borrowed_fts_worker(source, &projection, &fts, true).unwrap();
        assert_eq!(result.fts_match, expected, "{literal} {field:?}");
        assert_eq!(owned_segment_text_bytes(), before);
        let Value::Object(fields) = value else {
            panic!("object")
        };
        assert_eq!(fields.get("text"), Some(&Value::String("new".into())));
    }
}

#[test]
fn borrowed_source_evidence_excludes_overwritten_fields() {
    let schema = Schema::default();
    let fts = bind_fts(
        &FtsQuery {
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Literal,
            literal: "old".into(),
            pattern_parameter: None,
            fields: vec![FtsField {
                path: FieldPath::parse("name").unwrap(),
                query_path: None,
                binding: None,
            }],
        },
        &schema,
    )
    .unwrap();
    let mut scratch = BorrowedScanScratch::default();
    let bytes = br#"{"name":"old","name":"new"}"#;
    for retain_evidence in [false, true] {
        let result = scan_borrowed_segments(
            bytes,
            &BorrowedScanQuery {
                fts: Some(&fts),
                filter: None,
                decoded_scalars: true,
                retain_evidence,
            },
            &mut scratch,
        )
        .unwrap();
        assert!(!result.fts_match);
        assert!(result.evidence.is_empty());
    }
    let result = scan_borrowed_segments(
        br#"{"name":"new","name":"old"}"#,
        &BorrowedScanQuery {
            fts: Some(&fts),
            filter: None,
            decoded_scalars: true,
            retain_evidence: true,
        },
        &mut scratch,
    )
    .unwrap();
    assert!(result.fts_match);
    assert!(result.evidence.contains("name"));
}

#[test]
fn borrowed_decoder_uses_source_slice_or_reused_storage() {
    let plain = "\"Straße\"".as_bytes();
    let escaped = br#""before\u2139after\ud83d\ude80""#;
    let mut decoded = String::new();
    with_quoted_text(plain, 0..plain.len(), &mut decoded, |text| {
        assert_eq!(text.as_ptr(), plain[1..].as_ptr());
        assert_eq!(text, "Straße");
        Ok(())
    })
    .unwrap();
    assert_eq!(decoded.capacity(), 0);
    with_quoted_text(escaped, 0..escaped.len(), &mut decoded, |text| {
        assert_eq!(text, "beforeℹafter🚀");
        Ok(())
    })
    .unwrap();
    let capacity = decoded.capacity();
    let pointer = decoded.as_ptr();
    for token in [br#""\u0061""#.as_slice(), br#""\n""#, br#""\u00df""#] {
        with_quoted_text(token, 0..token.len(), &mut decoded, |_| Ok(())).unwrap();
        assert_eq!(decoded.as_ptr(), pointer);
        assert_eq!(decoded.capacity(), capacity);
    }
}

#[test]
#[ignore = "release-only decoder comparison; run serially with --nocapture"]
fn decoder_strategy_microbench() {
    use std::hint::black_box;
    use std::time::Instant;

    let cases = [
        ("plain-ascii", format!("\"{}\"", "a".repeat(8192))),
        ("plain-unicode", format!("\"{}\"", "Straßeℹ".repeat(1024))),
        (
            "sparse-simple",
            format!("\"{}\"", format!("{}\\n", "a".repeat(1024)).repeat(8)),
        ),
        ("dense-simple", format!("\"{}\"", "a\\n".repeat(2048))),
        ("unicode-escape", format!("\"{}\"", "\\u2139".repeat(1024))),
        (
            "surrogate-pair",
            format!("\"{}\"", "\\ud83d\\ude80".repeat(512)),
        ),
        ("alternating", format!("\"{}\"", "a\\n".repeat(2048))),
    ];
    let short = br#""x\n""#;
    for (name, source) in cases {
        let token = source.as_bytes();
        let expected: String = serde_json::from_slice(token).unwrap();
        for strategy in [0, 1, 2, 3] {
            let mut decoded = String::new();
            let (_, actual) = match strategy {
                0 => with_next_quoted_text_strategy::<0, _>(token, 0, &mut decoded, |text| {
                    Ok(text.to_owned())
                }),
                1 => with_next_quoted_text_strategy::<1, _>(token, 0, &mut decoded, |text| {
                    Ok(text.to_owned())
                }),
                2 => with_next_quoted_text_strategy::<2, _>(token, 0, &mut decoded, |text| {
                    Ok(text.to_owned())
                }),
                _ => with_next_quoted_text_strategy::<3, _>(token, 0, &mut decoded, |text| {
                    Ok(text.to_owned())
                }),
            }
            .unwrap();
            assert_eq!(actual, expected);
        }
        let iterations = (8 * 1024 * 1024 / token.len()).max(128);
        for (label, strategy) in [
            ("repeated-memchr", 0),
            ("first-memchr-byte-loop", 1),
            ("adaptive-8", 2),
            ("adaptive-4", 3),
            ("predecessor-serde-owned", 4),
        ] {
            let mut samples = Vec::new();
            for round in 0..6 {
                let mut decoded = String::with_capacity(expected.len());
                let started = Instant::now();
                for iteration in 0..iterations {
                    let input = if name == "alternating" && iteration % 2 == 0 {
                        short.as_slice()
                    } else {
                        token
                    };
                    match strategy {
                        0 => {
                            let (_, length) = with_next_quoted_text_strategy::<0, _>(
                                black_box(input),
                                0,
                                &mut decoded,
                                |text| Ok(text.len()),
                            )
                            .unwrap();
                            black_box(length);
                        }
                        1 => {
                            let (_, length) = with_next_quoted_text_strategy::<1, _>(
                                black_box(input),
                                0,
                                &mut decoded,
                                |text| Ok(text.len()),
                            )
                            .unwrap();
                            black_box(length);
                        }
                        2 => {
                            let (_, length) = with_next_quoted_text_strategy::<2, _>(
                                black_box(input),
                                0,
                                &mut decoded,
                                |text| Ok(text.len()),
                            )
                            .unwrap();
                            black_box(length);
                        }
                        3 => {
                            let (_, length) = with_next_quoted_text_strategy::<3, _>(
                                black_box(input),
                                0,
                                &mut decoded,
                                |text| Ok(text.len()),
                            )
                            .unwrap();
                            black_box(length);
                        }
                        _ => {
                            let value = serde_json::Deserializer::from_slice(black_box(input))
                                .into_iter::<String>()
                                .next()
                                .unwrap()
                                .unwrap();
                            black_box(value);
                        }
                    }
                }
                if round > 0 {
                    samples.push(started.elapsed().as_nanos() / iterations as u128);
                }
            }
            samples.sort_unstable();
            println!(
                "decoder {name} {label}: {} ns/token median of 5",
                samples[2]
            );
        }
    }
}

#[test]
fn borrowed_source_matching_preserves_unicode_folded_representations() {
    let schema = Schema::default();
    let fts = bind_fts(
        &FtsQuery {
            case_sensitivity: CaseSensitivity::Insensitive,
            mode: FtsMode::Literal,
            literal: "STRASSE".into(),
            pattern_parameter: None,
            fields: vec![FtsField {
                path: FieldPath::parse("name").unwrap(),
                query_path: None,
                binding: Some("lower".into()),
            }],
        },
        &schema,
    )
    .unwrap();
    let filter = bind_filter(&parse_filter("name:lower ~ 'str%se'").unwrap(), &schema).unwrap();
    let BoundExpr::Term(filter) = &filter else {
        panic!("one term")
    };
    let mut scratch = BorrowedScanScratch::default();
    let result = scan_borrowed_segments(
        "{\"name\":\"Straße\"}".as_bytes(),
        &BorrowedScanQuery {
            fts: Some(&fts),
            filter: Some(filter),
            decoded_scalars: true,
            retain_evidence: true,
        },
        &mut scratch,
    )
    .unwrap();
    assert!(result.fts_match);
    assert!(result.filter_match);
    assert!(result.evidence.contains("name"));
}

#[test]
fn borrowed_source_worker_scratch_reuses_capacity_across_records() {
    let schema = Schema::default();
    let fts = bind_fts(
        &FtsQuery {
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Literal,
            literal: "needle".into(),
            pattern_parameter: None,
            fields: vec![],
        },
        &schema,
    )
    .unwrap();
    let query = BorrowedScanQuery {
        fts: Some(&fts),
        filter: None,
        decoded_scalars: true,
        retain_evidence: false,
    };
    let owned_before = owned_query_construction_counts();
    let copied_before = owned_segment_text_bytes();
    let long = format!("{{\"text\":\"{}\\u006e{}\"}}", "x".repeat(4096), "eedle");
    assert!(
        scan_borrowed_segments_worker(long.as_bytes(), &query)
            .unwrap()
            .fts_match
    );
    let (pointer, capacity) = BORROWED_SCAN_SCRATCH.with(|scratch| {
        let scratch = scratch.borrow();
        (
            scratch.decoded_text.as_ptr(),
            scratch.decoded_text.capacity(),
        )
    });
    assert!(capacity >= 4096);
    for bytes in [br#"{"text":"plain"}"#.as_slice(), br#"{"text":"\u006e"}"#] {
        assert!(
            !scan_borrowed_segments_worker(bytes, &query)
                .unwrap()
                .fts_match
        );
        BORROWED_SCAN_SCRATCH.with(|scratch| {
            let scratch = scratch.borrow();
            assert_eq!(scratch.decoded_text.as_ptr(), pointer);
            assert_eq!(scratch.decoded_text.capacity(), capacity);
        });
    }
    let exceptionally_long = format!(
        "{{\"text\":\"{}\\u006e\"}}",
        "x".repeat(MAX_SCAN_SCRATCH_CAPACITY + 1)
    );
    let _ = scan_borrowed_segments_worker(exceptionally_long.as_bytes(), &query).unwrap();
    BORROWED_SCAN_SCRATCH.with(|scratch| {
        let scratch = scratch.borrow();
        assert!(scratch.decoded_text.capacity() <= MAX_SCAN_SCRATCH_CAPACITY);
        assert!(
            scratch.events.capacity() * std::mem::size_of::<BorrowedEvent>()
                <= MAX_SCAN_SCRATCH_CAPACITY
        );
    });
    assert_eq!(owned_query_construction_counts(), owned_before);
    assert_eq!(owned_segment_text_bytes(), copied_before);
}

#[test]
fn oversized_folded_segments_reuse_capacity_within_record_then_release_it() {
    let fts = bind_fts(
        &FtsQuery {
            case_sensitivity: CaseSensitivity::Insensitive,
            mode: FtsMode::Literal,
            literal: "needle".into(),
            pattern_parameter: None,
            fields: Vec::new(),
        },
        &Schema::default(),
    )
    .unwrap();
    let query = BorrowedScanQuery {
        fts: Some(&fts),
        filter: None,
        decoded_scalars: true,
        retain_evidence: true,
    };
    let segment = format!("{}NEEDLE", "x".repeat(MAX_SCAN_SCRATCH_CAPACITY + 4096));
    let source = format!(r#"{{"a":"{segment}","b":"{segment}"}}"#);
    let before = crate::reference::fold_scratch_stats().0;
    let matched = scan_borrowed_segments_worker(source.as_bytes(), &query).unwrap();
    assert!(matched.fts_match);
    let (grows, retained) = crate::reference::fold_scratch_stats();
    assert_eq!(
        grows - before,
        1,
        "second oversized segment should reuse capacity"
    );
    assert!(retained <= MAX_SCAN_SCRATCH_CAPACITY);
}

#[test]
fn decoded_scalar_text_uses_values_not_names_or_container_syntax() {
    let bytes = br#"{"name_only": ["a\n", 1.0, 1e0, -0, true, false, null, {"v":"\u00df"}]}"#;
    let all = scalar_segments(bytes, false, true).unwrap();
    assert_eq!(
        all.iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>(),
        ["a\n", "1.0", "1e0", "-0", "true", "false", "null", "ß"]
    );
    assert!(
        all.iter()
            .all(|segment| segment.path.first().is_some_and(|part| part == "name_only"))
    );
}

#[test]
fn all_text_keeps_duplicate_members_but_selected_text_uses_last_wins() {
    let bytes = br#"{"title":"old","title":{"nested":"new"},"array":[1,"two"]}"#;
    let all = scalar_segments(bytes, false, true).unwrap();
    assert_eq!(
        all.iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>(),
        ["old", "new", "1", "two"]
    );
    let resolved = scalar_segments(bytes, true, true).unwrap();
    assert_eq!(
        resolved
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>(),
        ["new", "1", "two"]
    );
}

#[test]
fn fresh_build_decodes_value_and_scalar_text_in_one_walk() {
    let documents: &[&[u8]] = &[
        br#"{"title":"first","title":"last","mixed":[null,true,1,1.0,1e0,-0,{"x":"\u00df"}]}"#,
        br#"["first",123,false,null,{"nested":["\ud83d\ude80",1e1000000]}]"#,
        br#""a root string""#,
        b"1234567890123456789012345678901234567890",
        b"true",
        b"null",
        b"[]",
        b"{}",
    ];
    for bytes in documents {
        let (value, text) = build_record(bytes).unwrap();
        let expected = Value::from_json(serde_json::from_slice(bytes).unwrap()).unwrap();
        assert_eq!(value, expected, "{}", String::from_utf8_lossy(bytes));
        assert_eq!(
            text.iter()
                .map(|segment| segment.text.as_str())
                .collect::<Vec<_>>(),
            scalar_segments(bytes, false, false)
                .unwrap()
                .iter()
                .map(|segment| segment.text.as_str())
                .collect::<Vec<_>>(),
            "{}",
            String::from_utf8_lossy(bytes),
        );
    }
}

#[test]
fn fresh_build_rejects_malformed_json_without_a_validated_first_pass() {
    for bytes in [
        b"[1,]".as_slice(),
        b"{\"a\":1,}".as_slice(),
        b"truefalse".as_slice(),
        b"1e".as_slice(),
        b"\"bad\\q\"".as_slice(),
    ] {
        assert!(build_record(bytes).is_err(), "{bytes:?}");
    }
}

#[test]
fn query_extraction_matches_resolved_values_and_source_scalar_text() {
    let documents: &[&[u8]] = &[
        br#"{"title":"old","title":{"inner":"new"},"n":1e0,"items":[true,null,"\ud83d\ude80"]}"#,
        br#"{"a":{"x":"lost"},"a":{},"b":[{"x":"first"},{"x":"last"}],"a.b":"dot"}"#,
        br#"{"\u0061":"first","a":["second",{"nested":-0}],"empty":[]}"#,
        br#"[{"a":"one"},{"a":"two"},12.5,false,null]"#,
        br#""root\u00df""#,
        b"1234567890123456789012345678901234567890",
        b"true",
        b"null",
        b"{}",
        b"[]",
    ];
    for bytes in documents {
        for selected in [
            None,
            Some(HashSet::from(["title".into(), "a".into(), "n".into()])),
        ] {
            for collect_text in [false, true] {
                for resolved_text_only in [false, true] {
                    let projection = ScanProjection {
                        top_level: selected.clone(),
                        collect_text,
                        resolved_text_only,
                    };
                    let (actual_value, actual_text) =
                        extract_query_record(bytes, &projection, false).unwrap();
                    let (trusted_value, trusted_text) =
                        extract_query_record(bytes, &projection, true).unwrap();
                    assert_eq!(actual_value, trusted_value, "{bytes:?}");
                    assert_eq!(
                        actual_text
                            .iter()
                            .map(|segment| (&segment.path, &segment.text))
                            .collect::<Vec<_>>(),
                        trusted_text
                            .iter()
                            .map(|segment| (&segment.path, &segment.text))
                            .collect::<Vec<_>>(),
                        "{bytes:?}",
                    );
                    let mut expected_json: serde_json::Value =
                        serde_json::from_slice(bytes).unwrap();
                    if let (Some(selected), Some(object)) =
                        (&selected, expected_json.as_object_mut())
                    {
                        object.retain(|name, _| selected.contains(name));
                    }
                    let expected_value = Value::from_json(expected_json).unwrap();
                    assert_eq!(actual_value, expected_value, "{bytes:?}");
                    let expected_text = if collect_text {
                        scalar_segments(bytes, resolved_text_only, true).unwrap()
                    } else {
                        Vec::new()
                    };
                    assert_eq!(
                        actual_text
                            .iter()
                            .map(|segment| (&segment.path, &segment.text))
                            .collect::<Vec<_>>(),
                        expected_text
                            .iter()
                            .map(|segment| (&segment.path, &segment.text))
                            .collect::<Vec<_>>(),
                        "{bytes:?}",
                    );
                }
            }
        }
    }
}

#[test]
fn trusted_projection_does_not_validate_omitted_content_or_trailing_bytes() {
    let projection = ScanProjection {
        top_level: Some(HashSet::from(["wanted".into()])),
        collect_text: false,
        resolved_text_only: false,
    };
    let edited = br#"{"wanted":5,"ignored":[1,]} trailing"#;
    assert!(extract_query_record(edited, &projection, false).is_err());
    let (value, text) = extract_query_record(edited, &projection, true).unwrap();
    assert_eq!(
        value,
        Value::Object(BTreeMap::from([(
            "wanted".into(),
            Value::Number(Number::integer(5)),
        )])),
    );
    assert!(text.is_empty());
    let escaped = br#"{"wanted":"a\n\"b","ignored":!} trailing"#;
    assert!(extract_query_record(escaped, &projection, false).is_err());
    let (value, _) = extract_query_record(escaped, &projection, true).unwrap();
    assert_eq!(
        value,
        Value::Object(BTreeMap::from([(
            "wanted".into(),
            Value::String("a\n\"b".into()),
        )])),
    );
    assert!(extract_query_record(br#"{"wanted":5,"ignored":[1,2}"#, &projection, true).is_err());
    let full_text = ScanProjection::full(true);
    assert!(extract_query_record(br#"{"title":"found"} stale"#, &full_text, false).is_err());
    let (_, text) = extract_query_record(br#"{"title":"found"} stale"#, &full_text, true).unwrap();
    assert_eq!(text.len(), 1);
    assert_eq!(text[0].text, "found");
}

#[test]
fn validating_query_extraction_rejects_invalid_json_independently_of_projection() {
    let projection = ScanProjection {
        top_level: Some(HashSet::from(["wanted".into()])),
        collect_text: false,
        resolved_text_only: false,
    };
    for bytes in [
        b"{\"wanted\":1,\"ignored\":[1,]}".as_slice(),
        b"{\"wanted\":1,\"ignored\":\"bad\\q\"}".as_slice(),
        b"{\"wanted\":1,\"ignored\":01}".as_slice(),
        b"{\"wanted\":1,\"ignored\":true} extra".as_slice(),
        b"{\"wanted\":1,\"ignored\":\"\xff\"}".as_slice(),
        b"{\"wanted\":1,\"ignored\":\x0b0}".as_slice(),
        b"[[1]".as_slice(),
    ] {
        assert!(serde_json::from_slice::<serde_json::Value>(bytes).is_err());
        assert!(
            extract_query_record(bytes, &projection, false).is_err(),
            "{bytes:?}"
        );
    }
}

#[test]
fn many_duplicate_members_preserve_all_text_and_erase_replaced_subtrees() {
    let mut source = String::from("{");
    for index in 0..256 {
        if index > 0 {
            source.push(',');
        }
        source.push_str(&format!(
            "\"item\":{{\"number\":{index},\"text\":\"value{index}\"}}"
        ));
    }
    source.push_str(",\"item\":{}}");
    let all = ScanProjection::full(true);
    let (value, text) = extract_query_record(source.as_bytes(), &all, false).unwrap();
    assert_eq!(
        value,
        Value::Object(BTreeMap::from([(
            "item".into(),
            Value::Object(BTreeMap::new()),
        )])),
    );
    assert_eq!(text.len(), 512);
    let resolved = ScanProjection::full_resolved_text();
    let (_, text) = extract_query_record(source.as_bytes(), &resolved, false).unwrap();
    assert!(text.is_empty());
}

#[test]
fn quote_search_handles_escaped_quotes_and_backslash_parity() {
    for slashes in 0..20 {
        let expected = format!("prefix{}\"suffix", "\\".repeat(slashes));
        let bytes = serde_json::to_vec(&expected).unwrap();
        let (value, text) =
            extract_query_record(&bytes, &ScanProjection::full(true), false).unwrap();
        assert_eq!(value, Value::String(expected.clone()));
        assert_eq!(text.len(), 1);
        assert_eq!(text[0].text, expected);
    }
}
