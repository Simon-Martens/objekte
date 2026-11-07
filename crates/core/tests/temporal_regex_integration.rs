use std::collections::HashMap;

use objekte_search_core::model::{FieldPath, LogicalType, Value};
use objekte_search_core::parameters::parse_parameter_json;
use objekte_search_core::query::{CaseSensitivity, FtsMode, Query, RegexOptions};
use objekte_search_core::reference::{FieldDefinition, Schema};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::{BigramAlphabet, ReturnMode, WorkloadCase};

fn definition() -> CollectionDefinition {
    let mut schema = Schema::default();
    for (name, kind) in [
        ("id", LogicalType::Id),
        ("title", LogicalType::String),
        ("issued", LogicalType::Date),
        ("declared_size", LogicalType::Number),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
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
        format: SourceFormat::Json,
        identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
        discovery: Default::default(),
        schema,
        sort_fields: vec![FieldPath::parse("declared_size").unwrap()],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }
}

fn request(size: &str) -> Query {
    Query::from_workload(&WorkloadCase {
        id: "temporal-regex-parameter".into(),
        collection: "data".into(),
        fts: Some("^Foo.*Bar$".into()),
        fts_case_sensitivity: CaseSensitivity::Sensitive,
        fts_mode: FtsMode::Regex(RegexOptions::default()),
        filter: Some(format!(
            "issued = 2024-02 && declared_size = {size} && id = $id SORT BY declared_size DESC"
        )),
        fts_fields: vec!["title".into()],
        sort: vec![],
        limit: 2,
        offset: 0,
        count: true,
        return_mode: ReturnMode::Logical,
        evidence: true,
    })
    .unwrap()
}

#[test]
fn temporal_regex_size_id_and_sort_keep_answers_across_cache_and_restore() {
    let root = std::env::temp_dir().join(format!(
        "objekte-temporal-regex-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (file, document) in [
        (
            "a",
            r#"{"id":"A","title":"FooBar","issued":"2024-02-29","declared_size":1536}"#,
        ),
        (
            "b",
            r#"{"id":"B","title":"FooBar","issued":"2024-03-01","declared_size":1536}"#,
        ),
        (
            "c",
            r#"{"id":"C","title":"FooBuzz","issued":"2024-02-01","declared_size":1536}"#,
        ),
        (
            "d",
            r#"{"id":"D","title":"Other","issued":"2024-02-15","declared_size":1536}"#,
        ),
    ] {
        std::fs::write(root.join(format!("data/{file}.json")), document).unwrap();
    }
    let template = request("1.5KiB");
    let equivalent = request("1536B");
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let options = RepositoryOptions {
            bigram_alphabet: alphabet,
            snapshot_mode: SnapshotMode::IfValid,
            verified_cache_bytes: 1 << 20,
            ..Default::default()
        };
        let built = Repository::open_with_options(&root, vec![definition()], options).unwrap();
        built
            .save_snapshot(&objekte_search_core::storage::snapshot_path(
                &root,
                options.bigram_alphabet,
            ))
            .unwrap();
        let restored = Repository::open_with_options(&root, vec![definition()], options).unwrap();
        assert!(restored.startup().restored);
        for repository in [&built, &restored] {
            for (step, id, expected) in [(0, "A", Some("A")), (1, "B", None), (2, "A", Some("A"))] {
                let params = HashMap::from([(
                    "id".to_owned(),
                    parse_parameter_json(&format!(r#"{{"type":"id","value":"{id}"}}"#)).unwrap(),
                )]);
                let actual = repository
                    .search_with_parameters(&template, &params)
                    .unwrap();
                let reference = repository
                    .search_reference_with_parameters(&template, &params)
                    .unwrap();
                assert_eq!(actual.hits, reference.hits);
                assert_eq!(actual.count, Some(usize::from(expected.is_some())));
                assert_eq!(
                    actual.hits.first().map(|hit| hit.record_id.as_str()),
                    expected
                );
                if let Some(hit) = actual.hits.first() {
                    assert!(matches!(
                        hit.payload,
                        Some(objekte_search_core::repository::Payload::Logical(
                            Value::Object(_)
                        ))
                    ));
                }
                if step == 2 {
                    assert!(actual.counters.verified_result_hit);
                    assert!(
                        repository
                            .search_with_parameters(&equivalent, &params)
                            .unwrap()
                            .counters
                            .verified_result_hit
                    );
                }
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
