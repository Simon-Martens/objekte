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
#[cfg(feature = "index-timings")]
mod profile;
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
    BigramEncoding, BigramFileRef, BigramFilter, BigramIndexBuilder, BigramSnapshot,
    CompactSnapshot, PostingChunkAccumulator, SnapshotFiles, extract_bigrams_with_encoding,
};
use crate::{
    Direction, Error, Field, IndexField, Result,
    error::io,
    query::Operator,
    source::{Source, Stamp},
};
use rayon::prelude::*;
use std::{
    borrow::Cow,
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
pub(crate) use candidates::SlotSelection;
pub(crate) use exact::ExactSelection;
pub use git::GitState;
pub(crate) use infer::ReferenceInventory;
use key_summary::KeySummary;
use modified_order::ModifiedOrder;
use posting::{ChunkRows, PostingRows, merge_chunk};
use presence::PresenceIndex;
#[cfg(feature = "index-timings")]
pub use profile::{BuildTimings, FieldIndexSize, IndexSizes, RestoreTimings, StageMeasurement};
#[cfg(feature = "index-timings")]
use profile::{StageTimer, WorkerStages};
use projection::{
    ProjectionOptions, ProjectionOutcome, projected_values_with_text,
    projected_values_with_text_buffer_and_sort,
};
use sort_index::SortIndex;
use text::{TextIndex, TextIndexBuilders, project_malformed_text_into};
use typed::{TypedIndex, TypedKey};
use view::ViewIndex;

fn sorted_bigrams(bytes: &[u8], encoding: BigramEncoding) -> Vec<u16> {
    let mut keys = extract_bigrams_with_encoding(bytes, encoding);
    keys.sort_unstable();
    keys
}


/// Stable slot metadata for one source JSON file.
#[derive(Debug, Clone)]
pub(crate) struct FileEntry {
    pub(crate) id_start: u32,
    pub(crate) id_len: u32,
    pub(crate) stamp: Stamp,
    pub(crate) revision: u64,
    pub(crate) changed_at_nanos: u128,
}


/// A saved filter plus overlay state awaiting source reconciliation.
struct RestoredBigram {
    filter: BigramFilter,
    changed: BTreeSet<usize>,
    changed_bigrams: BTreeMap<usize, Vec<u16>>,
    newly_changed: BTreeSet<usize>,
    dirty: bool,
}


enum CacheSnapshot {
    Public(BigramSnapshot),
    Compact(CompactSnapshot),
}


/// Stable numeric file slots plus a separate byte-sorted view of live IDs.
/// Changed/new slots bypass the immutable bigram index until it is rebuilt.
pub struct FilePicker {
    root: PathBuf,
    recursive: bool,
    encoding: BigramEncoding,
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
    text_paths: Vec<crate::JsonPath>,
    eager_text_index: bool,
    text_index: Option<TextIndex>,
    presence: PresenceIndex,
    typed: TypedIndex,
    advanced_paths: Vec<crate::JsonPath>,
    inferred_paths: BTreeSet<crate::JsonPath>,
    view: ViewIndex,
    sort: SortIndex,
    #[cfg(feature = "index-timings")]
    build_timings: BuildTimings,
    #[cfg(feature = "index-timings")]
    restore_timings: RestoreTimings,
}


impl SnapshotFiles for FilePicker {
    fn len(&self) -> usize {
        self.files.len()
    }

    fn entry(&self, slot: usize) -> BigramFileRef<'_> {
        let entry = self.files[slot].as_ref().expect("checked snapshot slot");
        BigramFileRef {
            id: self.id(slot).expect("validated snapshot ID"),
            size_bytes: entry.stamp.size,
            modified_unix_nanos: &entry.stamp.modified,
            identity: &entry.stamp.identity,
            filename_valid: true,
        }
    }

    fn id_arena(&self) -> Result<Cow<'_, [u8]>> {
        Ok(Cow::Borrowed(&self.id_bytes))
    }
}


impl FilePicker {
    pub fn open(root: &Path) -> Result<Self> {
        Self::open_with_encoding(root, BigramEncoding::Ascii)
    }

