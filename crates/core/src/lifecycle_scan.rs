//! Metadata-only source scan for lifecycle reconciliation.
//!
//! A failed collection scan is never treated as evidence of deletion. The
//! coordinator applies the resulting differences through ordinary file
//! mutations after rechecking each file's version.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::discovery::{PreparedScanSelection, SymlinkPolicy, discover_stream};
use crate::inventory::SourceRevision;
use crate::model::{CollectionId, FileId};
use crate::repository::CollectionDefinition;
use crate::source::{PreparedSource, SourceRegistry};
use roaring::RoaringBitmap;

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ScanChange {
    Add(FileStamp),
    Update {
        previous: FileStamp,
        observed: FileStamp,
    },
    Remove(FileStamp),
}

/// Bounded scan outcome. Existing file identities use one compact bitmap;
/// additions and replacements are handed to the caller in bounded batches.
pub(crate) struct SourceScanSummary {
    pub complete_collections: BTreeSet<CollectionId>,
    pub issues: Vec<ScanIssue>,
    pub examined: usize,
    /// Calls to `std::fs::metadata` for selected physical file observations.
    pub metadata_observations: usize,
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
            metadata_observations: 0,
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
    selection: Option<PreparedScanSelection>,
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
        let selection = match PreparedScanSelection::new(
            root,
            &definition.source_path,
            &definition.discovery,
            &source,
        ) {
            Ok(selection) => Some(selection),
            Err(message) => {
                summary.issues.push(ScanIssue {
                    collection: id,
                    message,
                });
                scopes.push(ScanScope {
                    id,
                    definition,
                    source,
                    selection: None,
                    aliases: None,
                    complete: false,
                });
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
                    selection,
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
                    selection,
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
                let selected_result = match (&candidate.aliases, &candidate.selection) {
                    (Some(aliases), _) => Ok(aliases.contains(&path)),
                    (_, None) => {
                        if relative.starts_with(&candidate.definition.source_path) {
                            uncertain = true;
                        }
                        Ok(false)
                    }
                    (_, Some(selection))
                        if scope.definition.source_path == candidate.definition.source_path
                            && scope.definition.discovery.symlinks == SymlinkPolicy::Skip
                            && candidate.definition.discovery.symlinks == SymlinkPolicy::Skip =>
                    {
                        Ok(selection.selects_lexical(relative))
                    }
                    (_, Some(selection)) => selection.selects_file(root, relative),
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
            summary.metadata_observations = summary.metadata_observations.saturating_add(1);
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
