use super::super::{owned_query_construction_counts, owned_segment_text_bytes};
use super::*;
use objekte_search_core::lifecycle::initial::test_hooks::{
    bind_scan_filter as bind_filter, bind_scan_text as bind_fts, fold_candidate_transforms,
    fold_scratch_stats,
};
use objekte_search_core::model::{FieldPath, Value};
use objekte_search_core::query::{CaseSensitivity, FtsField, FtsMode, FtsQuery, parse_filter};
use objekte_search_core::schema::Schema;
use objekte_search_core::source::ScanProjection;

#[test]
fn combined_borrowed_fts_and_filter_fold_each_segment_once() {
    let schema = Schema::default();
    let fts = bind_fts(
        &FtsQuery {
            case_sensitivity: CaseSensitivity::Insensitive,
            mode: FtsMode::Literal,
            literal: "STRASSE".into(),
            pattern_parameter: None,
            fields: Vec::new(),
        },
        &schema,
    )
    .unwrap();
    let filter = bind_filter(&parse_filter("name:lower ~ 'stra'").unwrap(), &schema).unwrap();
    let term = &filter;
    let query = BorrowedScanQuery {
        fts: Some((&fts).into()),
        filter: Some(term.into()),
        decoded_scalars: false,
        retain_evidence: true,
    };
    let before = fold_candidate_transforms();
    let result = scan_borrowed_segments_worker(r#"{"name":"Straße"}"#.as_bytes(), &query).unwrap();
    assert!(result.fts_match);
    assert!(result.filter_match);
    assert_eq!(fold_candidate_transforms() - before, 1);
}

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
    let filter = filter.as_ref();
    scan_borrowed_segments(
        bytes,
        &BorrowedScanQuery {
            fts: fts.as_ref().map(Into::into),
            filter: filter.map(Into::into),
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
        (
            br#"{"outer":{"name":"old"},"outer":{"name":"new"}}"#,
            "outer.name ~ 'old'",
            false,
        ),
        (
            br#"{"outer":{"name":"old"},"outer":{}}"#,
            "outer.name ~ ''",
            false,
        ),
        (br#"{"name":null}"#, "name !~ 'x'", false),
        (br#"{"name":7}"#, "name !~ 'x'", false),
        (br#"{"other":"x"}"#, "name !~ 'x'", false),
        (br#"{"name":"ab"}"#, "name ~ ''", true),
        (br#"{"name":"a\u0025b"}"#, "name ~ ''", true),
        (br#"{"name":"ab","name":null}"#, "name ~ ''", false),
        (br#"{"name":["Ada","Ada"]}"#, "name ~ 'A%a'", true),
        (br#"{"name":["Ada","Bob"]}"#, "name ~ 'A%a'", false),
        (br#"{"name":[]}"#, "name ~ 'A%a'", false),
        (br#"{"name":[]}"#, "name !~ 'x'", false),
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
    let unrelated_scalars = (0..1000)
        .map(|index| format!(r#""other{index}":"unused""#))
        .collect::<Vec<_>>()
        .join(",");
    let source = format!(r#"{{"wanted":"needle","irrelevant":{nested},{unrelated_scalars}}}"#);
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
fn unscoped_fts_with_filter_still_visits_nested_text() {
    let source = br#"{"nested":{"text":"needle"},"wanted":"yes"}"#;
    let mut scratch = BorrowedScanScratch::default();
    let result = borrowed_scan(
        source,
        Some("needle"),
        None,
        Some("wanted ~ 'yes'"),
        true,
        &mut scratch,
    );
    assert!(result.fts_match);
    assert!(result.filter_match);
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
            extract_query_record_borrowed_fts_worker(source, &projection, (&fts).into(), true)
                .unwrap();
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
                fts: Some((&fts).into()),
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
            fts: Some((&fts).into()),
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
#[ignore = "release-only in-memory production consumer timing; run serially with --nocapture"]
fn borrowed_consumer_microbench() {
    use std::hint::black_box;
    use std::time::Instant;

    let schema = Schema::default();
    let fts = bind_fts(
        &FtsQuery {
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Literal,
            literal: "needle".into(),
            pattern_parameter: None,
            fields: Vec::new(),
        },
        &schema,
    )
    .unwrap();
    let projection = ScanProjection::full(true);
    for (name, source) in [
        (
            "plain",
            format!(r#"{{"text":"{}needle"}}"#, "a".repeat(8192)),
        ),
        (
            "escaped",
            format!(r#"{{"text":"{}needle"}}"#, "a\\n".repeat(2048)),
        ),
        (
            "many-scalars",
            format!(r#"{{"text":[{}"needle"]}}"#, "\"abc\",".repeat(512)),
        ),
    ] {
        let bytes = source.as_bytes();
        let direct = BorrowedScanQuery {
            fts: Some((&fts).into()),
            filter: None,
            decoded_scalars: true,
            retain_evidence: false,
        };
        assert!(
            scan_borrowed_segments_worker(bytes, &direct)
                .unwrap()
                .fts_match
        );
        assert!(
            extract_query_record_borrowed_fts_worker(bytes, &projection, (&fts).into(), false)
                .unwrap()
                .1
                .fts_match
        );
        let iterations = (2 * 1024 * 1024 / bytes.len()).max(128);
        for (route, mut measure) in [
            (
                "direct",
                Box::new(|| {
                    black_box(scan_borrowed_segments_worker(black_box(bytes), &direct).unwrap());
                }) as Box<dyn FnMut() + '_>,
            ),
            (
                "combined-value",
                Box::new(|| {
                    black_box(
                        extract_query_record_borrowed_fts_worker(
                            black_box(bytes),
                            &projection,
                            (&fts).into(),
                            false,
                        )
                        .unwrap(),
                    );
                }) as Box<dyn FnMut() + '_>,
            ),
        ] {
            let mut samples = Vec::new();
            for round in 0..6 {
                let started = Instant::now();
                for _ in 0..iterations {
                    measure();
                }
                if round > 0 {
                    samples.push(started.elapsed().as_nanos() / iterations as u128);
                }
            }
            samples.sort_unstable();
            println!(
                "consumer {name} {route}: {} ns/record median of 5; min {}; max {}; bytes {}",
                samples[2],
                samples[0],
                samples[4],
                bytes.len()
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
    let filter = &filter;
    let mut scratch = BorrowedScanScratch::default();
    let result = scan_borrowed_segments(
        "{\"name\":\"Straße\"}".as_bytes(),
        &BorrowedScanQuery {
            fts: Some((&fts).into()),
            filter: Some(filter.into()),
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
        fts: Some((&fts).into()),
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
        fts: Some((&fts).into()),
        filter: None,
        decoded_scalars: true,
        retain_evidence: true,
    };
    let segment = format!("{}NEEDLE", "x".repeat(MAX_SCAN_SCRATCH_CAPACITY + 4096));
    let source = format!(r#"{{"a":"{segment}","b":"{segment}"}}"#);
    let before = fold_scratch_stats().0;
    let matched = scan_borrowed_segments_worker(source.as_bytes(), &query).unwrap();
    assert!(matched.fts_match);
    let (grows, retained) = fold_scratch_stats();
    assert_eq!(
        grows - before,
        1,
        "second oversized segment should reuse capacity"
    );
    assert!(retained <= MAX_SCAN_SCRATCH_CAPACITY);
}

#[test]
fn oversized_folded_scratch_is_released_on_scan_error() {
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
        fts: Some((&fts).into()),
        filter: None,
        decoded_scalars: true,
        retain_evidence: false,
    };
    let source = format!(
        r#"{{"text":"{}NEEDLE","broken": "#,
        "x".repeat(MAX_SCAN_SCRATCH_CAPACITY + 4096)
    );
    assert!(scan_borrowed_segments_worker(source.as_bytes(), &query).is_err());
    assert!(fold_scratch_stats().1 <= MAX_SCAN_SCRATCH_CAPACITY);
}
