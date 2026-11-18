use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use crate::lifecycle::{
    Index, IndexError, MaintenanceJob, MaintenanceOperation, MaintenancePhase, MaintenanceProgress,
    MaintenanceReport, RebuildReport,
};
use crate::repository::{CollectionDefinition, RepositoryOptions};
use crate::source::SourceFormat;
use crate::source::registry::SourceRegistry;

#[test]
fn maintenance_admission_shares_same_operation_until_worker_release() {
    let root = std::env::temp_dir().join(format!(
        "objekte-maintenance-admission-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), br#"{"name":"A"}"#).unwrap();
    let index = Index::start_build(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let revision = index.view().unwrap().revision().next().unwrap();
    let report = MaintenanceReport::Rebuild(RebuildReport {
        revision,
        attempts: 1,
        indexed_files: 1,
        live_records: 1,
        duration: Default::default(),
    });
    let job = Arc::new(MaintenanceJob {
        operation: MaintenanceOperation::Compact,
        started: Instant::now(),
        cancelled: AtomicBool::new(false),
        progress: Mutex::new(MaintenanceProgress {
            operation: MaintenanceOperation::Compact,
            phase: MaintenancePhase::Preparing,
            attempt: 1,
            elapsed: Default::default(),
        }),
        result: Mutex::new(None),
        complete: Condvar::new(),
        worker: Mutex::new(None),
    });
    let (release, wait_for_release) = mpsc::channel();
    let worker_job = job.clone();
    let worker = std::thread::spawn(move || {
        wait_for_release.recv().unwrap();
        *worker_job.result.lock().unwrap() = Some(Ok(report));
        worker_job.complete.notify_all();
    });
    *job.worker.lock().unwrap() = Some(worker);
    *index.0.maintenance_operation.lock().unwrap() = Some(job.clone());

    let first = index.start_compaction().unwrap();
    let second = index.start_compaction().unwrap();
    assert!(Arc::ptr_eq(&first.0, &second.0));
    let other = match index.start_rebuild() {
        Ok(_) => panic!("other maintenance operation must be rejected"),
        Err(error) => error,
    };
    assert!(matches!(other, IndexError::ResourceLimit(_)));

    release.send(()).unwrap();
    let first_report = first.wait().unwrap();
    let second_report = second.wait().unwrap();
    assert!(matches!(first_report, MaintenanceReport::Rebuild(_)));
    assert!(matches!(second_report, MaintenanceReport::Rebuild(_)));
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
