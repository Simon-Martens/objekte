//! Metadata-only source scan for lifecycle reconciliation.
//!
//! A failed collection scan is never treated as evidence of deletion. The
//! coordinator applies the resulting differences through ordinary file
//! mutations after rechecking each file's version.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::discovery::{
    SymlinkPolicy, discover_bounded, discover_stream, selects_file, selects_file_lexical,
};
use crate::inventory::SourceRevision;
use crate::model::{CollectionId, FileId};
use crate::repository::CollectionDefinition;
use crate::source::{PreparedSource, SourceRegistry};
use roaring::RoaringBitmap;

const MAX_RECONCILIATION_FILES: usize = 250_000;

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
            let paths = discover_bounded(
                root,
                &definition.source_path,
                &definition.discovery,
                &source,
                MAX_RECONCILIATION_FILES.saturating_sub(scan.files.len()),
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

/// Bounded scan outcome. Existing file identities use one compact bitmap;
/// additions and replacements are handed to the caller in bounded batches.
pub(crate) struct SourceScanSummary {
    pub complete_collections: BTreeSet<CollectionId>,
    pub issues: Vec<ScanIssue>,
    pub examined: usize,
    pub unchanged: usize,
    pub skipped: usize,
    pub planned_changes: usize,
    pub cancelled: bool,
    pub seen_existing: RoaringBitmap,
}

impl Default for SourceScanSummary {
    fn default() -> Self {
        Self {
            complete_collections: BTreeSet::new(),
            issues: Vec::new(),
            examined: 0,
            unchanged: 0,
            skipped: 0,
            planned_changes: 0,
            cancelled: false,
            seen_existing: RoaringBitmap::new(),
        }
    }
}

struct ScanScope<'a> {
    id: CollectionId,
    definition: &'a CollectionDefinition,
    source: PreparedSource,
    /// A followed-symlink collection must choose one lexical alias for each
    /// physical file before any file can be emitted. Ordinary scans keep none.
    aliases: Option<BTreeSet<PathBuf>>,
    complete: bool,
}

