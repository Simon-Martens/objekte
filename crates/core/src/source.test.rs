use super::*;
use crate::model::Number;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn source_failure_classifier_keeps_missing_revision_and_range_distinct() {
    use crate::measurement::SourceUnavailableKind;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("source-failure-classifier-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("record.json");
    std::fs::write(&path, b"{}").unwrap();
    let original = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    assert_eq!(
        classify_source_read_failure(&path, original, Some(ByteRange { start: 0, end: 3 })),
        SourceUnavailableKind::UnusableRange,
    );
    std::fs::write(&path, b"{\"id\":1}").unwrap();
    assert_eq!(
        classify_source_read_failure(&path, original, None),
        SourceUnavailableKind::RevisionMismatch,
    );
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        classify_source_read_failure(&path, original, None),
        SourceUnavailableKind::Missing,
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_jsonl_chunks_preserve_whole_file_ranges_and_line_origins() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("bounded-jsonl-source-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("records.jsonl");
    let bytes = b"\n \t\r\n{\"id\":\"a\"}\r\n{\"id\":\"b\"}\n\x0c\n{\"id\":\"c\"}";
    std::fs::write(&path, bytes).unwrap();
    let expected = JsonLinesReader.frame(bytes).unwrap();
    let mut reader = BoundedJsonLines::open(&path).unwrap();
    let mut ranges = Vec::new();
    let mut seen_bytes = 0usize;
    let mut previous_lines = 0u64;
    while let Some(chunk) = reader.next_chunk(18, 2).unwrap() {
        assert_eq!(chunk.file_offset as usize, seen_bytes);
        assert_eq!(chunk.preceding_lines, previous_lines);
        assert_eq!(
            chunk.bytes,
            bytes[seen_bytes..seen_bytes + chunk.bytes.len()]
        );
        assert!(chunk.bytes.len() <= 18 || chunk.ranges.len() <= 1);
        for range in chunk.ranges {
            ranges.push(ByteRange {
                start: chunk.file_offset + range.start,
                end: chunk.file_offset + range.end,
            });
        }
        previous_lines += chunk.bytes.iter().filter(|byte| **byte == b'\n').count() as u64;
        seen_bytes += chunk.bytes.len();
    }
    assert_eq!(seen_bytes, bytes.len());
    assert_eq!(ranges, expected);
    assert!(reader.next_chunk(18, 2).unwrap().is_none());

    let oversized = root.join("oversized.jsonl");
    let one = format!("{{\"x\":\"{}\"}}\n", "a".repeat(100));
    std::fs::write(&oversized, &one).unwrap();
    let mut reader = BoundedJsonLines::open(&oversized).unwrap();
    let chunk = reader.next_chunk(16, 2).unwrap().unwrap();
    assert_eq!(chunk.bytes, one.as_bytes());
    assert_eq!(chunk.ranges.len(), 1);
    assert!(reader.next_chunk(16, 2).unwrap().is_none());

    let empty = root.join("empty.jsonl");
    std::fs::write(&empty, b"").unwrap();
    assert!(
        BoundedJsonLines::open(&empty)
            .unwrap()
            .next_chunk(16, 2)
            .unwrap()
            .is_none()
    );
    assert!(JsonLinesReader.supports_bounded_json_lines_build());
    assert!(!JsonReader.supports_bounded_json_lines_build());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_jsonl_many_records_keep_offsets_and_chunk_limits() {
    let root = std::env::temp_dir().join(format!(
        "objekte-many-bounded-lines-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("records.jsonl");
    let mut bytes = Vec::new();
    for index in 0..600 {
        bytes.extend_from_slice(format!("{{\"id\":{index}}}\r\n").as_bytes());
        if index % 13 == 0 {
            bytes.extend_from_slice(b" \t\r\n");
        }
    }
    std::fs::write(&path, &bytes).unwrap();
    let expected = JsonLinesReader.frame(&bytes).unwrap();
    let mut framer = BoundedJsonLines::open(&path).unwrap();
    let mut actual = Vec::new();
    let mut chunk_count = 0;
    let mut read_bytes = 0;
    while let Some(chunk) = framer.next_chunk(256, 17).unwrap() {
        chunk_count += 1;
        assert!(chunk.bytes.len() <= 256);
        assert!(chunk.ranges.len() <= 17);
        assert_eq!(chunk.file_offset, read_bytes);
        for range in chunk.ranges {
            actual.push(ByteRange {
                start: range.start + chunk.file_offset,
                end: range.end + chunk.file_offset,
            });
        }
        read_bytes += chunk.bytes.len() as u64;
    }
    assert!(chunk_count > 30);
    assert_eq!(read_bytes, bytes.len() as u64);
    assert_eq!(actual, expected);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_jsonl_reports_read_errors_before_and_after_first_chunk() {
    struct FailingRead {
        bytes: std::io::Cursor<&'static [u8]>,
        limit: u64,
    }


    impl Read for FailingRead {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            if self.bytes.position() >= self.limit {
                return Err(std::io::Error::other("injected read failure"));
            }
            let available = (self.limit - self.bytes.position()) as usize;
            let count = output.len().min(available);
            self.bytes.read(&mut output[..count])
        }
    }


    const LINES: &[u8] = b"{\"a\":1}\n{\"a\":2}\n";
    let make = |limit| {
        BoundedJsonLines::from_reader(BufReader::with_capacity(
            1,
            FailingRead {
                bytes: std::io::Cursor::new(LINES),
                limit,
            },
        ))
    };
    assert!(make(0).next_chunk(8, 1).is_err());
    let mut after_first = make(8);
    assert_eq!(
        after_first.next_chunk(8, 1).unwrap().unwrap().ranges.len(),
        1
    );
    assert!(after_first.next_chunk(8, 1).is_err());
}

#[test]
fn typed_source_change_requires_a_confirmed_revision_mismatch() {
    use crate::streaming::{ExecutionContext, ExecutionErrorKind, ExecutionLimits};
    let root = std::env::temp_dir().join(format!(
        "objekte-typed-source-change-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("source.json");
    std::fs::write(&path, b"old").unwrap();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let range = ByteRange::checked(0, 3, revision.size).unwrap();
    let context = || {
        ExecutionContext::new(
            crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
            ExecutionLimits::default(),
            None,
        )
    };

    std::fs::write(&path, b"newer content").unwrap();
    let mismatch = context();
    let error = read_record_range_with_execution(&path, revision, range, &mismatch).unwrap_err();
    assert_eq!(
        mismatch.recorded_error(error).kind,
        ExecutionErrorKind::SourceChanged
    );

    std::fs::remove_file(&path).unwrap();
    let missing = context();
    let error = read_record_range_with_execution(&path, revision, range, &missing).unwrap_err();
    assert_eq!(
        missing.recorded_error(error).kind,
        ExecutionErrorKind::Execution
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn schema_free_json_reader_admits_every_root_shape() {
    use crate::reference::Schema;
    let reader = JsonReader;
    for text in [
        r#"{}"#,
        r#"[]"#,
        r#"["a",1,true,null,{}]"#,
        r#""root""#,
        "42",
        "true",
        "false",
        "null",
    ] {
        let mut record = reader.parse_outcome(text.as_bytes()).unwrap();
        Schema::default().coerce_record(&mut record.value).unwrap();
    }
    let mut declared = Schema::default();
    declared
        .register_field(crate::reference::FieldDefinition {
            path: vec!["title".into()],
            kind: crate::model::LogicalType::String,
            required: false,
            nullable: true,
            indexed: false,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let mut scalar = reader.parse_outcome(b"42").unwrap();
    assert!(declared.coerce_record(&mut scalar.value).is_err());
}

#[test]
fn direct_build_json_matches_existing_logical_reader_and_diagnostics() {
    let deep = format!("{}0{}", "[".repeat(130), "]".repeat(130));
    let cases: Vec<&[u8]> = vec![
        br#"null"#,
        br#"[]"#,
        br#"{}"#,
        br#"[null,true,0,-0,1.0,1e2,"x",{"a":[]}]"#,
        br#"{"a":"escaped\n\"quote","a":"last","emoji":"\uD83D\uDE03"}"#,
        br#"{"n":170141183460469231731687303715884105727}"#,
        br#"{"n":170141183460469231731687303715884105728}"#,
        br#"{"n":0.123456789012345678901234567890123456789}"#,
        br#"{"n":1e400}"#,
        br#"{"nested":[[null],[1,"x"],[]]}"#,
        br#"{"broken":,}"#,
        br#"{"broken":"\uD800"}"#,
        br#"{"broken":"#,
        b"{\"broken\":\"\xff\"}",
        deep.as_bytes(),
    ];
    for bytes in cases {
        let expected = parse_json_for_query_diagnosed(bytes, &ScanProjection::full(false));
        let actual = parse_json_for_build_diagnosed(bytes);
        match (actual, expected) {
            (Ok(actual), Ok(expected)) => {
                assert_eq!(
                    actual.value,
                    expected.value,
                    "{}",
                    String::from_utf8_lossy(bytes)
                );
                assert!(actual.text.is_empty());
            }
            (Err(actual), Err(expected)) => assert_eq!(actual, expected),
            (actual, expected) => panic!("direct/legacy mismatch: {actual:?} vs {expected:?}"),
        }
    }
    let decoded = JsonDecodedScalarsReader
        .parse_for_build_outcome(br#"{"a":1,"a":2}"#)
        .unwrap();
    let ordinary = JsonReader
        .parse_for_build_outcome(br#"{"a":1,"a":2}"#)
        .unwrap();
    assert_ne!(decoded.text.len(), ordinary.text.len());
}

#[test]
fn root_queries_match_reference_over_mixed_json_documents() {
    use crate::query::{Query, QueryOptions, parse_filter};
    use crate::repository::{CollectionDefinition, Repository, RepositoryOptions, SnapshotMode};
    use crate::workload::ReturnMode;

    let root = std::env::temp_dir().join(format!("objekte-json-roots-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, json) in [
        ("array", r#"["first",123,true,null,{"title":"Goethe"}]"#),
        ("string", r#""first""#),
        ("number", "123"),
        ("null", "null"),
        ("object", r#"{"title":"Goethe","values":["first",123]}"#),
        ("quoted", r#"{"@root":"real","a.b":"dot","":"empty"}"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), json).unwrap();
    }
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(repo.collections[0].generation.records.len(), 6);
    for (filter, expected) in [
        ("@root = 'first'", vec!["path:string.json"]),
        ("@root = 123", vec!["path:number.json"]),
        ("@root :null", vec!["path:null.json"]),
        ("@root[0] = 'first'", vec!["path:array.json"]),
        ("@root[0] = @root[0]", vec!["path:array.json"]),
        (
            "@root ?= 'first'",
            vec!["path:array.json", "path:string.json"],
        ),
        ("@root[*].title ~ 'Goethe'", vec!["path:array.json"]),
        (
            "@root.title ~ 'Goethe'",
            vec!["path:array.json", "path:object.json"],
        ),
        ("@root[\"a.b\"] = 'dot'", vec!["path:quoted.json"]),
        ("@root[\"\"] = 'empty'", vec!["path:quoted.json"]),
        ("[\"@root\"] = 'real'", vec!["path:quoted.json"]),
    ] {
        let query = Query {
            collections: vec!["data".into()],
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
        let optimized = repo.search(&query).unwrap();
        let reference = repo.search_reference(&query).unwrap();
        let ids = |result: &crate::repository::SearchResult| {
            result
                .hits
                .iter()
                .map(|hit| hit.record_id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&optimized), expected, "{filter}");
        assert_eq!(ids(&optimized), ids(&reference), "{filter}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn declared_schema_keeps_scalar_root_as_diagnosed_fallback() {
    use crate::inventory::RecordClass;
    use crate::model::LogicalType;
    use crate::reference::FieldDefinition;
    use crate::repository::{CollectionDefinition, Repository, RepositoryOptions, SnapshotMode};

    let root = std::env::temp_dir().join(format!("objekte-declared-root-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/root.json"), br#"["first", 2]"#).unwrap();
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["title".into()],
            kind: LogicalType::String,
            required: false,
            nullable: true,
            indexed: false,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let repo = Repository::open_with_options(
        &root,
        vec![definition],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    let records = &repo.collections[0].generation.records;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].class, RecordClass::RawFallback);
    assert!(
        repo.files.files[records[0].file.0 as usize]
            .diagnostics
            .iter()
            .any(|issue| issue.message.contains("root must be an object"))
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn exceptional_numbers_do_not_demote_array_roots_or_dynamic_predicates() {
    use crate::inventory::RecordClass;
    use crate::query::{Query, QueryOptions, parse_filter};
    use crate::repository::{
        CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
    };
    use crate::workload::ReturnMode;

    let root =
        std::env::temp_dir().join(format!("objekte-exact-json-numbers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/array.json"),
        b" [ \"first\", 1e1000000, true ] \n",
    )
    .unwrap();
    std::fs::write(root.join("data/object.json"), br#"{"n":-1e-1000000}"#).unwrap();
    let mut definition =
        CollectionDefinition::folder("data", "data", SourceFormat::JsonDecodedScalars);
    definition.identity = IdentityPolicy::RelativePath;
    let repo = Repository::open_with_options(
        &root,
        vec![definition],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        repo.collections[0]
            .generation
            .records
            .iter()
            .all(|record| record.class == RecordClass::Logical)
    );
    for (filter, expected) in [
        ("@root[0] = 'first'", vec!["path:array.json"]),
        ("@root[1] = 1e1000000", vec!["path:array.json"]),
        ("n < 0", vec!["path:object.json"]),
        ("n = -1e-1000000", vec!["path:object.json"]),
    ] {
        let query = Query {
            collections: vec!["data".into()],
            fts: None,
            filter: Some(parse_filter(filter).unwrap()),
            options: QueryOptions {
                sort: vec![],
                limit: 20,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: false,
            },
        };
        let optimized = repo.search(&query).unwrap();
        let reference = repo.search_reference(&query).unwrap();
        let ids = |result: &crate::repository::SearchResult| {
            result
                .hits
                .iter()
                .map(|hit| hit.record_id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&optimized), expected, "{filter}");
        assert_eq!(ids(&optimized), ids(&reference), "{filter}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn valid_json_beyond_numeric_resource_limit_is_fatal_not_malformed_fallback() {
    let reader = JsonDecodedScalarsReader;
    let result = reader.parse_outcome(br#"{"n":1e99999999999999999999}"#);
    assert!(
        matches!(result, Err(ReaderFailure::Fatal(message)) if message.contains("resource limit"))
    );
}

#[test]
fn syntactically_valid_deep_json_reports_resource_limit() {
    let reader = JsonDecodedScalarsReader;
    let nested = format!("{}0{}", "[".repeat(130), "]".repeat(130));
    let result = reader.parse_outcome(nested.as_bytes());
    assert!(
        matches!(result, Err(ReaderFailure::Fatal(message)) if message.contains("recursion limit"))
    );
}

#[test]
fn decoded_scalar_build_and_query_accept_the_same_json_documents() {
    let reader = JsonDecodedScalarsReader;
    let mut documents: Vec<Vec<u8>> = vec![
        br#"{"title":"first","title":"last","values":[null,true,1,1.0,1e0,-0,"\u00df","\ud83d\ude80"]}"#.to_vec(),
        br#"["first",123,false,null,{"nested":"\uD834\uDD1E"}]"#.to_vec(),
        br#""a root string""#.to_vec(),
        b"1234567890123456789012345678901234567890".to_vec(),
        b"true".to_vec(),
        b"false".to_vec(),
        b"null".to_vec(),
        b"[]".to_vec(),
        b"{}".to_vec(),
        b"[1,]".to_vec(),
        b"{\"a\":1,}".to_vec(),
        b"01".to_vec(),
        b"1e".to_vec(),
        b"1e+".to_vec(),
        b"-".to_vec(),
        b"\"bad\\q\"".to_vec(),
        b"\"\\ud800\"".to_vec(),
        b"\"\\udc00\"".to_vec(),
    ];
    for depth in [126, 127, 128, 129, 130] {
        for (open, close) in [("[", "]"), ("{\"a\":", "}")] {
            let bytes = format!("{}0{}", open.repeat(depth), close.repeat(depth));
            let built = reader.parse_for_build_outcome(bytes.as_bytes());
            let queried = reader.parse_outcome(bytes.as_bytes());
            assert_eq!(
                built.is_ok(),
                queried.is_ok(),
                "nesting depth {depth}: {open}"
            );
        }
    }
    for control in 0..=31 {
        documents.push(vec![b'"', b'a', control, b'b', b'"']);
    }
    documents.push(vec![b'"', 0xff, b'"']);
    documents.push(b"\xef\xbb\xbf{}".to_vec());
    for bytes in documents {
        let built = reader.parse_for_build_outcome(&bytes);
        let queried = reader.parse_outcome(&bytes);
        assert_eq!(
            built.is_ok(),
            queried.is_ok(),
            "fresh/query acceptance differs for {bytes:?}: build={built:?}, query={queried:?}"
        );
        if let (Ok(built), Ok(queried)) = (built, queried) {
            assert_eq!(built.value, queried.value, "{bytes:?}");
            assert_eq!(
                built
                    .text
                    .iter()
                    .map(|segment| &segment.text)
                    .collect::<Vec<_>>(),
                queried
                    .text
                    .iter()
                    .map(|segment| &segment.text)
                    .collect::<Vec<_>>(),
                "{bytes:?}"
            );
        }
    }
}

#[test]
fn trusted_builtin_query_readers_match_validating_results_on_valid_json() {
    let bytes = br#"{"title":"old","title":{"nested":"new"},"n":1e0,"array":["\u00df",true,null]}"#;
    let readers: [&dyn SourceReader; 3] =
        [&JsonReader, &JsonDecodedScalarsReader, &JsonLinesReader];
    let projections = [
        ScanProjection::full(true),
        ScanProjection {
            top_level: Some(HashSet::from(["title".into(), "n".into()])),
            collect_text: false,
            resolved_text_only: false,
        },
        ScanProjection {
            top_level: Some(HashSet::from(["title".into()])),
            collect_text: true,
            resolved_text_only: true,
        },
    ];
    for reader in readers {
        for projection in &projections {
            let validating = reader.parse_for_query(bytes, projection).unwrap();
            let trusted = reader.parse_for_query_trusted(bytes, projection).unwrap();
            assert_eq!(validating.value, trusted.value, "{}", reader.name());
            assert_eq!(
                validating
                    .text
                    .iter()
                    .map(|segment| (&segment.path, &segment.text))
                    .collect::<Vec<_>>(),
                trusted
                    .text
                    .iter()
                    .map(|segment| (&segment.path, &segment.text))
                    .collect::<Vec<_>>(),
                "{}",
                reader.name(),
            );
        }
    }
}

#[test]
fn trusted_builtin_projection_omits_search_time_validation_of_skipped_content() {
    let bytes = br#"{"wanted":5,"ignored":[1,]} trailing"#;
    let projection = ScanProjection {
        top_level: Some(HashSet::from(["wanted".into()])),
        collect_text: false,
        resolved_text_only: false,
    };
    for reader in [
        &JsonReader as &dyn SourceReader,
        &JsonDecodedScalarsReader,
        &JsonLinesReader,
    ] {
        assert!(
            reader.parse_for_query(bytes, &projection).is_err(),
            "{}",
            reader.name()
        );
        let record = reader.parse_for_query_trusted(bytes, &projection).unwrap();
        let Value::Object(fields) = record.value else {
            panic!("projected JSON root is not an object");
        };
        assert_eq!(
            fields.get("wanted"),
            Some(&Value::Number(Number::integer(5))),
            "{}",
            reader.name(),
        );
    }
}

#[test]
fn decoded_scalar_build_uses_exact_json_whitespace() {
    let reader = JsonDecodedScalarsReader;
    for control in (0..=31).chain([b' ', 127]) {
        for (prefix, suffix) in [
            (b"".as_slice(), b"0".as_slice()),
            (b"0".as_slice(), b"".as_slice()),
            (b"[".as_slice(), b"0]".as_slice()),
            (b"[0".as_slice(), b",1]".as_slice()),
            (b"{\"a\":".as_slice(), b"0}".as_slice()),
            (b"{\"a\":0".as_slice(), b"}".as_slice()),
        ] {
            let mut bytes = prefix.to_vec();
            bytes.push(control);
            bytes.extend_from_slice(suffix);
            let built = reader.parse_for_build_outcome(&bytes);
            let queried = reader.parse_outcome(&bytes);
            let allowed = matches!(control, b' ' | b'\t' | b'\n' | b'\r');
            assert_eq!(
                queried.is_ok(),
                allowed,
                "query control {control:#04x} in {bytes:?}"
            );
            assert_eq!(
                built.is_ok(),
                allowed,
                "build control {control:#04x} in {bytes:?}"
            );
        }
    }
}

#[test]
fn decoded_scalar_fts_matches_tokens_and_preserves_duplicate_name_rules() {
    use crate::query::{CaseSensitivity, FtsField, FtsMode, FtsQuery, Query, QueryOptions};
    use crate::repository::{
        CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
    };
    use crate::workload::{BigramAlphabet, ReturnMode};

    let root = std::env::temp_dir().join(format!("objekte-scalar-fts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, json) in [
        (
            "one",
            r#"{"needle_only":"x","value":"a\n","num":1.0,"title":"old","title":"new"}"#,
        ),
        ("two", r#"[true,null,1e0,"ß"]"#),
        ("three", r#"{"title":"not here","num":-0}"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), json).unwrap();
    }
    let cases: &[(&str, Option<&str>, &[&str])] = &[
        ("needle_only", None, &[]),
        ("1.0", None, &["path:one.json"]),
        ("1e0", None, &["path:two.json"]),
        ("-0", None, &["path:three.json"]),
        ("true", None, &["path:two.json"]),
        ("null", None, &["path:two.json"]),
        ("a\n", None, &["path:one.json"]),
        ("old", None, &["path:one.json"]),
        ("old", Some("title"), &[]),
        ("new", Some("title"), &["path:one.json"]),
        ("1e0", Some("@root"), &["path:two.json"]),
    ];
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let mut definition =
            CollectionDefinition::folder("data", "data", SourceFormat::JsonDecodedScalars);
        definition.identity = IdentityPolicy::RelativePath;
        let repo = Repository::open_with_options(
            &root,
            vec![definition],
            RepositoryOptions {
                bigram_alphabet: alphabet,
                snapshot_mode: SnapshotMode::Disabled,
                ..Default::default()
            },
        )
        .unwrap();
        for &(literal, selected, expected) in cases {
            let query = Query {
                collections: vec!["data".into()],
                fts: Some(FtsQuery {
                    case_sensitivity: CaseSensitivity::Sensitive,
                    mode: FtsMode::Literal,
                    literal: literal.into(),
                    pattern_parameter: None,
                    fields: selected
                        .into_iter()
                        .map(|field| FtsField {
                            path: crate::model::FieldPath::parse(field).unwrap(),
                            binding: None,
                            query_path: None,
                        })
                        .collect(),
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
            let optimized = repo.search(&query).unwrap();
            let ids = |result: &crate::repository::SearchResult| {
                result
                    .hits
                    .iter()
                    .map(|hit| hit.record_id.clone())
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                ids(&optimized),
                expected,
                "{literal:?} {selected:?} {alphabet:?}"
            );
            if selected.is_none() {
                let reference = repo.search_reference(&query).unwrap();
                assert_eq!(
                    ids(&optimized),
                    ids(&reference),
                    "{literal:?} {selected:?} {alphabet:?}"
                );
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn build_json_borrows_logical_strings_after_temporal_conversion() {
    use crate::bigram::BigramBuilder;
    use crate::model::LogicalType;
    use crate::record_preparation::finish_build_record;
    use crate::reference::{FieldDefinition, Schema};
    use crate::workload::BigramAlphabet;

    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::Json)
        .unwrap();
    let bytes =
        r#"{"date":"2026-01-02","title":"aßb","nested":{"names":["ab","cd",""]}}"#.as_bytes();
    let mut record = source.reader().parse_for_build_outcome(bytes).unwrap();
    assert!(record.text.is_empty());
    let mut schema = Schema::default();
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
    finish_build_record(&schema, &mut record).unwrap();
    let mut strings = Vec::new();
    visit_logical_strings(&record.value, &mut |text| strings.push(text.to_owned()));
    assert_eq!(strings, ["ab", "cd", "", "aßb"]);
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let mut borrowed = BigramBuilder::new(alphabet);
        borrowed
            .add_record_with(crate::model::RecordSlot(0), |emit| {
                visit_logical_strings(&record.value, emit);
            })
            .unwrap();
        let borrowed = borrowed.finish().unwrap();
        let mut owned = BigramBuilder::new(alphabet);
        owned
            .add_record(crate::model::RecordSlot(0), strings.iter())
            .unwrap();
        let owned = owned.finish().unwrap();
        assert_eq!(borrowed.normal, owned.normal);
        assert_eq!(borrowed.skip, owned.skip);
    }
}


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
                snapshot_mode: SnapshotMode::IfValid,
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
                if open == 0 {
                    repo.save_default_snapshot_for_test();
                }
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
        snapshot_mode: SnapshotMode::IfValid,
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
    built.save_default_snapshot_for_test();
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

#[test]
fn malformed_jsonl_coordinate_work_grows_with_file_size() {
    fn scan_work(frames: usize) -> (usize, usize) {
        let bytes = b"{oops}\n".repeat(frames);
        let ranges = JsonLinesReader.frame(&bytes).unwrap();
        FILE_COORDINATE_SCANNED_BYTES.with(|scanned| scanned.set(0));
        let mut malformed = 0;
        let counters = extract_framed_outcomes_impl(
            Path::new("malformed.jsonl"),
            &JsonLinesReader,
            &bytes,
            &ranges,
            true,
            |outcome| {
                let SourceOutcome::Malformed { diagnostic, .. } = outcome else {
                    panic!("invalid JSONL frame was accepted")
                };
                malformed += 1;
                assert_eq!(diagnostic.line, Some(malformed));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(counters.failed_parses, frames as u64);
        assert_eq!(malformed, frames as u64);
        let scanned = FILE_COORDINATE_SCANNED_BYTES.with(|scanned| scanned.get());
        (scanned, bytes.len())
    }

    let (small_work, small_size) = scan_work(128);
    let (large_work, large_size) = scan_work(256);
    assert!(small_work <= small_size, "{small_work} > {small_size}");
    assert!(large_work <= large_size, "{large_work} > {large_size}");
    assert!(large_work <= small_work * 2 + 8);
}

#[test]
fn file_coordinates_preserve_custom_reader_line_and_column_meaning() {
    let file = b"aa\r\nbb_cc\nmm\nnn";
    let mut cursor = FileCoordinateCursor::default();
    let mut diagnostic = ReaderDiagnostic::new(DiagnosticCategory::Syntax, "custom", "bad");
    diagnostic.line = Some(1);
    diagnostic.column = Some(2);
    diagnostic.relative_byte = Some(1);
    file_coordinates(
        file,
        ByteRange { start: 6, end: 9 },
        &mut diagnostic,
        &mut cursor,
    );
    assert_eq!((diagnostic.line, diagnostic.column), (Some(2), Some(4)));
    assert_eq!(diagnostic.relative_byte, Some(1));

    let mut at_boundary = ReaderDiagnostic::new(DiagnosticCategory::Syntax, "custom", "bad");
    at_boundary.line = Some(1);
    at_boundary.column = Some(1);
    file_coordinates(
        file,
        ByteRange { start: 13, end: 15 },
        &mut at_boundary,
        &mut cursor,
    );
    assert_eq!((at_boundary.line, at_boundary.column), (Some(4), Some(1)));

    let mut without_line = ReaderDiagnostic::new(DiagnosticCategory::Syntax, "custom", "bad");
    without_line.column = Some(7);
    file_coordinates(
        file,
        ByteRange { start: 15, end: 15 },
        &mut without_line,
        &mut cursor,
    );
    assert_eq!((without_line.line, without_line.column), (None, Some(7)));

    let mut multiline = ReaderDiagnostic::new(DiagnosticCategory::Syntax, "custom", "bad");
    multiline.line = Some(2);
    multiline.column = Some(1);
    file_coordinates(
        b"prefix\n{\n}\n",
        ByteRange { start: 7, end: 10 },
        &mut multiline,
        &mut FileCoordinateCursor::default(),
    );
    assert_eq!((multiline.line, multiline.column), (Some(3), Some(1)));
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
    repo.save_default_snapshot_for_test();
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
    built.save_default_snapshot_for_test();
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
        resolved_text_only: false,
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

#[test]
fn same_size_edit_with_restored_mtime_is_outside_revision_detection() {
    let root = std::env::temp_dir().join(format!(
        "objekte-same-revision-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("source.txt");
    std::fs::write(&path, b"original").unwrap();
    let metadata = std::fs::metadata(&path).unwrap();
    let revision = SourceRevision::from_metadata(&metadata).unwrap();
    let mut reader = RecordRangeReader::open(&path, revision).unwrap();
    std::fs::write(&path, b"modified").unwrap();
    std::fs::File::open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
        .unwrap();
    assert_eq!(
        SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
        revision
    );
    assert_eq!(
        reader
            .read(&[(0, ByteRange { start: 0, end: 8 })])
            .unwrap()
            .0[0]
            .1,
        b"modified"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn checked_range_reads_reject_replaced_path_with_spoofed_revision() {
    let root = std::env::temp_dir().join(format!(
        "objekte-source-incarnation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("source.txt");
    let moved = root.join("moved.txt");
    std::fs::write(&path, b"original").unwrap();
    let metadata = std::fs::metadata(&path).unwrap();
    let revision = SourceRevision::from_metadata(&metadata).unwrap();
    let created_ns = metadata
        .created()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as i128);
    let fingerprint: [u8; 32] = Sha256::digest(b"original").into();
    let range = ByteRange { start: 0, end: 8 };
    std::fs::rename(&path, &moved).unwrap();
    std::fs::write(&path, b"modified").unwrap();
    std::fs::File::open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
        .unwrap();
    assert_eq!(
        SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
        revision
    );
    if created_ns.is_some() {
        assert!(
            read_record_range_checked(&path, revision, created_ns, Some(fingerprint), range)
                .is_err()
        );
        assert!(
            RecordRangeReader::open_checked(&path, revision, created_ns, Some(fingerprint))
                .is_err()
        );
        assert_eq!(
            probe_source_unavailable_checked(
                &path,
                revision,
                created_ns,
                Some(fingerprint),
                Some(range)
            ),
            Some(SourceUnavailableKind::RevisionMismatch)
        );
    }
    assert!(read_record_range_checked(&path, revision, None, Some(fingerprint), range).is_err());
    assert!(RecordRangeReader::open_checked(&path, revision, None, None).is_err());
    assert_eq!(
        classify_source_read_failure_checked(&path, revision, None, Some(fingerprint), Some(range)),
        SourceUnavailableKind::RevisionMismatch
    );
    assert_eq!(
        read_record_range_checked(&moved, revision, created_ns, Some(fingerprint), range).unwrap(),
        b"original"
    );
    assert_eq!(
        RecordRangeReader::open_checked(&moved, revision, None, Some(fingerprint))
            .unwrap()
            .read(&[(0, range)])
            .unwrap()
            .0[0]
            .1,
        b"original"
    );
    let mut pinned_reader =
        RecordRangeReader::open_checked(&moved, revision, None, Some(fingerprint)).unwrap();
    std::fs::write(&moved, b"modified").unwrap();
    std::fs::File::open(&moved)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
        .unwrap();
    assert!(pinned_reader.read(&[(0, range)]).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn build_reads_use_actual_bytes_without_revision_probes() {
    let root =
        std::env::temp_dir().join(format!("objekte-build-stable-input-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("records.jsonl");
    std::fs::write(&path, b"{\"x\":1}\n{\"x\":2}\n").unwrap();
    let wrong_revision = SourceRevision {
        modified_ns: 0,
        size: 1,
    };
    let mut ranges = Vec::new();
    let counters =
        extract_file_build_outcomes(&path, &JsonLinesReader, wrong_revision, |outcome| {
            let SourceOutcome::Logical { range, .. } = outcome else {
                panic!("expected logical record")
            };
            ranges.push(range);
            Ok(())
        })
        .unwrap();
    assert_eq!(counters.record_parses, 2);
    assert_eq!(
        ranges,
        [
            ByteRange { start: 0, end: 7 },
            ByteRange { start: 8, end: 15 }
        ]
    );
    assert!(extract_file_outcomes(&path, &JsonLinesReader, wrong_revision, |_| Ok(())).is_err());
    let framed = frame_file_for_build(&path, &JsonLinesReader)
        .unwrap()
        .unwrap();
    assert_eq!(framed.0.len(), 16);
    assert_eq!(framed.1, ranges);

    let malformed = root.join("malformed.jsonl");
    std::fs::write(&malformed, b"{\n").unwrap();
    extract_file_build_outcomes(&malformed, &JsonLinesReader, wrong_revision, |outcome| {
        assert!(matches!(outcome, SourceOutcome::Malformed { .. }));
        Ok(())
    })
    .unwrap();
    let empty = root.join("empty.jsonl");
    std::fs::write(&empty, b"").unwrap();
    assert_eq!(
        extract_file_build_outcomes(&empty, &JsonLinesReader, wrong_revision, |_| panic!(
            "empty file must not emit a record"
        ))
        .unwrap()
        .framed_records,
        0
    );
    let missing = root.join("missing.jsonl");
    extract_file_build_outcomes(&missing, &JsonLinesReader, wrong_revision, |outcome| {
        assert!(matches!(outcome, SourceOutcome::Unreadable { .. }));
        Ok(())
    })
    .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
