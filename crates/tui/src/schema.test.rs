use super::*;
use objekte_search_core::model::LogicalType;
use objekte_search_core::query::{CaseSensitivity, FtsMode};
use objekte_search_core::query::{Query, QueryOptions, parse_filter};
use objekte_search_core::relation::RelationCardinality;
use objekte_search_core::repository::{Repository, RepositoryOptions, SnapshotMode};
use objekte_search_core::workload::{ReturnMode, WorkloadCase};

#[test]
fn schema_files_remain_an_addon_to_in_memory_definitions() {
    let temp = tempfile::tempdir().unwrap();
    let schemas = temp.path().join(".objekte/collections");
    std::fs::create_dir_all(&schemas).unwrap();
    std::fs::create_dir_all(temp.path().join("data/docs")).unwrap();
    std::fs::write(
        schemas.join("docs.schema.json"),
        r#"{"type":"object","properties":{"title":{"type":"string"},"year":{"type":"number"}},"x-index-paths":["year"],"x-sort-indexes":[[{"field":"year","direction":"asc"}]],"x-objekte":{"name":"docs","path":"data/docs","format":"json","id":"filename"}}"#,
    ).unwrap();
    std::fs::write(
        temp.path().join("data/docs/one.json"),
        r#"{"title":"needle","year":1810}"#,
    )
    .unwrap();
    let definitions = load_definitions(temp.path()).unwrap();
    assert_eq!(definitions.len(), 1);
    assert!(definitions[0].schema.fields()[&vec!["year".into()]].indexed);
    assert_eq!(definitions[0].sort_fields.len(), 1);
    let repository = Repository::open_with_options(
        temp.path(),
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..RepositoryOptions::default()
        },
    )
    .unwrap();
    let result = repository
        .search(&Query {
            collections: vec!["docs".into()],
            fts: None,
            filter: Some(parse_filter("year >= 1800").unwrap()),
            options: QueryOptions {
                sort: Vec::new(),
                limit: 10,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Bytes,
                evidence: true,
            },
        })
        .unwrap();
    assert_eq!(result.count.unwrap(), 1);
    assert_eq!(result.hits[0].record_id, "one");
}

#[test]
fn musenalm_schemas_load_without_core_specific_vocabulary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let definitions = load_definitions(&root).unwrap();
    assert_eq!(definitions.len(), 7);
    assert!(
        definitions
            .iter()
            .any(|definition| definition.name == "contents")
    );
    assert_eq!(
        definitions
            .iter()
            .map(|definition| definition.schema.relations().len())
            .sum::<usize>(),
        6
    );
    let contents = definitions
        .iter()
        .find(|definition| definition.name == "contents")
        .unwrap();
    assert_eq!(
        contents.schema.fields()[&vec!["entry".into()]].kind,
        LogicalType::Id
    );
    assert_eq!(
        contents.schema.fields()[&vec!["contributors".into(), "agents".into()]].kind,
        LogicalType::Id
    );
    assert_eq!(
        contents.schema.relations()[&vec!["contributors".into(), "agents".into()]].cardinality,
        RelationCardinality::Many
    );
    let series = definitions
        .iter()
        .find(|definition| definition.name == "series")
        .unwrap();
    assert_eq!(
        series.schema.fields()[&vec!["entries".into()]].kind,
        LogicalType::Array(Box::new(LogicalType::Id))
    );
    assert_eq!(
        series.schema.relations()[&vec!["related_entries".into(), "entry".into()]]
            .reverse_alias
            .as_deref(),
        Some("series__related_entries__entry")
    );
}

#[test]
fn musenalm_relation_adapter_resolves_real_records_and_keeps_id_text_searchable() {
    let musenalm = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let definitions = load_definitions(&musenalm).unwrap();
    let temp = tempfile::tempdir().unwrap();
    for definition in &definitions {
        std::fs::create_dir_all(temp.path().join(&definition.source_path)).unwrap();
    }
    for (collection, id) in [
        ("contents", "000dl3mfclrmjwu"),
        ("entries", "jv829b9kekdb5d3"),
    ] {
        let file = format!("data/{collection}/{id}.json");
        std::fs::copy(musenalm.join(&file), temp.path().join(&file)).unwrap();
    }
    let repository = Repository::open_with_options(
        temp.path(),
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..RepositoryOptions::default()
        },
    )
    .unwrap();
    assert_eq!(repository.collection_graph().unwrap().edges.len(), 6);
    let outgoing = repository
        .relation_outgoing_page("contents", &["entry".into()], "000dl3mfclrmjwu", 0, 10)
        .unwrap();
    assert_eq!(outgoing.total_occurrences, 1);
    assert_eq!(outgoing.references[0].target_id, "jv829b9kekdb5d3");
    assert!(outgoing.references[0].resolved.is_some());
    let backlinks = repository
        .relation_backlink_page("entries", "contents__entry", "jv829b9kekdb5d3", 0, 10)
        .unwrap();
    assert_eq!(backlinks.total_sources, 1);
    assert_eq!(backlinks.sources[0].id, "000dl3mfclrmjwu");
    let request = Query::from_workload(&WorkloadCase {
        id: "id-text".into(),
        collection: "contents".into(),
        fts: Some("jv829b9kekdb5d3".into()),
        fts_case_sensitivity: CaseSensitivity::Sensitive,
        fts_mode: FtsMode::Literal,
        filter: None,
        fts_fields: vec![],
        sort: vec![],
        limit: 10,
        offset: 0,
        count: true,
        return_mode: ReturnMode::Logical,
        evidence: true,
    })
    .unwrap();
    let result = repository.search(&request).unwrap();
    assert_eq!(result.count, Some(1));
    assert_eq!(result.hits[0].record_id, "000dl3mfclrmjwu");
}

#[test]
fn relation_annotations_reject_invalid_types_and_accept_explicit_aliases() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join(".objekte/collections");
    std::fs::create_dir_all(&directory).unwrap();
    let schema = directory.join("sources.json");
    let template = |relation: &str| {
        format!(
            r#"{{"type":"object","properties":{{"target":{{"type":"string","x-relation":{relation}}}}},"x-objekte":{{"name":"sources","path":"data/sources","format":"json","id":"filename"}}}}"#
        )
    };
    std::fs::write(
        &schema,
        template(r#"{"collection":"targets","reverse_alias":"incoming_sources"}"#),
    )
    .unwrap();
    let definitions = load_definitions(temp.path()).unwrap();
    assert_eq!(
        definitions[0].schema.relations()[&vec!["target".into()]]
            .reverse_alias
            .as_deref(),
        Some("incoming_sources")
    );
    for invalid in [
        r#"{"collection":3}"#,
        r#"{"collection":""}"#,
        r#"{"collection":"targets","reverse_alias":4}"#,
    ] {
        std::fs::write(&schema, template(invalid)).unwrap();
        assert!(load_definitions(temp.path()).is_err(), "{invalid}");
    }
}
