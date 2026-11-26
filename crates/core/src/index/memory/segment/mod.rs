//! Native segment policy, identity, ownership, and borrowed inspection.

pub(crate) mod admission;
mod compaction;
mod consolidation;
mod finalize;
mod projection;
mod record_versions;
pub(crate) mod statistics;

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
#[cfg(test)]
use std::num::NonZeroUsize;
use std::ops::{BitOrAssign, Deref, DerefMut};
use std::sync::Arc;

use roaring::RoaringBitmap;

use crate::index::bigram::BigramIndex;
use crate::index::memory::inference::InferenceState;
use crate::index::posting::{CustomPostingIndex, PostingCollection};
use crate::index::relation::RelationContribution;
use crate::index::sorting::SortCollection;
use crate::index::storage::file_catalog::FileCatalogHandle;
use crate::index::storage::order::{
    InferredOrderRows, OrderColumnReader, PostingOrderRequest, SortKey,
};
use crate::index::storage::ordered_slots::OrderedSlots;
use crate::index::storage::relation::{RelationMatches, RelationReader};
use crate::index::storage::segment::{
    RecordBacking, RecordOrigin, RecordWriteVersion, RetainedBlock, RetainedBlockReader,
    SegmentDescriptor, SegmentReader, SegmentSource,
};
use crate::index::storage::transfer::{BatchLimit, BatchStatus, StorageError, StorageResult};
use crate::inventory::{ActiveRecords, ByteRange, RecordEntry};
use crate::lifecycle::IndexRevision;
#[cfg(test)]
use crate::model::Value;
use crate::model::{CollectionId, FileId, Number, RecordSlot};
use crate::query_access::membership::SlotSet;
use crate::records::RecordVersion;

const ACCOUNTING_OVERHEAD: usize = 48;

/// Immutable segment columns are shared across publication-local link edits.
/// Construction still mutates an unshared value through copy-on-write.
#[derive(Clone)]
pub(crate) struct SharedColumn<T: Clone>(Arc<T>);

impl<T: Clone> From<T> for SharedColumn<T> {
    fn from(value: T) -> Self {
        Self(Arc::new(value))
    }
}

impl<T: Clone> Deref for SharedColumn<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Clone> DerefMut for SharedColumn<T> {
    fn deref_mut(&mut self) -> &mut T {
        Arc::make_mut(&mut self.0)
    }
}

pub(crate) use crate::index::storage::segment::SegmentRecordAddress;
pub use crate::index::storage::segment::{SegmentId, SegmentPolicy};

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

pub(crate) use crate::index::storage::segment::FileRangeLocator;
pub(crate) use crate::records::RetainedBody;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RetainedAddress {
    pub block: u32,
    pub offset: u32,
}

pub(super) struct MemoryRetainedBlock(pub(super) Vec<RetainedBody>);

impl RetainedBlockReader for MemoryRetainedBlock {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn body(&self, offset: u32) -> StorageResult<Cow<'_, RetainedBody>> {
        self.0
            .get(offset as usize)
            .map(Cow::Borrowed)
            .ok_or_else(|| StorageError::invalid("read retained body", "offset outside block"))
    }

    fn estimated_body_bytes(&self) -> usize {
        self.0.iter().fold(0usize, |total, body| {
            total
                .saturating_add(crate::ingestion::estimated_value_bytes(&body.value))
                .saturating_add(crate::ingestion::estimated_text_bytes(&body.text))
        })
    }
}

/// One backing is selected before a segment scan. Per-record coordinates live
/// in compact slot-indexed columns; registrations are segment-scoped.
#[derive(Clone)]
#[allow(dead_code)] // The other variants are populated by the direct registrars.
pub(crate) enum SegmentBacking {
    FileRange {
        locators: Vec<FileRangeLocator>,
    },
    Retained {
        blocks: Vec<RetainedBlock>,
        addresses: Vec<RetainedAddress>,
        links: Vec<Option<FileRangeLocator>>,
        registration: u32,
    },
    Provider {
        versions: Vec<RecordVersion>,
        links: Vec<Option<FileRangeLocator>>,
        registration: u32,
    },
}

#[allow(dead_code)] // Retained and provider accessors are used by their scan dispatch.
impl SegmentBacking {
    /// Bytes owned by this segment's backing columns. Shared retained blocks
    /// are charged here for maintenance; publication-wide reports deduplicate
    /// their allocations by block identity.
    fn estimated_storage_bytes(&self) -> usize {
        self.estimated_column_bytes()
            .saturating_add(self.retained_body_bytes())
    }

