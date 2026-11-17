//! Native segment policy, identity, ownership, and borrowed inspection.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::num::NonZeroUsize;
use std::ops::BitOrAssign;
use std::sync::Arc;

use rayon::prelude::*;
use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};

use crate::bigram::BigramIndex;
use crate::inference::{InferenceObservation, InferenceState};
use crate::inventory::{ActiveRecords, RecordEntry};
use crate::lifecycle::IndexRevision;
use crate::lifecycle_catalog::FileCatalog;
use crate::model::{CollectionId, FileId, RecordSlot};
use crate::posting::{CustomPostingIndex, PostingCollection, PostingIndex};
use crate::relation::RelationContribution;
use crate::segment_archive::{FileLookup, RecordLookup, RecordRows, SegmentFiles};
use crate::sorting::SortCollection;

const ACCOUNTING_OVERHEAD: usize = 48;

/// Deterministic soft target used by every ordinary segment-producing path.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SegmentPolicy {
    pub target_records: NonZeroUsize,
    pub max_files: NonZeroUsize,
}

impl Default for SegmentPolicy {
    fn default() -> Self {
        Self {
            target_records: NonZeroUsize::new(16_384).expect("nonzero default"),
            max_files: NonZeroUsize::new(16_384).expect("nonzero default"),
        }
    }
}

impl SegmentPolicy {
    pub(crate) fn validate(self) -> Result<Self, String> {
        let local_limit = u32::MAX as usize;
        if self.target_records.get() > local_limit {
            return Err("segment target exceeds the supported local-slot range".into());
        }
        if self.max_files.get() > local_limit {
            return Err("segment file limit exceeds the supported local-slot range".into());
        }
        Ok(self)
    }

    pub fn with_target_records(target_records: NonZeroUsize) -> Result<Self, String> {
        Self {
            target_records,
            max_files: target_records,
        }
        .validate()
    }

    /// Apply the soft boundary only between completed contributions. An empty
    /// draft always accepts one oversized indivisible contribution; the next
    /// contribution starts a fresh segment.
    pub(crate) fn can_grow(
        self,
        current_records: usize,
        current_files: usize,
        contribution_records: usize,
        contribution_new_files: usize,
    ) -> bool {
        if current_records == 0 && current_files == 0 {
            return true;
        }
        current_records
            .checked_add(contribution_records)
            .is_some_and(|records| records <= self.target_records.get())
            && current_files
                .checked_add(contribution_new_files)
                .is_some_and(|files| files <= self.max_files.get())
    }
}

/// Stable identity for one immutable native segment.
///
/// Segment-local record slots are meaningful only together with this ID and
/// the owning collection.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct SegmentId(u64);

impl SegmentId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for SegmentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Address that remains unambiguous after crossing an immutable segment
/// boundary. Record slots are meaningful only with the collection and segment
/// components of this value.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct SegmentRecordAddress {
    pub collection: CollectionId,
    pub segment: SegmentId,
    pub slot: RecordSlot,
}

/// Compact membership spanning segments without merging local slot domains.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SegmentRecordSet {
    members: BTreeMap<(CollectionId, SegmentId), RoaringBitmap>,
}

impl SegmentRecordSet {
    pub(crate) fn insert(&mut self, address: SegmentRecordAddress) -> bool {
        self.members
            .entry((address.collection, address.segment))
            .or_default()
            .insert(address.slot.0)
    }

    pub(crate) fn len(&self) -> u64 {
        self.members
            .values()
            .fold(0u64, |total, slots| total.saturating_add(slots.len()))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.members.values().all(RoaringBitmap::is_empty)
    }

    pub(crate) fn contains(&self, address: SegmentRecordAddress) -> bool {
        self.members
            .get(&(address.collection, address.segment))
            .is_some_and(|slots| slots.contains(address.slot.0))
    }

    pub(crate) fn local(
        &self,
        collection: CollectionId,
        segment: SegmentId,
    ) -> Option<&RoaringBitmap> {
        self.members.get(&(collection, segment))
    }