/// Scan large selections without collecting a second complete file inventory
/// or an unbounded pending preparation queue. A first traversal proves which
/// scopes can safely establish absence; the second yields metadata and bounded
/// change batches. Files selected by overlapping collections are observed once
/// and carry all proven memberships in the same transaction.
pub(crate) fn scan_sources_batched(
    root: &Path,
    definitions: &[CollectionDefinition],
    sources: &SourceRegistry,
    excluded: impl Fn(&Path) -> bool,
    lookup_existing: impl Fn(&Path) -> Option<(FileId, FileStamp)>,
    cancelled: impl Fn() -> bool,
    batch_files: usize,
    mut on_batch: impl FnMut(Vec<ScanChange>),
) -> SourceScanSummary {
    let mut summary = SourceScanSummary::default();
    let mut scopes = Vec::with_capacity(definitions.len());
    let mut unresolved_sources = Vec::new();
    for (number, definition) in definitions.iter().enumerate() {
        if cancelled() {
            summary.cancelled = true;
            break;
        }
        let id = match u32::try_from(number) {
            Ok(id) => CollectionId(id),
            Err(_) => {
                summary.issues.push(ScanIssue {
                    collection: CollectionId(u32::MAX),
                    message: "too many collections".into(),
                });
                break;
            }
        };
        let source = match sources.resolve(&definition.format) {
            Ok(source) => source,
            Err(message) => {
                summary.issues.push(ScanIssue {
                    collection: id,
                    message,
                });
                unresolved_sources.push(&definition.source_path);
                continue;
            }
        };
        let mut aliases =
            (definition.discovery.symlinks == SymlinkPolicy::FollowWithinRoot).then(BTreeSet::new);
        let outcome = discover_stream(
            root,
            &definition.source_path,
            &definition.discovery,
            &source,
            &cancelled,
            |path| {
                if let Some(aliases) = &mut aliases {
                    aliases.insert(path);
                }
                Ok(())
            },
        );
        match outcome {
            Ok(()) => {
                summary.complete_collections.insert(id);
                scopes.push(ScanScope {
                    id,
                    definition,
                    source,
                    aliases,
                    complete: true,
                });
            }
            Err(message) if message == "scan cancelled" => {
                summary.cancelled = true;
                break;
            }
            Err(message) => {
                summary.issues.push(ScanIssue {
                    collection: id,
                    message,
                });
                scopes.push(ScanScope {
                    id,
                    definition,
                    source,
                    aliases: None,
                    complete: false,
                });
            }
        }
    }

    let mut batch = Vec::with_capacity(batch_files.max(1));
    for number in 0..scopes.len() {
        if cancelled() {
            summary.cancelled = true;
            break;
        }
        let scope = &scopes[number];
        if !scope.complete {
            continue;
        }
        let selected_paths = scope.aliases.as_ref();
        let mut visit = |path: PathBuf| -> Result<(), String> {
            if cancelled() {
                return Err("scan cancelled".into());
            }
            let relative = path.strip_prefix(root).map_err(|error| error.to_string())?;
            if excluded(relative) {
                return Ok(());
            }
            let mut memberships = BTreeSet::new();
            let mut first = None;
            let mut uncertain = unresolved_sources
                .iter()
                .any(|source_path| relative.starts_with(source_path));
            for candidate in &scopes {
                let selected_result = if let Some(aliases) = &candidate.aliases {
                    Ok(aliases.contains(&path))
                } else if scope.definition.source_path == candidate.definition.source_path
                    && scope.definition.discovery.symlinks == SymlinkPolicy::Skip
                    && candidate.definition.discovery.symlinks == SymlinkPolicy::Skip
                {
                    selects_file_lexical(
                        &candidate.definition.source_path,
                        &candidate.definition.discovery,
                        &candidate.source,
                        relative,
                    )
                } else {
                    selects_file(
                        root,
                        &candidate.definition.source_path,
                        &candidate.definition.discovery,
                        &candidate.source,
                        relative,
                    )
                };
                let selected = match selected_result {
                    Ok(selected) => selected,
                    Err(_) if !candidate.complete => {
                        if relative.starts_with(&candidate.definition.source_path) {
                            uncertain = true;
                        }
                        false
                    }
                    Err(message) => return Err(message),
                };
                if selected {
                    if candidate.complete {
                        first.get_or_insert(candidate.id);
                        memberships.insert(candidate.id);
                    } else {
                        uncertain = true;
                    }
                }
            }
            if first != Some(scope.id) {
                return Ok(());
            }
            let existing = lookup_existing(relative);
            if let Some((id, _)) = &existing {
                summary.seen_existing.insert(id.0);
            }
            summary.examined = summary.examined.saturating_add(1);
            if uncertain
                || existing.as_ref().is_some_and(|(_, previous)| {
                    !previous
                        .collections
                        .is_subset(&summary.complete_collections)
                })
            {
                summary.skipped = summary.skipped.saturating_add(1);
                return Ok(());
            }
            let metadata =
                std::fs::metadata(&path).map_err(|error| format!("{}: {error}", path.display()))?;
            let revision = SourceRevision::from_metadata(&metadata)?;
            let created_ns = metadata
                .created()
                .ok()
                .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos() as i128);
            let observed = FileStamp {
                path: relative.to_owned(),
                revision,
                created_ns,
                collections: memberships,
                retry_unreadable: false,
            };
            match existing {
                None => batch.push(ScanChange::Add(observed)),
                Some((_, previous))
                    if previous.revision != observed.revision
                        || previous.created_ns != observed.created_ns
                        || previous.collections != observed.collections
                        || previous.retry_unreadable =>
                {
                    batch.push(ScanChange::Update { previous, observed });
                }
                Some(_) => summary.unchanged = summary.unchanged.saturating_add(1),
            }
            if batch.len() >= batch_files.max(1) {
                summary.planned_changes = summary.planned_changes.saturating_add(batch.len());
                on_batch(std::mem::take(&mut batch));
            }
            Ok(())
        };
        let outcome = if let Some(paths) = selected_paths {
            paths.iter().try_for_each(|path| visit(path.clone()))
        } else {
            discover_stream(
                root,
                &scope.definition.source_path,
                &scope.definition.discovery,
                &scope.source,
                &cancelled,
                &mut visit,
            )
        };
        match outcome {
            Ok(()) => {
                if !batch.is_empty() {
                    summary.planned_changes = summary.planned_changes.saturating_add(batch.len());
                    on_batch(std::mem::take(&mut batch));
                }
            }
            Err(message) if message == "scan cancelled" => {
                summary.cancelled = true;
                batch.clear();
                break;
            }
            Err(message) => {
                batch.clear();
                summary.complete_collections.remove(&scope.id);
                summary.issues.push(ScanIssue {
                    collection: scope.id,
                    message,
                });
            }
        }
    }
    summary
}

#[cfg(test)]
#[path = "lifecycle_scan.test.rs"]
mod tests;
