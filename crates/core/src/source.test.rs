use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};


struct TinyReader(Arc<AtomicUsize>);


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
        schema,
        sort_fields: vec![],
        sort_locale: "de".into(),
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
    assert_eq!(found.count, 1);
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