    pub(crate) fn insert_local(
        &mut self,
        collection: CollectionId,
        segment: SegmentId,
        slots: RoaringBitmap,
    ) {
        if slots.is_empty() {
            return;
        }
        *self.members.entry((collection, segment)).or_default() |= slots;
    }

    pub(crate) fn intersect(&mut self, other: &Self) {
        for (&(collection, segment), slots) in &mut self.members {
            if let Some(other) = other.members.get(&(collection, segment)) {
                *slots &= other;
            } else {
                slots.clear();
            }
        }
        self.members.retain(|_, slots| !slots.is_empty());
    }

    pub(crate) fn union(&mut self, other: &Self) {
        for (&(collection, segment), slots) in &other.members {
            self.members
                .entry((collection, segment))
                .or_default()
                .bitor_assign(slots);
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = SegmentRecordAddress> + '_ {
        self.members
            .iter()
            .flat_map(|(&(collection, segment), slots)| {
                slots.iter().map(move |slot| SegmentRecordAddress {
                    collection,
                    segment,
                    slot: RecordSlot(slot),
                })
            })
    }
}

/// One physical file in a segment. File IDs remain repository-global; slots
/// are local to the owning segment.
#[derive(Clone)]
pub(crate) struct SegmentFile {
    pub entry: crate::inventory::FileEntry,
    pub records: Vec<RecordSlot>,
}

/// Saturating native retained-size and query-planning estimates.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SegmentAccounting {
    pub retained_bytes_estimated: usize,
    pub source_bytes_estimated: u64,
    pub max_record_range_len: u64,
    pub index_family_bytes: usize,
}

/// Public read-only summary of one native segment for diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct NativeSegmentSummary {
    pub id: SegmentId,
    pub collection: CollectionId,
    pub records: usize,
    pub active_records: usize,
    pub files: usize,
    pub source_bytes: u64,
    pub bigram_stores: usize,
    pub distinct_gram_keys: usize,
    pub bigram_heap_bytes: usize,
    pub index_family_bytes: usize,
    pub active: bool,
}

/// Complete native search payload owned by one immutable segment.
#[derive(Clone)]
pub(crate) struct IndexedCollectionSegment {
    pub id: SegmentId,
    pub collection: CollectionId,
    pub source_id: String,
    pub source_version: u32,
    pub files: SegmentFiles,
    pub file_index: FileLookup,
    pub records: RecordRows,
    pub id_lookup: RecordLookup,
    pub identity_order: Vec<RecordSlot>,
    pub postings: PostingCollection,
    pub(crate) inference_observation: Option<InferenceObservation>,
    pub inference: Option<InferenceState>,
    pub custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    pub raw: Arc<BigramIndex>,
    pub fallback_text: Arc<BigramIndex>,
    pub custom_text: Arc<HashMap<(Vec<String>, String), BigramIndex>>,
    pub logical_records: RoaringBitmap,
    pub empty_array_records: RoaringBitmap,
    pub sorting: Arc<SortCollection>,
    /// Relation source references retain logical target IDs for merged-view
    /// resolution across segment and collection boundaries.
    pub relations: Vec<RelationContribution>,
    pub record_parses: u64,
    pub failed_parses: u64,
    pub accounting: SegmentAccounting,
    /// Set only after the query dictionaries passed completed-storage
    /// finalization. Component mutations clear this before resealing.
    pub(crate) storage_completed: bool,
}

/// One published payload and its independently versioned local liveness.
#[derive(Clone)]
pub(crate) struct PublishedSegment {
    pub id: SegmentId,
    pub index: Arc<IndexedCollectionSegment>,
    pub active: ActiveRecords,
}

