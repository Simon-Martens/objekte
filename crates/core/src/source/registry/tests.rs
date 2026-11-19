use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::execution_options::ReturnMode;
use crate::inventory::{ByteRange, SourceRevision};
use crate::model::{LogicalType, Value};
use crate::query::{
    CaseSensitivity, FtsField, FtsMode, FtsQuery, Query, QueryOptions, parse_filter,
};
use crate::repository::{CollectionDefinition, IdentityPolicy, Payload, RepositoryOptions};
use crate::schema::{FieldDefinition, Schema};
use crate::source::extraction::extract_file_outcomes;
use crate::source::test_support::{OutcomeReader, TinyReader};

use super::super::{
    DiagnosticCategory, ExtractedRecord, SourceFormat, SourceOutcome, SourceReader, TextSegment,
    registry::SourceRegistry,
};

struct CustomReader {
    parses: Arc<AtomicUsize>,
    name: &'static str,
}

impl SourceReader for CustomReader {
    fn name(&self) -> &'static str {
        self.name
    }

    fn logical_text_coverage(&self) -> super::super::LogicalTextCoverage {
        super::super::LogicalTextCoverage::AllStringFields
    }

    fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String> {
        let mut ranges = Vec::new();
        let mut start = 0usize;
        for (index, byte) in file.iter().enumerate() {
            if *byte == b'\n' {
                if start < index {
                    ranges.push(ByteRange::checked(
                        start as u64,
                        index as u64,
                        file.len() as u64,
                    )?);
                }
                start = index + 1;
            }
        }
        if start < file.len() {
            ranges.push(ByteRange::checked(
                start as u64,
                file.len() as u64,
                file.len() as u64,
            )?);
        }
        Ok(ranges)
    }

    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        self.parses.fetch_add(1, Ordering::Relaxed);
        let line = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
        let (title, body) = line.split_once('|').ok_or("missing custom separator")?;
        Ok(ExtractedRecord {
            value: Value::Object(std::collections::BTreeMap::from([
                ("id".into(), Value::String(format!("{title}-{body}"))),
                ("title".into(), Value::String(title.into())),
                ("body".into(), Value::String(body.into())),
            ])),
            text: vec![
                TextSegment {
                    path: vec!["title".into()],
                    text: title.into(),
                },
                TextSegment {
                    path: vec!["body".into()],
                    text: body.into(),
                },
            ],
        })
    }
}

fn schema() -> Schema {
    let mut schema = Schema::default();
    for (name, indexed) in [("id", false), ("title", true), ("body", false)] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind: LogicalType::String,
                required: true,
                nullable: false,
                indexed,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    schema
}

fn fts_query(evidence: bool) -> Query {
    Query {
        collections: vec!["data".into()],
        fts: Some(FtsQuery {
            pattern_parameter: None,
            mode: FtsMode::Literal,
            case_sensitivity: CaseSensitivity::Sensitive,
            literal: "needle".into(),
            fields: vec![FtsField {
                query_path: None,
                path: crate::model::FieldPath::parse("title").unwrap(),
                binding: None,
            }],
        }),
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence,
        },
    }
}

fn filter_query(expression: &str) -> Query {
    Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter(expression).unwrap()),
        options: QueryOptions {
            sort: Vec::new(),
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence: true,
        },
    }
}

fn combined_query(expression: &str) -> Query {
    let mut query = fts_query(true);
    query.filter = Some(parse_filter(expression).unwrap());
    query
}

