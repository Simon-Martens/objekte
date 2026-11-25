use crate::query::options::ReturnMode;
use crate::query::{CaseSensitivity, FtsMode, FtsQuery, Query, QueryOptions};
use crate::repository::configuration::{CollectionDefinition, IdentityPolicy, RepositoryOptions};
use crate::source::SourceFormat;

fn root(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("objekte-folder-{name}-{}", std::process::id()))
}

#[test]
fn folder_identity_is_supplied_by_the_caller() {
    for format in [
        SourceFormat::registered("plain"),
        SourceFormat::with_compatibility_encoding("plain", "OldSelector"),
    ] {
        let definition = CollectionDefinition::folder("data", "data", format.clone());
        assert_eq!(definition.identity, IdentityPolicy::Generated);
        let definition = CollectionDefinition::folder_with_identity(
            "data",
            "data",
            format,
            IdentityPolicy::RelativePath,
        );
        assert_eq!(definition.identity, IdentityPolicy::RelativePath);
    }
}

#[test]
fn recursive_schema_free_folder_uses_relative_path_identity_and_no_typed_fields() {
    let root = root("nested");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data/a")).unwrap();
    std::fs::create_dir_all(root.join("data/b")).unwrap();
    std::fs::create_dir_all(root.join(".objekte/collections")).unwrap();
    std::fs::write(root.join(".objekte/collections/data.row"), "not a schema").unwrap();
    std::fs::write(root.join("data/a/item.row"), r#"("title"="Goethe")"#).unwrap();
    std::fs::write(root.join("data/b/item.row"), r#"("title"="Other")"#).unwrap();
    let definition = crate::source::test_support::document_folder("data", "data");
    assert!(definition.schema.fields.is_empty());
    assert!(definition.discovery.recursive);
    assert_eq!(definition.identity, IdentityPolicy::RelativePath);
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition],
        RepositoryOptions {
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
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
    assert_eq!(result.hits[0].record_id, "path:a/item.row");
    assert_eq!(repo.collections[0].generation.records.len(), 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn lines_folder_uses_distinct_generated_record_ids() {
    let root = root("fixture-lines");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/items.rows"),
        "(\"name\"=\"one\")\n(\"name\"=\"two\")\n",
    )
    .unwrap();
    let definition =
        CollectionDefinition::folder("data", "data", crate::source::test_support::line_format());
    assert_eq!(definition.identity, IdentityPolicy::Generated);
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition],
        RepositoryOptions {
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
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