impl PublishedSegment {
    pub(crate) fn checked(
        id: SegmentId,
        index: Arc<IndexedCollectionSegment>,
        active: ActiveRecords,
    ) -> Result<Self, String> {
        if id != index.id {
            return Err("published segment descriptor and payload IDs disagree".into());
        }
        if active.slot_count() as usize != index.record_count() {
            return Err("published segment liveness domain is inconsistent".into());
        }
        if !index.storage_completed {
            return Err("published segment did not cross completed-storage validation".into());
        }
        if !index.has_published_storage() {
            return Err("published segment retained a mutable query dictionary".into());
        }
        Ok(Self { id, index, active })
    }
}

/// One payload and liveness pair at its native segment boundary.
#[derive(Clone, Copy)]
pub(crate) struct SegmentReadView<'a> {
    pub id: SegmentId,
    pub collection: CollectionId,
    pub index: &'a IndexedCollectionSegment,
    pub active: &'a ActiveRecords,
}

/// Borrowed view of the metadata, liveness, and segment payloads that belong
/// to one publication. Repository and lifecycle reads expose the same shape.
#[derive(Clone, Copy)]
pub(crate) struct NativeSnapshotRef<'a> {
    pub active: &'a [ActiveRecords],
    pub segments: &'a [Arc<Vec<Arc<PublishedSegment>>>],
    pub catalog: &'a FileCatalog,
    pub revision: IndexRevision,
}

/// Component kind exposed by borrowed inspection and future persistence
/// boundaries. The payload remains a concrete native field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeComponentKind {
    DeclaredPosting,
    InferredPosting,
    CustomPosting,
    RawText,
    FallbackText,
    FieldText,
    Sorting,
    Relations,
}

/// A small borrowed descriptor. It does not clone a posting or bitmap.
pub struct NativeComponentDescriptor<'a> {
    pub collection: CollectionId,
    pub segment: SegmentId,
    pub kind: NativeComponentKind,
    pub path: Option<&'a [String]>,
    pub binding: Option<&'a str>,
}

impl IndexedCollectionSegment {
    /// One completed-storage boundary for every ordinary and maintenance
    /// producer. Existing archives are shared rather than serialized again.
    pub(crate) fn seal_completed_storage(self) -> Result<Self, String> {
        self.seal_completed_storage_inner(None)
    }

    pub(crate) fn seal_completed_storage_in_pool(
        self,
        pool: &rayon::ThreadPool,
    ) -> Result<Self, String> {
        self.seal_completed_storage_inner(Some(pool))
    }

