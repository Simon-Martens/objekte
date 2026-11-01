//! A small JSON-only file picker and values-only content index.
//!
//! The layout follows FFF's separation of a stable file table, an immutable
//! bigram index, and a changed-file overlay. Relative record IDs are stored
//! once in a byte arena instead of FFF's general chunked path store.

mod exact;
mod git;
mod infer;
mod key_summary;
mod posting;
mod presence;
mod projection;
mod regex_plan;
mod scan_pool;
mod secondary;
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
use presence::PresenceIndex;
use projection::{
    ProjectionOptions, ProjectionOutcome, projected_values, projected_values_with_text,
};
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
    #[cfg(feature = "index-timings")]
    build_timings: BuildTimings,
}

impl FilePicker {
    pub fn open(root: &Path) -> Result<Self> {
        Self::open_with_indexes(root, false, &[], &[], &BTreeMap::new(), None, None)
    }

    pub(crate) fn open_with_indexes(
        root: &Path,
        recursive: bool,
        indexes: &[IndexField],
        advanced_paths: &[crate::JsonPath],
        definitions: &BTreeMap<String, Field>,
        snapshot: Option<BigramSnapshot>,
        snapshot_path: Option<&Path>,
    ) -> Result<Self> {
        Self::open_inner(
            root,
            recursive,
            indexes,
            advanced_paths,
            definitions,
            (snapshot, snapshot_path),
            false,
        )?
        .ok_or_else(|| Error::Cache("verified picker could not be opened".into()))
    }

    pub(crate) fn open_cached_unchecked(
        root: &Path,
        recursive: bool,
        indexes: &[IndexField],
        advanced_paths: &[crate::JsonPath],
        definitions: &BTreeMap<String, Field>,
        snapshot: BigramSnapshot,
        snapshot_path: &Path,
    ) -> Result<Option<Self>> {
        Self::open_inner(
            root,
            recursive,
            indexes,
            advanced_paths,
            definitions,
            (Some(snapshot), Some(snapshot_path)),
            true,
        )
    }

