use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};


struct TinyReader(Arc<AtomicUsize>);


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
    use crate::model::LogicalType;
    use crate::query::{FtsField, FtsQuery, Query, QueryOptions, parse_filter};
    use crate::reference::{FieldDefinition, Schema};
    use crate::repository::{
        CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
    };
    use crate::workload::{BigramAlphabet, ReturnMode};

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
            case_sensitivity: crate::query::CaseSensitivity::Sensitive,
            literal: text.into(),
            fields: field
                .map(|name| {
                    vec![FtsField {
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
                snapshot_mode: SnapshotMode::ReadWrite,
                ..Default::default()
            };
            for open in 0..2 {
                let repo = Repository::open_with_sources(
                    &root,
                    vec![definition(temporal)],
                    options,
                    sources(),
                )
                .unwrap();
                assert_eq!(repo.startup().restored, open == 1);
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


struct OutcomeReader {
    ranges: Vec<ByteRange>,
    parses: Arc<AtomicUsize>,
    fail_framing: bool,
}


impl SourceReader for OutcomeReader {
    fn name(&self) -> &'static str {
        "outcome"
    }

    fn frame(&self, _: &[u8]) -> Result<Vec<ByteRange>, String> {
        Ok(self.ranges.clone())
    }

    fn frame_outcome(&self, file: &[u8]) -> Result<Vec<ByteRange>, ReaderFailure> {
        if self.fail_framing {
            Err(ReaderFailure::Malformed(ReaderDiagnostic::new(
                DiagnosticCategory::Framing,
                "outcome.frame",
                "recoverable framing error",
            )))
        } else {
            self.frame(file).map_err(ReaderFailure::Fatal)
        }
    }

    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        self.parse_outcome(bytes)
            .map_err(|failure| format!("{failure:?}"))
    }

    fn parse_outcome(&self, bytes: &[u8]) -> Result<ExtractedRecord, ReaderFailure> {
        self.parses.fetch_add(1, Ordering::Relaxed);
        if bytes == b"bad" {
            Err(ReaderFailure::Malformed(ReaderDiagnostic::new(
                DiagnosticCategory::Syntax,
                "outcome.syntax",
                "recoverable record error",
            )))
        } else {
            Ok(ExtractedRecord {
                value: Value::Object(Default::default()),
                text: Vec::new(),
            })
        }
    }
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
    use crate::query::{FtsQuery, Query, QueryOptions};
    use crate::reference::Schema;
    use crate::repository::{
        CollectionDefinition, IdentityPolicy, Payload, Repository, RepositoryOptions, SnapshotMode,
    };
    use crate::workload::ReturnMode;
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
        snapshot_mode: SnapshotMode::ReadWrite,
        ..Default::default()
    };
    let built =
        Repository::open_with_sources(&root, vec![definition.clone()], options, sources.clone())
            .unwrap();
    assert_eq!(built.collections()[0].generation.records.len(), 2);
    assert_eq!(built.files().files[0].diagnostics[0].code, "outcome.syntax");
    let query = Query {
        collections: vec!["data".into()],
        filter: None,
        fts: Some(FtsQuery {
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
    let restored =
        Repository::open_with_sources(&root, vec![definition], options, sources).unwrap();
    assert!(restored.startup().restored);
    assert_eq!(restored.search(&query).unwrap().hits, result.hits);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unavailable_source_read_is_classified_without_fabricating_records() {
    let missing = std::env::temp_dir().join(format!(
        "objekte-missing-source-{}.json",
        std::process::id()
    ));
    let mut issues = Vec::new();
    let counts = extract_file_outcomes(
        &missing,
        &JsonReader,
        SourceRevision {
            modified_ns: 0,
            size: 0,
        },
        |outcome| {
            match outcome {
                SourceOutcome::Unreadable { diagnostic } => issues.push(diagnostic),
                _ => panic!("unreadable source produced a record"),
            }
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(counts.framed_records, 0);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].category, DiagnosticCategory::Io);
    assert_eq!(issues[0].code, "source.open");
}

#[test]
fn json_reader_reports_syntax_and_encoding_positions() {
    let reader = JsonReader;
    let syntax = reader.parse_outcome(b"{\n\"title\":,}").unwrap_err();
    let ReaderFailure::Malformed(syntax) = syntax else {
        panic!("malformed syntax")
    };
    assert_eq!(syntax.category, DiagnosticCategory::Syntax);
    assert_eq!(syntax.code, "json.parse");
    assert_eq!(syntax.line, Some(2));
    assert!(syntax.column.is_some());
    assert!(syntax.relative_byte.is_some_and(|offset| offset < 13));
    let encoding = reader.parse_outcome(b"{\"title\":\"\xff\"}").unwrap_err();
    let ReaderFailure::Malformed(encoding) = encoding else {
        panic!("malformed encoding")
    };
    assert_eq!(encoding.category, DiagnosticCategory::Encoding);
    assert_eq!(encoding.relative_byte, Some(10));
}


impl SourceReader for TinyReader {
    fn name(&self) -> &'static str {
        "tiny"
    }

    fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String> {
        let mut ranges = Vec::new();
        let mut start = 0;
        for end in 0..=file.len() {
            if end == file.len() || file[end] == b'\n' {
                if end > start {
                    ranges.push(ByteRange::checked(
                        start as u64,
                        end as u64,
                        file.len() as u64,
                    )?);
                }
                start = end + 1;
            }
        }
        Ok(ranges)
    }

    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        self.0.fetch_add(1, Ordering::Relaxed);
        let line = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
        let (title, body) = line.split_once('|').ok_or("missing tiny separator")?;
        let value = Value::Object(std::collections::BTreeMap::from([
            ("title".into(), Value::String(title.into())),
            ("body".into(), Value::String(body.into())),
        ]));
        Ok(ExtractedRecord {
            value,
            text: vec![TextSegment {
                path: vec!["body".into()],
                text: body.into(),
            }],
        })
    }
}


#[test]
fn body_only_reader_does_not_prune_unindexed_title() {
    use crate::model::LogicalType;
    use crate::query::{FtsField, FtsQuery, Query, QueryOptions, parse_filter};
    use crate::reference::{FieldDefinition, Schema};
    use crate::repository::{
        CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
    };
    use crate::workload::{BigramAlphabet, ReturnMode};

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
        let repo = Repository::open_with_sources(
            &root,
            vec![definition.clone()],
            RepositoryOptions {
                bigram_alphabet: alphabet,
                snapshot_mode: SnapshotMode::Disabled,
                ..Default::default()
            },
            sources(),
        )
        .unwrap();
        for (filter, fts, expected) in [
            (Some("title ~ 'hello world'"), None, 1),
            (
                None,
                Some(FtsQuery {
                    case_sensitivity: crate::query::CaseSensitivity::Sensitive,
                    literal: "hello world".into(),
                    fields: vec![FtsField {
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
    use crate::model::LogicalType;
    use crate::query::{FtsQuery, Query, QueryOptions};
    use crate::reference::{FieldDefinition, Schema};
    use crate::repository::{
        CollectionDefinition, IdentityPolicy, Payload, Repository, RepositoryOptions,
    };
    use crate::workload::ReturnMode;

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
    let repo = Repository::open_with_sources(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        registrations(1),
    )
    .unwrap();
    assert!(!repo.startup().restored);
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

    let restored = Repository::open_with_sources(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        registrations(1),
    )
    .unwrap();
    assert!(restored.startup().restored);
    assert_eq!(restored.build_parses(), 0);
    assert_eq!(parses.load(Ordering::Relaxed), after_query);
    assert_eq!(restored.search(&query).unwrap().hits, found.hits);
    drop(restored);

    let rebuilt = Repository::open_with_sources(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        registrations(2),
    )
    .unwrap();
    assert!(!rebuilt.startup().restored);
    assert_eq!(rebuilt.build_parses(), 2);
    drop(rebuilt);
    assert!(
        Repository::open_with_sources(
            &root,
            vec![definition],
            RepositoryOptions::default(),
            SourceRegistry::builtin()
        )
        .err()
        .unwrap()
        .contains("unknown source registration")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn discovery_and_intrinsic_ids_share_build_restore_rules() {
    use crate::discovery::{Discovery, SymlinkPolicy};
    use crate::repository::{CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions};

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
    let built = Repository::open_with_sources(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        sources.clone(),
    )
    .unwrap();
    assert_eq!(built.files().files.len(), 2);
    let generation = &built.collections()[0].generation;
    assert_eq!(
        generation
            .records
            .iter()
            .map(|r| r.id.0.as_str())
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
    let restored = Repository::open_with_sources(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        sources.clone(),
    )
    .unwrap();
    assert!(restored.startup().restored);
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
    let rebuilt = Repository::open_with_sources(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        sources.clone(),
    )
    .unwrap();
    assert!(!rebuilt.startup().restored);
    assert_eq!(rebuilt.collections()[0].generation.records.len(), 4);
    drop(rebuilt);

    let mut shallow = definition.clone();
    shallow.discovery.recursive = false;
    let shallow = Repository::open_with_sources(
        &root,
        vec![shallow],
        RepositoryOptions::default(),
        sources.clone(),
    )
    .unwrap();
    assert_eq!(shallow.collections()[0].generation.records.len(), 2);
    drop(shallow);

    let mut with_hidden = definition.clone();
    with_hidden.discovery.hidden = true;
    let with_hidden = Repository::open_with_sources(
        &root,
        vec![with_hidden],
        RepositoryOptions::default(),
        sources.clone(),
    )
    .unwrap();
    assert_eq!(with_hidden.collections()[0].generation.records.len(), 5);
    drop(with_hidden);

    assert!(
        Repository::open_with_sources(
            &root,
            vec![CollectionDefinition {
                source_path: "../tiny".into(),
                ..definition.clone()
            }],
            RepositoryOptions::default(),
            sources.clone()
        )
        .is_err()
    );
    std::fs::write(
        root.join("tiny/nested/duplicate.tiny"),
        b"first|duplicate\n",
    )
    .unwrap();
    let error = Repository::open_with_sources(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        sources,
    )
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
    use crate::repository::{CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions};

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
        let error = Repository::open_with_options(
            &root,
            vec![definition.clone()],
            RepositoryOptions::default(),
        )
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

#[test]
fn query_projection_skips_unneeded_fields_and_preserves_selected_values() {
    let bytes = br#"{"ignored":{"deep":[41,42]},"needed":{"name":"found"},"other":"hidden"}"#;
    let projection = ScanProjection {
        top_level: Some(HashSet::from(["needed".to_owned()])),
        collect_text: true,
    };
    for format in [SourceFormat::Json, SourceFormat::JsonLines] {
        let source = SourceRegistry::builtin().resolve(&format).unwrap();
        let selected = source.reader().parse_for_query(bytes, &projection).unwrap();
        assert_eq!(
            selected.value,
            Value::from_json(serde_json::json!({"needed":{"name":"found"}})).unwrap()
        );
        assert_eq!(selected.text.len(), 1);
        assert_eq!(selected.text[0].path, ["needed", "name"]);
        assert_eq!(selected.text[0].text, "found");
        let full = source
            .reader()
            .parse_for_query(bytes, &ScanProjection::full(false))
            .unwrap();
        assert!(matches!(full.value, Value::Object(ref fields) if fields.len() == 3));
        assert!(full.text.is_empty());
        assert!(
            source
                .reader()
                .parse_for_query(br#"{"needed":"ok","ignored":[1,]}"#, &projection)
                .is_err()
        );
    }
}

#[test]
fn standalone_jsonl_ranges_preserve_bytes_and_boundaries() {
    let bytes = "ü\n{\"x\":\"ä\"}\n\n{\"x\":\"b\"}\n".as_bytes();
    // A non-ASCII prefix is deliberately outside the valid records.
    let start = "ü\n".len();
    let valid = &bytes[start..];
    let ranges = JsonLinesReader.frame(valid).unwrap();
    assert_eq!(ranges.len(), 2);
    assert_eq!(
        &valid[ranges[0].start as usize..ranges[0].end as usize],
        "{\"x\":\"ä\"}".as_bytes()
    );
    for range in ranges {
        JsonLinesReader
            .parse(&valid[range.start as usize..range.end as usize])
            .unwrap();
    }
    assert!(JsonLinesReader.frame(b"").unwrap().is_empty());
}

#[test]
fn fused_extraction_parses_once_and_reports_bad_record() {
    let path = std::env::temp_dir().join(format!(
        "objekte-source-{}-{}.jsonl",
        std::process::id(),
        42
    ));
    std::fs::write(&path, b"{\"name\":\"A\"}\n{\"name\":\"B\"}\n").unwrap();
    let mut seen = Vec::new();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let stats = extract_file(&path, SourceFormat::JsonLines, revision, |range, record| {
        seen.push((range, record.text[0].text.clone()));
        Ok(())
    })
    .unwrap();
    assert_eq!(stats.record_parses, 2);
    assert_eq!(seen.len(), 2);
    std::fs::write(&path, b"{\"name\":\"A\"}\n{oops}\n").unwrap();
    assert!(
        extract_file(
            &path,
            SourceFormat::JsonLines,
            SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
            |_, _| Ok(())
        )
        .unwrap_err()
        .contains("..")
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn nearby_multi_record_ranges_share_one_read_window() {
    let path = std::env::temp_dir().join(format!("objekte-batch-{}.jsonl", std::process::id()));
    std::fs::write(&path, b"{\"x\":1}\n{\"x\":2}\n").unwrap();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let ranges = JsonLinesReader
        .frame(&std::fs::read(&path).unwrap())
        .unwrap();
    let (records, bytes_read) =
        read_record_ranges(&path, revision, &[(1, ranges[1]), (0, ranges[0])]).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].0, 0);
    assert_eq!(records[1].1, b"{\"x\":2}");
    assert_eq!(bytes_read, 15);
    let (single, single_bytes) = read_record_ranges(&path, revision, &[(1, ranges[1])]).unwrap();
    assert_eq!(single, vec![(1, b"{\"x\":2}".to_vec())]);
    assert_eq!(single_bytes, 7);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn positioned_reader_seeks_only_when_the_next_range_is_not_adjacent() {
    let root =
        std::env::temp_dir().join(format!("objekte-positioned-reader-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("source.txt");
    std::fs::write(&path, b"0123456789abcdef").unwrap();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let mut reader = RecordRangeReader::open(&path, revision).unwrap();
    let read = |reader: &mut RecordRangeReader, start, end| {
        reader
            .read(&[(0, ByteRange { start, end })])
            .unwrap()
            .0
            .remove(0)
            .1
    };
    assert_eq!(read(&mut reader, 0, 2), b"01");
    assert_eq!(reader.seeks, 0);
    assert_eq!(read(&mut reader, 2, 4), b"23");
    assert_eq!(reader.seeks, 0);
    assert_eq!(read(&mut reader, 0, 2), b"01");
    assert_eq!(reader.seeks, 1);
    assert_eq!(read(&mut reader, 8, 10), b"89");
    assert_eq!(reader.seeks, 2);
    assert_eq!(read(&mut reader, 10, 10), b"");
    assert_eq!(reader.seeks, 2);
    assert!(reader.read(&[]).unwrap().0.is_empty());
    assert_eq!(reader.seeks, 2);
    assert_eq!(
        reader.read(&[(0, ByteRange { start: 17, end: 18 })]),
        Err("invalid byte range 17..18 for file size 16".into())
    );
    assert_eq!(reader.seeks, 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn positioned_reader_tracks_coalesced_windows_and_failed_reads() {
    let root =
        std::env::temp_dir().join(format!("objekte-positioned-window-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("source.txt");
    std::fs::write(&path, b"0123456789abcdef").unwrap();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let mut reader = RecordRangeReader::open(&path, revision).unwrap();
    let (values, bytes_read) = reader
        .read(&[
            (2, ByteRange { start: 4, end: 6 }),
            (1, ByteRange { start: 0, end: 2 }),
        ])
        .unwrap();
    assert_eq!(values, vec![(1, b"01".to_vec()), (2, b"45".to_vec())]);
    assert_eq!(bytes_read, 6);
    assert_eq!(reader.seeks, 0);
    assert_eq!(reader.position, Some(6));
    assert_eq!(
        reader
            .read(&[(3, ByteRange { start: 6, end: 8 })])
            .unwrap()
            .0,
        vec![(3, b"67".to_vec())]
    );
    assert_eq!(reader.seeks, 0);
    assert_eq!(
        reader
            .read(&[(4, ByteRange { start: 1, end: 2 })])
            .unwrap()
            .0,
        vec![(4, b"1".to_vec())]
    );
    assert_eq!(reader.seeks, 1);
    let mut sparse = RecordRangeReader::open(&path, revision).unwrap();
    let (values, bytes_read) = sparse
        .read(&[
            (0, ByteRange { start: 0, end: 1 }),
            (1, ByteRange { start: 10, end: 11 }),
        ])
        .unwrap();
    assert_eq!(values, vec![(0, b"0".to_vec()), (1, b"a".to_vec())]);
    assert_eq!(bytes_read, 11);
    assert_eq!(sparse.seeks, 0);
    let mut failed = RecordRangeReader::open(&path, revision).unwrap();
    std::fs::write(&path, b"01").unwrap();
    assert!(failed.read(&[(0, ByteRange { start: 0, end: 4 })]).is_err());
    assert_eq!(failed.position, None);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn changed_source_rejects_old_ranges_and_incomplete_extraction() {
    let path = std::env::temp_dir().join(format!("objekte-stale-{}.jsonl", std::process::id()));
    std::fs::write(&path, b"{\"x\":1}\n{\"x\":2}\n").unwrap();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let ranges = JsonLinesReader
        .frame(&std::fs::read(&path).unwrap())
        .unwrap();
    let changed = extract_file(&path, SourceFormat::JsonLines, revision, |_, _| {
        std::fs::write(&path, b"{\"x\":1}\n{\"x\":22}\n").map_err(|e| e.to_string())?;
        Ok(())
    });
    assert!(changed.unwrap_err().contains("stale source revision"));
    assert!(
        read_record_range(&path, revision, ranges[1])
            .unwrap_err()
            .contains("stale source revision")
    );
    let current = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    assert!(
        read_record_ranges(
            &path,
            current,
            &[(
                0,
                ByteRange {
                    start: 0,
                    end: current.size + 1
                }
            )]
        )
        .unwrap_err()
        .contains("invalid byte range")
    );
    std::fs::remove_file(path).unwrap();
}