    fn seal_completed_storage_inner(
        mut self,
        pool: Option<&rayon::ThreadPool>,
    ) -> Result<Self, String> {
        if self.storage_completed {
            if !self.has_published_storage() {
                return Err("completed segment lost its published dictionaries".into());
            }
            return Ok(self);
        }
        self.postings.finish_stats();
        for custom in self.custom_postings.values_mut() {
            custom.index.finish_stats();
        }
        if let Some(inference) = &mut self.inference {
            for index in inference.indexes.values_mut() {
                index.finish_stats();
            }
        }
        let freeze_fields =
            |fields: &mut HashMap<Vec<String>, PostingIndex>| -> Result<(), String> {
                match pool {
                    Some(pool) => pool.install(|| {
                        fields
                            .par_iter_mut()
                            .try_for_each(|(_, index)| index.seal_native_storage())
                    }),
                    None => fields
                        .values_mut()
                        .try_for_each(PostingIndex::seal_native_storage),
                }
            };
        freeze_fields(&mut self.postings.fields)?;
        match pool {
            Some(pool) => pool.install(|| {
                self.custom_postings
                    .par_iter_mut()
                    .try_for_each(|(_, custom)| custom.index.seal_native_storage())
            })?,
            None => self
                .custom_postings
                .values_mut()
                .try_for_each(|custom| custom.index.seal_native_storage())?,
        }
        if let Some(inference) = &mut self.inference {
            match pool {
                Some(pool) => pool.install(|| {
                    inference
                        .indexes
                        .par_iter_mut()
                        .try_for_each(|(_, index)| index.seal_native_storage())
                })?,
                None => inference
                    .indexes
                    .values_mut()
                    .try_for_each(PostingIndex::seal_native_storage)?,
            }
        }
        let record_count = self.record_count();
        let logical_count = self.logical_records.len() as usize;
        let logical_domain = (logical_count != record_count).then_some(&self.logical_records);
        match pool {
            Some(pool) => pool.install(|| {
                self.postings.fields.par_iter().try_for_each(|(_, index)| {
                    index.validate_completed(record_count, logical_count, logical_domain)
                })
            })?,
            None => {
                self.postings
                    .validate_completed(record_count, logical_count, logical_domain)?
            }
        }
        match pool {
            Some(pool) => pool.install(|| {
                self.custom_postings.par_iter().try_for_each(|(_, custom)| {
                    custom
                        .index
                        .validate_completed(record_count, logical_count, logical_domain)
                })
            })?,
            None => {
                for custom in self.custom_postings.values() {
                    custom
                        .index
                        .validate_completed(record_count, logical_count, logical_domain)?;
                }
            }
        }
        if let Some(inference) = &self.inference {
            match pool {
                Some(pool) => pool.install(|| {
                    inference.indexes.par_iter().try_for_each(|(_, index)| {
                        index.validate_completed(record_count, logical_count, logical_domain)
                    })
                })?,
                None => {
                    for index in inference.indexes.values() {
                        index.validate_completed(record_count, logical_count, logical_domain)?;
                    }
                }
            }
        }
        crate::segment_archive::validate_segment_components(
            &self.files,
            &self.records,
            &self.file_index,
            &self.id_lookup,
            self.collection,
        )?;
        if !self.has_published_storage() {
            return Err("completed segment retained a mutable query dictionary".into());
        }
        crate::storage_usage::StorageWorkCounters::update(|counters| {
            counters.row_cross_checks = counters.row_cross_checks.saturating_add(1);
        });
        self.recompute_accounting();
        self.storage_completed = true;
        Ok(self)
    }

    pub(crate) fn has_published_storage(&self) -> bool {
        self.postings.all_published()
            && self
                .custom_postings
                .values()
                .all(|custom| custom.index.is_published())
            && self
                .inference
                .as_ref()
                .is_none_or(|inference| inference.indexes.values().all(PostingIndex::is_published))
    }

    pub(crate) fn all_storage_archived(&self) -> bool {
        self.files.is_archived()
            && self.file_index.is_archived()
            && self.id_lookup.is_archived()
            && self.postings.all_archived()
            && self
                .custom_postings
                .values()
                .all(|custom| custom.index.is_archived())
            && self
                .inference
                .as_ref()
                .is_none_or(|inference| inference.indexes.values().all(PostingIndex::is_archived))
    }

    pub(crate) fn archive_storage(&mut self) -> Result<(), String> {
        if !self.storage_completed || !self.has_published_storage() {
            return Err("segment storage is not semantically completed".into());
        }
        if !self.files.is_archived() {
            let (files, records) = self.files.archive_rows()?;
            self.files = files;
            self.records = records;
        }
        if !self.file_index.is_archived() {
            self.file_index = self.file_index.archive()?;
        }
        if !self.id_lookup.is_archived() {
            self.id_lookup = self.id_lookup.archive()?;
        }
        self.postings.archive_storage()?;
        for custom in self.custom_postings.values_mut() {
            custom.index.archive_storage()?;
        }
        if let Some(inference) = &mut self.inference {
            for index in inference.indexes.values_mut() {
                index.archive_storage()?;
            }
        }
        crate::segment_archive::validate_segment_components(
            &self.files,
            &self.records,
            &self.file_index,
            &self.id_lookup,
            self.collection,
        )?;
        Ok(())
    }

