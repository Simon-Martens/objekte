#[path = "support/legacy_storage.rs"]
mod legacy_storage;
use legacy_storage::*;
use std::collections::HashMap;
use std::path::PathBuf;

use objekte_search_core::lifecycle::Index;
use objekte_search_core::model::{FieldPath, LogicalType};
use objekte_search_core::query::Query;
use objekte_search_core::reference::{FieldDefinition, Schema};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions,
};
use objekte_search_core::source::{SourceFormat, SourceRegistry};
use objekte_search_core::workload::{ReturnMode, SortExpression, SortField, WorkloadCase};

fn fixture() -> (PathBuf, Repository) {
    let root = std::env::temp_dir().join(format!(
        "objekte-inline-sort-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (id, title, year) in [
        ("a", "Alpha", 2),
        ("b", "Zulu", 3),
        ("c", "Alpha", 3),
        ("d", "Alpha", 3),
        ("e", "Middle", 1),
    ] {
        std::fs::write(
            root.join(format!("data/{id}.json")),
            format!(r#"{{"id":"{id}","title":"{title}","year":{year}}}"#),
        )
        .unwrap();
    }
    let mut schema = Schema::default();
    for (name, kind) in [
        ("id", LogicalType::String),
        ("title", LogicalType::String),
        ("year", LogicalType::Number),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: true,
                nullable: false,
                indexed: name == "year",
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
    }
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
            discovery: Default::default(),
            schema,
            sort_fields: ["title", "year", "@file.size"]
                .into_iter()
                .map(|name| FieldPath::parse(name).unwrap())
                .collect(),
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: HashMap::new(),
        }],
        RepositoryOptions {
            ..Default::default()
        },
    )
    .unwrap();
    (root, repo)
}

fn case(filter: Option<&str>, sort: Vec<SortField>) -> WorkloadCase {
    WorkloadCase {
        id: "inline-sort".into(),
        collection: "data".into(),
        fts: None,
        fts_case_sensitivity: Default::default(),
        fts_mode: Default::default(),
        filter: filter.map(str::to_owned),
        fts_fields: vec![],
        sort,
        limit: 10,
        offset: 0,
        count: true,
        return_mode: ReturnMode::Locator,
        evidence: true,
    }
}

