use super::*;
use crate::backend::test_support::fixture;

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
