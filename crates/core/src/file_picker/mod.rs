//! A small JSON-only file picker and values-only content index.
//!
//! The layout follows FFF's separation of a stable file table, an immutable
//! bigram index, and a changed-file overlay. Relative record IDs are stored
//! once in a byte arena instead of FFF's general chunked path store.
//! This file owns the table and its construction. `refresh`, `rebuild`, and
//! `candidates` contain the three ways that table is changed or read.

mod candidates;
mod exact;
mod git;
mod infer;
mod key_summary;
mod modified_order;
mod posting;
mod presence;
mod projection;
mod rebuild;
mod refresh;
mod regex_plan;
mod scan_pool;
mod secondary;
mod sort_index;
mod text;
mod typed;
mod view;

use crate::bigram::{
    BigramFile, BigramFilter, BigramIndexBuilder, BigramSnapshot, extract_bigrams,
};
use crate::{
    Direction, Error, Field, IndexField, Result,
    error::io,
    query::Operator,
    source::{Source, Stamp},
};
use rayon::prelude::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
#[cfg(feature = "index-timings")]
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};
use unicode_casefold::UnicodeCaseFold;

use crate::text::TextRules;
pub(crate) use exact::ExactSelection;
pub use git::GitState;
pub(crate) use infer::ReferenceInventory;
use key_summary::KeySummary;
use modified_order::ModifiedOrder;
use presence::PresenceIndex;
use projection::{
    ProjectionOptions, ProjectionOutcome, projected_values, projected_values_with_text,
};
use sort_index::SortIndex;
use text::{TextIndex, TextIndexBuilders, project_malformed_text};
use typed::{TypedIndex, TypedKey};
use view::ViewIndex;

fn sorted_bigrams(bytes: &[u8]) -> Vec<u16> {
    let mut keys = extract_bigrams(bytes);
    keys.sort_unstable();
    keys
}


/// Stable slot metadata for one source JSON file.
#[derive(Debug, Clone)]
struct FileEntry {
    id_start: u32,
    id_len: u32,
    stamp: Stamp,
    revision: u64,
    changed_at_nanos: u128,
}


/// A saved filter plus overlay state awaiting source reconciliation.
struct RestoredBigram {
    filter: BigramFilter,
    changed: BTreeSet<usize>,
    changed_bigrams: BTreeMap<usize, Vec<u16>>,
    newly_changed: BTreeSet<usize>,
    dirty: bool,
}


/// Wall stages and summed worker time recorded while constructing a picker.
#[cfg(feature = "index-timings")]
#[derive(Clone, Copy, Default)]
pub(crate) struct BuildTimings {
    pub inventory_nanos: u64,
    pub slots_nanos: u64,
    pub scan_nanos: u64,
    pub project_worker_nanos: u64,
    pub add_worker_nanos: u64,
    pub compress_nanos: u64,
    pub typed_nanos: u64,
    pub view_nanos: u64,
}


/// Stable numeric file slots plus a separate byte-sorted view of live IDs.
/// Changed/new slots bypass the immutable bigram index until it is rebuilt.
pub struct FilePicker {
    root: PathBuf,
    recursive: bool,
    id_bytes: Vec<u8>,
    files: Vec<Option<FileEntry>>,
    sorted: Vec<usize>,
    modified_order: ModifiedOrder,
    tombstones: BTreeMap<String, (u64, u128)>,
    indexed_len: usize,
    loaded_snapshot: bool,
    secondary_loaded: bool,
    verified: bool,
    snapshot_dirty: bool,
    config_digest: [u8; 32],
    changed: BTreeSet<usize>,
    changed_bigrams: BTreeMap<usize, Vec<u16>>,
    malformed: BTreeSet<usize>,
    array_fields: BTreeSet<String>,
    key_summary: KeySummary,
    index: BigramFilter,
    text_rules: TextRules,
    eager_text_index: bool,
    text_index: Option<TextIndex>,
    presence: PresenceIndex,
    typed: TypedIndex,
    inferred_paths: BTreeSet<crate::JsonPath>,
    view: ViewIndex,
    sort: SortIndex,
    #[cfg(feature = "index-timings")]
    build_timings: BuildTimings,
}