    pub(crate) fn storage_representation_counts(&self) -> (usize, usize) {
        let mut native = 0usize;
        let mut archived = 0usize;
        for is_archived in [
            self.files.is_archived(),
            self.file_index.is_archived(),
            self.id_lookup.is_archived(),
        ] {
            if is_archived {
                archived += 1;
            } else {
                native += 1;
            }
        }
        let mut add = |(add_native, add_archived): (usize, usize)| {
            native += add_native;
            archived += add_archived;
        };
        add(self.postings.storage_representation_counts());
        for custom in self.custom_postings.values() {
            add(custom.index.storage_representation_counts());
        }
        if let Some(inference) = &self.inference {
            for index in inference.indexes.values() {
                add(index.storage_representation_counts());
            }
        }
        (native, archived)
    }

    pub(crate) fn read_view<'a>(published: &'a PublishedSegment) -> SegmentReadView<'a> {
        SegmentReadView {
            id: published.id,
            collection: published.index.collection,
            index: &published.index,
            active: &published.active,
        }
    }

    pub fn component_descriptors(&self) -> Vec<NativeComponentDescriptor<'_>> {
        let mut descriptors = Vec::with_capacity(
            self.postings.fields.len()
                + self
                    .inference
                    .as_ref()
                    .map_or(0, |inference| inference.indexes.len())
                + self.custom_postings.len()
                + self.custom_text.len()
                + 4,
        );
        descriptors.extend(self.postings.fields.iter().map(|(path, _)| {
            NativeComponentDescriptor {
                collection: self.collection,
                segment: self.id,
                kind: NativeComponentKind::DeclaredPosting,
                path: Some(path),
                binding: None,
            }
        }));
        if let Some(inference) = &self.inference {
            descriptors.extend(
                inference
                    .indexes
                    .keys()
                    .map(|path| NativeComponentDescriptor {
                        collection: self.collection,
                        segment: self.id,
                        kind: NativeComponentKind::InferredPosting,
                        path: Some(path),
                        binding: None,
                    }),
            );
        }
        descriptors.extend(self.custom_postings.keys().map(|(path, binding)| {
            NativeComponentDescriptor {
                collection: self.collection,
                segment: self.id,
                kind: NativeComponentKind::CustomPosting,
                path: Some(path),
                binding: Some(binding),
            }
        }));
        descriptors.push(NativeComponentDescriptor {
            collection: self.collection,
            segment: self.id,
            kind: NativeComponentKind::RawText,
            path: None,
            binding: None,
        });
        descriptors.push(NativeComponentDescriptor {
            collection: self.collection,
            segment: self.id,
            kind: NativeComponentKind::FallbackText,
            path: None,
            binding: None,
        });
        descriptors.extend(self.custom_text.keys().map(|(path, binding)| {
            NativeComponentDescriptor {
                collection: self.collection,
                segment: self.id,
                kind: NativeComponentKind::FieldText,
                path: Some(path),
                binding: Some(binding),
            }
        }));
        descriptors.push(NativeComponentDescriptor {
            collection: self.collection,
            segment: self.id,
            kind: NativeComponentKind::Sorting,
            path: None,
            binding: None,
        });
        descriptors.push(NativeComponentDescriptor {
            collection: self.collection,
            segment: self.id,
            kind: NativeComponentKind::Relations,
            path: None,
            binding: None,
        });
        descriptors
    }

    /// Recompute the cached planning estimate after native finalization or a
    /// metadata rewrite. Arc-reused segments do not repeat this traversal.
    pub(crate) fn recompute_accounting(&mut self) {
        let mut retained = self.source_id.len();
        retained = retained
            .saturating_add(self.files.archive_bytes())
            .saturating_add(self.files.compatibility_cache_bytes())
            .saturating_add(self.records.compatibility_cache_bytes())
            .saturating_add(self.identity_order.capacity() * std::mem::size_of::<RecordSlot>())
            .saturating_add(self.file_index.archive_bytes())
            .saturating_add(self.id_lookup.archive_bytes());
        for (path, index) in &self.postings.fields {
            retained = retained
                .saturating_add(path_bytes(path))
                .saturating_add(posting_bytes(index))
                .saturating_add(index.archive_bytes());
        }
        for ((path, binding), posting) in &self.custom_postings {
            retained = retained
                .saturating_add(path_bytes(path))
                .saturating_add(binding.len())
                .saturating_add(posting_bytes(&posting.index))
                .saturating_add(posting.index.archive_bytes())
                .saturating_add(ACCOUNTING_OVERHEAD);
        }
        retained = retained
            .saturating_add(gram_bytes(&self.raw))
            .saturating_add(gram_bytes(&self.fallback_text));
        for ((path, binding), index) in self.custom_text.iter() {
            retained = retained
                .saturating_add(path_bytes(path))
                .saturating_add(binding.len())
                .saturating_add(gram_bytes(index))
                .saturating_add(ACCOUNTING_OVERHEAD);
        }
        retained = retained
            .saturating_add(bitmap_bytes(&self.logical_records))
            .saturating_add(bitmap_bytes(&self.empty_array_records))
            .saturating_add(sort_bytes(&self.sorting))
            .saturating_add(
                self.relations
                    .iter()
                    .map(RelationContribution::estimated_bytes)
                    .sum::<usize>(),
            );
        if let Some(inference) = &self.inference {
            retained = retained.saturating_add(inference_bytes(inference));
        }
        let source_bytes_estimated = self.files.source_bytes();
        let max_record_range_len = self.records.max_record_range_len();
        self.accounting = SegmentAccounting {
            retained_bytes_estimated: retained,
            source_bytes_estimated,
            max_record_range_len,
            index_family_bytes: crate::lifecycle::index_family_total(
                &self.raw,
                &self.fallback_text,
                &self.custom_text,
                &self.postings,
                &self.sorting,
            ),
        };
    }

    pub(crate) fn record_count(&self) -> usize {
        self.records.len()
    }

    pub(crate) fn max_record_range_len(&self) -> u64 {
        self.accounting.max_record_range_len
    }

    pub(crate) fn file_count(&self) -> usize {
        self.files.len()
    }

    pub(crate) fn collect_storage_usage(
        &self,
        seen: &mut std::collections::HashSet<usize>,
        usage: &mut crate::storage_usage::StorageUsage,
    ) {
        if self.files.is_archived() {
            usage.record_archive(
                seen,
                self.files.archive_owner_id(),
                self.files.archive_bytes(),
                self.files.capacity(),
            );
        }
        if self.file_index.is_archived() {
            usage.record_archive(
                seen,
                self.file_index.archive_owner_id(),
                self.file_index.archive_bytes(),
                self.file_index.capacity(),
            );
        }
        if self.id_lookup.is_archived() {
            usage.record_archive(
                seen,
                self.id_lookup.archive_owner_id(),
                self.id_lookup.archive_bytes(),
                self.id_lookup.capacity(),
            );
        }
        {
            let mut record_posting = |owner: usize, length: usize, capacity: usize| {
                usage.record_archive(seen, owner, length, capacity);
            };
            self.postings.for_each_archive_owner(&mut record_posting);
            for custom in self.custom_postings.values() {
                custom.index.for_each_archive_owner(&mut record_posting);
            }
            if let Some(inference) = &self.inference {
                for index in inference.indexes.values() {
                    index.for_each_archive_owner(&mut record_posting);
                }
            }
        }
        usage.posting_bitmap_payload_bytes = usage.posting_bitmap_payload_bytes.saturating_add(
            self.postings
                .fields
                .values()
                .map(PostingIndex::native_bitmap_payload_bytes)
                .sum::<usize>() as u64,
        );
        for custom in self.custom_postings.values() {
            usage.posting_bitmap_payload_bytes = usage
                .posting_bitmap_payload_bytes
                .saturating_add(custom.index.native_bitmap_payload_bytes() as u64);
        }
        if let Some(inference) = &self.inference {
            for index in inference.indexes.values() {
                usage.posting_bitmap_payload_bytes = usage
                    .posting_bitmap_payload_bytes
                    .saturating_add(index.native_bitmap_payload_bytes() as u64);
            }
        }
        usage.bigram_payload_bytes = usage
            .bigram_payload_bytes
            .saturating_add(gram_bytes(&self.raw) as u64)
            .saturating_add(gram_bytes(&self.fallback_text) as u64)
            .saturating_add(
                self.custom_text
                    .values()
                    .map(|index| gram_bytes(index))
                    .sum::<usize>() as u64,
            );
        usage.sorting_payload_bytes = usage
            .sorting_payload_bytes
            .saturating_add(sort_bytes(&self.sorting) as u64);
        usage.relation_payload_bytes = usage.relation_payload_bytes.saturating_add(
            self.relations
                .iter()
                .map(RelationContribution::estimated_bytes)
                .sum::<usize>() as u64,
        );
        usage.active_and_control_metadata_bytes =
            usage.active_and_control_metadata_bytes.saturating_add(
                (self.identity_order.capacity() * std::mem::size_of::<RecordSlot>()) as u64,
            );
        usage.file_row_cache_bytes = usage
            .file_row_cache_bytes
            .saturating_add(self.files.compatibility_cache_bytes() as u64);
        usage.record_row_cache_bytes = usage
            .record_row_cache_bytes
            .saturating_add(self.records.compatibility_cache_bytes() as u64);
        usage.metadata_cache_initializations = usage
            .metadata_cache_initializations
            .saturating_add(self.files.cache_initialized() as u64)
            .saturating_add(self.records.cache_initialized() as u64);
    }

    pub(crate) fn extend_file_slots(
        &self,
        index: usize,
        matches: &mut RoaringBitmap,
    ) -> Result<(), String> {
        let file = self.files.get(index).ok_or("segment file row missing")?;
        matches.extend(file.records.iter().map(|slot| slot.0));
        Ok(())
    }

    pub fn file(&self, id: FileId) -> Option<&SegmentFile> {
        self.files.get(self.file_index.get(id)?)
    }

    pub fn record(&self, slot: RecordSlot) -> Option<&RecordEntry> {
        self.records.get(slot.0 as usize)
    }

    pub fn file_records(&self, id: FileId) -> Option<&[RecordSlot]> {
        Some(&self.file(id)?.records)
    }
}

