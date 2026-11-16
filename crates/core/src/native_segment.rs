//! Native segment policy, identity, ownership, and borrowed inspection.

use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroUsize;
use std::sync::Arc;

use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};

use crate::bigram::BigramIndex;
use crate::inference::InferenceState;
use crate::inventory::{ActiveRecords, RecordEntry};
use crate::lifecycle::IndexRevision;
use crate::lifecycle_catalog::FileCatalog;
use crate::model::{CollectionId, FileId, Number, RecordSlot};
use crate::posting::{CustomPostingIndex, PostingCollection};
use crate::relation::RelationContribution;
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

    pub(crate) const fn for_collection(collection: CollectionId, local: u64) -> Self {
        Self(((collection.0 as u64) << 32) | local)
    }
}

impl fmt::Display for SegmentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
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
    pub files: Vec<SegmentFile>,
    pub file_index: HashMap<FileId, usize>,
    pub records: Vec<RecordEntry>,
    pub id_lookup: HashMap<crate::model::RecordId, RecordSlot>,
    pub identity_order: Vec<RecordSlot>,
    pub postings: PostingCollection,
    pub inference: Option<InferenceState>,
    pub custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    pub raw: BigramIndex,
    pub fallback_text: BigramIndex,
    pub custom_text: HashMap<(Vec<String>, String), BigramIndex>,
    pub logical_records: RoaringBitmap,
    pub empty_array_records: RoaringBitmap,
    pub sorting: SortCollection,
    /// Relation source references retain logical target IDs for merged-view
    /// resolution across segment and collection boundaries.
    pub relations: Vec<RelationContribution>,
    pub record_parses: u64,
    pub failed_parses: u64,
    pub accounting: SegmentAccounting,
}

/// One published payload and its independently versioned local liveness.
#[derive(Clone)]
pub(crate) struct PublishedSegment {
    pub id: SegmentId,
    pub index: Arc<IndexedCollectionSegment>,
    pub active: ActiveRecords,
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
        retained =
            retained.saturating_add(self.files.capacity() * std::mem::size_of::<SegmentFile>());
        for file in &self.files {
            retained = retained
                .saturating_add(file.records.capacity() * std::mem::size_of::<RecordSlot>())
                .saturating_add(file.entry.relative_path.as_path().as_os_str().len())
                .saturating_add(
                    file.entry.collections.capacity() * std::mem::size_of::<CollectionId>(),
                )
                .saturating_add(
                    file.entry.diagnostics.capacity()
                        * std::mem::size_of::<crate::inventory::SourceDiagnostic>(),
                );
            for diagnostic in &file.entry.diagnostics {
                retained = retained
                    .saturating_add(diagnostic.code.len())
                    .saturating_add(diagnostic.message.len())
                    .saturating_add(diagnostic.source_id.len());
                if let Some(path) = &diagnostic.field_path {
                    retained = retained.saturating_add(path_bytes(path));
                }
            }
        }
        retained = retained
            .saturating_add(
                self.records.capacity() * std::mem::size_of::<crate::inventory::RecordEntry>(),
            )
            .saturating_add(self.identity_order.capacity() * std::mem::size_of::<RecordSlot>())
            .saturating_add(
                self.file_index
                    .capacity()
                    .saturating_mul(ACCOUNTING_OVERHEAD + std::mem::size_of::<usize>()),
            )
            .saturating_add(
                self.id_lookup
                    .capacity()
                    .saturating_mul(ACCOUNTING_OVERHEAD + std::mem::size_of::<RecordSlot>()),
            );
        for record in &self.records {
            retained = retained.saturating_add(record.id.0.len());
        }
        for (path, index) in &self.postings.fields {
            retained = retained
                .saturating_add(path_bytes(path))
                .saturating_add(posting_bytes(index));
        }
        for ((path, binding), posting) in &self.custom_postings {
            retained = retained
                .saturating_add(path_bytes(path))
                .saturating_add(binding.len())
                .saturating_add(posting_bytes(&posting.index))
                .saturating_add(ACCOUNTING_OVERHEAD);
        }
        retained = retained
            .saturating_add(gram_bytes(&self.raw))
            .saturating_add(gram_bytes(&self.fallback_text));
        for ((path, binding), index) in &self.custom_text {
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
        let source_bytes_estimated = self
            .files
            .iter()
            .map(|file| file.entry.revision.size)
            .fold(0u64, u64::saturating_add);
        let max_record_range_len = self
            .records
            .iter()
            .map(|record| record.range.end.saturating_sub(record.range.start))
            .max()
            .unwrap_or(0);
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
        self.files.get(*self.file_index.get(&id)?)
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
        total = total.saturating_add(facts.postings.len().saturating_mul(
            ACCOUNTING_OVERHEAD + std::mem::size_of::<crate::posting::StructuralKey>(),
        ));
        total = total.saturating_add(facts.postings.values().map(bitmap_bytes).sum::<usize>());
    }
    total =
        total.saturating_add(index.values.len().saturating_mul(
            ACCOUNTING_OVERHEAD + std::mem::size_of::<crate::posting::ScalarKey>(),
        ));
    for (key, bitmap) in &index.values {
        total = total.saturating_add(bitmap_bytes(bitmap));
        if let crate::posting::ScalarKey::String(value) | crate::posting::ScalarKey::Id(value) = key
        {
            total = total.saturating_add(value.len());
        }
    }
    total = total.saturating_add(
        index
            .lengths
            .len()
            .saturating_mul(ACCOUNTING_OVERHEAD + std::mem::size_of::<Number>()),
    );
    total.saturating_add(index.lengths.values().map(bitmap_bytes).sum::<usize>())
}

fn gram_bytes(index: &BigramIndex) -> usize {
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

fn sort_bytes(sorting: &SortCollection) -> usize {
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
            .saturating_add(posting_bytes(index));
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
