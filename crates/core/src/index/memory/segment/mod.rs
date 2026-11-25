//! Native segment policy, identity, ownership, and borrowed inspection.

pub(crate) mod admission;
mod compaction;
mod consolidation;
mod projection;

pub(crate) use projection::PartitionRouting;

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
#[cfg(test)]
use std::num::NonZeroUsize;
use std::ops::{BitOrAssign, Deref, DerefMut};
use std::sync::Arc;

use roaring::RoaringBitmap;
use serde::Serialize;

use crate::index::bigram::BigramIndex;
use crate::index::posting::{CustomPostingIndex, PostingCollection};
use crate::index::relation::RelationContribution;
use crate::index::sorting::SortCollection;
use crate::index::storage::file_catalog::FileCatalogHandle;
use crate::index::storage::order::SortKey;
use crate::index::storage::ordered_slots::OrderedSlots;
use crate::index::storage::segment::{SegmentDescriptor, SegmentReader};
use crate::index::storage::transfer::StorageResult;
use crate::inference::InferenceState;
use crate::inventory::{ActiveRecords, ByteRange, RecordEntry, SourceRevision};
use crate::lifecycle::IndexRevision;
#[cfg(test)]
use crate::model::Value;
use crate::model::{CollectionId, FileId, Number, RecordSlot};
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

/// Source-specific file coordinates, addressed by the common record slot.
/// A file-only provenance link can omit the byte range; ordinary file imports
/// always supply one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FileRangeLocator {
    pub file: FileId,
    pub range: Option<ByteRange>,
    pub revision: SourceRevision,
}

pub(crate) use crate::records::RetainedBody;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RetainedAddress {
    pub block: u32,
    pub offset: u32,
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
        blocks: Vec<Arc<[RetainedBody]>>,
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
                .saturating_mul(std::mem::size_of::<Arc<[RetainedBody]>>())
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
            block.iter().fold(total, |total, body| {
                total
                    .saturating_add(crate::ingestion::estimated_value_bytes(&body.value))
                    .saturating_add(crate::ingestion::estimated_text_bytes(&body.text))
            })
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

    pub(crate) fn retained_body(&self, slot: RecordSlot) -> Option<&RetainedBody> {
        let Self::Retained {
            blocks, addresses, ..
        } = self
        else {
            return None;
        };
        let address = addresses.get(slot.0 as usize)?;
        blocks
            .get(address.block as usize)?
            .get(address.offset as usize)
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
            blocks: vec![
                vec![RetainedBody {
                    value: Value::String(String::from("a")),
                    text: Vec::new(),
                }]
                .into(),
            ],
            addresses: vec![RetainedAddress {
                block: 0,
                offset: 0,
            }],
            links: vec![None],
            registration: 0,
        };
        let long = SegmentBacking::Retained {
            blocks: vec![
                vec![RetainedBody {
                    value: Value::String("a".repeat(4096)),
                    text: Vec::new(),
                }]
                .into(),
            ],
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
        let first_block: Arc<[RetainedBody]> = vec![RetainedBody {
            value: Value::Object(Default::default()),
            text: Vec::new(),
        }]
        .into();
        let second_block: Arc<[RetainedBody]> = vec![RetainedBody {
            value: Value::String("second".into()),
            text: Vec::new(),
        }]
        .into();
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
            matches!(combined.retained_body(RecordSlot(1)).unwrap().value, Value::String(ref value) if value == "second")
        );
    }
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
    pub posting_fields: usize,
    pub sort_columns: usize,
    pub relation_count: usize,
    pub relation_edges: usize,
    pub relation_heap_bytes: usize,
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
    pub records: Vec<crate::records::RecordHeader>,
    pub backing: SegmentBacking,
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
    pub(crate) observation: Option<Arc<crate::construction::inference::InferenceObservation>>,
}

impl PublishedSegment {
    pub(crate) fn summary(&self) -> NativeSegmentSummary {
        let index = &self.index;
        let bigrams = || {
            std::iter::once(&*index.raw)
                .chain(std::iter::once(&*index.fallback_text))
                .chain(index.custom_text.values())
        };
        NativeSegmentSummary {
            id: self.id,
            collection: index.collection,
            records: index.record_count(),
            active_records: self.active.len() as usize,
            files: index.file_count(),
            source_bytes: index.accounting.source_bytes_estimated,
            posting_fields: index.postings.fields.len(),
            sort_columns: index.sorting.columns.len(),
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
            active: self.active.len() != 0,
        }
    }

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
    fn descriptor(&self) -> SegmentDescriptor {
        SegmentDescriptor {
            id: self.id,
            collection: self.collection,
            slots: self.record_count(),
        }
    }

    fn slot_for_record(&self, id: &str) -> StorageResult<Option<RecordSlot>> {
        Ok(self.id_lookup.get(id).copied())
    }

    fn header(
        &self,
        slot: RecordSlot,
    ) -> StorageResult<Option<Cow<'_, crate::records::RecordHeader>>> {
        Ok(self.record(slot).map(Cow::Borrowed))
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

    fn stored_key(
        &self,
        path: &crate::model::FieldPath,
        slot: RecordSlot,
    ) -> StorageResult<Option<Cow<'_, SortKey>>> {
        Ok(self
            .sorting
            .columns
            .get(path)
            .and_then(|column| column.keys.get(slot.0 as usize))
            .map(Cow::Borrowed))
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
        Ok(scalar.and_then(|(path, descending)| {
            self.sorting.columns.get(path).map(|column| {
                if descending {
                    &column.descending as &dyn OrderedSlots
                } else {
                    &column.ascending
                }
            })
        }))
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
    pub fn component_descriptors(
        &self,
        published_id: SegmentId,
    ) -> Vec<NativeComponentDescriptor<'_>> {
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
                        segment: published_id,
                        kind: NativeComponentKind::InferredPosting,
                        path: Some(path),
                        binding: None,
                    }),
            );
        }
        descriptors.extend(self.custom_postings.keys().map(|(path, binding)| {
            NativeComponentDescriptor {
                collection: self.collection,
                segment: published_id,
                kind: NativeComponentKind::CustomPosting,
                path: Some(path),
                binding: Some(binding),
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
                path: Some(path),
                binding: Some(binding),
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
        retained = retained.saturating_add(self.backing.estimated_storage_bytes());
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
        debug_assert_eq!(self.records.len(), self.backing.record_count());
        self.backing.record_count()
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

    pub fn record(&self, slot: RecordSlot) -> Option<&crate::records::RecordHeader> {
        self.records.get(slot.0 as usize)
    }

    /// Reconstitute a file record for the legacy optimized file scanner.
    /// This value is transient; native segment storage keeps source columns
    /// separate from the common header.
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
    let mut total = 0usize;
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