    pub(crate) fn estimated_column_bytes(&self) -> usize {
        match self {
            Self::FileRange { locators } => locators
                .capacity()
                .saturating_mul(std::mem::size_of::<FileRangeLocator>()),
            Self::Retained {
                blocks,
                addresses,
                links,
                ..
            } => blocks
                .capacity()
                .saturating_mul(std::mem::size_of::<RetainedBlock>())
                .saturating_add(
                    addresses
                        .capacity()
                        .saturating_mul(std::mem::size_of::<RetainedAddress>()),
                )
                .saturating_add(
                    links
                        .capacity()
                        .saturating_mul(std::mem::size_of::<Option<FileRangeLocator>>()),
                ),
            Self::Provider {
                versions, links, ..
            } => versions
                .capacity()
                .saturating_mul(std::mem::size_of::<RecordVersion>())
                .saturating_add(
                    links
                        .capacity()
                        .saturating_mul(std::mem::size_of::<Option<FileRangeLocator>>()),
                ),
        }
    }

    pub(crate) fn retained_body_bytes(&self) -> usize {
        let Self::Retained { blocks, .. } = self else {
            return 0;
        };
        blocks.iter().fold(0usize, |total, block| {
            total.saturating_add(block.estimated_body_bytes())
        })
    }

    pub(crate) fn compatible_with(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::FileRange { .. }, Self::FileRange { .. }) => true,
            (
                Self::Retained {
                    registration: left, ..
                },
                Self::Retained {
                    registration: right,
                    ..
                },
            )
            | (
                Self::Provider {
                    registration: left, ..
                },
                Self::Provider {
                    registration: right,
                    ..
                },
            ) => left == right,
            _ => false,
        }
    }

    /// Concatenate compatible columns while sharing retained body blocks.
    pub(crate) fn concatenate(backings: &[&Self]) -> Result<Self, String> {
        let first = backings
            .first()
            .ok_or("no segment backing to concatenate")?;
        match first {
            Self::FileRange { .. } => {
                let mut locators = Vec::new();
                for backing in backings {
                    let Self::FileRange { locators: source } = backing else {
                        return Err("mixed segment backings in consolidation".into());
                    };
                    locators.extend_from_slice(source);
                }
                Ok(Self::FileRange { locators })
            }
            Self::Retained { registration, .. } => {
                let mut blocks = Vec::new();
                let mut addresses = Vec::new();
                let mut links = Vec::new();
                for backing in backings {
                    let Self::Retained {
                        blocks: source_blocks,
                        addresses: source_addresses,
                        links: source_links,
                        registration: source_registration,
                    } = backing
                    else {
                        return Err("mixed segment backings in consolidation".into());
                    };
                    if source_registration != registration {
                        return Err("retained registration mismatch in consolidation".into());
                    }
                    let offset =
                        u32::try_from(blocks.len()).map_err(|_| "too many retained blocks")?;
                    blocks.extend(source_blocks.iter().cloned());
                    for address in source_addresses {
                        addresses.push(RetainedAddress {
                            block: offset
                                .checked_add(address.block)
                                .ok_or("retained block index overflow")?,
                            offset: address.offset,
                        });
                    }
                    links.extend_from_slice(source_links);
                }
                Ok(Self::Retained {
                    blocks,
                    addresses,
                    links,
                    registration: *registration,
                })
            }
            Self::Provider { registration, .. } => {
                let mut versions = Vec::new();
                let mut links = Vec::new();
                for backing in backings {
                    let Self::Provider {
                        versions: source,
                        links: source_links,
                        registration: source_registration,
                    } = backing
                    else {
                        return Err("mixed segment backings in consolidation".into());
                    };
                    if source_registration != registration {
                        return Err("provider registration mismatch in consolidation".into());
                    }
                    versions.extend_from_slice(source);
                    links.extend_from_slice(source_links);
                }
                Ok(Self::Provider {
                    versions,
                    links,
                    registration: *registration,
                })
            }
        }
    }

    pub(crate) fn file_ranges(records: &[RecordEntry]) -> Self {
        Self::FileRange {
            locators: records
                .iter()
                .map(|record| FileRangeLocator {
                    file: record.file,
                    range: Some(record.range),
                    revision: record.revision,
                })
                .collect(),
        }
    }

    pub(crate) fn record_count(&self) -> usize {
        match self {
            Self::FileRange { locators } => locators.len(),
            Self::Retained { addresses, .. } => addresses.len(),
            Self::Provider { versions, .. } => versions.len(),
        }
    }

    pub(crate) fn file_locator(&self, slot: RecordSlot) -> Option<FileRangeLocator> {
        match self {
            Self::FileRange { locators } => locators.get(slot.0 as usize).copied(),
            Self::Retained { links, .. } | Self::Provider { links, .. } => {
                links.get(slot.0 as usize).copied().flatten()
            }
        }
    }

    pub(crate) fn max_file_range_len(&self) -> u64 {
        match self {
            Self::FileRange { locators } => locators
                .iter()
                .filter_map(|locator| locator.range)
                .map(ByteRange::len)
                .max()
                .unwrap_or(0),
            Self::Retained { .. } | Self::Provider { .. } => 0,
        }
    }

    pub(crate) fn retained_body(
        &self,
        slot: RecordSlot,
    ) -> StorageResult<Option<Cow<'_, RetainedBody>>> {
        let Self::Retained {
            blocks, addresses, ..
        } = self
        else {
            return Ok(None);
        };
        let Some(address) = addresses.get(slot.0 as usize) else {
            return Ok(None);
        };
        let block = blocks
            .get(address.block as usize)
            .ok_or_else(|| StorageError::invalid("read retained body", "retained block missing"))?;
        block.body(address.offset).map(Some)
    }

    pub(crate) fn provider_version(&self, slot: RecordSlot) -> Option<RecordVersion> {
        let Self::Provider { versions, .. } = self else {
            return None;
        };
        versions.get(slot.0 as usize).copied()
    }
}