impl FilePicker {
    pub fn open(root: &Path) -> Result<Self> {
        Self::open_with_indexes(
            root,
            false,
            &[],
            &[],
            &[],
            &BTreeMap::new(),
            None,
            None,
            None,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "index configuration is borrowed from repository metadata"
    )]
    pub(crate) fn open_with_indexes(
        root: &Path,
        recursive: bool,
        indexes: &[IndexField],
        advanced_paths: &[crate::JsonPath],
        sort_indexes: &[Vec<crate::Sort>],
        definitions: &BTreeMap<String, Field>,
        language: Option<&str>,
        snapshot: Option<BigramSnapshot>,
        snapshot_path: Option<&Path>,
    ) -> Result<Self> {
        Self::open_inner(
            root,
            recursive,
            indexes,
            advanced_paths,
            sort_indexes,
            definitions,
            language,
            (snapshot, snapshot_path),
            false,
        )?
        .ok_or_else(|| Error::Cache("verified picker could not be opened".into()))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "index configuration is borrowed from repository metadata"
    )]
    pub(crate) fn open_cached_unchecked(
        root: &Path,
        recursive: bool,
        indexes: &[IndexField],
        advanced_paths: &[crate::JsonPath],
        sort_indexes: &[Vec<crate::Sort>],
        definitions: &BTreeMap<String, Field>,
        language: Option<&str>,
        snapshot: BigramSnapshot,
        snapshot_path: &Path,
    ) -> Result<Option<Self>> {
        Self::open_inner(
            root,
            recursive,
            indexes,
            advanced_paths,
            sort_indexes,
            definitions,
            language,
            (Some(snapshot), Some(snapshot_path)),
            true,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "index configuration is borrowed from repository metadata"
    )]
    fn open_inner(
        root: &Path,
        recursive: bool,
        indexes: &[IndexField],
        advanced_paths: &[crate::JsonPath],
        sort_indexes: &[Vec<crate::Sort>],
        definitions: &BTreeMap<String, Field>,
        language: Option<&str>,
        cached: (Option<BigramSnapshot>, Option<&Path>),
        unchecked: bool,
    ) -> Result<Option<Self>> {
        let (mut snapshot, snapshot_path) = cached;
        let root = std::fs::canonicalize(root).map_err(|e| io(root, e))?;
        let source = Source::open_scoped(&root, recursive)?;
        #[cfg(feature = "index-timings")]
        let inventory_started = Instant::now();
        let inventory: Vec<(String, Stamp)> = if unchecked {
            if snapshot.as_ref().is_some_and(|snapshot| {
                snapshot
                    .files
                    .windows(2)
                    .any(|pair| pair[0].id >= pair[1].id)
            }) {
                source.finish()?;
                return Ok(None);
            }
            std::mem::take(
                &mut snapshot
                    .as_mut()
                    .expect("unchecked restore has a snapshot")
                    .files,
            )
            .into_iter()
            .map(|file| {
                (
                    file.id,
                    Stamp {
                        size: file.size_bytes,
                        modified: file.modified_unix_nanos,
                        identity: file.identity,
                    },
                )
            })
            .collect()
        } else {
            source.inventory()?.into_iter().collect()
        };
        let restored = if unchecked {
            snapshot.map(|snapshot| RestoredBigram {
                filter: snapshot.filter,
                changed: snapshot.changed_slots.into_iter().collect(),
                changed_bigrams: snapshot.changed_bigrams,
                newly_changed: BTreeSet::new(),
                dirty: false,
            })
        } else {
            snapshot.and_then(|snapshot| {
                if snapshot.files.len() != inventory.len()
                    || !snapshot
                        .files
                        .iter()
                        .zip(inventory.iter())
                        .all(|(file, (id, _))| {
                            file.id == *id && file.filename == format!("{id}.json")
                        })
                {
                    return None;
                }
                let mut changed = snapshot.changed_slots.into_iter().collect::<BTreeSet<_>>();
                let mut changed_bigrams = snapshot.changed_bigrams;
                let mut newly_changed = BTreeSet::new();
                for (slot, (file, (_, stamp))) in
                    snapshot.files.iter().zip(inventory.iter()).enumerate()
                {
                    if file.size_bytes != stamp.size
                        || file.modified_unix_nanos != stamp.modified
                        || file.identity != stamp.identity
                    {
                        changed.insert(slot);
                        newly_changed.insert(slot);
                        changed_bigrams.remove(&slot);
                    }
                }
                let dirty = !newly_changed.is_empty();
                Some(RestoredBigram {
                    filter: snapshot.filter,
                    changed,
                    changed_bigrams,
                    newly_changed,
                    dirty,
                })
            })
        };
        #[cfg(feature = "index-timings")]
        let inventory_nanos = elapsed_nanos(inventory_started);
        let mut indexed_fields = indexes
            .iter()
            .map(|field| field.name.clone())
            .collect::<Vec<_>>();
        indexed_fields.sort();
        indexed_fields.dedup();
        let text_rules = TextRules::new(definitions);
        let eager_text_index = text_rules.has_markup() || text_rules.has_exclusions();
        let mut picker = Self {
            root,
            recursive,
            id_bytes: Vec::new(),
            files: Vec::with_capacity(inventory.len()),
            sorted: Vec::with_capacity(inventory.len()),
            modified_order: ModifiedOrder::default(),
            tombstones: BTreeMap::new(),
            indexed_len: 0,
            loaded_snapshot: restored.is_some(),
            secondary_loaded: false,
            verified: !unchecked,
            snapshot_dirty: restored.as_ref().is_some_and(|restored| restored.dirty),
            config_digest: secondary::config_digest(
                indexes,
                advanced_paths,
                sort_indexes,
                definitions,
                language,
            )?,
            changed: BTreeSet::new(),
            changed_bigrams: BTreeMap::new(),
            malformed: BTreeSet::new(),
            array_fields: BTreeSet::new(),
            key_summary: KeySummary::default(),
            index: BigramIndexBuilder::new(0).compress(None),
            text_rules,
            eager_text_index,
            text_index: None,
            presence: PresenceIndex::new(definitions),
            view: ViewIndex::new(&indexed_fields, definitions),
            sort: SortIndex::new(sort_indexes, definitions, language)?,
            typed: TypedIndex::new(indexed_fields),
            inferred_paths: BTreeSet::new(),
            #[cfg(feature = "index-timings")]
            build_timings: BuildTimings {
                inventory_nanos: if unchecked { 0 } else { inventory_nanos },
                ..BuildTimings::default()
            },
        };
        #[cfg(feature = "index-timings")]
        let slots_started = Instant::now();
        // Source::inventory is already ordered by ID. Initial slots can be
        // appended in final order without entering the changed-file overlay.
        for (id, stamp) in inventory {
            let slot = picker.append_slot(&id, stamp, 1)?;
            picker.sorted.push(slot);
        }

        #[cfg(feature = "index-timings")]
        {
            picker.build_timings.slots_nanos = elapsed_nanos(slots_started);
        }
        let loaded_secondary = restored.is_some()
            && (!unchecked || !definitions.is_empty())
            && snapshot_path
                .is_some_and(|path| picker.load_secondary_snapshot(path).unwrap_or(false));
        if unchecked && !loaded_secondary && !definitions.is_empty() {
            source.finish()?;
            return Ok(None);
        }
        if loaded_secondary {
            let restored = restored.expect("checked restored snapshot");
            picker.index = restored.filter;
            picker.indexed_len = picker.files.len();
            picker.changed = restored.changed;
            picker.changed_bigrams = restored.changed_bigrams;
            for slot in restored.newly_changed {
                picker.update_restored_slot(&source, slot)?;
            }
        } else if unchecked {
            let restored = restored.expect("unchecked restore has a snapshot");
            picker.index = restored.filter;
            picker.indexed_len = picker.files.len();
            picker.changed = restored.changed;
            picker.changed_bigrams = restored.changed_bigrams;
            picker.key_summary.saturate();
        } else {
            picker.rebuild_index_from_source(&source, restored)?;
        }
        if !loaded_secondary {
            picker
                .modified_order
                .rebuild(&picker.files, &picker.id_bytes);
        } else {
            picker
                .modified_order
                .maybe_compact(&picker.files, &picker.id_bytes);
        }
        source.finish()?;
        let missing = advanced_paths
            .iter()
            .filter(|path| !picker.typed.has_posting(path))
            .cloned()
            .collect::<Vec<_>>();
        if unchecked && !missing.is_empty() {
            return Ok(None);
        }
        picker.build_postings(&missing)?;
        Ok(Some(picker))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn len(&self) -> usize {
        self.sorted.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sorted.is_empty()
    }

    /// Save the compressed normal bigram filter and its exact file-slot table.
    /// Rebuild first if source changes currently live in the overlay.
    pub fn save_bigram_snapshot(&self, path: &Path) -> Result<()> {
        if self.indexed_len != self.files.len() || self.files.iter().any(Option::is_none) {
            return Err(Error::Cache(
                "rebuild the bigram index before saving a snapshot".into(),
            ));
        }
        let files = self
            .files
            .iter()
            .enumerate()
            .map(|(slot, entry)| {
                let entry = entry.as_ref().expect("checked file table");
                let id = self.id(slot).expect("live snapshot slot").to_owned();
                BigramFile {
                    filename: format!("{id}.json"),
                    id,
                    size_bytes: entry.stamp.size,
                    modified_unix_nanos: entry.stamp.modified.clone(),
                    identity: entry.stamp.identity.clone(),
                }
            })
            .collect::<Vec<_>>();
        BigramSnapshot::save_filter_with_changes(
            path,
            &self.root,
            self.recursive,
            &files,
            &self.changed.iter().copied().collect::<Vec<_>>(),
            &self.changed_bigrams,
            &self.index,
        )?;
        self.save_secondary_snapshot(path)
    }

    pub(crate) fn loaded_snapshot(&self) -> bool {
        self.loaded_snapshot
    }

    pub(crate) fn secondary_loaded(&self) -> bool {
        self.secondary_loaded
    }

    pub(crate) fn verified(&self) -> bool {
        self.verified
    }

    pub(crate) fn snapshot_dirty(&self) -> bool {
        self.snapshot_dirty
    }

    pub(crate) fn can_save_bigram_snapshot(&self) -> bool {
        self.indexed_len == self.files.len() && self.files.iter().all(Option::is_some)
    }

    pub(crate) fn sorted_slots(&self) -> &[usize] {
        &self.sorted
    }

    pub(crate) fn view_record(&self, slot: usize) -> (&str, &Stamp) {
        (
            self.id(slot).expect("live view slot"),
            &self.files[slot].as_ref().expect("live view slot").stamp,
        )
    }

    pub(crate) fn typed_scalar_safe(&self, path: &crate::JsonPath) -> bool {
        self.typed.scalar_safe(path)
    }

    pub(crate) fn view_ordered_slots(
        &self,
        field: &str,
        direction: Direction,
    ) -> Option<Vec<usize>> {
        self.view.ordered_slots(field, direction)
    }

    pub(crate) fn view_filter_bits(
        &self,
        field: &str,
        operator: Operator,
        value: &serde_json::Value,
    ) -> Option<Vec<u64>> {
        self.view
            .filter_bits(field, operator, value, self.files.len())
    }

    pub(crate) fn has_array_value(&self, field: &str) -> bool {
        self.array_fields.contains(field)
    }

    pub(crate) fn definitely_lacks_key(&self, required: &[String]) -> bool {
        // The immutable summary may predate refreshed or inserted files.
        self.changed.is_empty()
            && required
                .iter()
                .any(|name| !self.key_summary.might_contain(name))
    }

    fn id(&self, slot: usize) -> Option<&str> {
        let entry = self.files.get(slot)?.as_ref()?;
        let start = entry.id_start as usize;
        let end = start + entry.id_len as usize;
        std::str::from_utf8(&self.id_bytes[start..end]).ok()
    }

    /// The latest revision, including a deletion tombstone for an absent ID.
    pub fn revision(&self, id: &str) -> Option<u64> {
        self.revision_record(id).map(|(revision, _, _)| revision)
    }

    pub(crate) fn revision_record(&self, id: &str) -> Option<(u64, u128, bool)> {
        self.position(id)
            .ok()
            .and_then(|position| self.files[self.sorted[position]].as_ref())
            .map(|entry| (entry.revision, entry.changed_at_nanos, true))
            .or_else(|| {
                self.tombstones
                    .get(id)
                    .map(|&(revision, changed)| (revision, changed, false))
            })
    }

    pub fn git_status(&self) -> Result<Option<BTreeMap<String, GitState>>> {
        git::status(&self.root)
    }

    pub fn index_bytes(&self) -> usize {
        self.index.heap_bytes()
            + self
                .text_index
                .as_ref()
                .map_or(0, |index| index.heap_bytes())
    }

    #[cfg(feature = "index-timings")]
    pub(crate) fn build_timings(&self) -> BuildTimings {
        self.build_timings
    }

    fn position(&self, id: &str) -> std::result::Result<usize, usize> {
        self.sorted.binary_search_by(|&slot| {
            self.id(slot)
                .expect("live sorted slot")
                .as_bytes()
                .cmp(id.as_bytes())
        })
    }

    fn insert(&mut self, id: &str, stamp: Stamp) -> Result<usize> {
        let position = self.position(id).expect_err("inserted ID must be new");
        let revision = self
            .tombstones
            .get(id)
            .map_or(1, |(revision, _)| revision + 1);
        let slot = self.append_slot(id, stamp, revision)?;
        self.tombstones.remove(id);
        self.sorted.insert(position, slot);
        self.changed.insert(slot);
        self.modified_order.touch(slot, &self.files, &self.id_bytes);
        Ok(slot)
    }

    fn append_slot(&mut self, id: &str, stamp: Stamp, revision: u64) -> Result<usize> {
        let start = u32::try_from(self.id_bytes.len())
            .map_err(|_| Error::Cache("file picker ID arena exceeds 4 GiB".into()))?;
        let len = u32::try_from(id.len())
            .map_err(|_| Error::Cache("file picker ID exceeds 4 GiB".into()))?;
        let _ = start
            .checked_add(len)
            .ok_or_else(|| Error::Cache("file picker ID arena exceeds 4 GiB".into()))?;
        self.id_bytes.extend_from_slice(id.as_bytes());
        let slot = self.files.len();
        self.files.push(Some(FileEntry {
            id_start: start,
            id_len: len,
            stamp,
            revision,
            changed_at_nanos: now_nanos(),
        }));
        Ok(slot)
    }
}


fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.as_nanos())
}

#[cfg(feature = "index-timings")]
fn elapsed_nanos(started: Instant) -> u64 {
    started.elapsed().as_nanos().min(u64::MAX as u128) as u64
}

#[cfg(test)]
#[path = "test.rs"]
mod tests;
