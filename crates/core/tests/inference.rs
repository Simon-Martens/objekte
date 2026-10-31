use objekte_core::{
    Freshness, InferOptions, InferredKind, InferredLinkKind, JsonQuery, JsonQueryBackend, Options,
    Repository, TextFormat,
};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn write(root: &Path, path: &str, value: Value) {
    let target = root.join(path);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(target, serde_json::to_vec(&value).unwrap()).unwrap();
}

fn fixture(schema: bool) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "data/people/person.json", json!({"name":"Ada"}));
    for number in 0..24 {
        write(
            root,
            &format!("data/docs/{number:02}.json"),
            json!({
                "name": format!("Document {number}"),
                "category": if number % 2 == 0 { "essay" } else { "letter" },
                "active": number % 2 == 0,
            "detail": {"rank": if number == 0 { Value::Null } else { json!(number) }, "a.b": number},
                "person": "person",
                "pointer": "#/detail/rank",
                "external_pointer": "../people/person.json#/name",
                "html": "<p>Hello <strong>world</strong></p>",
                "markdown": "# Heading\n\n**Hello** world",
                "xml": "<?xml version=\"1.0\"?><note>Hello</note>",
                "items": [{"score": number, "label": "A"}, {"score": number + 1, "label": "B"}],
            }),
        );
    }
    fs::write(root.join("data/docs/broken.json"), b"{broken").unwrap();
    if schema {
        write(
            root,
            ".editor/collections/docs/schema.json",
            json!({
            "type":"object", "properties":{"name":{"type":"string"},"active":{"type":"boolean"}}
            }),
        );
    }
    temp
}

fn field<'a>(
    report: &'a objekte_core::CollectionInference,
    path: &str,
) -> &'a objekte_core::FieldInference {
    report
        .fields
        .iter()
        .find(|field| field.display_path == path)
        .unwrap_or_else(|| {
            panic!(
                "missing {path}; paths: {:?}",
                report
                    .fields
                    .iter()
                    .map(|f| &f.display_path)
                    .collect::<Vec<_>>()
            )
        })
}

fn query(repository: &mut Repository, filter: &str) -> objekte_core::JsonQueryResult {
    repository
        .query_json(&JsonQuery {
            filter: filter.into(),
            collections: Some(vec!["docs".into()]),
            count: true,
            limit: 100,
            ..Default::default()
        })
        .unwrap()
}

