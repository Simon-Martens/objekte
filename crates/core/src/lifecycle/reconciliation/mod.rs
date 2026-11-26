//! Batch metadata reconciliation.

use std::time::Duration;

use std::path::Path;
use std::time::Instant;

use crate::construction::file_preparation::verify_current_source;
use crate::index::traits::file_catalog::{FileCatalogReader, FileRows};
use crate::lifecycle::{Index, IndexError, IndexView};

const MAX_RECONCILIATION_ISSUES: usize = 128;

/// Initial status vocabulary; background task state is supplied by the
/// reconciliation phase, separately from publication revisions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconciliationState {
    NotStarted,
    Pending,
    Running,
    Complete,
    Partial,
    Cancelled,
}

/// One metadata scan and its applied index changes. Errors affecting only a
/// source remain visible here while other independent files can proceed.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReconciliationReport {
    pub examined: usize,
    pub planned_changes: usize,
    pub unchanged: usize,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub skipped: usize,
    pub indexed_bytes: u64,
    pub duration: Duration,
    pub cancelled: bool,
    pub issues: Vec<String>,
    pub omitted_issues: usize,
}

impl ReconciliationReport {
    fn record_issue(&mut self, issue: String) {
        if self.issues.len() < MAX_RECONCILIATION_ISSUES {
            self.issues.push(issue);
        } else {
            self.omitted_issues = self.omitted_issues.saturating_add(1);
        }
    }
}

/// Small, copyable observation while a reconciliation pass is running.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReconciliationProgress {
    pub examined: usize,
    pub planned_changes: usize,
    pub unchanged: usize,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub skipped: usize,
    pub indexed_bytes: u64,
    pub duration: Duration,
    pub cancelled: bool,
    pub issue_count: usize,
}

impl From<&ReconciliationReport> for ReconciliationProgress {
    fn from(report: &ReconciliationReport) -> Self {
        Self {
            examined: report.examined,
            planned_changes: report.planned_changes,
            unchanged: report.unchanged,
            added: report.added,
            updated: report.updated,
            removed: report.removed,
            skipped: report.skipped,
            indexed_bytes: report.indexed_bytes,
            duration: report.duration,
            cancelled: report.cancelled,
            issue_count: report.issues.len().saturating_add(report.omitted_issues),
        }
    }
}

pub(crate) mod periodic;
pub(crate) mod scan;
pub(crate) mod task;

pub(crate) use periodic::PeriodicReconciliation;
pub(crate) use task::MaintenanceState;
pub use task::ReconciliationTask;

impl Index {
    /// Apply one complete metadata observation through the same file
    /// preparation/publication paths as explicit mutations. Callers schedule
    /// this outside query threads; an unchanged pass does not read content or
    /// publish a new data revision.
    #[cfg(test)]
    pub(crate) fn reconcile_pass(&self, cancelled: impl Fn() -> bool) -> ReconciliationReport {
        self.reconcile_pass_with_progress(cancelled, |_| {})
    }