    fn open_inner(
        root: &Path,
        recursive: bool,
        indexes: &[IndexField],
        advanced_paths: &[crate::JsonPath],
        definitions: &BTreeMap<String, Field>,
        cached: (Option<BigramSnapshot>, Option<&Path>),
        unchecked: bool,
    ) -> Result<Option<Self>> {
        let (snapshot, snapshot_path) = cached;
        let root = std::fs::canonicalize(root).map_err(|e| io(root, e))?;
        let source = Source::open_scoped(&root, recursive)?;
        #[cfg(feature = "index-timings")]
        let inventory_started = Instant::now();
        let inventory = if unchecked {
            snapshot
                .as_ref()
                .expect("unchecked restore has a snapshot")
                .files
                .iter()
                .map(|file| {
                    (
                        file.id.clone(),
                        Stamp {
                            size: file.size_bytes,
                            modified: file.modified_unix_nanos.clone(),
                            identity: file.identity.clone(),
                        },
                    )
                })
                .collect()
        } else {
            source.inventory()?
        };
        let restored = snapshot.and_then(|snapshot| {
            if snapshot.files.len() != inventory.len()
                || !snapshot
                    .files
                    .iter()
                    .zip(inventory.iter())
                    .all(|(file, (id, _))| file.id == *id && file.filename == format!("{id}.json"))
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
        });
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
            tombstones: BTreeMap::new(),
            indexed_len: 0,
            loaded_snapshot: restored.is_some(),
            secondary_loaded: false,
            verified: !unchecked,
            snapshot_dirty: restored.as_ref().is_some_and(|restored| restored.dirty),
            config_digest: secondary::config_digest(indexes, advanced_paths, definitions)?,
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

    fn update_restored_slot(&mut self, source: &Source, slot: usize) -> Result<()> {
        let id = self.id(slot).expect("restored slot").to_owned();
        let stamp = self.files[slot]
            .as_ref()
            .expect("restored file")
            .stamp
            .clone();
        let projection = projected_values(
            source,
            &id,
            &stamp,
            &[],
            self.presence.fields(),
            &self.view,
            true,
        )?;
        let keys = projection
            .as_ref()
            .map_or_else(|| self.view.missing_keys(), |p| p.view_keys.clone());
        if projection.is_none() {
            self.malformed.insert(slot);
            self.changed_bigrams.remove(&slot);
        } else {
            self.malformed.remove(&slot);
        }
        self.presence.update(
            slot,
            projection
                .as_ref()
                .map_or(&[], |p| p.present_fields.as_slice()),
        );
        if let Some(projection) = projection.as_ref() {
            self.array_fields
                .extend(projection.array_fields.iter().cloned());
            self.changed_bigrams
                .insert(slot, sorted_bigrams(&projection.bytes));
        }
        self.key_summary.saturate();
        let entry = self.files[slot].as_mut().expect("restored file");
        entry.revision += 1;
        entry.changed_at_nanos = now_nanos();
        self.view.update(slot, keys, &self.files, &self.id_bytes);
        Ok(())
    }

    /// Refreshes inventory from the authoritative directory. A few edits only
    /// enter the overlay; they never force a full bigram rebuild.
    pub fn refresh(&mut self) -> Result<usize> {
        let source = Source::open_scoped(&self.root, self.recursive)?;
        let inventory = source.inventory()?;
        let changed = self.refresh_from_inventory(&source, inventory)?;
        source.finish()?;
        self.verified = true;
        Ok(changed)
    }

    pub(crate) fn reconcile_inventory(
        &mut self,
        inventory: BTreeMap<String, Stamp>,
    ) -> Result<usize> {
        let source = Source::open_scoped(&self.root, self.recursive)?;
        let changed = self.refresh_from_inventory(&source, inventory)?;
        source.finish()?;
        self.verified = true;
        Ok(changed)
    }

    fn refresh_from_inventory(
        &mut self,
        source: &Source,
        inventory: BTreeMap<String, Stamp>,
    ) -> Result<usize> {
        let mut seen = vec![false; self.files.len()];
        let mut changed = 0;
        for (id, stamp) in inventory {
            match self.position(&id) {
                Ok(position) => {
                    let slot = self.sorted[position];
                    seen[slot] = true;
                    if self.files[slot].as_ref().expect("live sorted slot").stamp != stamp {
                        let projection = projected_values(
                            source,
                            &id,
                            &stamp,
                            &[],
                            self.presence.fields(),
                            &self.view,
                            true,
                        )?;
                        let keys = projection
                            .as_ref()
                            .map_or_else(|| self.view.missing_keys(), |p| p.view_keys.clone());
                        if projection.is_none() {
                            self.malformed.insert(slot);
                        } else {
                            self.malformed.remove(&slot);
                        }
                        self.presence.update(
                            slot,
                            projection
                                .as_ref()
                                .map_or(&[], |p| p.present_fields.as_slice()),
                        );
                        if let Some(projection) = projection.as_ref() {
                            self.array_fields
                                .extend(projection.array_fields.iter().cloned());
                        }
                        let entry = self.files[slot].as_mut().expect("live sorted slot");
                        entry.stamp = stamp;
                        entry.revision += 1;
                        entry.changed_at_nanos = now_nanos();
                        self.changed.insert(slot);
                        if let Some(projection) = projection.as_ref() {
                            self.changed_bigrams
                                .insert(slot, sorted_bigrams(&projection.bytes));
                        } else {
                            self.changed_bigrams.remove(&slot);
                        }
                        self.view.update(slot, keys, &self.files, &self.id_bytes);
                        changed += 1;
                    }
                }
                Err(_) => {
                    let projection = projected_values(
                        source,
                        &id,
                        &stamp,
                        &[],
                        self.presence.fields(),
                        &self.view,
                        true,
                    )?;
                    let keys = projection
                        .as_ref()
                        .map_or_else(|| self.view.missing_keys(), |p| p.view_keys.clone());
                    if let Some(projection) = projection.as_ref() {
                        self.array_fields
                            .extend(projection.array_fields.iter().cloned());
                    }
                    let slot = self.insert(&id, stamp)?;
                    if projection.is_none() {
                        self.malformed.insert(slot);
                    }
                    self.presence.update(
                        slot,
                        projection
                            .as_ref()
                            .map_or(&[], |p| p.present_fields.as_slice()),
                    );
                    self.view.update(slot, keys, &self.files, &self.id_bytes);
                    seen.push(true);
                    debug_assert_eq!(slot + 1, seen.len());
                    changed += 1;
                }
            }
        }
        for (slot, present) in seen.iter().enumerate() {
            if *present {
                continue;
            }
            if let Some(entry) = self.files[slot].take() {
                let start = entry.id_start as usize;
                let end = start + entry.id_len as usize;
                let id = std::str::from_utf8(&self.id_bytes[start..end])
                    .expect("validated ID")
                    .to_owned();
                self.tombstones
                    .insert(id, (entry.revision + 1, now_nanos()));
                self.changed.remove(&slot);
                self.changed_bigrams.remove(&slot);
                self.malformed.remove(&slot);
                self.presence.remove(slot);
                self.view.remove(slot);
                changed += 1;
            }
        }
        self.sorted.retain(|&slot| seen[slot]);
        if changed > 0 {
            self.key_summary.saturate();
            self.typed.invalidate_dynamic();
        }
        Ok(changed)
    }

    /// Updates a single path after an application edit or a future watcher
    /// event, without enumerating the directory.
    pub fn refresh_id(&mut self, id: &str) -> Result<()> {
        crate::repository::validate_id(id)?;
        self.key_summary.saturate();
        self.typed.invalidate_dynamic();
        let source = Source::open_scoped(&self.root, self.recursive)?;
        let file = match source.file(id) {
            Ok(file) => Some(file),
            Err(Error::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        match file {
            Some(file) => {
                let path = self.root.join(format!("{id}.json"));
                let stamp =
                    Stamp::from_metadata(&file.metadata().map_err(|e| io(&path, e))?, &path)?;
                match self.position(id) {
                    Ok(position) => {
                        let slot = self.sorted[position];
                        if self.files[slot].as_ref().expect("live sorted slot").stamp != stamp {
                            let projection = projected_values(
                                &source,
                                id,
                                &stamp,
                                &[],
                                self.presence.fields(),
                                &self.view,
                                true,
                            )?;
                            let keys = projection
                                .as_ref()
                                .map_or_else(|| self.view.missing_keys(), |p| p.view_keys.clone());
                            if projection.is_none() {
                                self.malformed.insert(slot);
                            } else {
                                self.malformed.remove(&slot);
                            }
                            self.presence.update(
                                slot,
                                projection
                                    .as_ref()
                                    .map_or(&[], |p| p.present_fields.as_slice()),
                            );
                            if let Some(projection) = projection.as_ref() {
                                self.array_fields
                                    .extend(projection.array_fields.iter().cloned());
                            }
                            let entry = self.files[slot].as_mut().expect("live sorted slot");
                            entry.stamp = stamp;
                            entry.revision += 1;
                            entry.changed_at_nanos = now_nanos();
                            self.changed.insert(slot);
                            if let Some(projection) = projection.as_ref() {
                                self.changed_bigrams
                                    .insert(slot, sorted_bigrams(&projection.bytes));
                            } else {
                                self.changed_bigrams.remove(&slot);
                            }
                            self.view.update(slot, keys, &self.files, &self.id_bytes);
                        }
                    }
                    Err(_) => {
                        let projection = projected_values(
                            &source,
                            id,
                            &stamp,
                            &[],
                            self.presence.fields(),
                            &self.view,
                            true,
                        )?;
                        let keys = projection
                            .as_ref()
                            .map_or_else(|| self.view.missing_keys(), |p| p.view_keys.clone());
                        if let Some(projection) = projection.as_ref() {
                            self.array_fields
                                .extend(projection.array_fields.iter().cloned());
                        }
                        let slot = self.insert(id, stamp)?;
                        if projection.is_none() {
                            self.malformed.insert(slot);
                        }
                        self.presence.update(
                            slot,
                            projection
                                .as_ref()
                                .map_or(&[], |p| p.present_fields.as_slice()),
                        );
                        self.view.update(slot, keys, &self.files, &self.id_bytes);
                    }
                }
            }
            None => {
                if let Ok(position) = self.position(id) {
                    let slot = self.sorted.remove(position);
                    if let Some(entry) = self.files[slot].take() {
                        self.tombstones
                            .insert(id.to_owned(), (entry.revision + 1, now_nanos()));
                    }
                    self.changed.remove(&slot);
                    self.changed_bigrams.remove(&slot);
                    self.malformed.remove(&slot);
                    self.presence.remove(slot);
                    self.view.remove(slot);
                }
            }
        }
        source.finish()?;
        Ok(())
    }

    /// Conservative candidates for an OR of necessary literal fragments.
    /// `None` means no safe prefilter; a missing posting also means scan all.
    fn changed_matches_literal(&self, slot: usize, literal: &str) -> bool {
        let Some(keys) = self.changed_bigrams.get(&slot) else {
            return true;
        };
        let required = extract_bigrams(literal.as_bytes());
        required.iter().all(|key| keys.binary_search(key).is_ok())
    }

    fn candidates(&self, literals: Option<&[String]>) -> Vec<usize> {
        let Some(literals) = literals.filter(|terms| !terms.is_empty()) else {
            return self.sorted.clone();
        };
        let mut bitsets = Vec::with_capacity(literals.len());
        for literal in literals {
            let Some(bits) = self.index.query(literal.as_bytes()) else {
                return self.sorted.clone();
            };
            bitsets.push(bits);
        }
        self.sorted
            .iter()
            .copied()
            .filter(|&slot| {
                if slot >= self.indexed_len {
                    true
                } else if self.changed.contains(&slot) {
                    literals
                        .iter()
                        .any(|literal| self.changed_matches_literal(slot, literal))
                } else {
                    bitsets
                        .iter()
                        .any(|bits| BigramFilter::is_candidate(bits, slot))
                }
            })
            .collect()
    }

    /// Returns byte-sorted candidate IDs for an OR of necessary literal
    /// fragments. The result must still be parsed and checked against the
    /// complete filter. `None` conservatively returns every file.
    pub fn candidate_ids(&self, literals: Option<&[String]>) -> Vec<&str> {
        self.candidates(literals)
            .into_iter()
            .map(|slot| self.id(slot).expect("live candidate slot"))
            .collect()
    }

    /// Candidate IDs for a regex over decoded scalar values. FFF's regex HIR
    /// planner narrows the immutable index; changed files bypass it. This is
    /// only a prefilter and may include false positives.
    pub fn regex_candidate_ids(&self, pattern: &str) -> Vec<&str> {
        self.regex_candidate_slots(pattern)
            .into_iter()
            .map(|slot| self.id(slot).expect("live candidate slot"))
            .collect()
    }

    fn regex_candidate_slots(&self, pattern: &str) -> Vec<usize> {
        let bits = regex_plan::candidates(&self.index, pattern);
        let overlay = regex_plan::OverlayQuery::new(pattern);
        self.sorted
            .iter()
            .copied()
            .filter(|&slot| {
                if slot >= self.indexed_len {
                    true
                } else if self.changed.contains(&slot) {
                    self.changed_bigrams
                        .get(&slot)
                        .is_none_or(|keys| overlay.matches(keys))
                } else {
                    bits.as_ref()
                        .is_none_or(|bits| BigramFilter::is_candidate(bits, slot))
                }
            })
            .collect()
    }

    pub(crate) fn candidate_records(
        &self,
        pattern: Option<&str>,
        typed_plan: Option<&crate::json_filter::TypedPlan>,
        required_fields: &[String],
    ) -> Vec<(&str, &Stamp)> {
        let bits = pattern.and_then(|regex| regex_plan::candidates(&self.index, regex));
        let overlay = pattern.map(regex_plan::OverlayQuery::new);
        let typed_bits = self
            .typed
            .candidate_bits(typed_plan, self.files.len(), &self.changed);
        let presence_bits = self.presence.candidates(required_fields);
        let count_bits =
            |bits: &[u64]| -> usize { bits.iter().map(|word| word.count_ones() as usize).sum() };
        let mut candidate_bits = None;
        let mut needs_overlay = false;
        let mut smallest = usize::MAX;
        for (next, overlay) in [
            (bits.as_deref(), true),
            (typed_bits.as_deref(), false),
            (presence_bits.as_deref(), false),
        ] {
            if let Some(next) = next {
                let count =
                    count_bits(next).saturating_add(if overlay { self.changed.len() } else { 0 });
                if count < smallest {
                    candidate_bits = Some(next);
                    needs_overlay = overlay;
                    smallest = count;
                }
            }
        }
        let selected = candidate_bits.and_then(|candidate_bits| {
            // Sorting a dense candidate set costs more than checking the
            // already sorted ID table. Enumerate set bits only when selective.
            if smallest.saturating_mul(8) >= self.sorted.len() {
                return None;
            }
            let mut slots = Vec::new();
            for (word_index, &word) in candidate_bits.iter().enumerate() {
                let mut remaining = word;
                while remaining != 0 {
                    let slot = word_index * 64 + remaining.trailing_zeros() as usize;
                    remaining &= remaining - 1;
                    if self.files.get(slot).is_some_and(Option::is_some) {
                        slots.push(slot);
                    }
                }
            }
            if needs_overlay {
                // New and changed files bypass immutable text postings. They
                // may already have an old posting, so deduplicate below.
                slots.extend(
                    self.changed
                        .iter()
                        .copied()
                        .filter(|&slot| self.files[slot].is_some()),
                );
            }
            // Slots follow index insertion order; result rows follow byte ID
            // order even after refresh appends new slots to the overlay.
            slots.sort_unstable_by(|&left, &right| {
                self.id(left)
                    .expect("live candidate slot")
                    .as_bytes()
                    .cmp(self.id(right).expect("live candidate slot").as_bytes())
            });
            slots.dedup();
            Some(slots)
        });
        selected
            .as_deref()
            .unwrap_or(&self.sorted)
            .iter()
            .copied()
            .filter(|&slot| {
                let text_match = if slot >= self.indexed_len {
                    true
                } else if self.changed.contains(&slot) {
                    self.changed_bigrams
                        .get(&slot)
                        .is_none_or(|keys| overlay.as_ref().is_none_or(|query| query.matches(keys)))
                } else {
                    bits.as_ref()
                        .is_none_or(|bits| BigramFilter::is_candidate(bits, slot))
                };
                let typed_match = typed_bits
                    .as_ref()
                    .is_none_or(|bits| bits[slot / 64] & (1 << (slot % 64)) != 0);
                let presence_match = presence_bits
                    .as_ref()
                    .is_none_or(|bits| bits[slot / 64] & (1 << (slot % 64)) != 0);
                text_match && typed_match && presence_match
            })
            .map(|slot| {
                (
                    self.id(slot).expect("live candidate slot"),
                    &self.files[slot].as_ref().expect("live candidate").stamp,
                )
            })
            .collect()
    }

    /// Complete regex search over decoded values or malformed-file text.
    /// Source bytes are reread for each candidate; the index never decides truth.
    pub fn search_values_regex(&self, pattern: &str) -> Result<Vec<String>> {
        let matcher = regex::bytes::RegexBuilder::new(pattern)
            .unicode(false)
            .build()
            .map_err(|e| Error::Request(format!("invalid values regex: {e}")))?;
        let source = Source::open_scoped(&self.root, self.recursive)?;
        let candidates = self.regex_candidate_ids(pattern);
        let matches = candidates
            .par_iter()
            .map(|&id| -> Result<Option<String>> {
                let slot = self.sorted[self.position(id).expect("candidate is live")];
                let stamp = &self.files[slot].as_ref().expect("live candidate").stamp;
                let text = projected_values_with_text(
                    &source,
                    id,
                    stamp,
                    &[],
                    &[],
                    &self.view,
                    ProjectionOptions {
                        folded_index: false,
                        text_rules: None,
                        key_summary: None,
                    },
                )?;
                let matched = match text {
                    ProjectionOutcome::Valid(text) => matcher.is_match(&text.bytes),
                    ProjectionOutcome::Malformed(text) => matcher.is_match(text.as_bytes()),
                };
                Ok(matched.then(|| id.to_owned()))
            })
            .collect::<Result<Vec<_>>>()?;
        source.finish()?;
        Ok(matches.into_iter().flatten().collect())
    }

    /// Rebuilds the immutable index after many edits. Source stamps are checked
    /// before and after every read so a racing edit cannot publish a bad index.
    pub fn rebuild_index(&mut self) -> Result<()> {
        let source = Source::open_scoped(&self.root, self.recursive)?;
        self.rebuild_index_from_source(&source, None)
    }

    fn rebuild_index_from_source(
        &mut self,
        source: &Source,
        restored: Option<RestoredBigram>,
    ) -> Result<()> {
        // A fresh picker already has compact ID-ordered slots. Later edits can
        // append slots or leave holes, in which case rebuild must compact them.
        #[cfg(feature = "index-timings")]
        let slots_started = Instant::now();
        let already_compact = self.sorted.len() == self.files.len()
            && self
                .sorted
                .iter()
                .enumerate()
                .all(|(position, &slot)| position == slot);
        let compacted = if already_compact {
            None
        } else {
            let mut ids = Vec::with_capacity(self.id_bytes.len());
            let mut files = Vec::with_capacity(self.sorted.len());
            for &slot in &self.sorted {
                let mut entry = self.files[slot].as_ref().expect("live sorted slot").clone();
                let id = self.id(slot).expect("validated ID");
                entry.id_start = u32::try_from(ids.len())
                    .map_err(|_| Error::Cache("file picker ID arena exceeds 4 GiB".into()))?;
                ids.extend_from_slice(id.as_bytes());
                files.push(Some(entry));
            }
            Some((ids, files))
        };
        let (ids, files) = compacted.as_ref().map_or(
            (self.id_bytes.as_slice(), self.files.as_slice()),
            |(ids, files)| (ids.as_slice(), files.as_slice()),
        );
        let main = BigramIndexBuilder::new(files.len());
        let skip = BigramIndexBuilder::new(files.len());
        // Once full-text postings are active, rebuild them with the normal
        // postings so slot compaction never requires a second source pass.
        let text_builders = (self.eager_text_index || self.text_index.is_some())
            .then(|| TextIndexBuilders::new(files.len()));
        let array_fields = std::sync::Mutex::new(BTreeSet::new());
        let key_summary = std::sync::Mutex::new(KeySummary::default());
        let malformed = std::sync::Mutex::new(BTreeSet::new());
        let overlay_bigrams = std::sync::Mutex::new(BTreeMap::new());
        let indexed_fields = self.typed.fields();
        let typed_rows = std::sync::Mutex::new(vec![Vec::new(); indexed_fields.len()]);
        let presence_fields = self.presence.fields();
        let presence_words = self.presence.build_words(files.len());
        let view_rows = std::sync::Mutex::new(Vec::new());
        let view = &self.view;
        #[cfg(feature = "index-timings")]
        {
            self.build_timings.slots_nanos += elapsed_nanos(slots_started);
        }

        #[cfg(feature = "index-timings")]
        let project_worker = AtomicU64::new(0);
        #[cfg(feature = "index-timings")]
        let add_worker = AtomicU64::new(0);
        #[cfg(feature = "index-timings")]
        let scan_started = Instant::now();
        scan_pool::run(|| {
            files
                .par_chunks(64)
                .enumerate()
                .try_for_each(|(chunk_index, chunk)| -> Result<()> {
                    let mut chunk_arrays = BTreeSet::new();
                    let mut chunk_keys = KeySummary::default();
                    let mut chunk_malformed = BTreeSet::new();
                    let mut chunk_typed = vec![Vec::new(); indexed_fields.len()];
                    let mut chunk_view = Vec::new();
                    #[cfg(feature = "index-timings")]
                    let mut chunk_project_nanos = 0;
                    #[cfg(feature = "index-timings")]
                    let mut chunk_add_nanos = 0;
                    for (within, entry) in chunk.iter().enumerate() {
                        let Some(entry) = entry else { continue };
                        let id = std::str::from_utf8(
                            &ids[entry.id_start as usize..(entry.id_start + entry.id_len) as usize],
                        )
                        .expect("validated UTF-8 ID");
                        #[cfg(feature = "index-timings")]
                        let project_started = Instant::now();
                        let projection = projected_values_with_text(
                            source,
                            id,
                            &entry.stamp,
                            indexed_fields,
                            presence_fields,
                            view,
                            ProjectionOptions {
                                folded_index: true,
                                text_rules: text_builders.as_ref().map(|_| &self.text_rules),
                                key_summary: Some(&mut chunk_keys),
                            },
                        )?;
                        #[cfg(feature = "index-timings")]
                        {
                            chunk_project_nanos += elapsed_nanos(project_started);
                        }
                        let slot = chunk_index * 64 + within;
                        #[cfg(feature = "index-timings")]
                        let add_started = Instant::now();
                        match projection {
                            ProjectionOutcome::Malformed(mut text) => {
                                chunk_malformed.insert(slot);
                                if let Some(builders) = &text_builders {
                                    builders.add(slot, &project_malformed_text(&text));
                                }
                                if !text.is_ascii() {
                                    let folded = text.case_fold().collect::<String>();
                                    text.push('\u{1f}');
                                    text.push_str(&folded);
                                }
                                if restored.is_none() {
                                    main.add_file_content(&skip, slot, text.as_bytes());
                                } else if restored
                                    .as_ref()
                                    .is_some_and(|restored| restored.newly_changed.contains(&slot))
                                {
                                    let mut keys = extract_bigrams(text.as_bytes());
                                    keys.sort_unstable();
                                    overlay_bigrams
                                        .lock()
                                        .expect("overlay bigram lock")
                                        .insert(slot, keys);
                                }
                            }
                            ProjectionOutcome::Valid(projection) => {
                                if restored.is_none() {
                                    main.add_file_content(&skip, slot, &projection.bytes);
                                } else if restored
                                    .as_ref()
                                    .is_some_and(|restored| restored.newly_changed.contains(&slot))
                                {
                                    let mut keys = extract_bigrams(&projection.bytes);
                                    keys.sort_unstable();
                                    overlay_bigrams
                                        .lock()
                                        .expect("overlay bigram lock")
                                        .insert(slot, keys);
                                }
                                if let (Some(builders), Some(text)) =
                                    (&text_builders, &projection.text)
                                {
                                    builders.add(slot, text);
                                }

                                chunk_arrays.extend(projection.array_fields);
                                PresenceIndex::add_build_row(
                                    &presence_words,
                                    slot,
                                    &projection.present_fields,
                                );
                                chunk_view.push((slot, projection.view_keys));
                                for (field, key) in projection.typed_values {
                                    chunk_typed[field].push((key, slot));
                                }
                            }
                        }

                        #[cfg(feature = "index-timings")]
                        {
                            chunk_add_nanos += elapsed_nanos(add_started);
                        }
                    }
                    array_fields
                        .lock()
                        .expect("array field lock")
                        .extend(chunk_arrays);
                    key_summary
                        .lock()
                        .expect("key summary lock")
                        .merge(chunk_keys);
                    malformed
                        .lock()
                        .expect("malformed lock")
                        .extend(chunk_malformed);
                    #[cfg(feature = "index-timings")]
                    {
                        project_worker.fetch_add(chunk_project_nanos, Ordering::Relaxed);
                    }

                    #[cfg(feature = "index-timings")]
                    {
                        add_worker.fetch_add(chunk_add_nanos, Ordering::Relaxed);
                    }
                    let mut rows = typed_rows.lock().expect("typed row lock");
                    for (all, chunk) in rows.iter_mut().zip(chunk_typed) {
                        all.extend(chunk);
                    }
                    view_rows.lock().expect("view row lock").extend(chunk_view);
                    Ok(())
                })
        })?;
        #[cfg(feature = "index-timings")]
        {
            self.build_timings.scan_nanos = elapsed_nanos(scan_started);
        }

        #[cfg(feature = "index-timings")]
        {
            self.build_timings.project_worker_nanos = project_worker.load(Ordering::Relaxed);
        }

        #[cfg(feature = "index-timings")]
        {
            self.build_timings.add_worker_nanos = add_worker.load(Ordering::Relaxed);
        }

        #[cfg(feature = "index-timings")]
        let compress_started = Instant::now();
        let (index, changed, mut changed_bigrams) = if let Some(restored) = restored {
            (restored.filter, restored.changed, restored.changed_bigrams)
        } else {
            let mut index = main.compress(None);
            index.set_skip_index(skip.compress(Some(12))); // FFF's current skip-index cutoff.
            (index, BTreeSet::new(), BTreeMap::new())
        };
        changed_bigrams.extend(overlay_bigrams.into_inner().expect("overlay bigram lock"));
        #[cfg(feature = "index-timings")]
        {
            self.build_timings.compress_nanos = elapsed_nanos(compress_started);
        }
        source.finish()?;
        if let Some((ids, files)) = compacted {
            self.id_bytes = ids;
            self.files = files;
            self.sorted = (0..self.files.len()).collect();
        }
        self.index = index;
        self.indexed_len = self.files.len();
        self.changed = changed;
        self.changed_bigrams = changed_bigrams;
        self.malformed = malformed.into_inner().expect("malformed lock");
        self.text_index = text_builders.map(TextIndexBuilders::finish);
        self.array_fields = array_fields.into_inner().expect("array field lock");
        self.key_summary = key_summary.into_inner().expect("key summary lock");
        self.presence.replace(presence_words);
        #[cfg(feature = "index-timings")]
        let typed_started = Instant::now();
        self.typed
            .replace_rows(typed_rows.into_inner().expect("typed row lock"));
        #[cfg(feature = "index-timings")]
        {
            self.build_timings.typed_nanos = elapsed_nanos(typed_started);
        }

        #[cfg(feature = "index-timings")]
        let view_started = Instant::now();
        self.view.replace(
            view_rows.into_inner().expect("view row lock"),
            &self.files,
            &self.id_bytes,
        );
        // Runtime path postings are rebuilt after slot compaction so their
        // posting lists refer to the current immutable file table.
        let runtime_paths = self.typed.dynamic_paths();
        let inferred_paths = self.inferred_paths.clone();
        self.typed.invalidate_dynamic();
        if let Err(error) = self.build_postings(&runtime_paths) {
            // A source edit during this second pass can leave the fresh
            // text snapshot behind. Require source verification next time.
            self.changed.extend(self.sorted.iter().copied());
            return Err(error);
        }
        self.inferred_paths = inferred_paths;

        #[cfg(feature = "index-timings")]
        {
            self.build_timings.view_nanos = elapsed_nanos(view_started);
        }
        Ok(())
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