#[cfg(test)]
mod backing_tests {
    use super::*;

    #[test]
    fn native_backing_accounting_includes_owned_columns_and_retained_bodies() {
        let short = SegmentBacking::Retained {
            blocks: vec![Arc::new(MemoryRetainedBlock(vec![RetainedBody {
                value: Value::String(String::from("a")),
                text: Vec::new(),
            }]))],
            addresses: vec![RetainedAddress {
                block: 0,
                offset: 0,
            }],
            links: vec![None],
            registration: 0,
        };
        let long = SegmentBacking::Retained {
            blocks: vec![Arc::new(MemoryRetainedBlock(vec![RetainedBody {
                value: Value::String("a".repeat(4096)),
                text: Vec::new(),
            }]))],
            addresses: vec![RetainedAddress {
                block: 0,
                offset: 0,
            }],
            links: vec![None],
            registration: 0,
        };
        assert!(long.estimated_storage_bytes() >= short.estimated_storage_bytes() + 4095);
        let provider = SegmentBacking::Provider {
            versions: vec![RecordVersion::new(1)],
            links: vec![None],
            registration: 0,
        };
        assert!(provider.estimated_storage_bytes() >= std::mem::size_of::<RecordVersion>());
    }

    #[test]
    fn retained_consolidation_shares_blocks_and_remaps_addresses() {
        let first_block: RetainedBlock = Arc::new(MemoryRetainedBlock(vec![RetainedBody {
            value: Value::Object(Default::default()),
            text: Vec::new(),
        }]));
        let second_block: RetainedBlock = Arc::new(MemoryRetainedBlock(vec![RetainedBody {
            value: Value::String("second".into()),
            text: Vec::new(),
        }]));
        let first = SegmentBacking::Retained {
            blocks: vec![Arc::clone(&first_block)],
            addresses: vec![RetainedAddress {
                block: 0,
                offset: 0,
            }],
            links: vec![None],
            registration: 4,
        };
        let second = SegmentBacking::Retained {
            blocks: vec![Arc::clone(&second_block)],
            addresses: vec![RetainedAddress {
                block: 0,
                offset: 0,
            }],
            links: vec![None],
            registration: 4,
        };
        let combined = SegmentBacking::concatenate(&[&first, &second]).unwrap();
        let SegmentBacking::Retained {
            blocks, addresses, ..
        } = &combined
        else {
            panic!("wrong backing");
        };
        assert!(Arc::ptr_eq(&blocks[0], &first_block));
        assert!(Arc::ptr_eq(&blocks[1], &second_block));
        assert_eq!(addresses[1].block, 1);
        assert!(
            matches!(combined.retained_body(RecordSlot(1)).unwrap().unwrap().value, Value::String(ref value) if value == "second")
        );
    }
}

pub use crate::index::storage::segment::NativeSegmentSummary;
pub(crate) use crate::index::storage::segment::SegmentEstimates as SegmentAccounting;

