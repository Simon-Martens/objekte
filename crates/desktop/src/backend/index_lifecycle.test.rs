use super::*;
use crate::backend::test_support::fixture;
use crate::backend::{OpenSpec, open};
use objekte_cli::arguments::Arguments;

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

#[test]
fn automatic_save_requires_configured_repository_and_selected_destination() {
    let fixture = fixture();
    assert_eq!(
        set_automatic_save(&fixture.session, true).unwrap_err().kind,
        BackendErrorKind::Options
    );

    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".objekte/collections")).unwrap();
    std::fs::create_dir_all(root.path().join("data")).unwrap();
    std::fs::write(
        root.path().join(".objekte/collections/data.schema.json"),
        br#"{"type":"object","x-objekte":{"name":"data","path":"data","format":"json","id":"filename"}}"#,
    )
    .unwrap();
    std::fs::write(root.path().join("data/a.json"), br#"{"name":"A"}"#).unwrap();
    let session = open(
        &OpenSpec {
            args: Arguments {
                repo: Some(root.path().to_path_buf()),
                ..Default::default()
            },
            cwd: root.path().to_path_buf(),
            configured_cache_policy: None,
        },
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(session.configuration.cache_save.is_some());
    set_automatic_save(&session, true).unwrap();
    set_automatic_save(&session, false).unwrap();
    assert!(session.configuration.cache_save.as_ref().unwrap().exists());
    assert!(!status(&session).unwrap().dirty());

    std::fs::write(root.path().join("data/b.json"), br#"{"name":"B"}"#).unwrap();
    check_for_changes(&session).unwrap().wait().unwrap();
    assert!(status(&session).unwrap().dirty());
    set_automatic_save(&session, true).unwrap();
    shutdown_with_final_save(&session).unwrap();
    assert!(!status(&session).unwrap().dirty());
}