fn bitmap_bytes(bitmap: &RoaringBitmap) -> usize {
    bitmap.serialized_size()
}

fn path_bytes(path: &[String]) -> usize {
    path.iter().map(String::len).sum()
}

fn field_path_bytes(path: &crate::model::FieldPath) -> usize {
    path_bytes(&path.segments)
}

fn posting_bytes(index: &crate::posting::PostingIndex) -> usize {
    let mut total = path_bytes(&index.path).saturating_add(ACCOUNTING_OVERHEAD);
    total = total.saturating_add(bitmap_bytes(&index.presence));
    total = total.saturating_add(bitmap_bytes(&index.empty_selections));
    total = total.saturating_add(bitmap_bytes(&index.nulls));
    if let Some(facts) = &index.structural {
        let _ = facts.postings.for_each_bitmap(|bitmap| {
            total = total.saturating_add(bitmap_bytes(bitmap));
            Ok(())
        });
    }
    let _ = index.values.for_each_bitmap(|bitmap| {
        total = total.saturating_add(bitmap_bytes(bitmap));
        Ok(())
    });
    let _ = index.lengths.for_each_bitmap(|bitmap| {
        total = total.saturating_add(bitmap_bytes(bitmap));
        Ok(())
    });
    total
}

pub(crate) fn gram_bytes(index: &BigramIndex) -> usize {
    index
        .normal
        .len()
        .saturating_add(index.skip.len())
        .saturating_mul(ACCOUNTING_OVERHEAD + std::mem::size_of::<u64>())
        .saturating_add(
            index
                .normal
                .values()
                .chain(index.skip.values())
                .map(bitmap_bytes)
                .sum::<usize>(),
        )
}

