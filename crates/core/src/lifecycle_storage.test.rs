use super::*;

use crate::index_storage::{IndexLocationResolver, IndexStorageFailureKind, IndexStorageLocation};
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
fn explicit_load_reports_absence_without_creating_a_store() {
    let root = std::env::temp_dir().join(format!(
        "objekte-explicit-absence-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), br#"{"text":"alpha"}"#).unwrap();
    let definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
    let loaded = Index::try_load(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        &IndexStorageTarget::RepositoryDirectory,
    )
    .unwrap();
    assert!(loaded.is_none());
    assert!(!root.join(".objekte").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_load_reuses_generation_without_writing_or_rebuilding() {
    let root = std::env::temp_dir().join(format!(
        "objekte-explicit-readonly-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), br#"{"text":"alpha"}"#).unwrap();
    let definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
    let policy = IndexStorageTarget::RepositoryDirectory.policy();
    let built = Index::open_with_storage(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        policy.clone(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    built.stop_sync_storage().unwrap();
    drop(built);
    let directory = policy
        .prepare(
            &root,
            &[definition.clone()],
            RepositoryOptions::default(),
            &SourceRegistry::builtin(),
        )
        .unwrap()
        .resolved
        .managed_directory
        .unwrap();
    let manifest = directory.join("manifest.toml");
    let original = std::fs::read(&manifest).unwrap();
    #[cfg(unix)]
    fn make_read_only(path: &std::path::Path, readonly: bool) {
        use std::os::unix::fs::PermissionsExt;
        if path.is_dir() {
            for entry in std::fs::read_dir(path).unwrap() {
                make_read_only(&entry.unwrap().path(), readonly);
            }
        }
        let mode = if path.is_dir() {
            if readonly { 0o500 } else { 0o700 }
        } else if readonly {
            0o400
        } else {
            0o600
        };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    #[cfg(unix)]
    make_read_only(&directory, true);
    let loaded = Index::try_load(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        &IndexStorageTarget::RepositoryDirectory,
    )
    .unwrap()
    .unwrap();
    assert!(loaded.restored_from_storage());
    assert_eq!(loaded.status().unwrap().build_parses, 0);
    assert_eq!(
        loaded.view().unwrap().search(&all_records()).unwrap().count,
        Some(1)
    );
    assert_eq!(std::fs::read(&manifest).unwrap(), original);
    loaded.shutdown().unwrap();
    #[cfg(unix)]
    make_read_only(&directory, false);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_flush_replaces_existing_generation_with_fresh_equal_revision() {
    let root = std::env::temp_dir().join(format!(
        "objekte-explicit-replace-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/old.json"), br#"{"text":"old"}"#).unwrap();
    let definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
    let target = IndexStorageTarget::RepositoryDirectory;
    let first = Index::build(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
    )
    .unwrap();
    let saved = first.flush_to(&target).unwrap();
    assert_eq!(saved.store_generation, 1);
    assert_eq!(saved.saved_revision, first.status().unwrap().revision);
    assert!(!saved.unchanged);
    assert_eq!(first.flush_to(&target).unwrap().store_generation, 1);
    assert!(first.flush_to(&target).unwrap().unchanged);
    let loaded = Index::try_load(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        &target,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        loaded.view().unwrap().search(&all_records()).unwrap().count,
        Some(1)
    );
    drop(loaded);
    std::fs::remove_file(root.join("data/old.json")).unwrap();
    std::fs::write(root.join("data/new.json"), br#"{"text":"new"}"#).unwrap();
    let second = Index::build(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
    )
    .unwrap();
    assert_eq!(
        second.status().unwrap().revision,
        first.status().unwrap().revision
    );
    let replaced = second.flush_to(&target).unwrap();
    assert_eq!(replaced.store_generation, 2);
    assert!(!replaced.unchanged);
    let current = Index::try_load(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        &target,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        current
            .view()
            .unwrap()
            .search(&all_records())
            .unwrap()
            .count,
        Some(1)
    );
    let paths = current
        .view()
        .unwrap()
        .catalogue_rows()
        .map(|row| row.relative_path.to_path_buf())
        .collect::<Vec<_>>();
    assert_eq!(paths, vec![std::path::PathBuf::from("data/new.json")]);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_load_without_catalog_reports_missing_catalog() {
    let root = std::env::temp_dir().join(format!(
        "objekte-managed-no-catalog-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), br#"{"text":"alpha"}"#).unwrap();
    let definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
    let storage = IndexLocationResolver {
        location: IndexStorageLocation::RepositoryDirectory,
    };
    let prepared = storage
        .prepare(
            &root,
            std::slice::from_ref(&definition),
            RepositoryOptions::default(),
            &SourceRegistry::builtin(),
        )
        .unwrap();
    let store = ManagedStore::open(&prepared.resolved).unwrap();
    store.commit_generation(None, 1, &[]).unwrap();
    let result = Index::try_load(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        &IndexStorageTarget::RepositoryDirectory,
    );
    assert!(matches!(result, Err(IndexError::InvalidCache(ref message)) if message.contains("catalog missing")));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_flush_to_second_target_keeps_loaded_backing_and_copies_all_data() {
    let root = std::env::temp_dir().join(format!(
        "objekte-explicit-copy-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::create_dir(root.join("source-store")).unwrap();
    std::fs::create_dir(root.join("destination-store")).unwrap();
    std::fs::write(root.join("data/a.json"), br#"{"text":"alpha"}"#).unwrap();
    let definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
    let source = IndexStorageTarget::CustomDirectory(root.join("source-store"));
    let destination = IndexStorageTarget::CustomDirectory(root.join("destination-store"));
    let built = Index::build(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
    )
    .unwrap();
    built.flush_to(&source).unwrap();
    let loaded = Index::try_load(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        &source,
    )
    .unwrap()
    .unwrap();
    let before = loaded.view().unwrap().search(&all_records()).unwrap();
    let report = loaded.flush_to(&destination).unwrap();
    assert!(!report.unchanged);
    assert!(loaded.restored_from_storage());
    assert_eq!(
        loaded.view().unwrap().search(&all_records()).unwrap().count,
        before.count
    );
    std::fs::remove_dir_all(root.join("source-store")).unwrap();
    let copied = Index::try_load(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        &destination,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        copied.view().unwrap().search(&all_records()).unwrap().count,
        Some(1)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn memory_build_and_repository_load_share_identity_when_source_is_root() {
    let root = std::env::temp_dir().join(format!(
        "objekte-explicit-root-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.json"), br#"{"text":"alpha"}"#).unwrap();
    let definition = CollectionDefinition::folder("items", "", SourceFormat::Json);
    let target = IndexStorageTarget::RepositoryDirectory;
    let index = Index::build(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
    )
    .unwrap();
    let first_count = index.view().unwrap().search(&all_records()).unwrap().count;
    index.flush_to(&target).unwrap();
    let rebuilt = Index::build(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
    )
    .unwrap();
    assert_eq!(
        rebuilt
            .view()
            .unwrap()
            .search(&all_records())
            .unwrap()
            .count,
        first_count
    );
    let loaded = Index::try_load(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        &target,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        loaded.view().unwrap().search(&all_records()).unwrap().count,
        first_count
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn stale_explicit_loaded_writer_cannot_replace_newer_generation() {
    let root = std::env::temp_dir().join(format!(
        "objekte-explicit-conflict-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), br#"{"text":"alpha"}"#).unwrap();
    let definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
    let target = IndexStorageTarget::RepositoryDirectory;
    let built = Index::build(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
    )
    .unwrap();
    built.flush_to(&target).unwrap();
    let first = Index::try_load(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        &target,
    )
    .unwrap()
    .unwrap();
    let stale = Index::try_load(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        &target,
    )
    .unwrap()
    .unwrap();
    first.wait_for_reconciliation().unwrap();
    stale.wait_for_reconciliation().unwrap();
    let b = root.join("data/b.json");
    std::fs::write(&b, br#"{"text":"beta"}"#).unwrap();
    first.add_file(&b).unwrap();
    let committed = first.flush_to(&target).unwrap();
    assert_eq!(committed.store_generation, 2);
    assert!(matches!(
        built.flush_to(&target),
        Err(IndexError::StoreConflict(_))
    ));
    let c = root.join("data/c.json");
    std::fs::write(&c, br#"{"text":"gamma"}"#).unwrap();
    stale.add_file(&c).unwrap();
    assert!(matches!(
        stale.flush_to(&target),
        Err(IndexError::StoreConflict(_))
    ));
    assert_eq!(
        stale.view().unwrap().search(&all_records()).unwrap().count,
        Some(2)
    );
    let current = Index::try_load(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        &target,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        current.storage_status().unwrap().durable_generation,
        Some(2)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_open_rejects_obsolete_program_version_without_modifying_it() {
    let root = std::env::temp_dir().join(format!(
        "objekte-managed-obsolete-reject-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), br#"{"text":"alpha"}"#).unwrap();
    let definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
    let storage = IndexLocationResolver {
        location: IndexStorageLocation::RepositoryDirectory,
    };
    let index = Index::open_with_storage(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        storage.clone(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    index.stop_sync_storage().unwrap();
    drop(index);
    let prepared = storage
        .prepare(
            &root,
            &[definition.clone()],
            RepositoryOptions::default(),
            &SourceRegistry::builtin(),
        )
        .unwrap();
    let manifest = prepared
        .resolved
        .managed_directory
        .unwrap()
        .join("manifest.toml");
    let original = std::fs::read_to_string(&manifest).unwrap();
    let obsolete = original.replace(
        &format!("version = {}", crate::PROGRAM_VERSION),
        &format!("version = {}", crate::PROGRAM_VERSION + 1),
    );
    assert_ne!(obsolete, original);
    std::fs::write(&manifest, &obsolete).unwrap();
    let result = Index::open_with_storage(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        storage,
        SourceRegistry::builtin(),
    );
    assert!(result.is_err());
    assert_eq!(std::fs::read_to_string(&manifest).unwrap(), obsolete);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_flush_keeps_old_durable_generation_and_reports_io() {
    let root = std::env::temp_dir().join(format!(
        "objekte-managed-failed-flush-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), br#"{"text":"alpha"}"#).unwrap();
    let definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
    let storage = IndexLocationResolver {
        location: IndexStorageLocation::RepositoryDirectory,
    };
    let index = Index::open_with_storage(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        storage.clone(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    index.stop_sync_storage().unwrap();
    let initial = index.storage_status().unwrap().durable_generation;
    let added = root.join("data/b.json");
    std::fs::write(&added, br#"{"text":"beta"}"#).unwrap();
    index.add_file(&added).unwrap();
    let prepared = storage
        .prepare(
            &root,
            &[definition],
            RepositoryOptions::default(),
            &SourceRegistry::builtin(),
        )
        .unwrap();
    let lock = prepared
        .resolved
        .managed_directory
        .unwrap()
        .join("store.lock");
    std::fs::remove_file(&lock).unwrap();
    std::fs::create_dir(&lock).unwrap();
    assert!(matches!(index.flush_storage(), Err(IndexError::Io(_))));
    let failed = index.storage_status().unwrap();
    assert_eq!(failed.durable_generation, initial);
    assert_eq!(failed.synchronization, IndexSynchronization::Failed);
    assert!(failed.last_error.is_some_and(|message| !message.is_empty()));
    assert_eq!(
        failed.last_error_kind,
        Some(IndexStorageFailureKind::Access)
    );
    assert_eq!(
        index.view().unwrap().search(&all_records()).unwrap().count,
        Some(2)
    );
    std::fs::remove_dir(&lock).unwrap();
    index.flush_storage().unwrap();
    let recovered = index.storage_status().unwrap();
    assert_eq!(
        recovered.synchronization,
        IndexSynchronization::Synchronized
    );
    assert_eq!(recovered.last_error, None);
    assert_eq!(recovered.last_error_kind, None);
    drop(index);
    std::fs::remove_dir_all(root).unwrap();
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
        IndexLocationResolver::default(),
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
        IndexLocationResolver::default(),
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
    let storage = IndexLocationResolver {
        location: IndexStorageLocation::RepositoryDirectory,
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
    assert!(
        first.view().unwrap().0.segments[0]
            .iter()
            .all(|segment| segment.index.raw.is_sealed())
    );
    assert_eq!(
        first.view().unwrap().search(&all_records()).unwrap().count,
        Some(1)
    );
    let first_status = first.storage_status().unwrap();
    assert_eq!(first_status.current_revision, IndexRevision(1));
    assert_eq!(first_status.durable_generation, Some(1));
    assert_eq!(first_status.segments_full_validated, 0);
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
            .join("manifest.toml")
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
    assert!(reopened.storage_status().unwrap().segments_write_validated > 0);
    assert_eq!(
        reopened.storage_status().unwrap().segments_full_validated,
        0
    );
    reopened.stop_sync_storage().unwrap();
    reopened.wait_for_reconciliation().unwrap();
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn repeated_replacements_rebase_unreferenced_historical_metadata() {
    let root = std::env::temp_dir().join(format!(
        "objekte-managed-history-rebase-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let source = root.join("data/a.json");
    std::fs::write(&source, br#"{"text":"first"}"#).unwrap();
    let definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
    let policy = IndexLocationResolver {
        location: IndexStorageLocation::RepositoryDirectory,
    };
    let index = Index::open_with_storage(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        policy.clone(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    index.stop_sync_storage().unwrap();
    let resolved = policy
        .prepare(
            &root,
            &[definition.clone()],
            RepositoryOptions::default(),
            &SourceRegistry::builtin(),
        )
        .unwrap()
        .resolved;
    let store = ManagedStore::open(&resolved).unwrap();
    let mut history_sizes = Vec::new();
    for revision in 0..70 {
        std::fs::write(&source, format!(r#"{{"text":"value {revision}"}}"#)).unwrap();
        let handle = index.view().unwrap().file_handle_for_path(&source).unwrap();
        index.update_file(handle, &source).unwrap();
        index.flush_storage().unwrap();
        let generation = store.read_generation().unwrap().unwrap();
        let catalog = RestoredGenerationCatalog::decode(
            generation.catalog.as_deref().unwrap(),
            resolved.configuration_identity,
            1,
        )
        .unwrap();
        history_sizes.push(catalog.historical_entries().len());
    }
    let opened = store.read_generation().unwrap().unwrap();
    let catalog = RestoredGenerationCatalog::decode(
        opened.catalog.as_deref().unwrap(),
        resolved.configuration_identity,
        1,
    )
    .unwrap();
    assert!(
        history_sizes
            .windows(2)
            .any(|pair| pair[0] > 64 && pair[1] < pair[0])
    );
    assert!(catalog.historical_entries().len() <= 64);
    assert_eq!(
        index.view().unwrap().search(&all_records()).unwrap().count,
        Some(1)
    );
    drop(opened);
    drop(index);
    let reopened = Index::open_with_storage(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        policy,
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
    let policy = IndexLocationResolver {
        location: IndexStorageLocation::RepositoryDirectory,
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
    assert!(
        complete.view().unwrap().0.segments[0]
            .iter()
            .all(|segment| segment.index.raw.is_sealed())
    );
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
    let storage = IndexLocationResolver {
        location: IndexStorageLocation::RepositoryDirectory,
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
    let opened = store.read_generation().unwrap().unwrap();
    let initial_names = opened
        .segments
        .iter()
        .map(|segment| segment.name.clone())
        .collect::<Vec<_>>();
    let added = root.join("data/b.json");
    std::fs::write(&added, br#"{"text":"beta"}"#).unwrap();
    index.add_file(&added).unwrap();
    assert!(
        index.view().unwrap().0.segments[0]
            .iter()
            .all(|segment| segment.index.raw.is_sealed())
    );
    index.flush_storage().unwrap();
    let opened = store.read_generation().unwrap().unwrap();
    let after_add = opened
        .segments
        .iter()
        .map(|segment| segment.name.clone())
        .collect::<Vec<_>>();
    assert!(initial_names.iter().all(|name| !after_add.contains(name)));
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        for name in &after_add {
            let path = opened.generation_directory.join("segments").join(name);
            assert_eq!(std::fs::metadata(path).unwrap().nlink(), 1);
        }
    }
    assert_eq!(
        index.view().unwrap().search(&all_records()).unwrap().count,
        Some(2)
    );
    let moved = root.join("data/c.json");
    let handle = index.view().unwrap().file_handle_for_path(&added).unwrap();
    std::fs::rename(&added, &moved).unwrap();
    index.move_file(handle, &moved).unwrap();
    index.flush_storage().unwrap();
    let after_move = store
        .read_generation()
        .unwrap()
        .unwrap()
        .segments
        .iter()
        .map(|segment| segment.name.clone())
        .collect::<Vec<_>>();
    assert!(after_add.iter().all(|name| !after_move.contains(name)));
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
        IndexLocationResolver {
            location: IndexStorageLocation::RepositoryDirectory,
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