#[test]
fn custom_readers_under_builtin_names_never_use_builtin_scalar_verification() {
    use crate::source::verification::ScalarVerification;

    let builtins = SourceRegistry::builtin();
    for (format, expected) in [
        (
            SourceFormat::Json,
            Some(ScalarVerification::ResolvedStrings),
        ),
        (
            SourceFormat::JsonLines,
            Some(ScalarVerification::ResolvedStrings),
        ),
        (
            SourceFormat::JsonDecodedScalars,
            Some(ScalarVerification::DecodedScalars),
        ),
    ] {
        let source = builtins.resolve(&format).unwrap();
        assert!(!source.is_extension());
        assert_eq!(source.scalar_verification(), expected);
    }

    for registration_id in ["json", "jsonl", "json_decoded_scalars", "custom-control"] {
        let root = std::env::temp_dir().join(format!(
            "objekte-custom-builtin-{registration_id}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(
            root.join("data/records.txt"),
            b"needle|alpha\nother|beta\nneedle|beta\n",
        )
        .unwrap();

        let parses = Arc::new(AtomicUsize::new(0));
        let mut sources = SourceRegistry::default();
        sources
            .register(
                registration_id,
                1,
                "txt",
                Arc::new(CustomReader {
                    parses: parses.clone(),
                    name: if registration_id == "json" {
                        "json"
                    } else {
                        "custom-builtin-name"
                    },
                }),
            )
            .unwrap();
        let resolved = sources
            .resolve(&SourceFormat::registered(registration_id))
            .unwrap();
        assert!(resolved.is_extension());
        assert_eq!(resolved.scalar_verification(), None);
        let definition = CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::registered(registration_id),
            identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
            discovery: Default::default(),
            schema: schema(),
            sort_fields: Vec::new(),
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        };
        let index = crate::lifecycle::Index::start_build(
            &root,
            vec![definition],
            RepositoryOptions::default(),
            sources,
        )
        .and_then(|task| task.wait_complete())
        .map_err(|error| error.to_string())
        .unwrap();
        let view = index.view().unwrap();
        assert_eq!(parses.load(Ordering::Relaxed), 3);

        let fts = fts_query(true);
        let fts_found = view.search(&fts).unwrap();
        let fts_reference = view.search_reference(&fts).unwrap();
        assert_eq!(fts_found.hits, fts_reference.hits);
        assert_eq!(fts_found.count, Some(2));
        assert_eq!(
            fts_found
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>(),
            ["needle-alpha", "needle-beta"]
        );
        assert!(fts_found.hits.iter().all(|hit| {
            hit.file_path.ends_with("data/records.txt")
                && hit.byte_end > hit.byte_start
                && hit.matched_fields == ["title"]
                && matches!(&hit.payload, Some(Payload::Logical(Value::Object(value)))
                    if matches!(value["title"], Value::String(ref title) if title == "needle"))
        }));

        let parses_before_filter = parses.load(Ordering::Relaxed);
        let filter = filter_query("body ~ 'alpha'");
        let filter_found = view.search(&filter).unwrap();
        let filter_reference = view.search_reference(&filter).unwrap();
        assert_eq!(filter_found.hits, filter_reference.hits);
        assert_eq!(filter_found.count, Some(1));
        assert_eq!(filter_found.hits[0].record_id, "needle-alpha");
        assert_eq!(filter_found.hits[0].matched_fields, ["body"]);
        assert!(
            parses.load(Ordering::Relaxed) > parses_before_filter,
            "custom reader must run for residual FILTER verification"
        );

        let combined = combined_query("body ~ 'alpha'");
        let found = view.search(&combined).unwrap();
        let reference = view.search_reference(&combined).unwrap();
        assert_eq!(found.hits, reference.hits);
        assert_eq!(found.count, Some(1));
        assert_eq!(found.hits[0].record_id, "needle-alpha");
        assert_eq!(found.hits[0].matched_fields.len(), 2);
        assert!(
            found.hits[0]
                .matched_fields
                .iter()
                .any(|field| field == "title")
        );
        assert!(
            found.hits[0]
                .matched_fields
                .iter()
                .any(|field| field == "body")
        );
        assert!(
            matches!(&found.hits[0].payload, Some(Payload::Logical(Value::Object(value)))
            if value["body"] == Value::String("alpha".into()))
        );

        let false_case = combined_query("body ~ 'missing'");
        let found = view.search(&false_case).unwrap();
        assert_eq!(found.count, Some(0));
        assert_eq!(found.hits, view.search_reference(&false_case).unwrap().hits);
        std::fs::remove_dir_all(root).unwrap();
    }
}

