use super::*;
use crate::query::{Query, QueryOptions};
use crate::source::SourceFormat;
use crate::workload::ReturnMode;

#[test]
fn explicit_build_ignores_implicit_cache_even_when_legacy_mode_requires_it() {
    let root = std::env::temp_dir().join(format!(
        "objekte-explicit-build-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::create_dir_all(root.join(".objekte")).unwrap();
    std::fs::write(root.join("data/a.json"), b"{\"name\":\"A\"}").unwrap();
    let cache = root.join(".objekte/search-core-ascii.snapshot");
    std::fs::write(&cache, b"invalid cache contents").unwrap();
    let index = Index::build(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Required,
            ..Default::default()
        },
    )
    .unwrap();
    let query = Query {
        collections: vec!["data".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    assert_eq!(index.view().unwrap().search(&query).unwrap().count, Some(1));
    assert_eq!(std::fs::read(&cache).unwrap(), b"invalid cache contents");
    assert!(index.status().unwrap().dirty());
    std::fs::remove_dir_all(root).unwrap();
}
