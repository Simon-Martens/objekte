use super::*;

use crate::index_storage::{IndexStorageLocation, IndexStoragePolicy};
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
    let first = Index::open_with_storage(
        root.clone(),
        vec![definition.clone()],
        RepositoryOptions::default(),
        storage.clone(),
        SourceRegistry::builtin(),
    )
    .unwrap();
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
        RepositoryOptions::default(),
        storage,
        SourceRegistry::builtin(),
    )
    .unwrap();
    assert_eq!(reopened.status().unwrap().build_source_reads, 0);
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
        vec![CollectionDefinition::folder("items", "data", SourceFormat::Json)],
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
        assert!(Instant::now() < deadline, "managed writer did not synchronize");
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    index.stop_sync_storage().unwrap();
    drop(index);
    std::fs::remove_dir_all(root).unwrap();
}