#[test]
fn inline_sort_matches_registered_options_and_reference() {
    let (root, repo) = fixture();
    let inline = Query::from_workload(&case(Some("SORT BY year DESC, title"), vec![])).unwrap();
    let external = Query::from_workload(&case(
        None,
        vec![
            SortField {
                field: "year".into(),
                descending: true,
                expression: None,
            },
            SortField {
                field: "title".into(),
                descending: false,
                expression: None,
            },
        ],
    ))
    .unwrap();
    let actual = repo.search(&inline).unwrap();
    let ids: Vec<_> = actual
        .hits
        .iter()
        .map(|hit| hit.record_id.as_str())
        .collect();
    assert_eq!(ids, ["c", "d", "b", "a", "e"]);
    assert_eq!(actual.count, Some(5));
    assert_eq!(actual.hits, repo.search(&external).unwrap().hits);
    assert_eq!(actual.hits, repo.search_reference(&inline).unwrap().hits);
    let counted_plan = repo.explain(&inline).unwrap();
    assert_eq!(counted_plan[0].sort_strategy, "bounded_top_k");
    assert_eq!(counted_plan[0].normalized_sort, inline.options.sort);
    repo.configure_verified_cache(1024 * 1024).unwrap();
    assert_eq!(repo.search(&inline).unwrap().hits, actual.hits);
    assert_eq!(repo.search(&external).unwrap().hits, actual.hits);

    let filtered = Query::from_workload(&case(
        Some("year >= 2 SORT BY title ASC, year DESC"),
        vec![],
    ))
    .unwrap();
    let filtered_ids: Vec<_> = repo
        .search(&filtered)
        .unwrap()
        .hits
        .into_iter()
        .map(|hit| hit.record_id)
        .collect();
    assert_eq!(filtered_ids, ["c", "d", "a", "b"]);
    assert_eq!(
        repo.search(&filtered).unwrap().hits,
        repo.search_reference(&filtered).unwrap().hits
    );

    let mut fts_case = case(Some("SORT BY year DESC, title"), vec![]);
    fts_case.fts = Some("Alpha".into());
    let fts_query = Query::from_workload(&fts_case).unwrap();
    let fts_ids: Vec<_> = repo
        .search(&fts_query)
        .unwrap()
        .hits
        .into_iter()
        .map(|hit| hit.record_id)
        .collect();
    assert_eq!(fts_ids, ["c", "d", "a"]);
    assert_eq!(
        repo.search(&fts_query).unwrap().hits,
        repo.search_reference(&fts_query).unwrap().hits
    );

    let metadata = Query::from_workload(&case(Some("SORT BY @file.size DESC"), vec![])).unwrap();
    assert_eq!(
        repo.search(&metadata).unwrap().hits,
        repo.search_reference(&metadata).unwrap().hits
    );

    let mut page = inline.clone();
    page.options.count = false;
    page.options.offset = 2;
    page.options.limit = 2;
    let page_result = repo.search(&page).unwrap();
    assert_eq!(
        repo.explain(&page).unwrap()[0].sort_strategy,
        "bounded_top_k"
    );
    assert_eq!(
        page_result
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["b", "a"]
    );
    assert_eq!(page_result.hits, repo.search_reference(&page).unwrap().hits);

    let mut first_page = Query::from_workload(&case(Some("SORT BY year DESC"), vec![])).unwrap();
    first_page.options.count = false;
    first_page.options.limit = 1;
    assert_eq!(
        repo.explain(&first_page).unwrap()[0].sort_strategy,
        "stored_order_page"
    );

    let missing = Query::from_workload(&case(Some("SORT BY absent"), vec![])).unwrap();
    assert_eq!(
        repo.search(&missing)
            .unwrap()
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c", "d", "e"]
    );
    drop(repo);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_composite_order_streams_and_matches_reference() {
    let (root, mut repo) = fixture();
    // The fixture constructor is reused below with a separate explicit schema.
    drop(repo);
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    for (name, kind) in [
        ("id", LogicalType::String),
        ("title", LogicalType::String),
        ("year", LogicalType::Number),
    ] {
        definition
            .schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: true,
                nullable: false,
                indexed: false,
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
    }
    definition.sort_fields = ["year", "title"]
        .into_iter()
        .map(|path| FieldPath::parse(path).unwrap())
        .collect();
    definition
        .schema
        .register_composite_sort(vec![
            SortField::computed(
                SortExpression {
                    path: "year".into(),
                    binding: None,
                    reducer: None,
                },
                true,
            ),
            SortField {
                field: "title".into(),
                descending: false,
                expression: None,
            },
        ])
        .unwrap();
    assert!(
        definition
            .schema
            .register_composite_sort(vec![
                SortField::simple("year", true),
                SortField::simple("title", false),
            ])
            .unwrap_err()
            .contains("duplicate composite")
    );
    let definitions = vec![definition];
    repo = Repository::open_with_options(
        &root,
        definitions.clone(),
        RepositoryOptions {
            ..Default::default()
        },
    )
    .unwrap();
    let mut query = Query::from_workload(&case(Some("SORT BY year DESC, title"), vec![])).unwrap();
    query.options.count = false;
    query.options.limit = 1;
    assert_eq!(
        repo.explain(&query).unwrap()[0].sort_strategy,
        "stored_composite_order_page"
    );
    let result = repo.search(&query).unwrap();
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["c"]
    );
    assert_eq!(result.hits, repo.search_reference(&query).unwrap().hits);
    assert!(result.counters.scanned_records < 5);
    let mut typed = query.clone();
    typed.options.sort[0] = SortField::computed(
        SortExpression {
            path: "year".into(),
            binding: None,
            reducer: None,
        },
        true,
    );
    typed.options.sort[1] = SortField::computed(
        SortExpression {
            path: "title".into(),
            binding: None,
            reducer: None,
        },
        false,
    );
    assert_eq!(
        repo.explain(&typed).unwrap()[0].sort_strategy,
        "stored_composite_order_page"
    );
    assert_eq!(repo.search(&typed).unwrap().hits, result.hits);
    drop(repo);
    let storage = IndexStoragePolicy {
        location: IndexStorageLocation::RepositoryDirectory,
    };
    let first = Index::open_with_storage(
        &root,
        definitions.clone(),
        RepositoryOptions::default(),
        storage.clone(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    assert_eq!(
        first.view().unwrap().search(&query).unwrap().hits,
        result.hits
    );
    first.stop_sync_storage().unwrap();
    let restored = Index::open_with_storage(
        &root,
        definitions,
        RepositoryOptions::default(),
        storage,
        SourceRegistry::builtin(),
    )
    .unwrap();
    assert!(restored.restored_from_storage());
    assert_eq!(
        restored.view().unwrap().search(&query).unwrap().hits,
        result.hits
    );
    restored.stop_sync_storage().unwrap();
    restored.wait_for_reconciliation().unwrap();
    drop(restored);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn schema_free_sort_reductions_positions_and_late_error() {
    let root = std::env::temp_dir().join(format!(
        "objekte-computed-sort-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, contents) in [
        (
            "a",
            r#"{"title":"Zulu","prices":[3,1],"contributors":[{"name":"Bea"},{"name":"Ada"}],"mixed":false}"#,
        ),
        (
            "b",
            r#"{"title":"Alpha","prices":[2,4],"contributors":[{"name":"Cal"}],"mixed":5}"#,
        ),
        (
            "c",
            r#"{"title":"Middle","prices":[],"contributors":[],"mixed":"x"}"#,
        ),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), contents).unwrap();
    }
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions {
            ..Default::default()
        },
    )
    .unwrap();
    for (expression, expected) in [
        (
            "SORT BY title",
            vec!["path:b.json", "path:c.json", "path:a.json"],
        ),
        (
            "SORT BY prices[0]",
            vec!["path:b.json", "path:a.json", "path:c.json"],
        ),
        (
            "SORT BY MIN(prices[*])",
            vec!["path:a.json", "path:b.json", "path:c.json"],
        ),
        (
            "SORT BY MAX(prices[*])",
            vec!["path:a.json", "path:b.json", "path:c.json"],
        ),
        (
            "SORT BY FIRST(contributors.name)",
            vec!["path:a.json", "path:b.json", "path:c.json"],
        ),
        (
            "SORT BY LAST(contributors.name)",
            vec!["path:a.json", "path:b.json", "path:c.json"],
        ),
        (
            "SORT BY mixed",
            vec!["path:a.json", "path:b.json", "path:c.json"],
        ),
        (
            "SORT BY MIN(prices[*]) DESC, title",
            vec!["path:b.json", "path:a.json", "path:c.json"],
        ),
    ] {
        let query = Query::from_workload(&case(Some(expression), vec![])).unwrap();
        let result = repo.search(&query).unwrap();
        assert_eq!(
            result
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>(),
            expected,
            "{expression}"
        );
        assert_eq!(
            result.hits,
            repo.search_reference(&query).unwrap().hits,
            "{expression}"
        );
        assert_eq!(result.counters.sort_key_evaluations, 3, "{expression}");
    }
    let mut invalid = Query::from_workload(&case(Some("SORT BY contributors"), vec![])).unwrap();
    invalid.options.count = false;
    invalid.options.limit = 1;
    assert!(
        repo.search(&invalid)
            .unwrap_err()
            .contains("multi-valued sort field")
    );
    std::fs::write(root.join("data/z.json"), r#"{"title":{"bad":"object"}}"#).unwrap();
    drop(repo);
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions {
            ..Default::default()
        },
    )
    .unwrap();
    let mut invalid = Query::from_workload(&case(Some("SORT BY title"), vec![])).unwrap();
    invalid.options.count = false;
    invalid.options.limit = 1;
    assert!(
        repo.search(&invalid)
            .unwrap_err()
            .contains("unsupported sort value")
    );
    drop(repo);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn computed_top_k_matches_an_independent_seeded_numeric_order() {
    let root = std::env::temp_dir().join(format!(
        "objekte-sort-topk-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for index in 0..113usize {
        std::fs::write(
            root.join(format!("data/{index:03}.json")),
            format!("{{\"score\":{}}}", (index * 37) % 113),
        )
        .unwrap();
    }
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions {
            ..Default::default()
        },
    )
    .unwrap();
    let mut expected: Vec<_> = (0..113usize).collect();
    expected.sort_by_key(|index| std::cmp::Reverse((index * 37) % 113));
    for (offset, limit) in [(0, 1), (2, 5), (37, 7), (108, 10), (0, 0)] {
        let mut request = Query::from_workload(&case(Some("SORT BY score DESC"), vec![])).unwrap();
        request.options.count = false;
        request.options.offset = offset;
        request.options.limit = limit;
        let result = repo.search(&request).unwrap();
        let ids: Vec<_> = result
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect();
        let oracle: Vec<_> = expected
            .iter()
            .skip(offset)
            .take(limit)
            .map(|index| format!("path:{index:03}.json"))
            .collect();
        assert_eq!(ids, oracle.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(result.counters.sort_key_evaluations, 113);
    }
    drop(repo);
    std::fs::remove_dir_all(root).unwrap();
}