pub(crate) fn sort_bytes(sorting: &SortCollection) -> usize {
    let mut total = 0usize;
    for (path, column) in &sorting.columns {
        total = total
            .saturating_add(field_path_bytes(path))
            .saturating_add(ACCOUNTING_OVERHEAD)
            .saturating_add(column.keys.capacity() * std::mem::size_of::<crate::sorting::SortKey>())
            .saturating_add(column.ascending.capacity() * std::mem::size_of::<RecordSlot>())
            .saturating_add(column.descending.capacity() * std::mem::size_of::<RecordSlot>())
            .saturating_add(column.locale.len())
            .saturating_add(column.collation_version.len());
        for key in &column.keys {
            total = total.saturating_add(match key {
                crate::sorting::SortKey::String(value) => value.len(),
                crate::sorting::SortKey::Id(value) => value.len(),
                _ => 0,
            });
        }
    }
    for composite in &sorting.composites {
        total = total
            .saturating_add(
                composite.fields.capacity()
                    * std::mem::size_of::<crate::sorting::CompositeSortPart>(),
            )
            .saturating_add(composite.order.capacity() * std::mem::size_of::<RecordSlot>());
        for part in &composite.fields {
            total = total.saturating_add(field_path_bytes(&part.path));
        }
    }
    total
}

