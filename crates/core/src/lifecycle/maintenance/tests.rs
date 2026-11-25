use std::sync::mpsc;
use std::time::Duration;

use crate::lifecycle::maintenance::{
    MaintenanceOperation, MaintenancePhase, MaintenanceReport, MaintenanceTask,
    MaintenanceWorkerEntryHook,
};
use crate::lifecycle::{Index, IndexError};
use crate::repository::configuration::{CollectionDefinition, RepositoryOptions};

const HOOK_TIMEOUT: Duration = Duration::from_secs(10);

struct ReleaseGuard {
    release: Option<mpsc::Sender<()>>,
}

impl ReleaseGuard {
    fn release(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}

impl Drop for ReleaseGuard {
    fn drop(&mut self) {
        self.release();
    }
}

fn wait_bounded(task: MaintenanceTask) -> MaintenanceReport {
    let (done, receive) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _ = done.send(task.wait());
    });
    let result = receive
        .recv_timeout(HOOK_TIMEOUT)
        .expect("maintenance task did not finish within the test bound")
        .expect("maintenance wait failed");
    worker.join().expect("maintenance wait harness panicked");
    result
}

fn wait_reconciliation_bounded(
    task: crate::lifecycle::reconciliation::ReconciliationTask,
) -> crate::lifecycle::reconciliation::ReconciliationReport {
    let (done, receive) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _ = done.send(task.wait());
    });
    let result = receive
        .recv_timeout(HOOK_TIMEOUT)
        .expect("reconciliation did not finish within the test bound")
        .expect("reconciliation wait failed");
    worker.join().expect("reconciliation wait harness panicked");
    result
}

