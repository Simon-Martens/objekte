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

fn collection() -> CollectionDefinition {
    let mut schema = Schema::default();
    for (name, kind) in [("id", LogicalType::Id), ("title", LogicalType::String)] {
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
        sort_fields: vec![FieldPath::parse("title").unwrap()],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }
}

#[test]
fn regex_fts_id_parameter_and_inline_sort_share_exact_result_identity() {
    let root = std::env::temp_dir().join(format!(
        "objekte-g2-interactions-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (file, document) in [
        ("one", r#"{"id":"A","title":"FooBar"}"#),
        ("two", r#"{"id":"a","title":"FooBar"}"#),
        ("three", r#"{"id":"C","title":"Other"}"#),
    ] {
        std::fs::write(root.join(format!("data/{file}.json")), document).unwrap();
    }
    let case = WorkloadCase {
        id: "g2-regex-id-sort".into(),
        collection: "data".into(),
        fts: Some("^Foo.*Bar$".into()),
        fts_case_sensitivity: CaseSensitivity::Sensitive,
        fts_mode: FtsMode::Regex(RegexOptions::default()),
        filter: Some("id = $id SORT BY title".into()),
        fts_fields: vec!["title".into()],
        sort: vec![],
        limit: 2,
        offset: 0,
        count: true,
        return_mode: ReturnMode::Logical,
        evidence: true,
    };
    let template = Query::from_workload(&case).unwrap();
    assert!(template.options.sort.len() == 1);
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let options = RepositoryOptions {
            bigram_alphabet: alphabet,
            snapshot_mode: SnapshotMode::IfValid,
            verified_cache_bytes: 1024 * 1024,
            ..Default::default()
        };
        let built = Repository::open_with_options(&root, vec![collection()], options).unwrap();
        built.save_snapshot(&objekte_search_core::storage::snapshot_path(&root, options.bigram_alphabet)).unwrap();
        let restored = Repository::open_with_options(&root, vec![collection()], options).unwrap();
        assert!(restored.startup().restored);
        for repository in [&built, &restored] {
            for (iteration, id) in ["A", "a", "A"].into_iter().enumerate() {
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
                assert_eq!(actual.count, Some(1));
                assert_eq!(actual.hits, reference.hits);
                assert_eq!(actual.hits[0].record_id, id);
                assert!(matches!(
                    actual.hits[0].payload,
                    Some(objekte_search_core::repository::Payload::Logical(
                        Value::Object(_)
                    ))
                ));
                if iteration == 2 {
                    assert!(actual.counters.verified_result_hit);
                }
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