#[test]
fn explicit_scan_infers_nested_types_links_markup_and_builds_postings() {
    let temp = fixture(false);
    let mut repository = Repository::open(temp.path()).unwrap();
    let report = repository
        .infer_and_auto_index(Some("docs"), InferOptions::default())
        .unwrap();
    assert_eq!(report.collections.len(), 1);
    let docs = &report.collections[0];
    assert_eq!((docs.files, docs.parsed, docs.malformed), (25, 24, 1));
    assert_eq!(field(docs, "category").kind, InferredKind::Enum);
    assert_eq!(
        field(docs, "category").enum_values.as_ref().unwrap(),
        &["essay", "letter"]
    );
    assert!(field(docs, "category").indexed);
    assert_eq!(field(docs, "active").kind, InferredKind::Boolean);
    assert!(field(docs, "detail.rank").indexed);
    assert_eq!(field(docs, "items[*].score").kind, InferredKind::Integer);
    assert!(field(docs, "items[*].score").indexed);
    assert!(field(docs, "items[*].label").indexed);
    let link = field(docs, "person").link.as_ref().unwrap();
    assert_eq!(link.kind, InferredLinkKind::RecordId);
    assert_eq!(link.target_collection, "people");
    assert_eq!(
        field(docs, "pointer").link.as_ref().unwrap().kind,
        InferredLinkKind::JsonPointer
    );
    assert_eq!(
        field(docs, "external_pointer")
            .link
            .as_ref()
            .unwrap()
            .target_collection,
        "people"
    );
    assert_eq!(field(docs, "html").text_format, Some(TextFormat::Html));
    assert_eq!(
        field(docs, "markdown").text_format,
        Some(TextFormat::Markdown)
    );
    assert_eq!(field(docs, "xml").text_format, Some(TextFormat::Xml));
    for (filter, total) in [
        ("category = 'essay'", 12),
        ("active = true", 12),
        ("detail.rank >= 20", 4),
        ("detail.rank = null", 1),
        ("missing(detail.rank)", 0),
        ("items[*].score ?= 21", 2),
        ("items[*].score ?>= 23", 2),
        ("items[*].label ?= 'B'", 24),
    ] {
        let result = query(&mut repository, filter);
        assert_eq!(result.total, Some(total), "{filter}");
        assert_eq!(result.metrics.grep_scans, 0, "{filter}");
        let direct = repository
            .query_json(&JsonQuery {
                filter: filter.into(),
                collections: Some(vec!["docs".into()]),
                count: true,
                limit: 100,
                backend: JsonQueryBackend::Direct,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(result.total, direct.total, "{filter}");
    }
    let second = repository
        .infer_and_auto_index(Some("docs"), InferOptions::default())
        .unwrap();
    assert_eq!(second.collections[0].indexed_paths, docs.indexed_paths);
}

#[test]
fn inference_without_indexing_leaves_queries_on_fallback() {
    let temp = fixture(false);
    let mut repository = Repository::open(temp.path()).unwrap();
    let report = repository
        .infer_and_auto_index(
            Some("docs"),
            InferOptions {
                auto_index: false,
                write_schema: false,
            },
        )
        .unwrap();
    assert_eq!(
        field(&report.collections[0], "category").kind,
        InferredKind::Enum
    );
    assert!(!field(&report.collections[0], "category").indexed);
    assert!(
        query(&mut repository, "category = 'essay'")
            .metrics
            .grep_scans
            > 0
    );
}

#[test]
fn optional_schema_write_preserves_declared_fields_and_adds_nested_types() {
    let temp = fixture(true);
    let mut repository = Repository::open(temp.path()).unwrap();
    let report = repository
        .infer_and_auto_index(
            Some("docs"),
            InferOptions {
                auto_index: true,
                write_schema: true,
            },
        )
        .unwrap();
    assert!(report.collections[0].schema_written);
    let path = temp.path().join(".editor/collections/docs/schema.json");
    let schema: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let properties = &schema["properties"];
    assert_eq!(properties["name"], json!({"type":"string"}));
    assert_eq!(properties["category"]["type"], "string");
    assert_eq!(properties["category"]["index"], true);
    assert_eq!(properties["active"]["type"], "boolean");
    assert!(properties["active"].get("index").is_none());
    assert_eq!(
        properties["detail"]["properties"]["rank"]["type"],
        json!(["integer", "null"])
    );
    assert_eq!(properties["person"]["x-relation"]["collection"], "people");
    assert!(properties["external_pointer"].get("x-relation").is_none());
    assert_eq!(properties["html"]["x-full-text"], "html");
    assert_eq!(
        properties["items"]["items"]["properties"]["score"]["type"],
        "integer"
    );
    let advanced = schema["x-index-paths"].as_array().unwrap();
    assert!(advanced.contains(&json!("detail.rank")));
    assert!(advanced.contains(&json!("active")));
    assert!(advanced.contains(&json!("items[*].score")));
    assert!(advanced.contains(&json!("detail[\"a.b\"]")));
    assert!(
        !repository
            .infer_and_auto_index(
                Some("docs"),
                InferOptions {
                    auto_index: true,
                    write_schema: true,
                }
            )
            .unwrap()
            .collections[0]
            .schema_written
    );
    assert_eq!(
        repository.model().collections["docs"].fields["active"].kind,
        objekte_core::ValueType::Boolean
    );
    assert_eq!(query(&mut repository, "active = true").total, Some(12));
    let mut reopened = Repository::open(temp.path()).unwrap();
    assert_eq!(
        query(&mut reopened, "detail.rank >= 20").metrics.grep_scans,
        0
    );
    assert_eq!(
        query(&mut reopened, "items[*].score ?= 21")
            .metrics
            .grep_scans,
        0
    );
    assert_eq!(
        query(&mut reopened, "detail[\"a.b\"] >= 20")
            .metrics
            .grep_scans,
        0
    );
}

#[test]
fn completed_report_can_be_written_later_but_rejects_changed_source() {
    let temp = fixture(true);
    let schema_path = temp.path().join(".editor/collections/docs/schema.json");
    let original = fs::read(&schema_path).unwrap();
    let mut repository = Repository::open(temp.path()).unwrap();
    let mut report = repository
        .infer_and_auto_index(Some("docs"), InferOptions::default())
        .unwrap();
    assert_eq!(fs::read(&schema_path).unwrap(), original);
    assert_eq!(repository.write_inferred_schema(&mut report).unwrap(), 1);
    assert!(report.collections[0].schema_written);
    let saved = fs::read(&schema_path).unwrap();
    let mut second = repository
        .infer_and_auto_index(Some("docs"), InferOptions::default())
        .unwrap();
    write(
        temp.path(),
        "data/people/person.json",
        json!({"name":"Changed"}),
    );
    let error = repository.write_inferred_schema(&mut second).unwrap_err();
    assert!(
        error.to_string().contains("source changed since inference"),
        "{error}"
    );
    assert_eq!(fs::read(&schema_path).unwrap(), saved);
}

#[test]
fn report_only_schema_write_records_types_without_new_index_rules() {
    let temp = fixture(true);
    let mut repository = Repository::open(temp.path()).unwrap();
    repository
        .infer_and_auto_index(
            Some("docs"),
            InferOptions {
                auto_index: false,
                write_schema: true,
            },
        )
        .unwrap();
    let schema: Value = serde_json::from_slice(
        &fs::read(temp.path().join(".editor/collections/docs/schema.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        schema["properties"]["detail"]["properties"]["rank"]["type"],
        json!(["integer", "null"])
    );
    assert!(schema.get("x-index-paths").is_none());
    assert!(schema["properties"]["category"].get("index").is_none());
}

#[test]
fn schema_rejects_duplicate_advanced_index_paths() {
    let temp = tempfile::tempdir().unwrap();
    write(
        temp.path(),
        "data/docs/a.json",
        json!({"detail":{"rank":1}}),
    );
    write(
        temp.path(),
        ".editor/collections/docs/schema.json",
        json!({
            "type":"object", "properties":{},
            "x-index-paths":["detail.rank", "detail.rank"]
        }),
    );
    let error = Repository::open(temp.path()).err().unwrap();
    assert!(
        error.to_string().contains("duplicate index path"),
        "{error}"
    );
}

#[test]
fn changed_schema_blocks_later_write() {
    let temp = fixture(true);
    let mut repository = Repository::open(temp.path()).unwrap();
    let mut report = repository
        .infer_and_auto_index(Some("docs"), InferOptions::default())
        .unwrap();
    let path = temp.path().join(".editor/collections/docs/schema.json");
    write(
        temp.path(),
        ".editor/collections/docs/schema.json",
        json!({
            "type":"object", "properties":{"name":{"type":"string"}, "active":{"type":"boolean"}, "new":{"type":"string"}}
        }),
    );
    let changed = fs::read(&path).unwrap();
    let error = repository.write_inferred_schema(&mut report).unwrap_err();
    assert!(
        error.to_string().contains("schema changed since inference"),
        "{error}"
    );
    assert_eq!(fs::read(path).unwrap(), changed);
}

#[test]
fn explicit_index_false_is_not_saved_as_an_advanced_rule() {
    let temp = fixture(true);
    write(
        temp.path(),
        ".editor/collections/docs/schema.json",
        json!({
            "type":"object", "properties":{"name":{"type":"string"}, "active":{"type":"boolean", "index":false}}
        }),
    );
    let mut repository = Repository::open(temp.path()).unwrap();
    repository
        .infer_and_auto_index(
            Some("docs"),
            InferOptions {
                auto_index: true,
                write_schema: true,
            },
        )
        .unwrap();
    let schema: Value = serde_json::from_slice(
        &fs::read(temp.path().join(".editor/collections/docs/schema.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(schema["properties"]["active"]["index"], false);
    assert!(
        !schema["x-index-paths"]
            .as_array()
            .unwrap()
            .contains(&json!("active"))
    );
}

#[test]
fn inferred_postings_survive_rebuild_and_hash_on_request() {
    let temp = fixture(false);
    let mut options = Options::new();
    options.freshness = Freshness::HashOnRequest;
    let mut repository = Repository::builder(temp.path())
        .options(options)
        .open()
        .unwrap();
    repository
        .infer_and_auto_index(Some("docs"), InferOptions::default())
        .unwrap();
    assert_eq!(
        query(&mut repository, "items[*].score ?= 21")
            .metrics
            .grep_scans,
        0
    );
    repository.rebuild_bigram_index("docs").unwrap();
    assert_eq!(
        query(&mut repository, "items[*].score ?= 21")
            .metrics
            .grep_scans,
        0
    );
    write(
        temp.path(),
        "data/docs/00.json",
        json!({
            "category":"changed", "active":true, "detail":{"rank":0},
            "items":[{"score":100,"label":"A"}],
        }),
    );
    repository
        .infer_and_auto_index(Some("docs"), InferOptions::default())
        .unwrap();
    assert_eq!(
        query(&mut repository, "category = 'changed'").total,
        Some(1)
    );
    assert_eq!(
        query(&mut repository, "category = 'changed'")
            .metrics
            .grep_scans,
        0
    );
}

#[test]
fn large_array_drops_its_partial_posting() {
    let temp = tempfile::tempdir().unwrap();
    write(
        temp.path(),
        "data/docs/large.json",
        json!({
            "values": (0..5_000).collect::<Vec<_>>()
        }),
    );
    let mut repository = Repository::open(temp.path()).unwrap();
    let report = repository
        .infer_and_auto_index(None, InferOptions::default())
        .unwrap();
    let values = field(&report.collections[0], "values[*]");
    assert_eq!(values.kind, InferredKind::Integer);
    assert!(!values.indexed);
    assert!(values.multiple_values);
}

#[test]
fn existential_comparison_keeps_invalid_array_record_with_static_scalar_index() {
    let temp = tempfile::tempdir().unwrap();
    write(temp.path(), "data/docs/array.json", json!({"tag":["A"]}));
    write(temp.path(), "data/docs/scalar.json", json!({"tag":"B"}));
    write(
        temp.path(),
        ".editor/collections/docs/schema.json",
        json!({
            "type":"object", "properties":{"tag":{"type":"string", "index":true}}
        }),
    );
    let mut repository = Repository::open(temp.path()).unwrap();
    let result = query(&mut repository, "tag ?= 'A'");
    assert_eq!(result.total, Some(1));
    assert_eq!(result.records[0].id, "array");
}
