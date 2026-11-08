use super::*;
use crate::model::CollectionId;
use crate::query::{Query, QueryOptions};
use crate::repository::{CollectionDefinition, RepositoryOptions};
use crate::source::SourceFormat;
use crate::workload::ReturnMode;

fn all() -> Query {
    Query {
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
    }
}

#[test]
fn compact_reclaims_base_and_segment_tombstones_without_source_files() {
    let root = std::env::temp_dir().join(format!(
        "objekte-compact-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let a = root.join("data/a.json");
    let b = root.join("data/b.json");
    std::fs::write(&a, br#"{"name":"A"}"#).unwrap();
    std::fs::write(&b, br#"{"name":"B"}"#).unwrap();
    let index = Index::build(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions::default(),
    )
    .unwrap();
    let initial = index.view().unwrap().revision();
    let no_op = index.compact().unwrap();
    assert_eq!(no_op.revision, initial);
    assert_eq!(no_op.retired_slots_removed, 0);
    let old = index.view().unwrap();
    let a_handle = old.file_handle_for_path(&a).unwrap();
    let b_handle = old.file_handle_for_path(&b).unwrap();
    index.delete_file(a_handle).unwrap();
    std::fs::write(&b, br#"{"name":"B2"}"#).unwrap();
    index.update_file(b_handle, &b).unwrap();
    std::fs::remove_file(&a).unwrap();
    std::fs::remove_file(&b).unwrap();
    let before = index.view().unwrap().search(&all()).unwrap();
    assert_eq!(before.count, Some(1));
    let revision_before = index.view().unwrap().revision();
    let checks = std::cell::Cell::new(0);
    let cancelled = index.compact_with_control(
        || {
            checks.set(checks.get() + 1);
            checks.get() >= 4
        },
        |_, _| {},
    );
    assert_eq!(cancelled.unwrap_err(), IndexError::Cancelled);
    assert_eq!(index.view().unwrap().revision(), revision_before);
    let report = index.compact().unwrap();
    assert_eq!(report.slots_before, 3);
    assert_eq!(report.slots_after, 1);
    assert_eq!(report.retired_slots_removed, 2);
    let after = index.view().unwrap().search(&all()).unwrap();
    assert_eq!(after.count, before.count);
    assert_eq!(after.hits.len(), before.hits.len());
    assert_eq!(after.hits[0].record_id, before.hits[0].record_id);
    assert_eq!(after.hits[0].file_path, before.hits[0].file_path);
    assert_eq!(after.hits[0].generation, report.revision.0);
    assert_eq!(old.search(&all()).unwrap().count, Some(2));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn compacted_base_contributions_round_trip_through_cache() {
    let root = std::env::temp_dir().join(format!(
        "objekte-compact-cache-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let a = root.join("data/a.json");
    let b = root.join("data/b.json");
    std::fs::write(&a, br#"{"name":"A"}"#).unwrap();
    std::fs::write(&b, br#"{"name":"B"}"#).unwrap();
    let definitions = vec![CollectionDefinition::folder(
        "data",
        "data",
        SourceFormat::Json,
    )];
    let index = Index::build(&root, definitions.clone(), RepositoryOptions::default()).unwrap();
    let handle = index.view().unwrap().file_handle_for_path(&a).unwrap();
    index.delete_file(handle).unwrap();
    std::fs::remove_file(&a).unwrap();
    assert_eq!(index.compact().unwrap().retired_slots_removed, 1);
    let cache = root.join("index.snapshot");
    index.flush_cache(&cache).unwrap();
    let restored =
        Index::load_cache(&root, definitions, RepositoryOptions::default(), &cache).unwrap();
    assert_eq!(
        restored.view().unwrap().search(&all()).unwrap().count,
        Some(1)
    );
    let scan = restored.wait_for_reconciliation().unwrap().unwrap();
    assert_eq!(scan.added + scan.updated + scan.removed, 0);
    restored.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn background_compaction_preserves_pinned_reader_and_reports_completion() {
    let root = std::env::temp_dir().join(format!(
        "objekte-background-compact-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let file = root.join("data/a.json");
    std::fs::write(&file, br#"{"name":"A"}"#).unwrap();
    let index = Index::build(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions::default(),
    )
    .unwrap();
    let old = index.view().unwrap();
    index
        .delete_file(old.file_handle_for_path(&file).unwrap())
        .unwrap();
    let task = index.start_compaction().unwrap();
    assert_eq!(
        task.progress().unwrap().operation,
        crate::lifecycle::MaintenanceOperation::Compact
    );
    let crate::lifecycle::MaintenanceReport::Compact(report) = task.wait().unwrap() else {
        panic!("wrong operation")
    };
    assert_eq!(report.retired_slots_removed, 1);
    assert!(task.is_complete().unwrap());
    assert_eq!(
        task.progress().unwrap().phase,
        crate::lifecycle::MaintenancePhase::Complete
    );
    assert_eq!(old.search(&all()).unwrap().count, Some(1));
    assert_eq!(index.view().unwrap().search(&all()).unwrap().count, Some(0));
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn segment_admission_keeps_large_partially_retired_shard_when_small_pairs_fit() {
    let root = std::env::temp_dir().join(format!(
        "objekte-segment-admission-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for name in ["a", "b", "c"] {
        std::fs::write(
            root.join(format!("data/{name}.json")),
            format!("{{\"name\":\"{name}\"}}"),
        )
        .unwrap();
    }
    let index = Index::build(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions::default(),
    )
    .unwrap();
    let a = root.join("data/a.json");
    let b = root.join("data/b.json");
    let initial = index.view().unwrap();
    index
        .delete_file(initial.file_handle_for_path(&a).unwrap())
        .unwrap();
    index.compact().unwrap();
    let b_handle = index.view().unwrap().file_handle_for_path(&b).unwrap();
    std::fs::write(&b, br#"{"name":"bb"}"#).unwrap();
    index.update_file(b_handle, &b).unwrap();
    let dense = index.view().unwrap().segments(CollectionId(0)).unwrap()[0].clone();
    assert!(dense.active.len() < dense.index.records.len() as u64);
    for number in 0..70 {
        let path = root.join(format!("data/new-{number}.json"));
        std::fs::write(&path, format!("{{\"name\":\"new-{number}\"}}")).unwrap();
        index.add_file(&path).unwrap();
    }
    let current = index.view().unwrap();
    assert!(
        current
            .segments(CollectionId(0))
            .unwrap()
            .iter()
            .any(|segment| Arc::ptr_eq(segment, &dense))
    );
    assert_eq!(current.search(&all()).unwrap().count, Some(72));
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
