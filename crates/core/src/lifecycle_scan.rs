//! Metadata-only source scan for lifecycle reconciliation.
//!
//! A failed collection scan is never treated as evidence of deletion. The
//! coordinator applies the resulting differences through ordinary file
//! mutations after rechecking each file's version.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::discovery::discover;
use crate::inventory::SourceRevision;
use crate::model::CollectionId;
use crate::repository::CollectionDefinition;
use crate::source::SourceRegistry;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FileStamp {
    pub path: PathBuf,
    pub revision: SourceRevision,
    pub created_ns: Option<i128>,
    pub collections: BTreeSet<CollectionId>,
    pub retry_unreadable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ScanIssue {
    pub collection: CollectionId,
    pub message: String,
}

#[derive(Default)]
pub(crate) struct SourceScan {
    pub files: BTreeMap<PathBuf, FileStamp>,
    pub complete_collections: BTreeSet<CollectionId>,
    pub issues: Vec<ScanIssue>,
    pub examined: usize,
    pub cancelled: bool,
}

/// Enumerate selected paths and metadata, without reading file content. One
/// collection is all-or-nothing: a directory or stat failure cannot cause
/// its absent files to be classified as removed.
pub(crate) fn scan_sources(
    root: &Path,
    definitions: &[CollectionDefinition],
    sources: &SourceRegistry,
    cancelled: impl Fn() -> bool,
) -> SourceScan {
    let mut scan = SourceScan::default();
    for (number, definition) in definitions.iter().enumerate() {
        if cancelled() {
            scan.cancelled = true;
            break;
        }
        let collection = match u32::try_from(number) {
            Ok(number) => CollectionId(number),
            Err(_) => {
                scan.issues.push(ScanIssue {
                    collection: CollectionId(u32::MAX),
                    message: "too many collections".into(),
                });
                break;
            }
        };
        let selected = (|| -> Result<Vec<(PathBuf, SourceRevision, Option<i128>)>, String> {
            let source = sources.resolve(&definition.format)?;
            let paths = discover(
                root,
                &definition.source_path,
                &definition.discovery,
                &source,
            )?;
            let mut selected = Vec::with_capacity(paths.len());
            for path in paths {
                if cancelled() {
                    return Err("scan cancelled".into());
                }
                let relative = path
                    .strip_prefix(root)
                    .map_err(|error| error.to_string())?
                    .to_owned();
                let metadata = std::fs::metadata(&path)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                let revision = SourceRevision::from_metadata(&metadata)?;
                let created_ns = metadata
                    .created()
                    .ok()
                    .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_nanos() as i128);
                selected.push((relative, revision, created_ns));
            }
            Ok(selected)
        })();
        match selected {
            Ok(selected) => {
                if let Some((path, _, _)) = selected.iter().find(|(path, revision, _)| {
                    scan.files
                        .get(path)
                        .is_some_and(|existing| existing.revision != *revision)
                }) {
                    scan.issues.push(ScanIssue {
                        collection,
                        message: format!("{} changed during the metadata scan", path.display()),
                    });
                    continue;
                }
                scan.examined += selected.len();
                for (path, revision, created_ns) in selected {
                    let entry = scan.files.entry(path.clone()).or_insert_with(|| FileStamp {
                        path,
                        revision,
                        created_ns,
                        collections: BTreeSet::new(),
                        retry_unreadable: false,
                    });
                    entry.collections.insert(collection);
                }
                scan.complete_collections.insert(collection);
            }
            Err(message) if message == "scan cancelled" => {
                scan.cancelled = true;
                break;
            }
            Err(message) => scan.issues.push(ScanIssue {
                collection,
                message,
            }),
        }
    }
    scan
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ScanChange {
    Add(FileStamp),
    Update {
        previous: FileStamp,
        observed: FileStamp,
    },
    Remove(FileStamp),
}

#[derive(Default)]
pub(crate) struct ScanPlan {
    pub changes: Vec<ScanChange>,
    pub unchanged: usize,
    pub skipped: usize,
}

/// Compare a pinned active inventory with a scan. A file touching a failed
/// collection is left alone until that collection has a complete scan.
pub(crate) fn plan_scan(current: &BTreeMap<PathBuf, FileStamp>, scan: &SourceScan) -> ScanPlan {
    let mut plan = ScanPlan::default();
    for (path, previous) in current {
        if !previous.collections.is_subset(&scan.complete_collections) {
            plan.skipped += 1;
            continue;
        }
        match scan.files.get(path) {
            None => plan.changes.push(ScanChange::Remove(previous.clone())),
            Some(observed)
                if previous.revision != observed.revision
                    || previous.created_ns != observed.created_ns
                    || previous.collections != observed.collections
                    || previous.retry_unreadable =>
            {
                plan.changes.push(ScanChange::Update {
                    previous: previous.clone(),
                    observed: observed.clone(),
                });
            }
            Some(_) => plan.unchanged += 1,
        }
    }
    for (path, observed) in &scan.files {
        if !current.contains_key(path) {
            plan.changes.push(ScanChange::Add(observed.clone()));
        }
    }
    plan
}

#[cfg(test)]
#[path = "lifecycle_scan.test.rs"]
mod tests;