/// Complete native search payload owned by one immutable segment.
#[derive(Clone)]
pub(crate) struct IndexedCollectionSegment {
    pub id: SegmentId,
    pub collection: CollectionId,
    pub source_id: String,
    pub source_version: u32,
    pub files: Vec<SegmentFile>,
    pub file_index: HashMap<FileId, usize>,
    pub records: Vec<crate::records::RecordHeader>,
    pub backing: SegmentBacking,
    pub(crate) write_versions: record_versions::RecordWriteVersions,
    /// Every raw text contribution in this native shard was derived from its
    /// converted logical string fields. Explicit adapter text is independent.
    pub raw_covers_logical_fields: bool,
    pub id_lookup: HashMap<crate::model::RecordId, RecordSlot>,
    pub identity_order: Vec<RecordSlot>,
    pub postings: SharedColumn<PostingCollection>,
    pub inference: Option<InferenceState>,
    pub custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    pub raw: SharedColumn<BigramIndex>,
    pub fallback_text: SharedColumn<BigramIndex>,
    pub custom_text: HashMap<(Vec<String>, String), BigramIndex>,
    pub logical_records: RoaringBitmap,
    pub empty_array_records: RoaringBitmap,
    pub sorting: SharedColumn<SortCollection>,
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
    pub(crate) observation: Option<crate::index::storage::inference::InferenceObservationHandle>,
}

impl crate::index::storage::segment::SegmentReaderDelegate for PublishedSegment {
    type Payload = IndexedCollectionSegment;

    fn payload(&self) -> &Self::Payload {
        &self.index
    }

    fn placement_id(&self) -> Option<SegmentId> {
        Some(self.id)
    }
}

impl crate::index::storage::segment_catalog::SegmentRouteSource for PublishedSegment {
    fn segment_descriptor(&self) -> SegmentDescriptor {
        self.descriptor()
    }

    fn visit_live_records(
        &self,
        visit: &mut dyn FnMut(&[(&crate::model::RecordId, RecordSlot)]) -> StorageResult<()>,
    ) -> StorageResult<()> {
        crate::index::storage::segment_catalog::visit_live_routes(self, &self.active, visit)
    }

    fn visit_files(
        &self,
        visit: &mut dyn FnMut(&[FileId]) -> StorageResult<()>,
    ) -> StorageResult<()> {
        crate::index::storage::segment_catalog::visit_file_routes(self, visit)
    }
}

impl PublishedSegment {
    pub(crate) fn construction_observation_bytes(&self) -> usize {
        self.observation
            .as_ref()
            .map_or(0, |observation| observation.estimated_bytes())
    }
}

/// Borrowed view of the metadata, liveness, and segment payloads that belong
/// to one publication. Repository and lifecycle reads expose the same shape.
#[derive(Clone, Copy)]
pub(crate) struct NativeSnapshotRef<'a> {
    pub active: &'a [ActiveRecords],
    pub segments: &'a [Arc<Vec<Arc<PublishedSegment>>>],
    pub catalog: &'a FileCatalogHandle,
    pub revision: IndexRevision,
}

pub use crate::index::storage::component::{
    ComponentDescriptor as NativeComponentDescriptor, ComponentKind as NativeComponentKind,
};

impl SegmentReader for IndexedCollectionSegment {
    fn summary(&self, active_records: usize) -> NativeSegmentSummary {
        let index = self;
        let bigrams = || {
            std::iter::once(&*index.raw)
                .chain(std::iter::once(&*index.fallback_text))
                .chain(index.custom_text.values())
        };
        NativeSegmentSummary {
            id: self.id,
            collection: index.collection,
            records: index.record_count(),
            active_records: active_records,
            files: index.file_count(),
            source_bytes: index.accounting.source_bytes_estimated,
            posting_fields: index.postings.fields.len(),
            sort_columns: index.sorting.column_count(),
            relation_count: index.relations.len(),
            relation_edges: index
                .relations
                .iter()
                .map(|relation| relation.edge_count())
                .sum(),
            relation_heap_bytes: index
                .relations
                .iter()
                .map(|relation| relation.estimated_bytes())
                .sum(),
            bigram_stores: 2 + index.custom_text.len(),
            distinct_gram_keys: bigrams()
                .map(|index| index.normal.len() + index.skip.len())
                .sum(),
            bigram_heap_bytes: bigrams().map(BigramIndex::estimated_heap_bytes).sum(),
            index_family_bytes: index.accounting.index_family_bytes,
            active: active_records != 0,
        }
    }

    fn estimates(&self) -> SegmentAccounting {
        self.accounting
    }

    fn observe_backing(&self, usage: &mut crate::index::storage::segment::SegmentResourceUsage) {
        usage.add_backing_columns(self.backing.estimated_column_bytes());
        if let SegmentBacking::Retained { blocks, .. } = &self.backing {
            for block in blocks {
                usage.add_retained_block(block);
            }
        }
    }

