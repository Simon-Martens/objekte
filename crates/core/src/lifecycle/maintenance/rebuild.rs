use std::time::{Duration, Instant};

use crate::index::segment::SegmentBacking;
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
            if repository
                .collections()
                .iter()
                .all(|collection| collection.file_source.is_none())
            {
                // These registrations own their records independently of the
                // file catalog. Their immutable indexed segments already hold
                // the retained input needed by a new publication. Validate
                // versioned external sources before publishing them again.
                for group in &view.0.segments {
                    for segment in group.iter() {
                        match &segment.index.backing {
                            SegmentBacking::Retained { .. } => {}
                            SegmentBacking::Provider {
                                registration,
                                versions,
                                ..
                            } => {
                                let source = repository
                                    .record_registrations
                                    .get(*registration as usize)
                                    .ok_or_else(|| {
                                        IndexError::Build("provider registration missing".into())
                                    })?;
                                let reader = source.reader.as_ref().ok_or_else(|| {
                                    IndexError::SourceUnavailable("provider reader missing".into())
                                })?;
                                for slot in segment.active.bitmap().iter() {
                                    if cancelled() {
                                        return Err(IndexError::Cancelled);
                                    }
                                    let record = segment
                                        .index
                                        .record(crate::model::RecordSlot(slot))
                                        .ok_or_else(|| {
                                            IndexError::Build("provider record missing".into())
                                        })?;
                                    let expected =
                                        *versions.get(slot as usize).ok_or_else(|| {
                                            IndexError::Build("provider version missing".into())
                                        })?;
                                    let key = crate::records::RecordKey::new(
                                        segment.index.collection,
                                        record.id.clone(),
                                    );
                                    let read = reader
                                        .read_projected(
                                            &key,
                                            expected,
                                            &crate::records::reader::RecordProjection::full(),
                                        )
                                        .map_err(|error| {
                                            IndexError::SourceUnavailable(error.to_string())
                                        })?;
                                    if read.version != expected {
                                        return Err(IndexError::SourceUnavailable(
                                            "provider returned a different record version".into(),
                                        ));
                                    }
                                }
                            }
                            SegmentBacking::FileRange { .. } => {
                                return Err(IndexError::IncompatibleConfiguration(
                                    "file-backed segment has no file source".into(),
                                ));
                            }
                        }
                    }
                }
                let mut next = (*view.0).clone();
                next.revision = view.revision().next()?;
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
                    Err(error @ IndexError::RevisionConflict { .. }) => {
                        conflict = Some(error);
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            }
            let definitions = repository
                .collections()
                .iter()
                .map(|collection| collection.required_file_definition().clone())
                .collect::<Vec<_>>();
            let task = Self::start_build(
                repository.files().required_root().to_path_buf(),
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
