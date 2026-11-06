use std::collections::HashMap;
use std::path::PathBuf;

use objekte_search_core::model::{FieldPath, LogicalType};
use objekte_search_core::query::Query;
use objekte_search_core::reference::{FieldDefinition, Schema};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::{ReturnMode, SortField, WorkloadCase};

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
            snapshot_mode: SnapshotMode::Disabled,
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
            },
            SortField {
                field: "title".into(),
                descending: false,
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

    let invalid = Query::from_workload(&case(Some("SORT BY absent"), vec![])).unwrap();
    assert!(
        repo.search(&invalid)
            .unwrap_err()
            .contains("sort field is not registered")
    );
    drop(repo);
    std::fs::remove_dir_all(root).unwrap();
}