struct TemporalSegmentsReader;

impl SourceReader for TemporalSegmentsReader {
    fn name(&self) -> &'static str {
        "temporal_segments"
    }

    fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String> {
        Ok(vec![ByteRange::checked(
            0,
            file.len() as u64,
            file.len() as u64,
        )?])
    }

    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
        let mut parts = text.split('|');
        let title = parts.next().ok_or("missing title")?;
        let body = parts.next().ok_or("missing body")?;
        let date = parts.next().ok_or("missing date")?;
        let (first, second) = body.split_once('^').ok_or("missing body boundary")?;
        Ok(ExtractedRecord {
            value: Value::Object(std::collections::BTreeMap::from([
                ("title".into(), Value::String(title.into())),
                ("body".into(), Value::String(body.into())),
                ("date".into(), Value::String(date.into())),
            ])),
            text: vec![
                TextSegment {
                    path: vec!["body".into()],
                    text: first.into(),
                },
                TextSegment {
                    path: vec!["body".into()],
                    text: second.into(),
                },
            ],
        })
    }
}

#[test]
fn temporal_registration_does_not_replace_reader_owned_text() {
    use crate::execution_options::{BigramAlphabet, ReturnMode};
    use crate::model::LogicalType;
    use crate::query::{FtsField, FtsQuery, Query, QueryOptions, parse_filter};
    use crate::repository::{CollectionDefinition, IdentityPolicy, RepositoryOptions};
    use crate::schema::{FieldDefinition, Schema};

    let root =
        std::env::temp_dir().join(format!("objekte-temporal-segments-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/a.seg"),
        b"secret title|needle^here|2026-01-02",
    )
    .unwrap();
    let definition = |temporal| {
        let mut schema = Schema::default();
        if temporal {
            schema
                .register_field(FieldDefinition {
                    path: vec!["date".into()],
                    kind: LogicalType::Date,
                    required: true,
                    nullable: false,
                    indexed: true,
                    multi: false,
                    element_nullable: vec![],
                })
                .unwrap();
        }
        CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::registered("seg"),
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }
    };
    let sources = || {
        let mut sources = SourceRegistry::builtin();
        sources
            .register("seg", 1, "seg", Arc::new(TemporalSegmentsReader))
            .unwrap();
        sources
    };
    let fts = |text: &str, field: Option<&str>| Query {
        collections: vec!["data".into()],
        filter: None,
        fts: Some(FtsQuery {
            pattern_parameter: None,
            mode: crate::query::FtsMode::Literal,
            case_sensitivity: crate::query::CaseSensitivity::Sensitive,
            literal: text.into(),
            fields: field
                .map(|name| {
                    vec![FtsField {
                        query_path: None,
                        path: crate::model::FieldPath::parse(name).unwrap(),
                        binding: None,
                    }]
                })
                .unwrap_or_default(),
        }),
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    };
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        for temporal in [false, true] {
            let options = RepositoryOptions {
                bigram_alphabet: alphabet,
                ..Default::default()
            };
            for open in 0..2 {
                let repo = crate::lifecycle::Index::build_repository_for_test(
                    &root,
                    vec![definition(temporal)],
                    options,
                    sources(),
                )
                .map_err(|error| error.to_string())
                .unwrap();
                if open == 0 {}
                for (text, field, count, evidence) in [
                    ("needle", None, 1, vec!["body"]),
                    ("here", Some("body"), 1, vec!["body"]),
                    ("needle^here", None, 0, vec![]),
                    ("secret", None, 0, vec![]),
                    ("secret", Some("title"), 0, vec![]),
                ] {
                    let found = repo.search(&fts(text, field)).unwrap();
                    assert_eq!(
                        found.count,
                        Some(count),
                        "{alphabet:?} {temporal} {text} {field:?}"
                    );
                    if count == 1 {
                        assert_eq!(found.hits[0].matched_fields, evidence);
                    }
                }
                if temporal {
                    let query = Query {
                        collections: vec!["data".into()],
                        fts: None,
                        filter: Some(parse_filter("date = '2026-01-02'").unwrap()),
                        options: QueryOptions {
                            sort: vec![],
                            limit: 10,
                            offset: 0,
                            count: true,
                            return_mode: ReturnMode::Logical,
                            evidence: true,
                        },
                    };
                    let result = repo.search(&query).unwrap();
                    assert_eq!(result.count, Some(1));
                    assert!(matches!(&result.hits[0].payload,
                        Some(crate::repository::Payload::Logical(Value::Object(fields)))
                        if matches!(fields["date"], Value::Date(_))));
                }
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn registered_ranges_are_validated_before_slicing_and_outcomes_are_typed() {
    let root = std::env::temp_dir().join(format!("objekte-reader-ranges-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("source.outcome");
    std::fs::write(&path, b"ok\nbad").unwrap();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let parses = Arc::new(AtomicUsize::new(0));
    for ranges in [
        vec![ByteRange { start: 5, end: 2 }],
        vec![ByteRange { start: 0, end: 7 }],
        vec![
            ByteRange { start: 0, end: 3 },
            ByteRange { start: 2, end: 6 },
        ],
        vec![
            ByteRange { start: 4, end: 6 },
            ByteRange { start: 0, end: 2 },
        ],
    ] {
        let reader = OutcomeReader {
            ranges,
            parses: parses.clone(),
            fail_framing: false,
        };
        let error = extract_file_outcomes(&path, &reader, revision, |_| Ok(())).unwrap_err();
        assert!(error.contains("reader contract"), "{error}");
        assert_eq!(parses.load(Ordering::Relaxed), 0);
    }
    let reader = OutcomeReader {
        ranges: vec![
            ByteRange { start: 0, end: 2 },
            ByteRange { start: 3, end: 6 },
        ],
        parses: parses.clone(),
        fail_framing: false,
    };
    let mut kinds = Vec::new();
    let counters = extract_file_outcomes(&path, &reader, revision, |outcome| {
        kinds.push(match outcome {
            SourceOutcome::Logical { range, .. } => (range, "logical"),
            SourceOutcome::Malformed {
                range, diagnostic, ..
            } => {
                assert_eq!(diagnostic.code, "outcome.syntax");
                (range, "malformed")
            }
            SourceOutcome::Unreadable { .. } => panic!("unexpected unreadable source"),
        });
        Ok(())
    })
    .unwrap();
    assert_eq!(
        kinds,
        [
            (ByteRange { start: 0, end: 2 }, "logical"),
            (ByteRange { start: 3, end: 6 }, "malformed")
        ]
    );
    assert_eq!(counters.framed_records, 2);
    assert_eq!(counters.record_parses, 1);
    assert_eq!(counters.failed_parses, 1);
    let framing = OutcomeReader {
        ranges: Vec::new(),
        parses: parses.clone(),
        fail_framing: true,
    };
    let mut fallback = None;
    extract_file_outcomes(&path, &framing, revision, |outcome| {
        if let SourceOutcome::Malformed {
            range, diagnostic, ..
        } = outcome
        {
            fallback = Some((range, diagnostic.category));
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(
        fallback,
        Some((ByteRange { start: 0, end: 6 }, DiagnosticCategory::Framing))
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn seeded_reader_ranges_obey_checked_nonoverlap_contract() {
    const SEED: u64 = 0xA11C_2026_1106;
    let root = std::env::temp_dir().join(format!("objekte-range-property-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("source.outcome");
    std::fs::write(&path, b"ok\nbad").unwrap();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let mut state = SEED;
    for case in 0..256 {
        let mut ranges = Vec::new();
        for _ in 0..1 + (case % 4) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let start = state % 9;
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let end = state % 9;
            ranges.push(ByteRange { start, end });
        }
        let mut previous_end = 0;
        let valid = ranges.iter().all(|range| {
            let valid = range.start <= range.end && range.end <= 6 && range.start >= previous_end;
            previous_end = range.end;
            valid
        });
        let reader = OutcomeReader {
            ranges: ranges.clone(),
            parses: Arc::new(AtomicUsize::new(0)),
            fail_framing: false,
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            extract_file_outcomes(&path, &reader, revision, |_| Ok(()))
        }));
        assert!(
            result.is_ok(),
            "seed={SEED:#x} case={case} ranges={ranges:?}"
        );
        let outcome = result.unwrap();
        assert_eq!(
            outcome.is_ok(),
            valid,
            "seed={SEED:#x} case={case} ranges={ranges:?} outcome={outcome:?}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn registered_recoverable_error_survives_repository_query_and_restore() {
    use crate::execution_options::ReturnMode;
    use crate::query::{FtsQuery, Query, QueryOptions};
    use crate::repository::{CollectionDefinition, IdentityPolicy, Payload, RepositoryOptions};
    use crate::schema::Schema;
    let root = std::env::temp_dir().join(format!(
        "objekte-registered-fallback-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/source.outcome"), b"ok\nbad").unwrap();
    let mut sources = SourceRegistry::builtin();
    sources
        .register(
            "outcome",
            1,
            "outcome",
            Arc::new(OutcomeReader {
                ranges: vec![
                    ByteRange { start: 0, end: 2 },
                    ByteRange { start: 3, end: 6 },
                ],
                parses: Arc::new(AtomicUsize::new(0)),
                fail_framing: false,
            }),
        )
        .unwrap();
    let definition = CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::registered("outcome"),
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema: Schema::default(),
        sort_fields: vec![],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    let options = RepositoryOptions {
        ..Default::default()
    };
    let built = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition.clone()],
        options,
        sources.clone(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert_eq!(built.collections()[0].generation.records.len(), 2);
    assert_eq!(built.files().files[0].diagnostics[0].code, "outcome.syntax");
    let query = Query {
        collections: vec!["data".into()],
        filter: None,
        fts: Some(FtsQuery {
            pattern_parameter: None,
            mode: crate::query::FtsMode::Literal,
            case_sensitivity: crate::query::CaseSensitivity::Sensitive,
            literal: "bad".into(),
            fields: vec![],
        }),
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence: true,
        },
    };
    let result = built.search(&query).unwrap();
    assert_eq!(result.count, Some(1));
    assert!(matches!(
        result.hits[0].payload,
        Some(Payload::LogicalUnavailable { .. })
    ));
    assert_eq!(result.hits[0].matched_fields, ["@record.raw_text"]);
    let restored = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition],
        options,
        sources,
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert_eq!(restored.search(&query).unwrap().hits, result.hits);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn body_only_reader_does_not_prune_unindexed_title() {
    use crate::execution_options::{BigramAlphabet, ReturnMode};
    use crate::model::LogicalType;
    use crate::query::{FtsField, FtsQuery, Query, QueryOptions, parse_filter};
    use crate::repository::{CollectionDefinition, IdentityPolicy, RepositoryOptions};
    use crate::schema::{FieldDefinition, Schema};

    let root = std::env::temp_dir().join(format!("objekte-body-only-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/items.tiny"),
        b"hello world|ordinary\nother|ordinary\n",
    )
    .unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["title".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let definition = CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::registered("tiny"),
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema,
        sort_fields: vec![],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    let sources = || {
        let mut sources = SourceRegistry::builtin();
        sources
            .register(
                "tiny",
                1,
                "tiny",
                Arc::new(TinyReader(Arc::new(AtomicUsize::new(0)))),
            )
            .unwrap();
        sources
    };
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let repo = crate::lifecycle::Index::build_repository_for_test(
            &root,
            vec![definition.clone()],
            RepositoryOptions {
                bigram_alphabet: alphabet,
                ..Default::default()
            },
            sources(),
        )
        .map_err(|error| error.to_string())
        .unwrap();
        for (filter, fts, expected) in [
            (Some("title ~ 'hello world'"), None, 1),
            (
                None,
                Some(FtsQuery {
                    pattern_parameter: None,
                    mode: crate::query::FtsMode::Literal,
                    case_sensitivity: crate::query::CaseSensitivity::Sensitive,
                    literal: "hello world".into(),
                    fields: vec![FtsField {
                        query_path: None,
                        path: crate::model::FieldPath::parse("title").unwrap(),
                        binding: None,
                    }],
                }),
                0,
            ),
        ] {
            let query = Query {
                collections: vec!["data".into()],
                filter: filter.map(|s| parse_filter(s).unwrap()),
                fts,
                options: QueryOptions {
                    sort: vec![],
                    limit: 10,
                    offset: 0,
                    count: true,
                    return_mode: ReturnMode::Bytes,
                    evidence: true,
                },
            };
            let found = repo.search(&query).unwrap();
            assert_eq!(found.count, Some(expected), "{alphabet:?} {filter:?}");
            if expected == 1 {
                assert!(
                    matches!(&found.hits[0].payload, Some(crate::repository::Payload::Bytes(raw)) if raw == b"hello world|ordinary")
                );
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn registered_non_json_reader_reaches_build_query_payload_and_restore() {
    use crate::execution_options::ReturnMode;
    use crate::model::LogicalType;
    use crate::query::{FtsQuery, Query, QueryOptions};
    use crate::repository::{CollectionDefinition, IdentityPolicy, Payload, RepositoryOptions};
    use crate::schema::{FieldDefinition, Schema};

    let root = std::env::temp_dir().join(format!("objekte-tiny-source-{}", std::process::id()));
    std::fs::create_dir_all(root.join("tiny")).unwrap();
    std::fs::write(
        root.join("tiny/records.tiny"),
        b"first|ordinary\nsecond|needle text\n",
    )
    .unwrap();
    let mut schema = Schema::default();
    for name in ["title", "body"] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind: LogicalType::String,
                required: true,
                nullable: false,
                indexed: name == "title",
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    let definition = CollectionDefinition {
        name: "tiny".into(),
        source_path: "tiny".into(),
        format: SourceFormat::registered("tiny"),
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema,
        sort_fields: vec![],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    let parses = Arc::new(AtomicUsize::new(0));
    let registrations = |version| {
        let mut sources = SourceRegistry::builtin();
        sources
            .register(
                "tiny",
                version,
                "tiny",
                Arc::new(TinyReader(parses.clone())),
            )
            .unwrap();
        sources
    };
    let query = Query {
        collections: vec![],
        fts: Some(FtsQuery {
            pattern_parameter: None,
            mode: crate::query::FtsMode::Literal,
            case_sensitivity: crate::query::CaseSensitivity::Sensitive,
            literal: "needle".into(),
            fields: vec![],
        }),
        filter: None,
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence: true,
        },
    };
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        registrations(1),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert_eq!(repo.build_parses(), 2);
    assert_eq!(parses.load(Ordering::Relaxed), 2);
    let found = repo.search(&query).unwrap();
    assert_eq!(found.count.unwrap(), 1);
    assert_eq!(found.counters.record_parses, 1);
    assert!(
        matches!(&found.hits[0].payload, Some(Payload::Logical(Value::Object(fields))) if fields.get("body") == Some(&Value::String("needle text".into())))
    );
    assert_eq!(found.hits[0].matched_fields, ["body"]);
    assert_eq!(found.hits, repo.search_reference(&query).unwrap().hits);
    let mut bytes_query = query.clone();
    bytes_query.options.return_mode = ReturnMode::Bytes;
    let bytes = repo.search(&bytes_query).unwrap();
    assert!(
        matches!(&bytes.hits[0].payload, Some(Payload::Bytes(raw)) if raw == b"second|needle text")
    );
    let after_query = parses.load(Ordering::Relaxed);
    drop(repo);

    let restored = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition.clone()],
        RepositoryOptions {
            ..Default::default()
        },
        registrations(1),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert_eq!(restored.build_parses(), 2);
    assert_eq!(parses.load(Ordering::Relaxed), after_query + 2);
    assert_eq!(restored.search(&query).unwrap().hits, found.hits);
    drop(restored);

    let rebuilt = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        registrations(2),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert_eq!(rebuilt.build_parses(), 2);
    drop(rebuilt);
    assert!(
        crate::lifecycle::Index::build_repository_for_test(
            &root,
            vec![definition],
            RepositoryOptions::default(),
            SourceRegistry::builtin()
        )
        .map_err(|error| error.to_string())
        .err()
        .unwrap()
        .contains("unknown source registration")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn discovery_and_intrinsic_ids_share_build_restore_rules() {
    use crate::discovery::{Discovery, SymlinkPolicy};
    use crate::repository::{CollectionDefinition, IdentityPolicy, RepositoryOptions};

    let root = std::env::temp_dir().join(format!(
        "objekte-discovery-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    for dir in ["tiny/nested", "tiny/skip", "tiny/.hidden"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    std::fs::write(root.join("tiny/main.tiny"), b"first|one\nsecond|two\n").unwrap();
    std::fs::write(root.join("tiny/nested/third.tiny"), b"third|three\n").unwrap();
    std::fs::write(root.join("tiny/skip/ignored.tiny"), b"ignored|four\n").unwrap();
    std::fs::write(root.join("tiny/.hidden/hidden.tiny"), b"hidden|five\n").unwrap();
    std::fs::write(root.join("tiny/other.json"), b"{}").unwrap();
    let mut sources = SourceRegistry::builtin();
    sources
        .register(
            "tiny",
            1,
            "tiny",
            Arc::new(TinyReader(Arc::new(AtomicUsize::new(0)))),
        )
        .unwrap();
    let rules = Discovery {
        recursive: true,
        include: vec!["**/*.tiny".into(), "*.tiny".into()],
        exclude: vec!["skip/**".into()],
        exclude_files: vec![],
        exclude_directories: vec![],
        hidden: false,
        symlinks: SymlinkPolicy::Skip,
    };
    let definition = CollectionDefinition {
        name: "tiny".into(),
        source_path: "tiny".into(),
        format: SourceFormat::registered("tiny"),
        identity: IdentityPolicy::Intrinsic(vec!["title".into()]),
        discovery: rules.clone(),
        schema: Default::default(),
        sort_fields: vec![],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    let built = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        sources.clone(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert_eq!(built.files().files.len(), 2);
    let generation = &built.collections()[0].generation;
    assert_eq!(
        generation
            .records
            .iter()
            .map(|r| r.id.0.as_ref())
            .collect::<Vec<_>>(),
        ["first", "second", "third"]
    );
    let main_id = built
        .files()
        .files
        .iter()
        .find(|f| f.relative_path.ends_with("main.tiny"))
        .unwrap()
        .id;
    assert_eq!(
        generation.file_records.records_for(main_id).unwrap().len(),
        2
    );
    drop(built);
    let restored = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition.clone()],
        RepositoryOptions {
            ..Default::default()
        },
        sources.clone(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert_eq!(
        restored.collections()[0]
            .generation
            .file_records
            .records_for(main_id)
            .unwrap()
            .len(),
        2
    );
    drop(restored);

    std::fs::write(root.join("tiny/nested/fourth.tiny"), b"fourth|four\n").unwrap();
    let rebuilt = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        sources.clone(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert_eq!(rebuilt.collections()[0].generation.records.len(), 4);
    drop(rebuilt);

    let mut shallow = definition.clone();
    shallow.discovery.recursive = false;
    let shallow = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![shallow],
        RepositoryOptions::default(),
        sources.clone(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert_eq!(shallow.collections()[0].generation.records.len(), 2);
    drop(shallow);

    let mut with_hidden = definition.clone();
    with_hidden.discovery.hidden = true;
    let with_hidden = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![with_hidden],
        RepositoryOptions::default(),
        sources.clone(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert_eq!(with_hidden.collections()[0].generation.records.len(), 5);
    drop(with_hidden);

    assert!(
        crate::lifecycle::Index::build_repository_for_test(
            &root,
            vec![CollectionDefinition {
                source_path: "../tiny".into(),
                ..definition.clone()
            }],
            RepositoryOptions::default(),
            sources.clone()
        )
        .map_err(|error| error.to_string())
        .is_err()
    );
    std::fs::write(
        root.join("tiny/nested/duplicate.tiny"),
        b"first|duplicate\n",
    )
    .unwrap();
    let error = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        sources,
    )
    .map_err(|error| error.to_string())
    .err()
    .unwrap();
    assert!(
        error.contains("duplicate record ID") && error.contains("duplicate.tiny"),
        "{error}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn intrinsic_id_rejects_missing_invalid_and_generated_namespace() {
    use crate::repository::{CollectionDefinition, IdentityPolicy, RepositoryOptions};

    let root = std::env::temp_dir().join(format!("objekte-intrinsic-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let definition = CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::JsonLines,
        identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
        discovery: Default::default(),
        schema: Default::default(),
        sort_fields: vec![],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    for (body, expected) in [
        (b"{}\n".as_slice(), "missing intrinsic record ID"),
        (b"{\"id\":42}\n".as_slice(), "must be a string"),
        (b"{\"id\":\"\"}\n".as_slice(), "invalid intrinsic record ID"),
        (
            b"{\"id\":\"~generated:1\"}\n".as_slice(),
            "invalid intrinsic record ID",
        ),
    ] {
        std::fs::write(root.join("data/one.jsonl"), body).unwrap();
        let error = crate::lifecycle::Index::build_repository_for_test(
            &root,
            vec![definition.clone()],
            RepositoryOptions::default(),
            crate::source::registry::SourceRegistry::builtin(),
        )
        .map_err(|error| error.to_string())
        .err()
        .unwrap();
        assert!(
            error.contains(expected) && error.contains("one.jsonl"),
            "{error}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn discovery_symlink_policy_confines_sources_and_avoids_cycles() {
    use crate::discovery::{Discovery, SymlinkPolicy, discover};
    use std::os::unix::fs::symlink;

    let root = std::env::temp_dir().join(format!("objekte-symlinks-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data/nested")).unwrap();
    std::fs::write(root.join("data/nested/real.json"), b"{}").unwrap();
    let outside = std::env::temp_dir().join(format!("objekte-outside-{}.json", std::process::id()));
    std::fs::write(&outside, b"{}").unwrap();
    symlink(root.join("data/nested"), root.join("data/alias")).unwrap();
    symlink(root.join("data"), root.join("data/nested/cycle")).unwrap();
    symlink(&outside, root.join("data/outside.json")).unwrap();
    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::Json)
        .unwrap();
    let mut rules = Discovery {
        recursive: true,
        ..Discovery::default()
    };
    let skipped = discover(&root, Path::new("data"), &rules, &source).unwrap();
    assert_eq!(skipped, [root.join("data/nested/real.json")]);
    rules.symlinks = SymlinkPolicy::FollowWithinRoot;
    let followed = discover(&root, Path::new("data"), &rules, &source).unwrap();
    assert_eq!(followed.len(), 1);
    assert!(followed[0].starts_with(root.join("data")));
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_file(outside).unwrap();
}
