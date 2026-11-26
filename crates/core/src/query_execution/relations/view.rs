//! Pinned relation lookup over one immutable base-plus-delta publication.

use crate::index::storage::file_catalog::FileCatalogReader;
use crate::index::storage::relation::RelationRow;
use crate::index::storage::segment::{RecordBacking, RecordOrigin, SegmentReader};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::index::segment::{SegmentId, SegmentRecordAddress};
use crate::index::storage::publication::PublishedSegment;
use crate::inventory::{ActiveRecords, RecordClass};
use crate::measurement::{SourceAvailabilityLog, SourceDiagnostic};
use crate::model::{CollectionId, RecordSlot, Value};
use crate::repository::Repository;
use crate::repository::state::CollectionState;
use crate::schema::relations::{RelationCardinality, RelationId};
use crate::source::ScanProjection;
use crate::source::access::{
    classify_source_read_failure_for_search, read_record_range_for_search,
};

use super::evaluation::{LogicalReferences, logical_references};
use super::work::{MAX_QUERY_RELATION_BYTES, RelationWork};

/// One record address in a pinned base-plus-delta publication. A local slot
/// alone cannot identify a record after multiple segments have been added.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct MergedRecordSlot {
    pub collection: CollectionId,
    pub segment: Option<usize>,
    pub slot: RecordSlot,
}

pub(crate) struct MergedRecord<'a> {
    pub record: crate::inventory::RecordEntry,
    pub file: Cow<'a, crate::inventory::FileEntry>,
    pub collection: &'a CollectionState,
}

/// Query-local relation lookup over a single immutable publication. Forward
/// edges and reverse postings are borrowed from the relevant segment on demand.
pub(crate) struct MergedRelationView<'a> {
    repo: &'a Repository,
    base_active: &'a [ActiveRecords],
    segments: &'a [Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &'a dyn FileCatalogReader,
    segment_positions: Arc<HashMap<(CollectionId, SegmentId), usize>>,
    source_errors: RefCell<SourceAvailabilityLog>,
    reference_identity: RefCell<HashMap<CollectionId, Arc<HashMap<String, MergedRecordSlot>>>>,
    reference_backlinks: RefCell<HashMap<RelationId, Arc<ReferenceBacklinkLookup>>>,
    // Source-dependent comparisons often revisit one target for many source
    // records. Keep only small decoded targets for this query publication.
    target_values: Arc<Vec<Mutex<(HashMap<MergedRecordSlot, Arc<Value>>, usize)>>>,
}

struct ReferenceBacklinkLookup {
    by_target: HashMap<String, Vec<MergedRecordSlot>>,
    complete: bool,
}

pub(crate) struct ReferenceBacklinkSources {
    pub(crate) sources: Vec<MergedRecordSlot>,
    pub(crate) complete: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ReferenceValueVisit {
    Visited,
    SkippedRawFallback,
    Unavailable,
}

impl<'a> MergedRelationView<'a> {
    pub(crate) fn collection_state(&self, id: CollectionId) -> Result<&'a CollectionState, String> {
        self.repo
            .collections
            .get(id.0 as usize)
            .ok_or_else(|| "merged collection missing".into())
    }

    /// A worker keeps its own source-availability log while borrowing the
    /// same immutable pinned publication and catalog.
    pub(crate) fn fork_for_worker(&self) -> Self {
        Self {
            repo: self.repo,
            base_active: self.base_active,
            segments: self.segments,
            catalog: self.catalog,
            segment_positions: Arc::clone(&self.segment_positions),
            source_errors: RefCell::new(SourceAvailabilityLog::default()),
            reference_identity: RefCell::new(HashMap::new()),
            reference_backlinks: RefCell::new(HashMap::new()),
            target_values: self.target_values.clone(),
        }
    }

    pub(crate) fn source_error_events(&self) -> u64 {
        self.source_errors.borrow().events()
    }

    pub(crate) fn cached_value(&self, address: MergedRecordSlot) -> Option<Arc<Value>> {
        let shard = (address.slot.0 as usize
            ^ address.segment.unwrap_or(0).wrapping_mul(131)
            ^ (address.collection.0 as usize).wrapping_mul(17))
            % self.target_values.len();
        self.target_values[shard]
            .lock()
            .ok()
            .and_then(|cache| cache.0.get(&address).cloned())
    }