    fn index_family_estimate(&self) -> crate::lifecycle::IndexFamilyEstimate {
        statistics::index_family_estimate(
            &self.raw,
            &self.fallback_text,
            &self.custom_text,
            &self.postings,
            &self.sorting,
        )
    }

    fn components(&self) -> StorageResult<Vec<NativeComponentDescriptor<'_>>> {
        Ok(self.component_descriptors(self.id))
    }

    fn posting(
        &self,
        component: &NativeComponentDescriptor<'_>,
    ) -> StorageResult<Option<&dyn crate::index::storage::posting::PostingReader>> {
        let Some(path) = component.path.as_deref() else {
            return Ok(None);
        };
        let reader = match component.kind {
            NativeComponentKind::DeclaredPosting => self.postings.fields.get(path),
            NativeComponentKind::InferredPosting => self
                .inference
                .as_ref()
                .and_then(|state| state.indexes.get(path)),
            NativeComponentKind::CustomPosting => {
                let Some(binding) = component.binding.as_deref() else {
                    return Ok(None);
                };
                self.custom_postings
                    .get(&(path.to_vec(), binding.to_owned()))
                    .map(|posting| &posting.index)
            }
            _ => None,
        };
        Ok(reader.map(|reader| reader as &dyn crate::index::storage::posting::PostingReader))
    }

    fn grams(
        &self,
        component: &NativeComponentDescriptor<'_>,
    ) -> StorageResult<Option<&dyn crate::index::storage::text::GramReader>> {
        let reader = match component.kind {
            NativeComponentKind::RawText => Some(&*self.raw),
            NativeComponentKind::FallbackText => Some(&*self.fallback_text),
            NativeComponentKind::FieldText => {
                let (Some(path), Some(binding)) =
                    (component.path.as_deref(), component.binding.as_deref())
                else {
                    return Ok(None);
                };
                self.custom_text.get(&(path.to_vec(), binding.to_owned()))
            }
            _ => None,
        };
        Ok(reader.map(|reader| reader as &dyn crate::index::storage::text::GramReader))
    }

    fn order_columns(&self) -> StorageResult<Vec<&dyn OrderColumnReader>> {
        Ok(self.sorting.readers().collect())
    }

    fn descriptor(&self) -> SegmentDescriptor {
        SegmentDescriptor {
            id: self.id,
            collection: self.collection,
            slots: self.record_count(),
        }
    }

    fn active_fallback_count(&self, active: &ActiveRecords) -> u64 {
        active.len()
            - active.intersection_len(crate::query_access::membership::MembershipView::Borrowed(
                &self.logical_records,
            ))
    }

    fn slot_members(
        &self,
        fact: crate::index::storage::segment::SegmentSlotFact,
    ) -> StorageResult<SlotSet<'_>> {
        use crate::index::storage::segment::SegmentSlotFact::{EmptyArray, Logical};
        Ok(SlotSet::borrowed(match fact {
            Logical => &self.logical_records,
            EmptyArray => &self.empty_array_records,
        }))
    }

    fn source(&self) -> SegmentSource<'_> {
        let registration = match &self.backing {
            SegmentBacking::FileRange { .. } => None,
            SegmentBacking::Retained { registration, .. }
            | SegmentBacking::Provider { registration, .. } => Some(*registration),
        };
        SegmentSource {
            adapter: &self.source_id,
            version: self.source_version,
            registration,
        }
    }

    fn read_header_batch(
        &self,
        start: usize,
        limit: BatchLimit,
    ) -> StorageResult<(Cow<'_, [crate::records::RecordHeader]>, BatchStatus)> {
        const OP: &str = "read native segment headers";
        let remaining = self
            .records
            .get(start..)
            .ok_or_else(|| StorageError::invalid(OP, "start beyond last slot"))?;
        let len = remaining
            .len()
            .min(limit.fixed_batch_len(OP, size_of::<crate::records::RecordHeader>())?);
        let status = if len == remaining.len() {
            BatchStatus::Done
        } else {
            BatchStatus::More
        };
        Ok((Cow::Borrowed(&remaining[..len]), status))
    }

    fn backing_kind(&self) -> RecordBacking {
        match self.backing {
            SegmentBacking::FileRange { .. } => RecordBacking::FileRange,
            SegmentBacking::Retained { .. } => RecordBacking::Retained,
            SegmentBacking::Provider { .. } => RecordBacking::Provider,
        }
    }

    fn slot_for_record(&self, id: &str) -> StorageResult<Option<RecordSlot>> {
        Ok(self.id_lookup.get(id).copied())
    }

    fn write_version(&self, slot: RecordSlot) -> StorageResult<Option<RecordWriteVersion>> {
        Ok(self.write_versions.get(slot))
    }

    fn header(
        &self,
        slot: RecordSlot,
    ) -> StorageResult<Option<Cow<'_, crate::records::RecordHeader>>> {
        Ok(self.record(slot).map(Cow::Borrowed))
    }

    fn origin(&self, slot: RecordSlot) -> StorageResult<Option<RecordOrigin>> {
        if self.record(slot).is_none() {
            return Ok(None);
        }
        let file = self.backing.file_locator(slot);
        let corrupt = || {
            crate::index::storage::transfer::StorageError::invalid(
                "read record origin",
                "origin is missing for an existing slot",
            )
        };
        Ok(Some(match &self.backing {
            SegmentBacking::FileRange { .. } => RecordOrigin::FileRange(file.ok_or_else(corrupt)?),
            SegmentBacking::Retained { .. } => RecordOrigin::Retained(file),
            SegmentBacking::Provider { registration, .. } => RecordOrigin::Provider {
                registration: *registration,
                version: self.backing.provider_version(slot).ok_or_else(corrupt)?,
                file,
            },
        }))
    }

    fn retained_body(&self, slot: RecordSlot) -> StorageResult<Option<Cow<'_, RetainedBody>>> {
        self.backing.retained_body(slot)
    }

    fn file_count(&self) -> usize {
        self.files.len()
    }

    fn read_file_batch<'a>(
        &'a self,
        start: usize,
        limit: BatchLimit,
        entries: &mut Vec<Cow<'a, crate::inventory::FileEntry>>,
    ) -> StorageResult<BatchStatus> {
        const OP: &str = "read native segment file entries";
        let remaining = self
            .files
            .get(start..)
            .ok_or_else(|| StorageError::invalid(OP, "start beyond last file"))?;
        let len = remaining
            .len()
            .min(limit.fixed_batch_len(OP, size_of::<Cow<'_, crate::inventory::FileEntry>>())?);
        entries.extend(
            remaining[..len]
                .iter()
                .map(|file| Cow::Borrowed(&file.entry)),
        );
        Ok(if len == remaining.len() {
            BatchStatus::Done
        } else {
            BatchStatus::More
        })
    }

    fn contains_file(&self, file: FileId) -> StorageResult<bool> {
        Ok(self.file_index.contains_key(&file))
    }

    fn file_metadata(
        &self,
        file: FileId,
    ) -> StorageResult<Option<Cow<'_, crate::inventory::FileEntry>>> {
        Ok(self.file(file).map(|file| Cow::Borrowed(&file.entry)))
    }

    fn visit_file_slots(
        &self,
        file: FileId,
        limit: crate::index::storage::transfer::BatchLimit,
        visit: &mut dyn FnMut(&[RecordSlot]) -> StorageResult<()>,
    ) -> StorageResult<()> {
        if let Some(file) = self.file(file) {
            let batch_len =
                limit.fixed_batch_len("read segment file slots", size_of::<RecordSlot>())?;
            for slots in file.records.chunks(batch_len) {
                visit(slots)?;
            }
        }
        Ok(())
    }

    fn with_predicate_access(
        &self,
        access: crate::index::storage::segment::SegmentPredicateAccess<'_>,
        use_provider: &mut crate::index::storage::segment::UseAccessProvider<'_>,
    ) -> StorageResult<()> {
        let provider = super::access::segment::NativeSegmentProvider::for_access(self, access);
        use_provider(Some(&provider))
    }

    fn with_text_access(
        &self,
        access: crate::index::storage::text::SegmentTextAccess<'_>,
        use_provider: &mut crate::index::storage::segment::UseAccessProvider<'_>,
    ) -> StorageResult<()> {
        let provider = super::access::text::NativeSegmentTextProvider::for_access(self, access);
        use_provider(Some(&provider))
    }

    fn supports_posting_order(&self, path: &crate::model::FieldPath) -> StorageResult<bool> {
        Ok(super::access::catalog::sortable_segment_posting(self, &path.segments).is_some())
    }

    fn build_posting_order(
        &self,
        request: PostingOrderRequest<'_>,
    ) -> StorageResult<Option<InferredOrderRows>> {
        let index = super::access::catalog::sortable_segment_posting(self, &request.path.segments)
            .ok_or_else(|| {
                StorageError::invalid(
                    "build inferred order",
                    "selected posting lost scalar coverage",
                )
            })?;
        super::sorting::build_inferred_order_rows(
            index,
            &self.records,
            request.path.clone(),
            request.locale,
            request.options,
            request.descending,
            request.execution,
        )
        .map_err(|error| StorageError::invalid("build inferred order", error))
    }

    fn relation_count(&self) -> usize {
        self.relations.len()
    }

    fn relation(&self, ordinal: usize) -> StorageResult<Option<&dyn RelationReader>> {
        Ok(self
            .relations
            .get(ordinal)
            .map(|relation| relation as &dyn RelationReader))
    }

    fn reverse_relation(&self, ordinal: usize, target: &str) -> StorageResult<Option<SlotSet<'_>>> {
        let relation = self
            .relations
            .get(ordinal)
            .ok_or_else(|| StorageError::invalid("read native backlink", "unknown relation"))?;
        let Some(posting) = self.postings.fields.get(&relation.declaration().path) else {
            return Ok(None);
        };
        let members =
            posting.value_posting(&crate::index::posting::ScalarKey::Id(target.to_owned()));
        Ok(Some(members.map_or(SlotSet::Empty, SlotSet::borrowed)))
    }

    fn reverse_relation_batch(
        &self,
        ordinal: usize,
        targets: &std::collections::HashSet<String>,
    ) -> StorageResult<Option<RelationMatches<'_>>> {
        let relation = self
            .relations
            .get(ordinal)
            .ok_or_else(|| StorageError::invalid("read native backlinks", "unknown relation"))?;
        let Some(posting) = self.postings.fields.get(&relation.declaration().path) else {
            return Ok(None);
        };
        let (slots, lists, members) = posting.union_id_postings(targets);
        Ok(Some(RelationMatches {
            slots: SlotSet::from_owned(slots),
            lists,
            members,
        }))
    }

    fn order_column(
        &self,
        path: &crate::model::FieldPath,
    ) -> StorageResult<Option<&dyn OrderColumnReader>> {
        Ok(self.sorting.reader(path))
    }

    fn stored_key(
        &self,
        path: &crate::model::FieldPath,
        slot: RecordSlot,
    ) -> StorageResult<Option<Cow<'_, SortKey>>> {
        self.sorting.stored_key(path, slot)
    }

    fn ordered_slots(
        &self,
        sort: &[crate::query::options::SortField],
        scalar: Option<(&crate::model::FieldPath, bool)>,
    ) -> StorageResult<Option<&dyn OrderedSlots>> {
        if sort.is_empty() {
            return Ok(Some(&self.identity_order));
        }
        if sort.len() > 1 {
            return Ok(self
                .sorting
                .composite(sort)
                .map(|column| &column.order as &dyn OrderedSlots));
        }
        let Some((path, descending)) = scalar else {
            return Ok(None);
        };
        self.sorting
            .reader(path)
            .map(|column| column.slots(descending))
            .transpose()
    }
}

