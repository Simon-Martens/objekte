use super::*;

#[test]
fn stable_icu_keys_and_fixed_null_placement() {
    let locale: Locale = "de".parse().unwrap();
    let collator = CollatorBorrowed::try_new(locale.into(), CollatorOptions::default()).unwrap();
    let key = |s: &str| {
        let mut out = Vec::new();
        collator.write_sort_key_to(s, &mut out).unwrap();
        SortKey::String(out)
    };
    assert!(key("Ähre").compare(&key("Zug"), false).is_lt());
    assert!(key("Ähre").compare(&key("Zug"), true).is_gt());
    assert!(SortKey::Null.compare(&key("Zug"), true).is_gt());
    assert!(SortKey::Missing.compare(&SortKey::Null, true).is_gt());
    assert!(
        SortKey::Number(Number::integer(9_007_199_254_740_993))
            .compare(
                &SortKey::Number(Number::integer(9_007_199_254_740_992)),
                false
            )
            .is_gt()
    );
}

#[test]
fn configured_collation_changes_order_and_restore_compatibility() {
    use crate::model::LogicalType;
    use crate::query::{Query, QueryOptions};
    use crate::reference::{FieldDefinition, Schema};
    use crate::repository::{CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions};
    use crate::source::SourceFormat;
    use crate::workload::{ReturnMode, SortField};

    let root = std::env::temp_dir().join(format!("objekte-collation-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (id, body) in [
        ("a", r#"{"name":"ä"}"#),
        ("b", r#"{"name":"z"}"#),
        ("c", r#"{"name":"a10"}"#),
        ("d", r#"{"name":"a2"}"#),
        ("e", r#"{"name":null}"#),
        ("f", r#"{}"#),
        ("g", r#"{"name":"A"}"#),
        ("h", r#"{"name":"a"}"#),
    ] {
        std::fs::write(root.join(format!("data/{id}.json")), body).unwrap();
    }
    let path = FieldPath::parse("name").unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["name".into()],
            kind: LogicalType::String,
            required: false,
            nullable: true,
            indexed: false,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let definition = CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::Json,
        identity: IdentityPolicy::Filename,
        discovery: Default::default(),
        schema,
        sort_fields: vec![path.clone()],
        sort_locale: "de".into(),
        sort_options: SortOptions::default(),
        sort_overrides: Default::default(),
    };
    let mut query = Query {
        collections: vec!["data".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: vec![SortField {
                field: "name".into(),
                descending: false,
            }],
            limit: 8,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let ids = |repo: &Repository, query: &Query| {
        repo.search(query)
            .unwrap()
            .hits
            .iter()
            .map(|h| h.record_id.clone())
            .collect::<Vec<_>>()
    };
    let built = Repository::open_with_options(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
    )
    .unwrap();
    let default_order = ids(&built, &query);
    assert!(
        default_order.iter().position(|id| id == "a").unwrap()
            < default_order.iter().position(|id| id == "b").unwrap()
    );
    assert!(
        default_order.iter().position(|id| id == "c").unwrap()
            < default_order.iter().position(|id| id == "d").unwrap()
    );
    assert_eq!(&default_order[6..], ["e", "f"]);
    drop(built);
    let restored = Repository::open_with_options(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
    )
    .unwrap();
    assert!(restored.startup().restored);
    assert_eq!(ids(&restored, &query), default_order);
    drop(restored);

    let mut changed = definition.clone();
    changed.sort_overrides.insert(
        path,
        SortConfig {
            locale: "sv".into(),
            options: SortOptions {
                strength: CollationStrength::Primary,
                numeric: true,
                nulls: Placement::First,
                missing: Placement::First,
            },
        },
    );
    let rebuilt =
        Repository::open_with_options(&root, vec![changed.clone()], RepositoryOptions::default())
            .unwrap();
    assert!(!rebuilt.startup().restored);
    let configured = ids(&rebuilt, &query);
    assert_eq!(&configured[..2], ["e", "f"]);
    assert!(
        configured.iter().position(|id| id == "b").unwrap()
            < configured.iter().position(|id| id == "a").unwrap()
    );
    assert!(
        configured.iter().position(|id| id == "d").unwrap()
            < configured.iter().position(|id| id == "c").unwrap()
    );
    assert!(
        configured.iter().position(|id| id == "g").unwrap()
            < configured.iter().position(|id| id == "h").unwrap()
    );
    let column = rebuilt.collections()[0].sorting.column("name").unwrap();
    let records = &rebuilt.collections()[0].generation.records;
    let g = records
        .iter()
        .find(|record| record.id.0 == "g")
        .unwrap()
        .slot;
    let h = records
        .iter()
        .find(|record| record.id.0 == "h")
        .unwrap()
        .slot;
    assert_eq!(column.keys[g.0 as usize], column.keys[h.0 as usize]);
    query.options.sort[0].descending = true;
    let full_descending = ids(&rebuilt, &query);
    assert!(
        full_descending.iter().position(|id| id == "g").unwrap()
            < full_descending.iter().position(|id| id == "h").unwrap()
    );
    query.options.offset = 1;
    query.options.limit = 3;
    let descending = ids(&rebuilt, &query);
    assert_eq!(descending, ["f", "a", "b"]);
    drop(rebuilt);
    let reopened =
        Repository::open_with_options(&root, vec![changed], RepositoryOptions::default()).unwrap();
    assert!(reopened.startup().restored);
    assert_eq!(ids(&reopened, &query), descending);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cross_collection_sort_rejects_different_collation() {
    use crate::model::LogicalType;
    use crate::query::{Query, QueryOptions};
    use crate::reference::{FieldDefinition, Schema};
    use crate::repository::{
        CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
    };
    use crate::source::SourceFormat;
    use crate::workload::{ReturnMode, SortField};

    let root = std::env::temp_dir().join(format!("objekte-cross-sort-{}", std::process::id()));
    for name in ["a", "b"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
        std::fs::write(root.join(format!("{name}/one.json")), r#"{"name":"one"}"#).unwrap();
    }
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["name".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let definitions = ["a", "b"].map(|name| CollectionDefinition {
        name: name.into(),
        source_path: name.into(),
        format: SourceFormat::Json,
        identity: IdentityPolicy::Filename,
        discovery: Default::default(),
        schema: schema.clone(),
        sort_fields: vec![FieldPath::parse("name").unwrap()],
        sort_locale: if name == "a" { "de" } else { "sv" }.into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    });
    let repo = Repository::open_with_options(
        &root,
        definitions.to_vec(),
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    let query = Query {
        collections: vec![],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: vec![SortField {
                field: "name".into(),
                descending: false,
            }],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    assert!(
        repo.search(&query)
            .unwrap_err()
            .contains("incompatible cross-collection sort")
    );
    let mut scoped = query;
    scoped.collections = vec!["a".into()];
    assert_eq!(repo.search(&scoped).unwrap().count, 1);
    std::fs::remove_dir_all(root).unwrap();
}