    pub(crate) fn reconcile_pass_with_progress(
        &self,
        cancelled: impl Fn() -> bool,
        mut on_progress: impl FnMut(&ReconciliationReport),
    ) -> ReconciliationReport {
        use crate::lifecycle::reconciliation::scan::{ScanChange, scan_sources_batched_with_ids};

        let started = Instant::now();
        let mut report = ReconciliationReport::default();
        let snapshot = match self.view() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                report.record_issue(error.to_string());
                report.duration = started.elapsed();
                on_progress(&report);
                return report;
            }
        };
        let repository = snapshot.repository();
        let Some(root) = repository.repository_root() else {
            report.duration = started.elapsed();
            on_progress(&report);
            return report;
        };
        let registrations = repository
            .collections()
            .iter()
            .enumerate()
            .filter_map(|(number, collection)| {
                collection.file_definition.as_ref().map(|definition| {
                    (
                        crate::model::CollectionId(number as u32),
                        definition.clone(),
                    )
                })
            })
            .collect::<Vec<_>>();
        if registrations.is_empty() {
            report.duration = started.elapsed();
            on_progress(&report);
            return report;
        }
        let ids = registrations.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        let definitions = registrations
            .into_iter()
            .map(|(_, definition)| definition)
            .collect::<Vec<_>>();
        let storage_error = std::cell::RefCell::new(None);
        let mut apply = |change: ScanChange| {
            if cancelled() || storage_error.borrow().is_some() {
                report.cancelled = true;
                return;
            }
            report.planned_changes = report.planned_changes.saturating_add(1);
            let bytes = match &change {
                ScanChange::Add(observed) | ScanChange::Update { observed, .. } => {
                    observed.revision.size
                }
                ScanChange::Remove(_) => 0,
            };
            let path = match &change {
                ScanChange::Add(observed) | ScanChange::Remove(observed) => &observed.path,
                ScanChange::Update { observed, .. } => &observed.path,
            }
            .clone();
            let kind = match &change {
                ScanChange::Add(_) => 0,
                ScanChange::Update { .. } => 1,
                ScanChange::Remove(_) => 2,
            };
            let outcome = self.apply_reconciliation_change(&snapshot, root, change);
            match outcome {
                Ok(()) => {
                    report.indexed_bytes = report.indexed_bytes.saturating_add(bytes);
                    match kind {
                        0 => report.added += 1,
                        1 => report.updated += 1,
                        _ => report.removed += 1,
                    }
                }
                Err(error) => {
                    report.skipped += 1;
                    report.record_issue(format!("{}: {error}", path.display()));
                }
            }
            report.duration = started.elapsed();
            on_progress(&report);
        };
        let scan = scan_sources_batched_with_ids(
            root,
            &definitions,
            &ids,
            &repository.sources,
            |_| false,
            |path| {
                let observed: Result<_, IndexError> = (|| {
                    let Some(id) = snapshot.0.catalog.id_for_path(path)? else {
                        return Ok(None);
                    };
                    let entry = snapshot.file_entry(id)?.ok_or_else(|| {
                        IndexError::Storage("file path routes to a missing catalog row".into())
                    })?;
                    let mut stamp = file_stamp(&entry);
                    stamp.collections = snapshot.0.imported_collections(&entry)?;
                    Ok(Some((id, stamp)))
                })();
                match observed {
                    Ok(observed) => observed,
                    Err(error) => {
                        storage_error.replace(Some(error));
                        None
                    }
                }
            },
            &|| cancelled() || storage_error.borrow().is_some(),
            256,
            |batch| {
                for change in batch {
                    apply(change);
                }
            },
        );
        if !scan.cancelled && storage_error.borrow().is_none() {
            for row in FileRows::new(&snapshot.0.catalog) {
                if cancelled() {
                    break;
                }
                let entry = match row {
                    Ok(row) => row.entry,
                    Err(error) => {
                        storage_error.replace(Some(error.into()));
                        break;
                    }
                };
                if scan.seen_existing.contains(entry.id.0) {
                    continue;
                }
                let imported = match snapshot.0.imported_collections(&entry) {
                    Ok(imported) => imported,
                    Err(error) => {
                        storage_error.replace(Some(error));
                        break;
                    }
                };
                if !imported.is_empty() && imported.is_subset(&scan.complete_collections) {
                    let mut stamp = file_stamp(&entry);
                    stamp.collections = imported;
                    apply(ScanChange::Remove(stamp));
                }
            }
        }
        drop(apply);
        if let Some(error) = storage_error.into_inner() {
            report.record_issue(error.to_string());
        }
        report.examined = scan.examined;
        report.unchanged = scan.unchanged;
        report.skipped = report.skipped.saturating_add(scan.skipped);
        report.cancelled |= scan.cancelled || cancelled();
        for issue in &scan.issues {
            report.record_issue(format!(
                "collection {}: {}",
                issue.collection.0, issue.message
            ));
        }
        report.duration = started.elapsed();
        on_progress(&report);
        report
    }

    fn apply_reconciliation_change(
        &self,
        snapshot: &IndexView,
        root: &Path,
        change: crate::lifecycle::reconciliation::scan::ScanChange,
    ) -> Result<(), IndexError> {
        use crate::lifecycle::reconciliation::scan::ScanChange;
        match change {
            ScanChange::Add(observed) => {
                let prepared = self.prepare_add_file(&observed.path)?;
                if prepared.file.source.revision != observed.revision {
                    return Err(IndexError::SourceUnavailable(
                        "source changed after metadata scan".into(),
                    ));
                }
                let source = prepared.file.source.clone();
                let backend = prepared.file.reader(snapshot.repository());
                let identity = prepared.file.identity;
                let fingerprint = prepared.file.fingerprint;
                let expected = prepared.expected_revision;
                let next = self.stage_prepared_add(prepared)?;
                verify_current_source(
                    root,
                    &source,
                    identity,
                    fingerprint,
                    backend.map(|source| source.reader()),
                )
                .map_err(IndexError::SourceUnavailable)?;
                self.publish_prepared(expected, next).map(|_| ())
            }
            ScanChange::Update { previous, observed } => {
                let handle = snapshot
                    .file_handle_for_path(&previous.path)?
                    .ok_or(IndexError::UnknownFile)?;
                let prepared = self.prepare_replace_file(handle, &observed.path)?;
                if prepared.file.source.revision != observed.revision {
                    return Err(IndexError::SourceUnavailable(
                        "source changed after metadata scan".into(),
                    ));
                }
                let source = prepared.file.source.clone();
                let backend = prepared.file.reader(snapshot.repository());
                let identity = prepared.file.identity;
                let fingerprint = prepared.file.fingerprint;
                let expected = prepared.expected_revision;
                let next = self.stage_prepared_replace(prepared)?;
                verify_current_source(
                    root,
                    &source,
                    identity,
                    fingerprint,
                    backend.map(|source| source.reader()),
                )
                .map_err(IndexError::SourceUnavailable)?;
                self.publish_prepared(expected, next).map(|_| ())
            }
            ScanChange::Remove(observed) => {
                let handle = snapshot
                    .file_handle_for_path(&observed.path)?
                    .ok_or(IndexError::UnknownFile)?;
                match std::fs::metadata(root.join(&observed.path)) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        self.delete_file(handle).map(|_| ())
                    }
                    Ok(_) => Err(IndexError::SourceUnavailable(
                        "source reappeared after metadata scan".into(),
                    )),
                    Err(error) => Err(IndexError::Io(error.to_string())),
                }
            }
        }
    }
}

fn file_stamp(entry: &crate::inventory::FileEntry) -> scan::FileStamp {
    scan::FileStamp {
        path: entry.relative_path.to_path_buf(),
        revision: entry.revision,
        created_ns: entry.created_ns,
        collections: entry.collections.iter().copied().collect(),
        retry_unreadable: entry.status == crate::inventory::FileStatus::Unreadable,
    }
}
