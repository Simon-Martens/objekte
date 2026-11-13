use super::*;

use crate::index_storage::{IndexStorageLocation, IndexStoragePolicy};
use crate::inference::{InferenceConfig, InferenceMode};
use crate::query::{Query, QueryOptions};
use crate::source::SourceFormat;
use crate::workload::ReturnMode;

fn all_records() -> Query {
    Query {
        collections: vec!["items".into()],
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
fn memory_only_build_and_updates_publish_owned_sealed_segments() {
    let root = std::env::temp_dir().join(format!(
        "objekte-memory-sealed-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), br#"{"text":"alpha"}"#).unwrap();
    let definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
    let index = Index::open_with_storage(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        IndexStoragePolicy::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    let first = index.view().unwrap();
    assert!(first.0.segments[0].iter().all(|segment| {
        segment.index.raw.is_sealed()
            && segment.index.raw.normal.is_empty()
            && segment.index.raw.skip.is_empty()
    }));
    assert_eq!(first.search(&all_records()).unwrap().count, Some(1));
    drop(first);
    let added = root.join("data/b.json");
    std::fs::write(&added, br#"{"text":"beta"}"#).unwrap();
    index.add_file(&added).unwrap();
    let updated = index.view().unwrap();
    assert!(
        updated.0.segments[0]
            .iter()
            .all(|segment| segment.index.raw.is_sealed())
    );
    assert_eq!(updated.search(&all_records()).unwrap().count, Some(2));
    drop(updated);

    let task = Index::start_incremental_build_with_storage(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        IndexStoragePolicy::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    let complete = task.wait_complete().unwrap();
    let final_view = complete.view().unwrap();
    assert!(
        final_view.0.segments[0]
            .iter()
            .all(|segment| segment.index.raw.is_sealed())
    );
    assert_eq!(final_view.search(&all_records()).unwrap().count, Some(2));
    assert!(
        complete
            .storage_status()
            .unwrap()
            .managed_directory
            .is_none()
    );
    drop(final_view);
    drop(complete);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_open_reuses_complete_generation_without_source_build() {
    let root = std::env::temp_dir().join(format!(
        "objekte-managed-open-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), br#"{"text":"alpha"}"#).unwrap();
    let definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
    let storage = IndexStoragePolicy {
        location: IndexStorageLocation::RepositoryDirectory,
        residency: IndexResidency::OnDemand,
    };
    let options = RepositoryOptions {
        inference: InferenceConfig {
            mode: InferenceMode::AutoIndex,
            ..Default::default()
        },
        ..Default::default()
    };
    let first = Index::open_with_storage(
        root.clone(),
        vec![definition.clone()],
        options,
        storage.clone(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    assert!(first.view().unwrap().0.segments[0]
        .iter()
        .all(|segment| segment.index.raw.is_sealed()));
    assert_eq!(
        first.view().unwrap().search(&all_records()).unwrap().count,
        Some(1)
    );
    let first_status = first.storage_status().unwrap();
    assert_eq!(first_status.current_generation, 1);
    assert_eq!(first_status.durable_generation, Some(1));
    assert_eq!(
        first_status.synchronization,
        IndexSynchronization::Synchronized
    );
    let expected_diagnostics = serde_json::to_value(
        first
            .view()
            .unwrap()
            .repository()
            .inference_index_diagnostics("items"),
    )
    .unwrap();
    assert_ne!(expected_diagnostics, serde_json::Value::Null);
    assert!(
        first_status
            .managed_directory
            .unwrap()
            .join("manifest.bin")
            .exists()
    );
    first.stop_sync_storage().unwrap();
    drop(first);

    let reopened = Index::open_with_storage(
        root.clone(),
        vec![definition],
        options,
        storage,
        SourceRegistry::builtin(),
    )
    .unwrap();
    assert_eq!(reopened.status().unwrap().build_source_reads, 0);
    let restored_view = reopened.view().unwrap();
    let restored_segment = &restored_view.0.segments[0][0].index;
    assert!(restored_segment.raw.normal.is_empty());
    assert!(restored_segment.raw.skip.is_empty());
    assert!(restored_segment.inference.as_ref().is_none_or(|inference| {
        inference.indexes.values().all(|posting| {
            posting.sealed.is_some() && posting.values.is_empty() && posting.lengths.is_empty()
        })
    }));
    assert_eq!(
        serde_json::to_value(
            reopened
                .view()
                .unwrap()
                .repository()
                .inference_index_diagnostics("items")
        )
        .unwrap(),
        expected_diagnostics
    );
    assert_eq!(
        reopened
            .view()
            .unwrap()
            .search(&all_records())
            .unwrap()
            .count,
        Some(1)
    );
    assert_eq!(
        reopened.storage_status().unwrap().durable_generation,
        Some(1)
    );
    reopened.stop_sync_storage().unwrap();
    reopened.wait_for_reconciliation().unwrap();
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn progressive_managed_build_flushes_only_complete_generation() {
    let root = std::env::temp_dir().join(format!(
        "objekte-managed-progressive-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..8 {
        std::fs::write(
            root.join(format!("data/{number}.json")),
            format!("{{\"text\":\"item {number}\"}}"),
        )
        .unwrap();
    }
    let definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
    let policy = IndexStoragePolicy {
        location: IndexStorageLocation::RepositoryDirectory,
        residency: IndexResidency::OnDemand,
    };
    let task = Index::start_incremental_build_with_storage(
        root.clone(),
        vec![definition.clone()],
        RepositoryOptions::default(),
        policy.clone(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    let catalogue = task.wait_catalogue().unwrap();
    assert!(catalogue.status().unwrap().indexed_files >= 8);
    let complete = task.wait_complete().unwrap();
    assert!(complete.view().unwrap().0.segments[0]
        .iter()
        .all(|segment| segment.index.raw.is_sealed()));
    assert_eq!(
        complete.storage_status().unwrap().synchronization,
        IndexSynchronization::Synchronized
    );
    assert_eq!(
        complete
            .view()
            .unwrap()
            .search(&all_records())
            .unwrap()
            .count,
        Some(8)
    );
    complete.stop_sync_storage().unwrap();
    drop(complete);
    let reopened = Index::start_incremental_build_with_storage(
        root.clone(),
        vec![definition],
        RepositoryOptions::default(),
        policy,
        SourceRegistry::builtin(),
    )
    .unwrap()
    .wait_complete()
    .unwrap();
    assert_eq!(reopened.status().unwrap().build_source_reads, 0);
    assert_eq!(
        reopened
            .view()
            .unwrap()
            .search(&all_records())
            .unwrap()
            .count,
        Some(8)
    );
    reopened.stop_sync_storage().unwrap();
    reopened.wait_for_reconciliation().unwrap();
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_generation_reopens_add_move_and_retirement() {
    let root = std::env::temp_dir().join(format!(
        "objekte-managed-updates-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let initial = root.join("data/a.json");
    std::fs::write(&initial, br#"{"text":"alpha"}"#).unwrap();
    let definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
    let storage = IndexStoragePolicy {
        location: IndexStorageLocation::RepositoryDirectory,
        residency: IndexResidency::OnDemand,
    };
    let index = Index::open_with_storage(
        root.clone(),
        vec![definition.clone()],
        RepositoryOptions::default(),
        storage.clone(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    let prepared = storage
        .prepare(
            &root,
            &[definition.clone()],
            RepositoryOptions::default(),
            &SourceRegistry::builtin(),
        )
        .unwrap();
    let store = ManagedStore::open(&prepared.resolved).unwrap();
    let initial_names = store.read_generation().unwrap().unwrap().segment_names;
    let added = root.join("data/b.json");
    std::fs::write(&added, br#"{"text":"beta"}"#).unwrap();
    index.add_file(&added).unwrap();
    assert!(index.view().unwrap().0.segments[0]
        .iter()
        .all(|segment| segment.index.raw.is_sealed()));
    index.flush_storage().unwrap();
    let after_add = store.read_generation().unwrap().unwrap().segment_names;
    assert!(initial_names.iter().all(|name| after_add.contains(name)));
    assert_eq!(
        index.view().unwrap().search(&all_records()).unwrap().count,
        Some(2)
    );
    let moved = root.join("data/c.json");
    let handle = index.view().unwrap().file_handle_for_path(&added).unwrap();
    std::fs::rename(&added, &moved).unwrap();
    index.move_file(handle, &moved).unwrap();
    index.flush_storage().unwrap();
    let after_move = store.read_generation().unwrap().unwrap().segment_names;
    assert!(initial_names.iter().all(|name| after_move.contains(name)));
    assert!(index.view().unwrap().file_handle_for_path(&added).is_none());
    assert!(index.view().unwrap().file_handle_for_path(&moved).is_some());
    let initial_handle = index
        .view()
        .unwrap()
        .file_handle_for_path(&initial)
        .unwrap();
    std::fs::remove_file(&initial).unwrap();
    index.delete_file(initial_handle).unwrap();
    index.flush_storage().unwrap();
    assert_eq!(
        index.view().unwrap().search(&all_records()).unwrap().count,
        Some(1)
    );
    index.stop_sync_storage().unwrap();
    drop(index);

    let reopened = Index::open_with_storage(
        root.clone(),
        vec![definition],
        RepositoryOptions::default(),
        storage,
        SourceRegistry::builtin(),
    )
    .unwrap();
    assert_eq!(
        reopened
            .view()
            .unwrap()
            .search(&all_records())
            .unwrap()
            .count,
        Some(1)
    );
    assert!(
        reopened
            .view()
            .unwrap()
            .file_handle_for_path(&initial)
            .is_none()
    );
    assert!(
        reopened
            .view()
            .unwrap()
            .file_handle_for_path(&added)
            .is_none()
    );
    assert!(
        reopened
            .view()
            .unwrap()
            .file_handle_for_path(&moved)
            .is_some()
    );
    reopened.stop_sync_storage().unwrap();
    reopened.wait_for_reconciliation().unwrap();
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_worker_synchronizes_completed_mutations() {
    let root = std::env::temp_dir().join(format!(
        "objekte-managed-worker-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), br#"{"text":"alpha"}"#).unwrap();
    let index = Index::open_with_storage(
        root.clone(),
        vec![CollectionDefinition::folder(
            "items",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions::default(),
        IndexStoragePolicy {
            location: IndexStorageLocation::RepositoryDirectory,
            residency: IndexResidency::OnDemand,
        },
        SourceRegistry::builtin(),
    )
    .unwrap();
    let added = root.join("data/b.json");
    std::fs::write(&added, br#"{"text":"beta"}"#).unwrap();
    index.add_file(&added).unwrap();
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let status = index.storage_status().unwrap();
        if status.durable_generation == Some(2) {
            assert_eq!(status.synchronization, IndexSynchronization::Synchronized);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "managed writer did not synchronize"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    index.stop_sync_storage().unwrap();
    drop(index);
    std::fs::remove_dir_all(root).unwrap();
}