    pub(super) fn lock_target_value_cache(
        &self,
        address: MergedRecordSlot,
        enabled: bool,
    ) -> Option<std::sync::MutexGuard<'_, (HashMap<MergedRecordSlot, Arc<Value>>, usize)>> {
        if !enabled {
            return None;
        }
        let shard = (address.slot.0 as usize
            ^ address.segment.unwrap_or(0).wrapping_mul(131)
            ^ (address.collection.0 as usize).wrapping_mul(17))
            % self.target_values.len();
        Some(
            self.target_values[shard]
                .lock()
                .unwrap_or_else(|_| panic!("relation target cache poisoned")),
        )
    }

    pub(crate) fn take_source_errors(&self) -> SourceAvailabilityLog {
        std::mem::take(&mut *self.source_errors.borrow_mut())
    }

    pub(crate) fn record_source_failure(
        &self,
        file: &crate::inventory::FileEntry,
        range: crate::inventory::ByteRange,
        message: String,
    ) {
        let path = self.repo.required_file_root().join(&file.relative_path);
        self.source_errors.borrow_mut().record(SourceDiagnostic {
            file_id: file.id.0,
            file_path: file.relative_path.to_path_buf(),
            kind: classify_source_read_failure_for_search(&path, file.revision, Some(range)),
            message,
        });
    }

    pub(crate) fn repository_root(&self) -> &std::path::Path {
        self.repo.required_file_root()
    }

    pub(crate) fn repository(&self) -> &'a Repository {
        self.repo
    }

    pub(crate) fn catalog(&self) -> &'a dyn FileCatalogReader {
        self.catalog
    }

    pub(crate) fn segments(
        &self,
        collection: CollectionId,
    ) -> Result<&'a [Arc<PublishedSegment>], String> {
        self.segments
            .get(collection.0 as usize)
            .map(|segments| segments.as_slice())
            .ok_or_else(|| "merged segment collection missing".into())
    }

    pub(crate) fn segment_ordinal(&self, address: MergedRecordSlot) -> Option<u64> {
        address
            .segment
            .map(|index| self.segments[address.collection.0 as usize][index].id.get())
    }

    pub(crate) fn relation_cardinality(
        &self,
        relation: RelationId,
    ) -> Result<RelationCardinality, String> {
        Ok(self.repo.collections[relation.source.0 as usize]
            .relation_declaration(relation.ordinal)
            .ok_or("merged relation declaration missing")?
            .cardinality)
    }

    pub(crate) fn new(
        repo: &'a Repository,
        base_active: &'a [ActiveRecords],
        segments: &'a [Arc<Vec<Arc<PublishedSegment>>>],
        catalog: &'a dyn FileCatalogReader,
    ) -> Result<Self, String> {
        if base_active.len() != repo.collections.len() || segments.len() != repo.collections.len() {
            return Err("merged relation collection count mismatch".into());
        }
        for (collection_index, shards) in segments.iter().enumerate() {
            for published in shards.iter() {
                let descriptor = published.descriptor();
                if descriptor.collection.0 as usize != collection_index
                    || published.active.slot_count() as usize != descriptor.slots
                {
                    return Err("merged relation segment scope mismatch".into());
                }
                for relation_index in 0..published.relation_count() {
                    let relation = published
                        .relation(relation_index)?
                        .ok_or("merged relation segment declaration missing")?;
                    if relation.source_slots() != descriptor.slots
                        || repo.collections[collection_index]
                            .relation_declaration(relation_index as u32)
                            != Some(relation.declaration())
                    {
                        return Err("merged relation segment declaration mismatch".into());
                    }
                }
            }
        }
        let mut segment_positions = HashMap::new();
        for (collection_index, shards) in segments.iter().enumerate() {
            for (position, segment) in shards.iter().enumerate() {
                let collection = CollectionId(collection_index as u32);
                if segment_positions
                    .insert((collection, segment.id), position)
                    .is_some()
                {
                    return Err("duplicate segment identity in merged relation view".into());
                }
            }
        }
        Ok(Self {
            repo,
            base_active,
            segments,
            catalog,
            segment_positions: Arc::new(segment_positions),
            source_errors: RefCell::new(SourceAvailabilityLog::default()),
            reference_identity: RefCell::new(HashMap::new()),
            reference_backlinks: RefCell::new(HashMap::new()),
            target_values: Arc::new((0..16).map(|_| Mutex::new((HashMap::new(), 0))).collect()),
        })
    }

    pub(crate) fn view_address(
        &self,
        address: SegmentRecordAddress,
    ) -> Result<MergedRecordSlot, String> {
        let position = *self
            .segment_positions
            .get(&(address.collection, address.segment))
            .ok_or("native segment address is outside the pinned view")?;
        let published = self.segments[address.collection.0 as usize]
            .get(position)
            .ok_or("native segment position is outside the pinned view")?;
        if !published.active.contains(address.slot) || published.header(address.slot)?.is_none() {
            return Err("native segment address is not active in the pinned view".into());
        }
        Ok(MergedRecordSlot {
            collection: address.collection,
            segment: Some(position),
            slot: address.slot,
        })
    }

    pub(crate) fn native_address(
        &self,
        address: MergedRecordSlot,
    ) -> Result<Option<SegmentRecordAddress>, String> {
        let Some(position) = address.segment else {
            return Ok(None);
        };
        let published = self.segments[address.collection.0 as usize]
            .get(position)
            .ok_or("merged segment position is outside the pinned view")?;
        if !published.active.contains(address.slot) || published.header(address.slot)?.is_none() {
            return Err("merged segment address is not active in the pinned view".into());
        }
        Ok(Some(SegmentRecordAddress {
            collection: address.collection,
            segment: published.id,
            slot: address.slot,
        }))
    }

    pub(crate) fn native_address_for_flat_slot(
        &self,
        collection: CollectionId,
        slot: RecordSlot,
    ) -> Result<Option<SegmentRecordAddress>, String> {
        let state = self
            .repo
            .collections
            .get(collection.0 as usize)
            .ok_or("flat address collection missing")?;
        let record = state.base_record(slot)?;
        self.resolve_id(collection, &record.id.0)?
            .map(|address| {
                self.native_address(address)?
                    .ok_or_else(|| "flat address resolved to a legacy base".to_string())
            })
            .transpose()
    }

    pub(crate) fn record_header(
        &self,
        address: MergedRecordSlot,
    ) -> Result<Option<crate::records::RecordHeader>, String> {
        let index = address.collection.0 as usize;
        if let Some(segment_index) = address.segment {
            let published = self.segments[index]
                .get(segment_index)
                .ok_or("merged segment missing")?;
            if !published.active.contains(address.slot) {
                return Ok(None);
            }
            return published
                .header(address.slot)?
                .map(std::borrow::Cow::into_owned)
                .map(Some)
                .ok_or_else(|| "merged segment slot missing".into());
        }
        if !self.base_active[index].contains(address.slot) {
            return Ok(None);
        }
        let record = self.repo.collections[index].base_record(address.slot)?;
        Ok(Some(crate::records::RecordHeader::from(record.as_ref())))
    }

    pub(crate) fn is_native(&self, address: MergedRecordSlot) -> bool {
        address.segment.is_some_and(|position| {
            !matches!(
                self.segments[address.collection.0 as usize][position].backing_kind(),
                RecordBacking::FileRange
            )
        })
    }

    pub(crate) fn is_native_provider(&self, address: MergedRecordSlot) -> bool {
        address.segment.is_some_and(|position| {
            matches!(
                self.segments[address.collection.0 as usize][position].backing_kind(),
                RecordBacking::Provider
            )
        })
    }

    pub(crate) fn native_value(
        &self,
        address: MergedRecordSlot,
        collect_text: bool,
    ) -> Result<
        Option<(
            std::borrow::Cow<'_, Value>,
            std::borrow::Cow<'_, [crate::source::TextSegment]>,
        )>,
        String,
    > {
        self.native_value_projected(
            address,
            &crate::records::reader::RecordProjection {
                text: collect_text,
                ..Default::default()
            },
        )
    }

    pub(crate) fn native_value_projected(
        &self,
        address: MergedRecordSlot,
        projection: &crate::records::reader::RecordProjection,
    ) -> Result<
        Option<(
            std::borrow::Cow<'_, Value>,
            std::borrow::Cow<'_, [crate::source::TextSegment]>,
        )>,
        String,
    > {
        let Some(position) = address.segment else {
            return Ok(None);
        };
        let published = self.segments[address.collection.0 as usize]
            .get(position)
            .ok_or("native segment missing")?;
        if !published.active.contains(address.slot) {
            return Ok(None);
        }
        let segment = &published.index;
        match segment.backing_kind() {
            RecordBacking::FileRange => Ok(None),
            RecordBacking::Retained => {
                let body = segment
                    .retained_body(address.slot)?
                    .ok_or("retained record body missing")?;
                Ok(Some(match body {
                    Cow::Borrowed(body) => (Cow::Borrowed(&body.value), Cow::Borrowed(&body.text)),
                    Cow::Owned(body) => (Cow::Owned(body.value), Cow::Owned(body.text)),
                }))
            }
            RecordBacking::Provider => {
                let record = crate::query_execution::runner::read_provider(
                    self.repo,
                    segment,
                    address.slot,
                    projection,
                )?;
                Ok(Some((Cow::Owned(record.value), Cow::Owned(record.text))))
            }
        }
    }

    /// One registration and segment share a projection and reader call. The
    /// caller bounds `slots` and keeps the returned values only for its chunk.
    pub(crate) fn native_provider_batch(
        &self,
        address: MergedRecordSlot,
        slots: &[RecordSlot],
        projection: &crate::records::reader::RecordProjection,
    ) -> Result<Vec<crate::records::reader::RecordRead>, String> {
        let position = address.segment.ok_or("native provider segment missing")?;
        let published = self.segments[address.collection.0 as usize]
            .get(position)
            .ok_or("native provider segment missing")?;
        if slots.iter().any(|slot| !published.active.contains(*slot)) {
            return Err("native provider slot retired".into());
        }
        crate::query_execution::runner::read_provider_batch(
            self.repo,
            &published.index,
            slots,
            projection,
        )
    }

    pub(crate) fn native_origin(
        &self,
        address: MergedRecordSlot,
    ) -> Result<
        (
            Option<Cow<'a, crate::inventory::FileEntry>>,
            Option<crate::inventory::ByteRange>,
        ),
        String,
    > {
        let Some(position) = address.segment else {
            return Ok((None, None));
        };
        let segment = &self.segments[address.collection.0 as usize][position].index;
        let Some(locator) = segment.origin(address.slot)?.and_then(RecordOrigin::file) else {
            return Ok((None, None));
        };
        let file = self
            .catalog
            .file(locator.file)?
            .ok_or("native link file retired")?;
        if file.revision != locator.revision || !file.collections.contains(&address.collection) {
            return Err("native link file changed without retirement".into());
        }
        Ok((Some(file), locator.range))
    }

    pub(crate) fn record(
        &self,
        address: MergedRecordSlot,
    ) -> Result<Option<MergedRecord<'a>>, String> {
        let index = address.collection.0 as usize;
        let collection = self
            .repo
            .collections
            .get(index)
            .ok_or("merged collection missing")?;
        if let Some(segment_index) = address.segment {
            let published = self.segments[index]
                .get(segment_index)
                .ok_or("merged segment missing")?;
            return crate::query_execution::runner::segment_record_ref(
                &published.index,
                &published.active,
                self.catalog,
                address.slot,
            )
            .map(|record| {
                record.map(|record| MergedRecord {
                    record: record.record,
                    file: record.file,
                    collection,
                })
            });
        }
        if !self.base_active[index].contains(address.slot) {
            return Ok(None);
        }
        let record = collection.base_record(address.slot)?;
        let original_revision = self.repo.base_file(record.file)?.revision;
        let file = self
            .catalog
            .file(record.file)?
            .ok_or("active merged record file is retired")?;
        if file.revision != original_revision || !file.collections.contains(&address.collection) {
            return Err("active merged record file changed without retirement".into());
        }
        Ok(Some(MergedRecord {
            record: record.into_owned(),
            file,
            collection,
        }))
    }

    pub(crate) fn resolve_id(
        &self,
        collection: CollectionId,
        id: &str,
    ) -> Result<Option<MergedRecordSlot>, String> {
        let index = collection.0 as usize;
        let state = self
            .repo
            .collections
            .get(index)
            .ok_or("merged target collection missing")?;
        let mut found = None;
        if let Some(slot) = state.base_slot_for_record(id)?
            && self.base_active[index].contains(slot)
        {
            found = Some(MergedRecordSlot {
                collection,
                segment: None,
                slot,
            });
        }
        for (segment_index, published) in self.segments[index].iter().enumerate() {
            if let Some(slot) = published.slot_for_record(id)?
                && published.active.contains(slot)
            {
                if found.is_some() {
                    return Err("duplicate active merged record ID".into());
                }
                found = Some(MergedRecordSlot {
                    collection,
                    segment: Some(segment_index),
                    slot,
                });
            }
        }
        Ok(found)
    }

    pub(crate) fn forward_edges(
        &self,
        address: MergedRecordSlot,
        ordinal: usize,
    ) -> Result<RelationRow<'a>, String> {
        let index = address.collection.0 as usize;
        let relation = if let Some(position) = address.segment {
            self.segments[index][position].relation(ordinal)?
        } else {
            self.repo.collections[index].relation_reader(ordinal)?
        }
        .ok_or("merged relation missing")?;
        Ok(relation.forward(address.slot)?)
    }

    pub(crate) fn backlink_sources(
        &self,
        relation: RelationId,
        target_id: &str,
    ) -> Result<Vec<MergedRecordSlot>, String> {
        let index = relation.source.0 as usize;
        let collection = self
            .repo
            .collections
            .get(index)
            .ok_or("backlink source missing")?;
        let ordinal = relation.ordinal as usize;
        let mut sources = Vec::new();
        let mut add = |segment, slot| {
            sources.push(MergedRecordSlot {
                collection: relation.source,
                segment,
                slot,
            });
        };
        if let Some(base) = &collection.base_records {
            visit_reverse_sources(
                base.as_ref(),
                &self.base_active[index],
                ordinal,
                target_id,
                |slot| add(None, slot),
            )?;
        } else if self.base_active[index].len() != 0 {
            let base = collection
                .relations
                .get(ordinal)
                .ok_or("backlink relation missing")?;
            if let Some(slots) = self.repo.relation_reverse_sources(base, target_id) {
                for slot in slots
                    .iter()
                    .map(RecordSlot)
                    .filter(|slot| self.base_active[index].contains(*slot))
                {
                    add(None, slot);
                }
            }
        }
        for (position, published) in self.segments[index].iter().enumerate() {
            visit_reverse_sources(
                published.as_ref(),
                &published.active,
                ordinal,
                target_id,
                |slot| add(Some(position), slot),
            )?;
        }
        let mut keyed = Vec::with_capacity(sources.len());
        for address in sources {
            let id = if let Some(segment_index) = address.segment {
                self.segments[index][segment_index]
                    .header(address.slot)?
                    .ok_or("backlink segment slot missing")?
                    .id
                    .0
                    .to_string()
            } else {
                collection.base_record(address.slot)?.id.0.to_string()
            };
            keyed.push((id, address));
        }
        keyed.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        keyed.dedup_by(|a, b| a.1 == b.1);
        Ok(keyed.into_iter().map(|(_, address)| address).collect())
    }

    pub(super) fn reference_value(
        &self,
        address: MergedRecordSlot,
        work: Option<&mut RelationWork>,
    ) -> Result<Option<Arc<Value>>, String> {
        if let Some(value) = self.cached_value(address) {
            return Ok(Some(value));
        }
        if self.is_native(address) {
            let Some(record) = self.record_header(address)? else {
                return Ok(None);
            };
            if record.class == RecordClass::RawFallback {
                return Ok(None);
            }
            let mut work = work;
            if let Some(work) = work.as_deref_mut() {
                work.before_read(0, false)?;
            }
            let Some((value, _)) = self.native_value(address, false)? else {
                return Ok(None);
            };
            if self.is_native_provider(address)
                && let Some(work) = work.as_deref_mut()
            {
                work.source_read(0)?;
            }
            let value = Arc::new(value.into_owned());
            return Ok(Some(value));
        }
        let Some(resolved) = self.record(address)? else {
            return Ok(None);
        };
        if resolved.record.class == RecordClass::RawFallback {
            return Ok(None);
        }
        let mut work = work;
        if let Some(work) = work.as_deref_mut() {
            work.before_read(resolved.record.range.len(), false)?;
            if let Some(execution) = work.execution() {
                execution.check_string()?;
            }
        }
        let path = self
            .repo
            .required_file_root()
            .join(&resolved.file.relative_path);
        let bytes = match read_record_range_for_search(
            resolved.collection.required_file_reader().reader(),
            &path,
            resolved.file.revision,
            resolved.record.range,
            work.as_deref().and_then(|work| work.execution()),
        ) {
            Ok(bytes) => bytes,
            Err(message) => {
                self.record_source_failure(&resolved.file, resolved.record.range, message);
                return Ok(None);
            }
        };
        if let Some(work) = work.as_deref_mut() {
            work.source_read(bytes.len() as u64)?;
        }
        let extracted = crate::source::preparation::prepare_query_record_with_execution(
            resolved.record.class,
            &resolved.collection.required_file_reader(),
            &resolved.collection.definition.schema,
            &bytes,
            &ScanProjection::full(false),
            false,
            work.as_deref().and_then(|work| work.execution()),
        )?;
        let value = Arc::new(extracted.value);
        if bytes.len() <= 64 * 1024 {
            let shard = (address.slot.0 as usize
                ^ address.segment.unwrap_or(0).wrapping_mul(131)
                ^ (address.collection.0 as usize).wrapping_mul(17))
                % self.target_values.len();
            if let Ok(mut cache) = self.target_values[shard].lock()
                && cache.1.saturating_add(bytes.len()) <= 512 * 1024
            {
                cache.1 += bytes.len();
                cache.0.insert(address, value.clone());
            }
        }
        Ok(Some(value))
    }

    pub(crate) fn reference_forward_edges(
        &self,
        address: MergedRecordSlot,
        relation: RelationId,
        work: Option<&mut RelationWork>,
    ) -> Result<Option<LogicalReferences>, String> {
        self.reference_forward_edges_with_source(address, relation, None, work)
    }

    pub(crate) fn reference_forward_edges_with_source(
        &self,
        address: MergedRecordSlot,
        relation: RelationId,
        source_value: Option<&Value>,
        mut work: Option<&mut RelationWork>,
    ) -> Result<Option<LogicalReferences>, String> {
        let owned;
        let value = if let Some(source_value) = source_value {
            source_value
        } else {
            let Some(value) = self.reference_value(address, work.as_deref_mut())? else {
                return Ok(None);
            };
            owned = value;
            &owned
        };
        let declaration = self.repo.collections[relation.source.0 as usize]
            .relation_declaration(relation.ordinal)
            .ok_or("reference relation declaration missing")?;
        logical_references(value, &declaration.path).map(Some)
    }

    pub(crate) fn reference_backlink_sources(
        &self,
        relation: RelationId,
        target_id: &str,
        work: Option<&mut RelationWork>,
    ) -> Result<Vec<MergedRecordSlot>, String> {
        self.reference_backlink_sources_with_completeness(relation, target_id, work)
            .map(|lookup| lookup.sources)
    }

    pub(crate) fn reference_backlink_sources_with_completeness(
        &self,
        relation: RelationId,
        target_id: &str,
        mut work: Option<&mut RelationWork>,
    ) -> Result<ReferenceBacklinkSources, String> {
        let cached = self.reference_backlinks.borrow().get(&relation).cloned();
        if let Some(cached) = cached {
            return Ok(ReferenceBacklinkSources {
                sources: cached.by_target.get(target_id).cloned().unwrap_or_default(),
                complete: cached.complete,
            });
        }
        let declaration = self.repo.collections[relation.source.0 as usize]
            .relation_declaration(relation.ordinal)
            .ok_or("reference backlink declaration missing")?;
        let mut keyed = self
            .visible_addresses(relation.source)?
            .into_iter()
            .map(|address| {
                let record = self
                    .record_header(address)?
                    .ok_or("active reference source record missing")?;
                if record.class != crate::inventory::RecordClass::Logical {
                    return Ok(None);
                }
                Ok(Some((record.id.0.to_string(), address)))
            })
            .filter_map(|entry| entry.transpose())
            .collect::<Result<Vec<_>, String>>()?;
        keyed.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        let mut backlinks = HashMap::<String, Vec<MergedRecordSlot>>::new();
        let mut complete = true;
        for (_, address) in keyed {
            let visited = self.visit_reference_value(address, work.as_deref_mut(), |value| {
                let references = logical_references(value, &declaration.path)?;
                let mut seen = std::collections::HashSet::new();
                for (target, _) in references.values {
                    if seen.insert(target.clone()) {
                        backlinks.entry(target).or_default().push(address);
                    }
                }
                Ok(())
            })?;
            complete &= visited != ReferenceValueVisit::Unavailable;
        }
        let lookup = Arc::new(ReferenceBacklinkLookup {
            by_target: backlinks,
            complete,
        });
        self.reference_backlinks
            .borrow_mut()
            .insert(relation, Arc::clone(&lookup));
        Ok(ReferenceBacklinkSources {
            sources: lookup.by_target.get(target_id).cloned().unwrap_or_default(),
            complete,
        })
    }

    fn visit_reference_value(
        &self,
        address: MergedRecordSlot,
        mut work: Option<&mut RelationWork>,
        mut visit: impl FnMut(&Value) -> Result<(), String>,
    ) -> Result<ReferenceValueVisit, String> {
        if let Some(value) = self.cached_value(address) {
            visit(&value)?;
            return Ok(ReferenceValueVisit::Visited);
        }
        if self.is_native(address) {
            let Some(value) = self.reference_value(address, work.as_deref_mut())? else {
                return Ok(ReferenceValueVisit::Unavailable);
            };
            visit(&value)?;
            return Ok(ReferenceValueVisit::Visited);
        }
        let Some(resolved) = self.record(address)? else {
            return Ok(ReferenceValueVisit::Unavailable);
        };
        if resolved.record.class == RecordClass::RawFallback {
            return Ok(ReferenceValueVisit::SkippedRawFallback);
        }
        if let Some(work) = work.as_deref_mut() {
            work.before_read(resolved.record.range.len(), false)?;
            if let Some(execution) = work.execution() {
                execution.check_string()?;
            }
        }
        let path = self
            .repo
            .required_file_root()
            .join(&resolved.file.relative_path);
        let bytes = match read_record_range_for_search(
            resolved.collection.required_file_reader().reader(),
            &path,
            resolved.file.revision,
            resolved.record.range,
            work.as_deref().and_then(|work| work.execution()),
        ) {
            Ok(bytes) => bytes,
            Err(message) => {
                self.record_source_failure(&resolved.file, resolved.record.range, message);
                return Ok(ReferenceValueVisit::Unavailable);
            }
        };
        if let Some(work) = work.as_deref_mut() {
            work.source_read(bytes.len() as u64)?;
        }
        let extracted = crate::source::preparation::prepare_query_record_with_execution(
            resolved.record.class,
            &resolved.collection.required_file_reader(),
            &resolved.collection.definition.schema,
            &bytes,
            &ScanProjection::full(false),
            false,
            work.as_deref().and_then(|work| work.execution()),
        )?;
        visit(&extracted.value)?;
        Ok(ReferenceValueVisit::Visited)
    }

    pub(crate) fn reference_resolve_id(
        &self,
        collection: CollectionId,
        id: &str,
    ) -> Result<Option<MergedRecordSlot>, String> {
        if let Some(identity) = self.reference_identity.borrow().get(&collection).cloned() {
            return Ok(identity.get(id).copied());
        }
        let identity = Arc::new(self.build_reference_identity(collection)?);
        let result = identity.get(id).copied();
        self.reference_identity
            .borrow_mut()
            .insert(collection, identity);
        Ok(result)
    }

    fn build_reference_identity(
        &self,
        collection: CollectionId,
    ) -> Result<HashMap<String, MergedRecordSlot>, String> {
        let collection_index = collection.0 as usize;
        let mut identity = HashMap::new();
        for address in self.visible_addresses(collection)? {
            let record = self
                .record_header(address)?
                .ok_or("active reference identity record missing")?;
            if identity.insert(record.id.0.to_string(), address).is_some() {
                return Err("duplicate active merged record ID".into());
            }
        }
        for segment in self.segments[collection_index].iter() {
            if segment.descriptor().collection != collection {
                return Err("reference identity segment collection mismatch".into());
            }
        }
        Ok(identity)
    }

    pub(super) fn visible_addresses(
        &self,
        collection: CollectionId,
    ) -> Result<Vec<MergedRecordSlot>, String> {
        let collection_index = collection.0 as usize;
        let state = self
            .repo
            .collections
            .get(collection_index)
            .ok_or("reference collection missing")?;
        let mut addresses = Vec::new();
        let base_count = state.base_slot_count();
        for slot in 0..base_count {
            let slot = RecordSlot(slot as u32);
            if self.base_active[collection_index].contains(slot) {
                addresses.push(MergedRecordSlot {
                    collection,
                    segment: None,
                    slot,
                });
            }
        }
        for (segment, published) in self.segments[collection_index].iter().enumerate() {
            for slot in 0..published.descriptor().slots {
                let slot = RecordSlot(slot as u32);
                if published.active.contains(slot) {
                    addresses.push(MergedRecordSlot {
                        collection,
                        segment: Some(segment),
                        slot,
                    });
                }
            }
        }
        Ok(addresses)
    }
}

