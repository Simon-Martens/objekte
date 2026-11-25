use std::io::{self, Read};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use objekte_loader_json::configuration::load_optional_schema;
use objekte_search_core::inventory::ByteRange;
use objekte_search_core::lifecycle::Index;
use objekte_search_core::model::LogicalType;
use objekte_search_core::query::options::ReturnMode;
use objekte_search_core::query::{
    CaseSensitivity, FtsMode, FtsQuery, Query, QueryOptions, parse_filter,
};
use objekte_search_core::query_results::Payload;
use objekte_search_core::repository::configuration::{CollectionDefinition, RepositoryOptions};
use objekte_search_core::schema::FieldDefinition;
use objekte_search_core::source::registry::SourceRegistry;
use objekte_search_core::source::{ExtractedRecord, SourceFormat, SourceReader};

fn query(collections: &[&str], mode: ReturnMode) -> Query {
    Query {
        collections: collections.iter().map(|name| (*name).into()).collect(),
        fts: None,
        filter: None,
        options: QueryOptions {
            count: true,
            limit: 10,
            offset: 0,
            sort: Vec::new(),
            return_mode: mode,
            evidence: false,
        },
    }
}

#[test]
fn loader_registration_is_independent_and_preserves_bytes() {
    let root = tempfile::tempdir().unwrap();
    let document = br#"{"title":"M\u00fcnchen","number":9007199254740993}"#;
    let lines = [b" \t\r\n".as_slice(), document, b"\r\n", document].concat();
    std::fs::write(root.path().join("one.json"), document).unwrap();
    std::fs::write(root.path().join("two.jsonl"), &lines).unwrap();
    let json = CollectionDefinition::folder("documents", "", SourceFormat::JsonDecodedScalars);
    let lines = CollectionDefinition::folder("lines", "", SourceFormat::JsonLines);

    let mut only_lines = SourceRegistry::default();
    objekte_loader_jsonl::register(&mut only_lines).unwrap();
    assert!(only_lines.resolve(&SourceFormat::Json).is_err());
    assert!(
        Index::start_build(
            root.path(),
            vec![json.clone()],
            RepositoryOptions::default(),
            only_lines.clone(),
        )
        .and_then(|task| task.wait_complete())
        .is_err()
    );
    let only_line_index = Index::start_build(
        root.path(),
        vec![lines.clone()],
        RepositoryOptions::default(),
        only_lines.clone(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    assert_eq!(
        only_line_index
            .view()
            .unwrap()
            .search(&query(&["lines"], ReturnMode::Locator))
            .unwrap()
            .count,
        Some(2)
    );

    let mut both = only_lines;
    objekte_loader_json::register(&mut both).unwrap();
    let index = Index::start_build(
        root.path(),
        vec![json, lines],
        RepositoryOptions::default(),
        both,
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let mut request = query(&["documents", "lines"], ReturnMode::Bytes);
    request.fts = Some(FtsQuery {
        case_sensitivity: CaseSensitivity::Sensitive,
        mode: FtsMode::Literal,
        literal: "München".into(),
        pattern_parameter: None,
        fields: Vec::new(),
    });
    request.options.evidence = true;
    let result = index.view().unwrap().search(&request).unwrap();
    assert_eq!(result.count, Some(3));
    for hit in result.hits {
        assert!(!hit.matched_fields.is_empty());
        let range = hit.source_range().unwrap();
        let mut expected = document.to_vec();
        if hit.collection == "lines" && range.start == 4 {
            expected.push(b'\r');
        }
        assert_eq!(hit.payload, Some(Payload::Bytes(expected)));
    }
}

#[test]
fn optional_schema_matches_programmatic_declarations_and_is_prepared_once() {
    let root = tempfile::tempdir().unwrap();
    let schema = root.path().join("schema.json");
    let schema_bytes = br#"{"type":"object","properties":{"year":{"type":"integer","index":true},"title":{"type":"string","index":false}}}"#;
    std::fs::write(&schema, schema_bytes).unwrap();
    std::fs::write(
        root.path().join("a.json"),
        br#"{"year":1808,"title":"Leipzig"}"#,
    )
    .unwrap();
    std::fs::write(
        root.path().join("b.json"),
        br#"{"year":1809,"title":"Weimar"}"#,
    )
    .unwrap();
    assert!(load_optional_schema(None, "documents").unwrap().is_none());
    let base = CollectionDefinition::folder("documents", "", SourceFormat::Json);
    let mut plain = base.clone();
    plain.discovery.exclude_files.push("schema.json".into());
    let mut explicit = plain.clone();
    let prepared = load_optional_schema(Some(&schema), &explicit.name)
        .unwrap()
        .unwrap();
    use sha2::{Digest, Sha256};
    assert_eq!(
        prepared.compiled.schema.source_digest(),
        Some(Sha256::digest(schema_bytes).into())
    );
    explicit.schema = prepared.compiled.schema;
    let mut programmatic = plain.clone();
    for (field, kind, indexed) in [
        ("year", LogicalType::Number, true),
        ("title", LogicalType::String, false),
    ] {
        programmatic
            .schema
            .register_field(FieldDefinition {
                path: vec![field.into()],
                kind,
                required: false,
                nullable: false,
                indexed,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    programmatic
        .schema
        .prohibit_auto_index(vec!["title".into()])
        .unwrap();
    let mut sources = SourceRegistry::default();
    objekte_loader_json::register(&mut sources).unwrap();
    assert!(sources.resolve(&SourceFormat::JsonLines).is_err());
    let mut request = query(&["documents"], ReturnMode::Logical);
    request.filter = Some(parse_filter("year = 1808").unwrap());
    let mut expected = None;
    for definition in [plain, explicit, programmatic] {
        let index = Index::start_build(
            root.path(),
            vec![definition],
            RepositoryOptions::default(),
            sources.clone(),
        )
        .and_then(|task| task.wait_complete())
        .unwrap();
        assert_eq!(index.status().unwrap().live_records, 2);
        let result = index.view().unwrap().search(&request).unwrap();
        assert_eq!(result.count, Some(1));
        let hit = result.hits.into_iter().next().unwrap();
        if let Some(expected) = &expected {
            assert_eq!(&hit.payload, expected);
        } else {
            expected = Some(hit.payload);
            // Remove the schema after preparing it, before indexing the schema-backed
            // collection. Repeated queries below use the compiled declarations.
            std::fs::remove_file(&schema).unwrap();
        }
        assert_eq!(
            index.view().unwrap().search(&request).unwrap().count,
            Some(1)
        );
    }
}

#[derive(Default)]
struct PlainTextReader {
    documents: AtomicUsize,
    ranges: AtomicUsize,
}

impl SourceReader for PlainTextReader {
    fn name(&self) -> &'static str {
        "json"
    }

    fn read_document(&self, input: &mut dyn Read, output: &mut Vec<u8>) -> io::Result<()> {
        self.documents.fetch_add(1, Ordering::Relaxed);
        input.read_to_end(output).map(|_| ())
    }

    fn read_range(&self, input: &mut dyn Read, output: &mut [u8]) -> io::Result<()> {
        self.ranges.fetch_add(1, Ordering::Relaxed);
        input.read_exact(output)
    }

    fn frame(&self, bytes: &[u8]) -> Result<Vec<ByteRange>, String> {
        Ok(vec![ByteRange {
            start: 0,
            end: bytes.len() as u64,
        }])
    }

    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
        Ok(ExtractedRecord {
            value: objekte_search_core::model::Value::String(text.into()),
            text: Vec::new(),
        })
    }
}

#[test]
fn opened_inputs_reach_the_registered_reader_and_exact_counts_skip_content() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("one.txt");
    std::fs::write(&path, b"first").unwrap();
    let backend = Arc::new(PlainTextReader::default());
    let mut registry = SourceRegistry::default();
    registry
        .register("json", 1, "txt", backend.clone())
        .unwrap();
    assert!(
        registry
            .resolve(&SourceFormat::Json)
            .unwrap()
            .reader()
            .scalar_verification()
            .is_none()
    );
    let collection = CollectionDefinition::folder("texts", "", SourceFormat::registered("json"));
    let index = Index::start_build(
        root.path(),
        vec![collection],
        RepositoryOptions::default(),
        registry,
    )
    .and_then(|task| task.wait_complete())
    .unwrap();

    assert_eq!(backend.documents.load(Ordering::Relaxed), 1);
    assert_eq!(
        index
            .view()
            .unwrap()
            .search(&query(&["texts"], ReturnMode::Locator))
            .unwrap()
            .count,
        Some(1)
    );
    assert_eq!(backend.ranges.load(Ordering::Relaxed), 0);
    let result = index
        .view()
        .unwrap()
        .search(&query(&["texts"], ReturnMode::Bytes))
        .unwrap();
    assert_eq!(
        result.hits[0].payload,
        Some(Payload::Bytes(b"first".to_vec()))
    );
    assert_eq!(backend.ranges.load(Ordering::Relaxed), 1);

    std::fs::write(root.path().join("second.txt"), b"second").unwrap();
    index.add_file(Path::new("second.txt")).unwrap();
    assert_eq!(backend.documents.load(Ordering::Relaxed), 2);
    assert_eq!(
        index
            .view()
            .unwrap()
            .search(&query(&["texts"], ReturnMode::Locator))
            .unwrap()
            .count,
        Some(2)
    );
    assert_eq!(backend.ranges.load(Ordering::Relaxed), 1);
}

#[test]
fn several_large_jsonl_files_preserve_one_pass_locators_and_publication() {
    use objekte_search_core::index::segment::SegmentPolicy;
    use objekte_search_core::lifecycle::IndexRevision;
    use objekte_search_core::repository::configuration::IdentityPolicy;
    use std::io::{BufWriter, Write};
    use std::num::NonZeroUsize;

    let root = tempfile::tempdir().unwrap();
    let mut expected = Vec::new();
    let mut total_bytes = 0;
    let count = 9_000;
    let files = 5;
    let padding = "x".repeat(1024);
    for file in 0..files {
        let path = root.path().join(format!("{file}.jsonl"));
        let mut output = BufWriter::new(std::fs::File::create(&path).unwrap());
        let mut offset = 0u64;
        for number in 0..count {
            let id = format!("r{file}-{number:05}");
            let line = format!(r#"{{"id":"{id}","title":"{padding}"}}"#);
            let end = offset + line.len() as u64;
            if number == 0 || number == count / 2 || number == count - 1 {
                expected.push((id, path.clone(), ByteRange { start: offset, end }));
            }
            writeln!(output, "{line}").unwrap();
            offset = end + 1;
        }
        output.flush().unwrap();
        total_bytes += offset;
    }
    let mut sources = SourceRegistry::default();
    objekte_loader_jsonl::register(&mut sources).unwrap();
    let mut collection = CollectionDefinition::folder("lines", "", SourceFormat::JsonLines);
    collection.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    for workers in [1, 4] {
        let options = RepositoryOptions {
            build_workers: NonZeroUsize::new(workers),
            segment_policy: SegmentPolicy {
                target_records: NonZeroUsize::new(1024).unwrap(),
                max_files: NonZeroUsize::new(1).unwrap(),
            },
            ..Default::default()
        };
        let index = Index::start_build(
            root.path(),
            vec![collection.clone()],
            options,
            sources.clone(),
        )
        .and_then(|task| task.wait_complete())
        .unwrap();
        let view = index.view().unwrap();
        let status = index.status().unwrap();
        assert_eq!(status.live_records, (files * count) as u64);
        assert_eq!(status.build_source_reads, files as u64);
        assert_eq!(status.build_source_bytes_read, total_bytes);
        assert_eq!(
            view.revision().0 - IndexRevision::INITIAL.0 - 1,
            files as u64
        );
        for (id, path, range) in &expected {
            let hit = view
                .get_record("lines", id, ReturnMode::Locator)
                .unwrap()
                .unwrap();
            assert_eq!(hit.source_range(), Some(*range));
            let file = view
                .file_catalog()
                .unwrap()
                .file_by_path(path.strip_prefix(root.path()).unwrap())
                .unwrap()
                .id;
            assert_eq!(hit.file.unwrap().file_id, file);
        }
    }
}

#[test]
fn exact_schema_bytes_change_configuration_with_two_collections_using_one_loader() {
    let root = tempfile::tempdir().unwrap();
    let left = root.path().join("one");
    let right = root.path().join("two");
    std::fs::create_dir(&left).unwrap();
    std::fs::create_dir(&right).unwrap();
    std::fs::write(left.join("record.json"), br#"{"value":5}"#).unwrap();
    std::fs::write(right.join("record.json"), br#"{"value":"five"}"#).unwrap();
    let one = root.path().join("one.schema.json");
    let two = root.path().join("two.schema.json");
    let first_schema =
        r#"{"type":"object","properties":{"value":{"type":"integer","index":true}}}"#;
    std::fs::write(&one, first_schema).unwrap();
    std::fs::write(
        &two,
        r#"{"type":"object","properties":{"value":{"type":"string","index":true}}}"#,
    )
    .unwrap();
    let mut sources = SourceRegistry::default();
    objekte_loader_json::register(&mut sources).unwrap();

    let build = || {
        let mut one_definition = CollectionDefinition::folder("first", "one", SourceFormat::Json);
        one_definition.schema = load_optional_schema(Some(&one), "first")
            .unwrap()
            .unwrap()
            .compiled
            .schema;
        let mut two_definition = CollectionDefinition::folder("second", "two", SourceFormat::Json);
        two_definition.schema = load_optional_schema(Some(&two), "second")
            .unwrap()
            .unwrap()
            .compiled
            .schema;
        assert_ne!(
            one_definition.schema.source_digest(),
            two_definition.schema.source_digest()
        );
        Index::start_build(
            root.path(),
            vec![one_definition, two_definition],
            RepositoryOptions::default(),
            sources.clone(),
        )
        .and_then(|task| task.wait_complete())
        .unwrap()
    };
    let before = build().view().unwrap();
    std::fs::write(&one, format!("{first_schema}\n")).unwrap();
    let after = build().view().unwrap();
    assert_ne!(
        before.configuration_identity(),
        after.configuration_identity()
    );
    for (name, expression) in [("first", "value = 5"), ("second", "value = 'five'")] {
        let mut request = query(&[name], ReturnMode::Logical);
        request.filter = Some(parse_filter(expression).unwrap());
        let old = before.search(&request).unwrap();
        let new = after.search(&request).unwrap();
        assert_eq!(old.count, Some(1));
        assert_eq!(old.hits[0].payload, new.hits[0].payload);
    }
}