fn inference_bytes(state: &InferenceState) -> usize {
    let mut total = 0usize;
    for (path, index) in &state.indexes {
        total = total
            .saturating_add(path_bytes(path))
            .saturating_add(posting_bytes(index))
            .saturating_add(index.archive_bytes());
    }
    for (path, bitmap) in state
        .empty_selections
        .iter()
        .chain(state.short_string_coverage.iter())
    {
        total = total
            .saturating_add(path_bytes(path))
            .saturating_add(bitmap_bytes(bitmap))
            .saturating_add(ACCOUNTING_OVERHEAD);
    }
    for field in &state.report.fields {
        total = total
            .saturating_add(path_bytes(&field.path))
            .saturating_add(field.reason.len());
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_policy_defaults_and_validates_slot_range() {
        let policy = SegmentPolicy::default();
        assert_eq!(policy.target_records.get(), 16_384);
        assert_eq!(policy.max_files.get(), 16_384);
        assert_eq!(
            SegmentPolicy {
                target_records: NonZeroUsize::new(u32::MAX as usize + 1).unwrap(),
                max_files: NonZeroUsize::new(1).unwrap(),
            }
            .validate(),
            Err("segment target exceeds the supported local-slot range".into())
        );
    }

    #[test]
    fn segment_policy_seals_before_either_soft_limit() {
        let policy = SegmentPolicy {
            target_records: NonZeroUsize::new(3).unwrap(),
            max_files: NonZeroUsize::new(2).unwrap(),
        };
        assert!(policy.can_grow(2, 2, 1, 0));
        assert!(!policy.can_grow(3, 2, 1, 0));
        assert!(!policy.can_grow(2, 2, 0, 1));
        assert!(policy.can_grow(2, 1, 0, 1));
    }

    #[test]
    fn segment_policy_accepts_one_oversized_contribution_in_empty_draft() {
        let policy = SegmentPolicy {
            target_records: NonZeroUsize::new(2).unwrap(),
            max_files: NonZeroUsize::new(1).unwrap(),
        };
        assert!(policy.can_grow(0, 0, 7, 3));
        assert!(!policy.can_grow(7, 3, 1, 1));
    }

    #[test]
    fn segment_policy_uses_checked_boundary_arithmetic() {
        let policy = SegmentPolicy::default();
        assert!(!policy.can_grow(usize::MAX, 1, 1, 0));
        assert!(!policy.can_grow(1, usize::MAX, 0, 1));
    }
}