fn visit_reverse_sources(
    source: &dyn SegmentReader,
    active: &ActiveRecords,
    ordinal: usize,
    target_id: &str,
    mut visit: impl FnMut(RecordSlot),
) -> Result<(), String> {
    if active.len() == 0 {
        return Ok(());
    }
    if let Some(members) = source.reverse_relation(ordinal, target_id)? {
        let members = members
            .explicit_view()
            .ok_or("reverse relation returned an unrestricted domain")?;
        if members
            .last()
            .is_some_and(|slot| slot >= active.slot_count())
        {
            return Err("reverse relation source is outside its domain".into());
        }
        for slot in members
            .iter()
            .map(RecordSlot)
            .filter(|&slot| active.contains(slot))
        {
            visit(slot);
        }
        return Ok(());
    }
    let relation = source
        .relation(ordinal)?
        .ok_or("backlink relation reader missing")?;
    for slot in active.bitmap().iter().map(RecordSlot) {
        let mut found = false;
        relation.visit_targets(slot, &mut |id, _| {
            found = id == target_id;
            Ok(found)
        })?;
        if found {
            visit(slot);
        }
    }
    Ok(())
}

pub(super) fn read_reference_value(
    repo: &Repository,
    collection_index: usize,
    slot: RecordSlot,
    work: &mut RelationWork,
) -> Result<Option<crate::model::Value>, String> {
    let collection = &repo.collections[collection_index];
    if !work.is_active(collection.collection_id(), slot) {
        return Ok(None);
    }
    let record = collection.base_record(slot)?;
    if record.class == RecordClass::RawFallback {
        return Ok(None);
    }
    let file = repo.base_file(record.file)?;
    let path = repo.required_file_root().join(&file.relative_path);
    if record.range.len() > MAX_QUERY_RELATION_BYTES {
        return Err(format!(
            "relation source record exceeds {MAX_QUERY_RELATION_BYTES} source bytes"
        ));
    }
    work.before_read(record.range.len(), false)?;
    let bytes = read_record_range_for_search(
        collection.required_file_reader().reader(),
        &path,
        record.revision,
        record.range,
        work.execution(),
    )?;
    work.source_read(bytes.len() as u64)?;
    let extracted = crate::source::preparation::prepare_query_record_with_execution(
        record.class,
        &collection.required_file_reader(),
        &collection.definition.schema,
        &bytes,
        &ScanProjection::full(false),
        false,
        work.execution(),
    )?;
    Ok(Some(extracted.value))
}

pub(crate) fn positions_match(selectors: &[Option<u32>], positions: &[u32]) -> bool {
    selectors.len() <= positions.len()
        && selectors
            .iter()
            .zip(positions)
            .all(|(selector, position)| selector.is_none_or(|index| index == *position))
}