#[test]
fn maintenance_admission_shares_only_the_real_worker() {
    let root = std::env::temp_dir().join(format!(
        "objekte-maintenance-admission-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/records.rows"),
        b"(\"name\"=\"A\")\n(\"name\"=\"B\")\n",
    )
    .unwrap();
    let index = Index::start_build(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            crate::source::test_support::line_format(),
        )],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let retired = index
        .view()
        .unwrap()
        .file_handle_for_path(std::path::Path::new("data/records.rows"))
        .unwrap();
    index.delete_file(retired).unwrap();
    std::fs::remove_file(root.join("data/records.rows")).unwrap();

    let (reached, arrivals) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let hook = MaintenanceWorkerEntryHook {
        reached,
        release: std::sync::Mutex::new(released),
    };
    *index.0.maintenance_worker_hook.lock().unwrap() = Some(std::sync::Arc::new(hook));
    let mut release_guard = ReleaseGuard {
        release: Some(release),
    };

    let first = index.start_compaction().unwrap();
    assert_eq!(
        arrivals
            .recv_timeout(HOOK_TIMEOUT)
            .expect("compaction worker did not reach its entry barrier"),
        MaintenanceOperation::Compact
    );
    let second = index.start_compaction().unwrap();
    assert!(std::sync::Arc::ptr_eq(&first.0, &second.0));
    assert!(matches!(
        index.start_rebuild(),
        Err(IndexError::ResourceLimit(_))
    ));
    let reconcile = index.start_reconcile().unwrap();
    let observed = index
        .current_reconciliation_task()
        .unwrap()
        .expect("reconciliation must have its own live slot");
    assert!(std::sync::Arc::ptr_eq(&reconcile.0, &observed.0));
    let reconciliation = wait_reconciliation_bounded(reconcile);
    assert!(!reconciliation.cancelled);

    *index.0.maintenance_worker_hook.lock().unwrap() = None;
    release_guard.release();
    let first_report = wait_bounded(first.clone());
    let second_report = wait_bounded(second);
    let (MaintenanceReport::Compact(first_compact), MaintenanceReport::Compact(second_compact)) =
        (first_report, second_report)
    else {
        panic!("both shared handles must receive the compact report")
    };
    assert_eq!(first_compact.revision, second_compact.revision);
    assert_eq!(first_compact.slots_after, second_compact.slots_after);
    assert_eq!(first_compact.attempts, second_compact.attempts);
    assert_eq!(
        first_compact.input_records_examined,
        second_compact.input_records_examined
    );
    assert_eq!(
        first_compact.input_bytes_budgeted,
        second_compact.input_bytes_budgeted
    );
    assert!(matches!(
        arrivals.try_recv(),
        Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected)
    ));
    assert_eq!(first.progress().unwrap().phase, MaintenancePhase::Complete);

    let rebuild = wait_bounded(index.start_rebuild().unwrap());
    assert!(matches!(rebuild, MaintenanceReport::Rebuild(_)));
    assert!(matches!(first_compact.attempts, 1..=3));
    assert!(first_compact.input_records_examined > 0);

    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn concurrent_quiesce_and_shutdown_wait_for_the_same_worker() {
    let root = std::env::temp_dir().join(format!(
        "objekte-concurrent-shutdown-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/records.rows"),
        b"(\"name\"=\"A\")\n(\"name\"=\"B\")\n",
    )
    .unwrap();
    let index = Index::start_build(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            crate::source::test_support::line_format(),
        )],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let pinned = index.view().unwrap();
    let pinned_revision = pinned.revision();
    let retired = pinned
        .file_handle_for_path(std::path::Path::new("data/records.rows"))
        .unwrap();
    index.delete_file(retired).unwrap();
    std::fs::remove_file(root.join("data/records.rows")).unwrap();
    let revision = index.view().unwrap().revision();

    let (reached, arrivals) = mpsc::channel();
    let (release, released) = mpsc::channel();
    *index.0.maintenance_worker_hook.lock().unwrap() =
        Some(std::sync::Arc::new(MaintenanceWorkerEntryHook {
            reached,
            release: std::sync::Mutex::new(released),
        }));
    let mut release_guard = ReleaseGuard {
        release: Some(release),
    };
    let compaction = index.start_compaction().unwrap();
    assert_eq!(
        arrivals
            .recv_timeout(HOOK_TIMEOUT)
            .expect("compaction worker did not reach its entry barrier"),
        MaintenanceOperation::Compact
    );
    let expected_identity = std::sync::Arc::as_ptr(&compaction.0) as usize;

    let (observed_slot, observations) = mpsc::channel();
    *index.0.quiesce_slot_observation.lock().unwrap() = Some(observed_slot);
    let (first_done, first_result) = mpsc::channel();
    let first_owner = index.clone();
    let first = std::thread::spawn(move || {
        let _ = first_done.send(first_owner.quiesce());
    });
    assert_eq!(
        observations
            .recv_timeout(HOOK_TIMEOUT)
            .expect("first quiesce did not inspect the maintenance slot"),
        Some(expected_identity)
    );

    let (second_done, second_result) = mpsc::channel();
    let second_owner = index.clone();
    let second = std::thread::spawn(move || {
        let _ = second_done.send(second_owner.shutdown());
    });
    assert_eq!(
        observations
            .recv_timeout(HOOK_TIMEOUT)
            .expect("shutdown did not inspect the maintenance slot"),
        Some(expected_identity)
    );

    assert!(matches!(
        first_result.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert!(matches!(
        second_result.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert!(matches!(
        index.start_compaction(),
        Err(IndexError::Cancelled)
    ));
    assert!(matches!(
        index.start_reconcile(),
        Err(IndexError::Cancelled)
    ));
    assert_eq!(index.view().unwrap().revision(), revision);

    *index.0.maintenance_worker_hook.lock().unwrap() = None;
    release_guard.release();
    first_result
        .recv_timeout(HOOK_TIMEOUT)
        .expect("first quiesce did not return")
        .expect("first quiesce failed");
    second_result
        .recv_timeout(HOOK_TIMEOUT)
        .expect("shutdown did not return")
        .expect("shutdown failed");
    first.join().expect("first quiesce harness panicked");
    second.join().expect("shutdown harness panicked");

    assert_eq!(
        compaction.progress().unwrap().phase,
        MaintenancePhase::Cancelled
    );
    assert!(matches!(compaction.wait(), Err(IndexError::Cancelled)));
    assert_eq!(index.view().unwrap().revision(), revision);
    assert_eq!(pinned.revision(), pinned_revision);
    assert!(matches!(
        arrivals.try_recv(),
        Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected)
    ));
    index.shutdown().unwrap();

    index.resume_after_quiesce().unwrap();
    let rebuilt = wait_bounded(index.start_rebuild().unwrap());
    assert!(matches!(rebuilt, MaintenanceReport::Rebuild(_)));

    drop(pinned);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