    pub fn open_with_encoding(root: &Path, encoding: BigramEncoding) -> Result<Self> {
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
            encoding,
        )
    }

    /// Profile a fresh collection build without consulting repository caches.
    #[cfg(feature = "index-timings")]
    pub fn profile_collection(
        collection: &crate::Collection,
        encoding: BigramEncoding,
    ) -> Result<Self> {
        Self::open_with_indexes(
            &collection.path,
            collection.recursive,
            &collection.indexes,
            &collection.advanced_index_paths,
            &collection.sort_indexes,
            &collection.fields,
            collection
                .schema
                .as_ref()
                .and_then(|schema| schema.field.language.as_deref()),
            None,
            None,
            collection.bigram_encoding.unwrap_or(encoding),
        )
    }

    /// Time complete early restoration of a profile's base and secondary snapshot.
    #[cfg(feature = "index-timings")]
    pub fn profile_restore_collection(
        collection: &crate::Collection,
        encoding: BigramEncoding,
        snapshot_path: &Path,
    ) -> Result<Self> {
        let _ = BigramFilter::take_sparse_validation_nanos();
        let started = Instant::now();
        let (snapshot, load) = BigramSnapshot::load_profiled(snapshot_path)?;
        let mut picker = Self::open_cached_unchecked(
            &collection.path,
            collection.recursive,
            &collection.indexes,
            &collection.advanced_index_paths,
            &collection.sort_indexes,
            &collection.fields,
            collection
                .schema
                .as_ref()
                .and_then(|schema| schema.field.language.as_deref()),
            snapshot,
            snapshot_path,
            collection.bigram_encoding.unwrap_or(encoding),
        )?
        .ok_or_else(|| Error::Cache("profile snapshot could not be restored".into()))?;
        picker.restore_timings.base_checksum_nanos = load.base_checksum_nanos;
        picker.restore_timings.base_decode_checksum_nanos = load.base_decode_checksum_nanos;
        picker.restore_timings.manifest_decode_nanos = load.manifest_decode_nanos;
        picker.restore_timings.base_filter_decode_nanos = load.base_filter_decode_nanos;
        picker.restore_timings.manifest_validation_nanos = load.manifest_validation_nanos;
        picker.restore_timings.snapshot_bytes_read += load.bytes_read;
        picker.restore_timings.posting_columns += load.posting_columns;
        picker.restore_timings.sparse_validation_nanos =
            BigramFilter::take_sparse_validation_nanos();
        picker.restore_timings.restore_total_nanos = elapsed_nanos(started);
        Ok(picker)
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
        encoding: BigramEncoding,
    ) -> Result<Self> {
        Self::open_inner(
            root,
            recursive,
            indexes,
            advanced_paths,
            sort_indexes,
            definitions,
            language,
            (snapshot.map(CacheSnapshot::Public), snapshot_path),
            false,
            encoding,
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
        snapshot: CompactSnapshot,
        snapshot_path: &Path,
        encoding: BigramEncoding,
    ) -> Result<Option<Self>> {
        Self::open_inner(
            root,
            recursive,
            indexes,
            advanced_paths,
            sort_indexes,
            definitions,
            language,
            (Some(CacheSnapshot::Compact(snapshot)), Some(snapshot_path)),
            true,
            encoding,
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
        cached: (Option<CacheSnapshot>, Option<&Path>),
        unchecked: bool,
        encoding: BigramEncoding,
    ) -> Result<Option<Self>> {
        let (mut snapshot, snapshot_path) = cached;
        let scalar_snapshot = snapshot.as_ref().is_some_and(|saved| match saved {
            CacheSnapshot::Public(saved) => saved.uses_scalar_postings(),
            CacheSnapshot::Compact(saved) => saved.uses_scalar_postings(),
        });
        if snapshot.as_ref().is_some_and(|saved| {
            let filter = match saved {
                CacheSnapshot::Public(saved) => &saved.filter,
                CacheSnapshot::Compact(saved) => &saved.filter,
            };
            filter.encoding() != encoding || (!indexes.is_empty() && !scalar_snapshot)
        }) {
            if unchecked {
                return Ok(None);
            }
            snapshot = None;
        }
        let combined_build = !unchecked && snapshot.is_none();
        let root = std::fs::canonicalize(root).map_err(|e| io(root, e))?;
        let source = Source::open_scoped(&root, recursive)?;
        #[cfg(feature = "index-timings")]
        let inventory_started = Instant::now();
        let inventory: Vec<(String, Stamp)> = if unchecked {
            Vec::new()
        } else if combined_build {
            source
                .inventory_ids()?
                .into_iter()
                .map(|id| {
                    (
                        id,
                        Stamp {
                            size: 0,
                            modified: String::new(),
                            identity: String::new(),
                        },
                    )
                })
                .collect()
        } else {
            source.inventory()?.into_iter().collect()
        };
        let (compact_files, restored) = if unchecked {
            let CacheSnapshot::Compact(saved) = snapshot
                .take()
                .expect("unchecked restore has a compact snapshot")
            else {
                unreachable!("unchecked restore always uses compact snapshots");
            };
            let restored = RestoredBigram {
                filter: saved.filter,
                changed: saved.changed_slots.into_iter().collect(),
                changed_bigrams: saved.changed_bigrams,
                newly_changed: BTreeSet::new(),
                dirty: false,
            };
            (Some((saved.id_bytes, saved.entries)), Some(restored))
        } else {
            let restored = snapshot.and_then(|snapshot| {
                let CacheSnapshot::Public(snapshot) = snapshot else {
                    unreachable!("checked restore uses public snapshots");
                };
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
            });
            (None, restored)
        };
        #[cfg(feature = "index-timings")]
        let inventory_nanos = elapsed_nanos(inventory_started);
        let mut indexed_fields = indexes
            .iter()
            .map(|field| field.name.clone())
            .collect::<Vec<_>>();
        indexed_fields.sort();
        indexed_fields.dedup();
        let mut text_rules = TextRules::new(definitions);
        let mut text_paths = text_rules.registered_paths();
        text_paths.sort();
        text_paths.dedup();
        text_rules.compile_index_fields(&text_paths);
        let eager_text_index =
            text_rules.has_markup() || text_rules.has_exclusions() || !text_paths.is_empty();
        let mut picker = Self {
            root,
            recursive,
            encoding,
            id_bytes: Vec::new(),
            files: Vec::with_capacity(
                compact_files
                    .as_ref()
                    .map_or(inventory.len(), |(_, e)| e.len()),
            ),
            sorted: Vec::with_capacity(
                compact_files
                    .as_ref()
                    .map_or(inventory.len(), |(_, e)| e.len()),
            ),
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
                encoding,
            )?,
            changed: BTreeSet::new(),
            changed_bigrams: BTreeMap::new(),
            malformed: BTreeSet::new(),
            array_fields: BTreeSet::new(),
            key_summary: KeySummary::default(),
            index: BigramIndexBuilder::new_with_encoding(0, encoding).compress(None),
            text_rules,
            text_paths,
            eager_text_index,
            text_index: None,
            presence: PresenceIndex::new(definitions),
            view: ViewIndex::new(&indexed_fields, definitions),
            sort: SortIndex::new(sort_indexes, definitions, language)?,
            typed: TypedIndex::new(indexed_fields),
            advanced_paths: advanced_paths.to_vec(),
            inferred_paths: BTreeSet::new(),
            #[cfg(feature = "index-timings")]
            build_timings: BuildTimings {
                inventory_nanos: if unchecked { 0 } else { inventory_nanos },
                ..BuildTimings::default()
            },
            #[cfg(feature = "index-timings")]
            restore_timings: RestoreTimings::default(),
        };
        #[cfg(feature = "index-timings")]
        let slots_started = Instant::now();
        // Source::inventory is already ordered by ID. Initial slots can be
        // appended in final order without entering the changed-file overlay.
        if let Some((id_bytes, entries)) = compact_files {
            picker.id_bytes = id_bytes;
            picker.files = entries;
            picker.sorted.extend(0..picker.files.len());
        } else {
            for (id, stamp) in inventory {
                let slot = picker.append_slot(&id, stamp, 1)?;
                picker.sorted.push(slot);
            }
        }

        #[cfg(feature = "index-timings")]
        {
            picker.build_timings.slots_nanos = elapsed_nanos(slots_started);
            if unchecked {
                picker.restore_timings.slot_restore_nanos =
                    inventory_nanos.saturating_add(picker.build_timings.slots_nanos);
                picker.restore_timings.restored_files = picker.files.len();
            }
        }
        let loaded_secondary = restored.is_some()
            && (!unchecked || !definitions.is_empty())
            && snapshot_path
                .is_some_and(|path| picker.load_secondary_snapshot(path).unwrap_or(false));
        if (scalar_snapshot || !picker.text_paths.is_empty()) && !loaded_secondary {
            // Without the companion's configuration fingerprint, an older
            // base filter may omit fields that are no longer typed-indexed.
            if unchecked {
                source.finish()?;
                return Ok(None);
            }
            picker.loaded_snapshot = false;
            picker.snapshot_dirty = false;
        }
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
            picker.rebuild_index_from_source_mode(
                &source,
                if (scalar_snapshot || !picker.text_paths.is_empty()) && !loaded_secondary {
                    None
                } else {
                    restored
                },
                combined_build,
            )?;
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
        BigramSnapshot::save_filter_with_changes_borrowed(
            path,
            &self.root,
            self.recursive,
            self,
            &self.changed.iter().copied().collect::<Vec<_>>(),
            &self.changed_bigrams,
            &self.index,
            !self.typed.fields().is_empty(),
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

    pub(crate) fn slot_count(&self) -> usize {
        self.files.len()
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

    pub(crate) fn typed_fields(&self) -> &[String] {
        self.typed.fields()
    }

    pub(crate) fn view_ordered_slots(
        &self,
        field: &str,
        direction: Direction,
    ) -> Option<view::ViewOrder<'_>> {
        self.view.ordered_slots(field, direction)
    }

    pub(crate) fn view_id_order(&self, direction: Direction) -> view::ViewOrder<'_> {
        view::ViewOrder::ids(&self.sorted, direction)
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
    pub fn profile_index_sizes(&self) -> IndexSizes {
        let mut sizes = IndexSizes {
            main_bytes: self.index.heap_bytes(),
            rendered_global_bytes: 0,
            fields: Vec::new(),
        };
        if let Some(text) = &self.text_index {
            let (rendered, _, fields, _) = text.filters();
            sizes.rendered_global_bytes = rendered.heap_bytes();
            sizes.fields = fields
                .iter()
                .map(|field| FieldIndexSize {
                    path: crate::json_filter::format_path(&field.path),
                    normal_bytes: field.normal.heap_bytes(),
                    rendered_bytes: field.clean.heap_bytes(),
                })
                .collect();
        }
        sizes
    }

    #[cfg(feature = "index-timings")]
    pub fn build_timings(&self) -> BuildTimings {
        self.build_timings
    }

    #[cfg(feature = "index-timings")]
    pub fn restore_timings(&self) -> RestoreTimings {
        self.restore_timings
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


pub(crate) fn now_nanos() -> u128 {
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
