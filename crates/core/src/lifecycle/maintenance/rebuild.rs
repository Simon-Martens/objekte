use std::time::{Duration, Instant};

use crate::inventory::{FileVersion, FileVersions};
use crate::lifecycle::initial::InitialBuildPhase;
use crate::lifecycle::maintenance::MaintenancePhase;
use crate::lifecycle::{Index, IndexError, IndexRevision};

/// Outcome of explicit full reconstruction from the configured source files.
#[derive(Clone, Debug)]
pub struct RebuildReport {
    pub revision: IndexRevision,
    pub attempts: usize,
    pub indexed_files: usize,
    pub live_records: u64,
    pub duration: Duration,
}
impl Index {
    /// Rebuild from the current explicit configuration. Queries keep using
    /// their pinned prior view while source extraction runs. A concurrent
    /// commit makes the candidate stale and causes a fresh bounded retry.
    pub fn rebuild(&self) -> Result<RebuildReport, IndexError> {
        self.rebuild_with_before_publish(|_| {})
    }

    pub(crate) fn rebuild_with_before_publish(
        &self,
        mut before_publish: impl FnMut(usize),
    ) -> Result<RebuildReport, IndexError> {
        self.rebuild_with_control(|| false, |_, _| {}, |attempt| before_publish(attempt))
    }

    pub(crate) fn rebuild_with_control(
        &self,
        cancelled: impl Fn() -> bool,
        mut progress: impl FnMut(MaintenancePhase, usize),
        mut before_publish: impl FnMut(usize),
    ) -> Result<RebuildReport, IndexError> {
        let started = Instant::now();
        let mut conflict = None;
        for attempt in 1..=3 {
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            progress(MaintenancePhase::Preparing, attempt);
            let view = self.view()?;
            let repository = view.repository();
            let definitions = repository
                .collections()
                .iter()
                .map(|collection| collection.required_file_definition().clone())
                .collect::<Vec<_>>();
            let task = Self::start_build(
                repository.files().root.clone(),
                definitions,
                repository.options(),
                repository.sources.clone(),
            )?;
            let fresh = loop {
                if cancelled() {
                    task.cancel();
                    let _ = task.wait_complete();
                    return Err(IndexError::Cancelled);
                }
                match task.status()?.phase {
                    InitialBuildPhase::Complete => break task.wait_complete()?,
                    InitialBuildPhase::Cancelled => return Err(IndexError::Cancelled),
                    InitialBuildPhase::Failed => {
                        return match task.wait_complete() {
                            Err(error) => Err(error),
                            Ok(_) => unreachable!("failed build task cannot complete successfully"),
                        };
                    }
                    InitialBuildPhase::Discovering | InitialBuildPhase::Building => {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
            };
            let next_revision = view.revision().next()?;
            let mut next = (*fresh.view()?.0).clone();
            next.instance_id = self.instance_id();
            next.revision = next_revision;
            next.file_versions =
                FileVersions::filled(next.catalog.allocated_files(), FileVersion(next_revision.0));
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            progress(MaintenancePhase::Publishing, attempt);
            before_publish(attempt);
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            match self.publish_prepared(view.revision(), next) {
                Ok(revision) => {
                    let status = self.status()?;
                    return Ok(RebuildReport {
                        revision,
                        attempts: attempt,
                        indexed_files: status.indexed_files,
                        live_records: status.live_records,
                        duration: started.elapsed(),
                    });
                }
                Err(error @ IndexError::RevisionConflict { .. }) => conflict = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(conflict.expect("rebuild retry always captures a revision conflict"))
    }
}
