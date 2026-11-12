use objekte_search_core::lifecycle::Index;
use objekte_search_core::query::{Query, QueryOptions, parse_structured_filter};
use objekte_search_core::repository::{CollectionDefinition, RepositoryOptions};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::ReturnMode;

#[test]
fn staged_verified_membership_reranks_computed_sorts() {
    let root = std::env::temp_dir().join(format!(
        "objekte-staged-computed-sort-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, rank) in [("a", 3), ("b", 1), ("c", 2)] {
        std::fs::write(
            root.join(format!("data/{name}.json")),
            format!(r#"{{"score":1,"nested":{{"rank":{rank}}}}}"#),
        )
        .unwrap();
    }
    let index = Index::start_staged_build(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions::default(),
    )
    .unwrap()
    .wait_complete()
    .unwrap();
    index.configure_verified_cache(1024 * 1024).unwrap();
    let view = index.view().unwrap();
    let parsed = parse_structured_filter("score >= 0 SORT BY nested.rank ASC").unwrap();
    let query = Query {
        collections: vec!["data".into()],
        fts: None,
        filter: parsed.filter,
        options: QueryOptions {
            sort: parsed.sort.unwrap(),
            limit: 2,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let first = view.search(&query).unwrap();
    let second = view.search(&query).unwrap();
    assert_eq!(first.count, Some(3));
    assert_eq!(second.count, first.count);
    assert_eq!(second.hits, first.hits);
    assert!(second.counters.verified_result_hit);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
