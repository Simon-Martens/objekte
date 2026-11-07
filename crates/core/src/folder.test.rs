use crate::query::{CaseSensitivity, FtsMode, FtsQuery, Query, QueryOptions};
use crate::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
};
use crate::source::SourceFormat;
use crate::workload::ReturnMode;

fn root(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("objekte-folder-{name}-{}", std::process::id()))
}

#[test]
fn recursive_schema_free_folder_uses_relative_path_identity_and_no_typed_fields() {
    let root = root("nested");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data/a")).unwrap();
    std::fs::create_dir_all(root.join("data/b")).unwrap();
    std::fs::create_dir_all(root.join(".objekte/collections")).unwrap();
    std::fs::write(root.join(".objekte/collections/data.json"), "not a schema").unwrap();
    std::fs::write(root.join("data/a/item.json"), r#"{"title":"Goethe"}"#).unwrap();
    std::fs::write(root.join("data/b/item.json"), r#"{"title":"Other"}"#).unwrap();
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    assert!(definition.schema.fields.is_empty());
    assert!(definition.discovery.recursive);
    assert_eq!(definition.identity, IdentityPolicy::RelativePath);
    let repo = Repository::open_with_options(
        &root,
        vec![definition],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(repo.collections[0].postings.fields.is_empty());
    assert!(repo.collections[0].sorting.columns.is_empty());
    let request = Query {
        collections: vec!["data".into()],
        fts: Some(FtsQuery {
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Literal,
            literal: "Goethe".into(),
            pattern_parameter: None,
            fields: vec![],
        }),
        filter: None,
        options: QueryOptions {
            sort: vec![],
            limit: 20,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let result = repo.search(&request).unwrap();
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].record_id, "path:a/item.json");
    assert_eq!(repo.collections[0].generation.records.len(), 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn jsonl_folder_uses_distinct_generated_record_ids() {
    let root = root("jsonl");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/items.jsonl"),
        "{\"name\":\"one\"}\n{\"name\":\"two\"}\n",
    )
    .unwrap();
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    assert_eq!(definition.identity, IdentityPolicy::Generated);
    let repo = Repository::open_with_options(
        &root,
        vec![definition],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    let ids: std::collections::HashSet<_> = repo.collections[0]
        .generation
        .records
        .iter()
        .map(|record| record.id.clone())
        .collect();
    assert_eq!(ids.len(), 2);
    std::fs::remove_dir_all(root).unwrap();
}
