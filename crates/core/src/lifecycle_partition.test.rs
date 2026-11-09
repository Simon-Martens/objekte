use super::{compact_source_parts_with_limits, partition_base_with_limits};
use crate::lifecycle::Index;
use crate::query::{Query, QueryOptions};
use crate::repository::{CollectionDefinition, RepositoryOptions};
use crate::source::SourceFormat;
use crate::workload::ReturnMode;

#[test]
fn partitions_static_base_without_changing_search_or_file_routing() {
    let root = std::env::temp_dir().join(format!(
        "objekte-partition-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..7 {
        std::fs::write(
            root.join(format!("data/{number}.jsonl")),
            format!("{{\"name\":\"item-{number}\"}}\n"),
        )
        .unwrap();
    }
    let original = Index::build(
        &root,
        vec![CollectionDefinition::folder(
            "items",
            "data",
            SourceFormat::JsonLines,
        )],
        RepositoryOptions::default(),
    )
    .unwrap();
    let query = Query {
        collections: vec!["items".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 100,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let before = original.view().unwrap().search(&query).unwrap();
    let published = (*original.view().unwrap().0).clone();
    let partitioned = partition_base_with_limits(published, 2, 2).unwrap();
    assert_eq!(
        partitioned.repository.collections()[0]
            .generation
            .records
            .len(),
        0
    );
    assert!(partitioned.segments[0].len() >= 4);
    assert!(
        partitioned.segments[0]
            .iter()
            .all(|part| part.index.records.len() <= 2)
    );
    let converted = Index::from_published(partitioned);
    let after = converted.view().unwrap().search(&query).unwrap();
    assert_eq!(before.count, after.count);
    let paths = |result: crate::repository::SearchResult| {
        result
            .hits
            .into_iter()
            .map(|hit| hit.file_path)
            .collect::<Vec<_>>()
    };
    assert_eq!(paths(before), paths(after));
    for number in 0..7 {
        assert!(
            converted
                .view()
                .unwrap()
                .file_handle_for_path(std::path::Path::new(&format!("data/{number}.jsonl")))
                .is_some()
        );
    }
    converted.shutdown().unwrap();
    original.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn compacts_one_large_file_in_source_free_parts() {
    let root = std::env::temp_dir().join(format!(
        "objekte-partition-large-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root.join("data/many.jsonl");
    let bytes = (0..9)
        .map(|number| format!("{{\"name\":\"item-{number}\"}}\n"))
        .collect::<String>();
    std::fs::write(&path, bytes).unwrap();
    let index = Index::build(
        &root,
        vec![CollectionDefinition::folder(
            "items",
            "data",
            SourceFormat::JsonLines,
        )],
        RepositoryOptions::default(),
    )
    .unwrap();
    let view = index.view().unwrap();
    let collection = &view.repository().collections()[0];
    let source =
        crate::lifecycle_compaction::base_as_segment(collection, view.repository().files())
            .unwrap();
    std::fs::remove_file(&path).unwrap();
    let parts =
        compact_source_parts_with_limits(&source, &view.0.active[0], &view.0.catalog, 2, 2, || {
            false
        })
        .unwrap();
    assert_eq!(parts.len(), 5);
    assert!(parts.iter().all(|part| part.records.len() <= 2));
    assert_eq!(
        parts.iter().map(|part| part.records.len()).sum::<usize>(),
        9
    );
    assert!(parts.iter().all(|part| part.files.len() == 1));
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
