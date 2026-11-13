use super::*;
use crate::backend::test_support::fixture;
use crate::backend::{OpenSpec, open};
use objekte_cli::arguments::Arguments;
use objekte_search_core::index_storage::{IndexResidency, IndexStorageLocation, IndexStoragePolicy};
use objekte_search_core::streaming::CancellationToken;

#[test]
fn manual_shutdown_does_not_write_an_index() {
    let fixture = fixture();
    let root = fixture._root.path();
    let mut before = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    before.sort();
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
    resume_after_cancelled_close(&session, None).unwrap();
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
