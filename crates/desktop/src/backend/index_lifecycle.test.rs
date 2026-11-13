use super::*;
use crate::backend::test_support::fixture;
use crate::backend::{OpenSpec, open};
use objekte_cli::arguments::Arguments;
use objekte_search_core::inference::InferenceMode;
use objekte_search_core::index_storage::{IndexResidency, IndexStorageLocation, IndexStoragePolicy};
use objekte_search_core::workload::BigramAlphabet;

#[test]
fn manual_shutdown_does_not_write_an_index() {
    let fixture = fixture();
    let root = fixture._root.path();
    let mut before = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    before.sort();
    assert!(fixture.session.configuration.cache_save.is_none());
    shutdown(&fixture.session).unwrap();
    let mut after = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    after.sort();
    assert_eq!(before, after);
    assert_eq!(status(&fixture.session).unwrap().saved_revision, None);
}

#[test]
fn managed_close_flushes_latest_revision_and_cancel_resumes_sync() {
    let root = tempfile::tempdir().unwrap();
    crate::backend::test_support::write_records(root.path());
    let session = open(
        &OpenSpec {
            storage_policy: Some(IndexStoragePolicy {
                location: IndexStorageLocation::RepositoryDirectory,
                residency: IndexResidency::OnDemand,
            }),
            args: Arguments {
                path: vec![root.path().to_path_buf()],
                ..Default::default()
            },
            cwd: root.path().to_path_buf(),
        },
        &CancellationToken::new(),
    )
    .unwrap();
    let index = session.repository.index();
    index.stop_sync_storage().unwrap();
    std::fs::write(root.path().join("late.json"), r#"{"title":"Late"}"#).unwrap();
    index.add_file(&root.path().join("late.json")).unwrap();
    assert_ne!(index.storage_status().unwrap().durable_generation, Some(index.status().unwrap().revision.0));
    shutdown_with_storage_flush(&session).unwrap();
    assert_eq!(index.storage_status().unwrap().durable_generation, Some(index.status().unwrap().revision.0));
    resume_after_cancelled_close(&session, None, false).unwrap();
    std::fs::write(root.path().join("later.json"), r#"{"title":"Later"}"#).unwrap();
    index.add_file(&root.path().join("later.json")).unwrap();
    shutdown_with_storage_flush(&session).unwrap();
}

#[test]
fn commands_act_on_the_open_sessions_live_index() {
    let fixture = fixture();
    let session = &fixture.session;
    let before = status(session).unwrap();
    std::fs::write(
        fixture._root.path().join("new.json"),
        r#"{"title":"New record"}"#,
    )
    .unwrap();
    let report = check_for_changes(session).unwrap().wait().unwrap();
    assert_eq!(report.added, 1);
    let after = status(session).unwrap();
    assert_eq!(after.instance_id, before.instance_id);
    assert!(after.revision > before.revision);
    assert_eq!(after.live_records, before.live_records + 1);
    let compact = start_maintenance(session, MaintenanceOperation::Compact)
        .unwrap()
        .wait()
        .unwrap();
    assert!(matches!(
        compact,
        objekte_search_core::lifecycle::MaintenanceReport::Compact(_)
    ));
    let rebound = session.refresh_base_metadata().unwrap();
    assert_eq!(rebound.repository.index().instance_id(), after.instance_id);
}

#[test]
fn save_revision_and_load_replacement_are_explicit() {
    let fixture = fixture();
    let session = &fixture.session;
    let output = tempfile::tempdir().unwrap();
    let cache = output.path().join("index.snapshot");
    let saved = save_index(session, &cache, false).unwrap();
    assert_eq!(saved.destination, cache);
    assert_eq!(saved.saved_revision, saved.current_status.revision);
    assert!(cache.exists());
    assert_eq!(
        save_index(session, &cache, false).unwrap_err().kind,
        BackendErrorKind::Options
    );
    let old_instance = status(session).unwrap().instance_id;
    let invalid = output.path().join("invalid.snapshot");
    std::fs::write(&invalid, "invalid").unwrap();
    assert!(load_index(session, &invalid, &CancellationToken::new()).is_err());
    assert_eq!(status(session).unwrap().instance_id, old_instance);
    let restored = load_index(session, &cache, &CancellationToken::new()).unwrap();
    assert_ne!(status(&restored).unwrap().instance_id, old_instance);
    assert_eq!(
        restored.configuration.cache_load.as_deref(),
        Some(cache.as_path())
    );
    assert!(current_reconciliation(&restored).unwrap().is_some());
    shutdown(&restored).unwrap();
}

#[test]
fn selected_index_uses_its_saved_build_settings() {
    let fixture = fixture();
    let session = &fixture.session;
    let mut options = session.configuration.options;
    options.bigram_alphabet = BigramAlphabet::Unicode;
    options.inference.mode = InferenceMode::AutoIndex;
    let index = Index::build(
        session.paths.root.clone(),
        session.configuration.definitions.as_ref().clone(),
        options,
    )
    .unwrap();
    let output = tempfile::tempdir().unwrap();
    let cache = output.path().join("unicode.snapshot");
    index.flush_cache(&cache).unwrap();
    let mut unrelated = session.configuration.definitions.as_ref().clone();
    unrelated[0].name.push_str("-different");
    assert!(
        objekte_search_core::storage::snapshot_build_configuration(
            &cache,
            &unrelated,
            session.configuration.options,
        )
        .unwrap_err()
        .contains("different collection or schema")
    );

    let loaded = load_index(session, &cache, &CancellationToken::new()).unwrap();
    assert_eq!(
        loaded.configuration.options.bigram_alphabet,
        BigramAlphabet::Unicode
    );
    assert_eq!(
        loaded.configuration.options.inference.mode,
        InferenceMode::AutoIndex
    );
    assert_eq!(
        loaded.args.index_alphabet,
        objekte_cli::arguments::Alphabet::Unicode
    );
    assert!(loaded.args.auto_index);
    assert_eq!(
        status(&loaded).unwrap().live_records,
        status(session).unwrap().live_records
    );
    shutdown(&loaded).unwrap();
    // Snapshots written before structural capability flags need a new build.
    let mut old = std::fs::read(&cache).unwrap();
    old[8..12].copy_from_slice(&20u32.to_le_bytes());
    std::fs::write(&cache, old).unwrap();
    assert!(load_index(session, &cache, &CancellationToken::new()).is_err());
    index.shutdown().unwrap();
}

#[test]
fn save_as_copy_does_not_select_an_automatic_destination() {
    let fixture = fixture();
    let output = tempfile::tempdir().unwrap();
    let copy = output.path().join("copy.snapshot");
    let before = status(&fixture.session).unwrap();
    assert!(before.dirty());
    let saved = save_index_copy(&fixture.session, &copy, false).unwrap();
    assert_eq!(saved.saved_revision, before.revision);
    assert!(copy.exists());
    let after = status(&fixture.session).unwrap();
    assert_eq!(after.saved_revision, None);
    assert!(after.dirty());
    save_index(&fixture.session, &copy, true).unwrap();
    assert!(!status(&fixture.session).unwrap().dirty());
}