impl IndexedCollectionSegment {
    pub(crate) fn refresh_metadata_sort_slots(
        &mut self,
        slots: impl IntoIterator<Item = RecordSlot>,
        root: Option<&std::path::Path>,
        collection_name: &str,
    ) -> Result<(), String> {
        let backing = &self.backing;
        let files = &self.files;
        let file_index = &self.file_index;
        if !self
            .sorting
            .columns
            .keys()
            .any(|path| matches!(path.root, crate::model::PathRoot::Metadata(_)))
        {
            return Ok(());
        }
        self.sorting.refresh_metadata_slots(
            &self.records,
            slots,
            |slot| {
                let locator = backing.file_locator(slot)?;
                let file = files.get(*file_index.get(&locator.file)?)?;
                Some((&file.entry, locator.range))
            },
            root,
            collection_name,
        )
    }
    fn component_descriptors(&self, published_id: SegmentId) -> Vec<NativeComponentDescriptor<'_>> {
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
                segment: published_id,
                kind: NativeComponentKind::DeclaredPosting,
                path: Some(Cow::Borrowed(path)),
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
                        segment: published_id,
                        kind: NativeComponentKind::InferredPosting,
                        path: Some(Cow::Borrowed(path)),
                        binding: None,
                    }),
            );
        }
        descriptors.extend(self.custom_postings.keys().map(|(path, binding)| {
            NativeComponentDescriptor {
                collection: self.collection,
                segment: published_id,
                kind: NativeComponentKind::CustomPosting,
                path: Some(Cow::Borrowed(path)),
                binding: Some(Cow::Borrowed(binding)),
            }
        }));
        descriptors.push(NativeComponentDescriptor {
            collection: self.collection,
            segment: published_id,
            kind: NativeComponentKind::RawText,
            path: None,
            binding: None,
        });
        descriptors.push(NativeComponentDescriptor {
            collection: self.collection,
            segment: published_id,
            kind: NativeComponentKind::FallbackText,
            path: None,
            binding: None,
        });
        descriptors.extend(self.custom_text.keys().map(|(path, binding)| {
            NativeComponentDescriptor {
                collection: self.collection,
                segment: published_id,
                kind: NativeComponentKind::FieldText,
                path: Some(Cow::Borrowed(path)),
                binding: Some(Cow::Borrowed(binding)),
            }
        }));
        descriptors.push(NativeComponentDescriptor {
            collection: self.collection,
            segment: published_id,
            kind: NativeComponentKind::Sorting,
            path: None,
            binding: None,
        });
        descriptors.push(NativeComponentDescriptor {
            collection: self.collection,
            segment: published_id,
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
                self.records.capacity() * std::mem::size_of::<crate::records::RecordHeader>(),
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
        retained = retained
            .saturating_add(self.backing.estimated_storage_bytes())
            .saturating_add(self.write_versions.resident_bytes());
        let source_bytes_estimated = self
            .files
            .iter()
            .map(|file| file.entry.revision.size)
            .fold(0u64, u64::saturating_add);
        let max_record_range_len = self.backing.max_file_range_len();
        self.accounting = SegmentAccounting {
            retained_bytes_estimated: retained,
            source_bytes_estimated,
            max_record_range_len,
            index_family_bytes: statistics::index_family_total(
                &self.raw,
                &self.fallback_text,
                &self.custom_text,
                &self.postings,
                &self.sorting,
            ),
        };
    }

    pub(crate) fn record_count(&self) -> usize {
        debug_assert_eq!(self.records.len(), self.backing.record_count());
        self.backing.record_count()
    }

    #[cfg(test)]
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

    pub fn record(&self, slot: RecordSlot) -> Option<&crate::records::RecordHeader> {
        self.records.get(slot.0 as usize)
    }

    /// Reconstitute a file record for native layout assertions.
    #[cfg(test)]
    pub(crate) fn file_record(&self, slot: RecordSlot) -> Option<RecordEntry> {
        if !matches!(self.backing, SegmentBacking::FileRange { .. }) {
            return None;
        }
        let header = self.record(slot)?;
        let locator = self.backing.file_locator(slot)?;
        Some(RecordEntry {
            slot: header.slot,
            id: header.id.clone(),
            file: locator.file,
            range: locator.range?,
            revision: locator.revision,
            class: header.class,
            diagnostic_ref: header.diagnostic_ref,
        })
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

fn posting_bytes(index: &crate::index::posting::PostingIndex) -> usize {
    let mut total = path_bytes(&index.path).saturating_add(ACCOUNTING_OVERHEAD);
    total = total.saturating_add(bitmap_bytes(&index.presence));
    total = total.saturating_add(index.values.len().saturating_mul(
        ACCOUNTING_OVERHEAD + std::mem::size_of::<crate::index::posting::ScalarKey>(),
    ));
    for (key, bitmap) in &index.values {
        total = total.saturating_add(bitmap_bytes(bitmap));
        if let crate::index::posting::ScalarKey::String(value)
        | crate::index::posting::ScalarKey::Id(value) = key
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
    let mut total = sorting.placed_bytes();
    for (path, column) in &sorting.columns {
        total = total
            .saturating_add(field_path_bytes(path))
            .saturating_add(ACCOUNTING_OVERHEAD)
            .saturating_add(
                column.keys.capacity() * std::mem::size_of::<crate::index::sorting::SortKey>(),
            )
            .saturating_add(column.ascending.capacity() * std::mem::size_of::<RecordSlot>())
            .saturating_add(column.descending.capacity() * std::mem::size_of::<RecordSlot>())
            .saturating_add(column.locale.len())
            .saturating_add(column.collation_version.len());
        for key in &column.keys {
            total = total.saturating_add(match key {
                crate::index::sorting::SortKey::String(value) => value.len(),
                crate::index::sorting::SortKey::Id(value) => value.len(),
                _ => 0,
            });
        }
    }
    for composite in &sorting.composites {
        total = total
            .saturating_add(
                composite.fields.capacity()
                    * std::mem::size_of::<crate::index::sorting::CompositeSortPart>(),
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
    for (path, bitmap) in &state.short_string_coverage {
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

#[cfg(test)]
#[path = "consolidation.test.rs"]
mod consolidation_tests;

#[cfg(test)]
#[path = "compaction.test.rs"]
mod compaction_tests;
