//! Repository-aware binding and exact evaluation of finite relation paths.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::index::posting::ScalarKey;
use crate::index::relation::RelationTarget;
use crate::index::segment::PublishedSegment;
use crate::index::segment::{SegmentId, SegmentRecordAddress};
use crate::inventory::{ActiveRecords, RecordClass};
use crate::lifecycle::catalog::FileCatalog;
use crate::measurement::{SourceAvailabilityLog, SourceDiagnostic};
use crate::model::{CollectionId, PathRoot, RecordSlot, Value};
use crate::query::Quantifier;
use crate::query::binding::fts::BoundFts;
use crate::query::binding::relation::{BoundRelationHop, HopDirection};
use crate::query::semantics::comparison::eval_cross_record_field_term;
use crate::query::semantics::record::RecordContext;
use crate::query::{binding::filter::BoundExpr, binding::filter::BoundTerm};
use crate::reference::filter::eval_term;
use crate::reference::{
    eval_bound_fts, eval_bound_fts_field, eval_bound_fts_membership,
    fts::eval_borrowed_value_fts_field,
};
use crate::relation::{RelationCardinality, RelationId};
use crate::relation_target_cache::TargetValueCache;
use crate::repository::CollectionState;
use crate::repository::Repository;
use crate::source::access::{
    classify_source_read_failure_for_search, read_record_range_for_search,
};
use crate::source::{ScanProjection, TextSegment};

const MAX_QUERY_EDGE_VISITS: u64 = 32_000_000;
const MAX_QUERY_BACKLINK_PROBES: u64 = 32_000_000;
const MAX_QUERY_RECORD_READS: u64 = 5_000_000;
const MAX_QUERY_RELATION_BYTES: u64 = 1024 * 1024 * 1024;

fn relation_target_uses_value_text(collection: &CollectionState) -> bool {
    collection.source.scalar_verification()
        == Some(crate::source::verification::ScalarVerification::ResolvedStrings)
}

fn relation_source_uses_value_text(repo: &Repository, source: &RecordContext<'_>) -> bool {
    source.record.class == RecordClass::Logical
        && repo
            .collections
            .iter()
            .find(|collection| collection.definition.name == source.collection_name)
            .is_some_and(relation_target_uses_value_text)
}

#[cfg(test)]
static BORROWED_RELATION_TARGETS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub(crate) fn borrowed_relation_targets() -> usize {
    BORROWED_RELATION_TARGETS.load(std::sync::atomic::Ordering::Relaxed)
}

/// One record address in a pinned base-plus-delta publication. A local slot
/// alone cannot identify a record after multiple segments have been added.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct MergedRecordSlot {
    pub collection: CollectionId,
    pub segment: Option<usize>,
    pub slot: RecordSlot,
}

pub(crate) struct MergedRecord<'a> {
    pub record: &'a crate::inventory::RecordEntry,
    pub file: &'a crate::inventory::FileEntry,
    pub collection: &'a CollectionState,
}

/// Query-local relation lookup over a single immutable publication. Forward
/// edges and reverse postings are borrowed from the relevant segment on demand.
pub(crate) struct MergedRelationView<'a> {
    repo: &'a Repository,
    base_active: &'a [ActiveRecords],
    segments: &'a [Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &'a FileCatalog,
    segment_positions: Arc<HashMap<(CollectionId, SegmentId), usize>>,
    source_errors: RefCell<SourceAvailabilityLog>,
    reference_identity: RefCell<HashMap<CollectionId, Arc<HashMap<String, MergedRecordSlot>>>>,
    reference_backlinks: RefCell<HashMap<RelationId, Arc<HashMap<String, Vec<MergedRecordSlot>>>>>,
    // Source-dependent comparisons often revisit one target for many source
    // records. Keep only small decoded targets for this query publication.
    target_values: Arc<Vec<Mutex<(HashMap<MergedRecordSlot, Arc<Value>>, usize)>>>,
}

impl<'a> MergedRelationView<'a> {
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

    pub(crate) fn take_source_errors(&self) -> SourceAvailabilityLog {
        std::mem::take(&mut *self.source_errors.borrow_mut())
    }

    pub(crate) fn record_source_failure(
        &self,
        file: &crate::inventory::FileEntry,
        range: crate::inventory::ByteRange,
        message: String,
    ) {
        let path = self.repo.files.root.join(&file.relative_path);
        self.source_errors.borrow_mut().record(SourceDiagnostic {
            file_id: file.id.0,
            file_path: file.relative_path.to_path_buf(),
            kind: classify_source_read_failure_for_search(&path, file.revision, Some(range)),
            message,
        });
    }

    pub(crate) fn repository_root(&self) -> &std::path::Path {
        &self.repo.files.root
    }

    pub(crate) fn repository(&self) -> &'a Repository {
        self.repo
    }

    pub(crate) fn catalog(&self) -> &'a FileCatalog {
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
            .relations
            .get(relation.ordinal as usize)
            .ok_or("merged relation declaration missing")?
            .declaration
            .cardinality)
    }

    pub(crate) fn new(
        repo: &'a Repository,
        base_active: &'a [ActiveRecords],
        segments: &'a [Arc<Vec<Arc<PublishedSegment>>>],
        catalog: &'a FileCatalog,
    ) -> Result<Self, String> {
        if base_active.len() != repo.collections.len() || segments.len() != repo.collections.len() {
            return Err("merged relation collection count mismatch".into());
        }
        for (collection_index, shards) in segments.iter().enumerate() {
            for published in shards.iter() {
                if published.index.collection.0 as usize != collection_index
                    || published.active.slot_count() as usize != published.index.record_count()
                {
                    return Err("merged relation segment scope mismatch".into());
                }
                for (relation_index, relation) in published.index.relations.iter().enumerate() {
                    if relation.record_count() as usize != published.index.record_count()
                        || repo.collections[collection_index]
                            .relations
                            .get(relation_index)
                            .is_none_or(|base| base.declaration != *relation.declaration())
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
        if !published.active.contains(address.slot)
            || published.index.record(address.slot).is_none()
        {
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
        if !published.active.contains(address.slot)
            || published.index.record(address.slot).is_none()
        {
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
        let record = state
            .generation
            .records
            .get(slot.0 as usize)
            .ok_or("flat address slot missing")?;
        self.resolve_id(collection, &record.id.0)?
            .map(|address| {
                self.native_address(address)?
                    .ok_or_else(|| "flat address resolved to a legacy base".to_string())
            })
            .transpose()
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
        let (record, original_revision) = if let Some(segment_index) = address.segment {
            let published = self.segments[index]
                .get(segment_index)
                .ok_or("merged segment missing")?;
            if !published.active.contains(address.slot) {
                return Ok(None);
            }
            let segment = &published.index;
            let record = segment
                .record(address.slot)
                .ok_or("merged segment slot missing")?;
            let file_index = segment
                .file_index
                .get(&record.file)
                .copied()
                .ok_or("merged segment file missing")?;
            let file = segment
                .files
                .get(file_index)
                .map(|file| file.entry.revision)
                .ok_or("merged segment file missing")?;
            (record, file)
        } else {
            if !self.base_active[index].contains(address.slot) {
                return Ok(None);
            }
            let record = collection
                .generation
                .records
                .get(address.slot.0 as usize)
                .ok_or("merged base slot missing")?;
            let file = self
                .repo
                .files
                .files
                .get(record.file.0 as usize)
                .ok_or("merged base file missing")?;
            (record, file.revision)
        };
        let file = self
            .catalog
            .entry(record.file)
            .ok_or("active merged record file is retired")?;
        if file.revision != original_revision || !file.collections.contains(&address.collection) {
            return Err("active merged record file changed without retirement".into());
        }
        Ok(Some(MergedRecord {
            record,
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
        if let Some(slot) = state.id_lookup.get(id).copied()
            && self.base_active[index].contains(slot)
        {
            found = Some(MergedRecordSlot {
                collection,
                segment: None,
                slot,
            });
        }
        for (segment_index, published) in self.segments[index].iter().enumerate() {
            if let Some(slot) = published.index.id_lookup.get(id).copied()
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
        relation_index: usize,
    ) -> Result<(Vec<(&str, &[u32])>, bool, bool, bool, bool), String> {
        let index = address.collection.0 as usize;
        if let Some(segment_index) = address.segment {
            let published = &self.segments[index][segment_index];
            let contribution = published
                .index
                .relations
                .get(relation_index)
                .ok_or("merged segment relation missing")?;
            Ok((
                contribution.references_for_source(address.slot).collect(),
                contribution.present().contains(address.slot.0),
                contribution.nulls().contains(address.slot.0),
                contribution.empty_arrays().contains(address.slot.0),
                contribution.unavailable().contains(address.slot.0),
            ))
        } else {
            let relation = self.repo.collections[index]
                .relations
                .get(relation_index)
                .ok_or("merged base relation missing")?;
            Ok((
                relation
                    .edges(address.slot)
                    .ok_or("merged base relation slot missing")?
                    .iter()
                    .map(|edge| (edge.target_id(), edge.positions.as_slice()))
                    .collect(),
                relation.present().contains(address.slot.0),
                relation.nulls().contains(address.slot.0),
                relation.empty_arrays().contains(address.slot.0),
                relation.unavailable().contains(address.slot.0),
            ))
        }
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
        let base = collection
            .relations
            .get(relation.ordinal as usize)
            .ok_or("backlink relation missing")?;
        let mut sources = Vec::new();
        if let Some(slots) = self.repo.relation_reverse_sources(base, target_id) {
            for slot in slots.iter() {
                let slot = RecordSlot(slot);
                if self.base_active[index].contains(slot) {
                    sources.push(MergedRecordSlot {
                        collection: relation.source,
                        segment: None,
                        slot,
                    });
                }
            }
        }
        for (segment_index, published) in self.segments[index].iter().enumerate() {
            let relation_contribution = published
                .index
                .relations
                .get(relation.ordinal as usize)
                .ok_or("backlink segment relation missing")?;
            if let Some(field) = published
                .index
                .postings
                .fields
                .get(&relation_contribution.declaration().path)
            {
                let Some(posting) = field.value_posting(&ScalarKey::Id(target_id.to_owned()))
                else {
                    continue;
                };
                for slot in posting.iter() {
                    let slot = RecordSlot(slot);
                    if published.active.contains(slot) {
                        sources.push(MergedRecordSlot {
                            collection: relation.source,
                            segment: Some(segment_index),
                            slot,
                        });
                    }
                }
            }
        }
        let mut keyed = Vec::with_capacity(sources.len());
        for address in sources {
            let id = if let Some(segment_index) = address.segment {
                self.segments[index][segment_index]
                    .index
                    .record(address.slot)
                    .map(|record| record.id.0.as_ref())
                    .ok_or("backlink segment slot missing")?
                    .to_string()
            } else {
                collection.generation.records[address.slot.0 as usize]
                    .id
                    .0
                    .to_string()
            };
            keyed.push((id, address));
        }
        keyed.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        keyed.dedup_by(|a, b| a.1 == b.1);
        Ok(keyed.into_iter().map(|(_, address)| address).collect())
    }

    fn reference_value(
        &self,
        address: MergedRecordSlot,
        work: Option<&mut RelationWork>,
    ) -> Result<Option<Arc<Value>>, String> {
        if let Some(value) = self.cached_value(address) {
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
            if let Some(execution) = work.execution.as_ref() {
                execution.check_string()?;
            }
        }
        let path = self.repo.files.root.join(&resolved.file.relative_path);
        let bytes = match read_record_range_for_search(
            &path,
            resolved.file.revision,
            resolved.record.range,
            work.as_deref().and_then(|work| work.execution.as_ref()),
        ) {
            Ok(bytes) => bytes,
            Err(message) => {
                self.record_source_failure(resolved.file, resolved.record.range, message);
                return Ok(None);
            }
        };
        if let Some(work) = work.as_deref_mut() {
            work.source_read(bytes.len() as u64)?;
        }
        let extracted = crate::record_preparation::prepare_query_record_with_execution(
            resolved.record.class,
            &resolved.collection.source,
            &resolved.collection.definition.schema,
            &bytes,
            &ScanProjection::full(false),
            false,
            work.as_deref().and_then(|work| work.execution.as_ref()),
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
        let Some(value) = self.reference_value(address, work)? else {
            return Ok(None);
        };
        let declaration = &self.repo.collections[relation.source.0 as usize]
            .relations
            .get(relation.ordinal as usize)
            .ok_or("reference relation declaration missing")?
            .declaration;
        logical_references(&value, &declaration.path).map(Some)
    }

    pub(crate) fn reference_backlink_sources(
        &self,
        relation: RelationId,
        target_id: &str,
        mut work: Option<&mut RelationWork>,
    ) -> Result<Vec<MergedRecordSlot>, String> {
        let cached = self.reference_backlinks.borrow().get(&relation).cloned();
        if let Some(cached) = cached {
            return Ok(cached.get(target_id).cloned().unwrap_or_default());
        }
        let declaration = &self.repo.collections[relation.source.0 as usize]
            .relations
            .get(relation.ordinal as usize)
            .ok_or("reference backlink declaration missing")?
            .declaration
            .clone();
        let mut keyed = self
            .visible_addresses(relation.source)?
            .into_iter()
            .map(|address| {
                let record = self
                    .record(address)?
                    .ok_or("active reference source record missing")?;
                if record.record.class != crate::inventory::RecordClass::Logical {
                    return Ok(None);
                }
                Ok(Some((record.record.id.0.to_string(), address)))
            })
            .filter_map(|entry| entry.transpose())
            .collect::<Result<Vec<_>, String>>()?;
        keyed.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        let mut backlinks = HashMap::<String, Vec<MergedRecordSlot>>::new();
        for (_, address) in keyed {
            self.visit_reference_value(address, work.as_deref_mut(), |value| {
                let references = logical_references(value, &declaration.path)?;
                let mut seen = std::collections::HashSet::new();
                for (target, _) in references.values {
                    if seen.insert(target.clone()) {
                        backlinks.entry(target).or_default().push(address);
                    }
                }
                Ok(())
            })?;
        }
        let backlinks = Arc::new(backlinks);
        self.reference_backlinks
            .borrow_mut()
            .insert(relation, Arc::clone(&backlinks));
        Ok(backlinks.get(target_id).cloned().unwrap_or_default())
    }

    fn visit_reference_value(
        &self,
        address: MergedRecordSlot,
        mut work: Option<&mut RelationWork>,
        mut visit: impl FnMut(&Value) -> Result<(), String>,
    ) -> Result<bool, String> {
        if let Some(value) = self.cached_value(address) {
            visit(&value)?;
            return Ok(true);
        }
        let Some(resolved) = self.record(address)? else {
            return Ok(false);
        };
        if resolved.record.class == RecordClass::RawFallback {
            return Ok(false);
        }
        if let Some(work) = work.as_deref_mut() {
            work.before_read(resolved.record.range.len(), false)?;
            if let Some(execution) = work.execution.as_ref() {
                execution.check_string()?;
            }
        }
        let path = self.repo.files.root.join(&resolved.file.relative_path);
        let bytes = match read_record_range_for_search(
            &path,
            resolved.file.revision,
            resolved.record.range,
            work.as_deref().and_then(|work| work.execution.as_ref()),
        ) {
            Ok(bytes) => bytes,
            Err(message) => {
                self.record_source_failure(resolved.file, resolved.record.range, message);
                return Ok(false);
            }
        };
        if let Some(work) = work.as_deref_mut() {
            work.source_read(bytes.len() as u64)?;
        }
        let extracted = crate::record_preparation::prepare_query_record_with_execution(
            resolved.record.class,
            &resolved.collection.source,
            &resolved.collection.definition.schema,
            &bytes,
            &ScanProjection::full(false),
            false,
            work.as_deref().and_then(|work| work.execution.as_ref()),
        )?;
        visit(&extracted.value)?;
        Ok(true)
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
                .record(address)?
                .ok_or("active reference identity record missing")?;
            if identity
                .insert(record.record.id.0.to_string(), address)
                .is_some()
            {
                return Err("duplicate active merged record ID".into());
            }
        }
        for segment in self.segments[collection_index].iter() {
            if segment.index.collection != collection {
                return Err("reference identity segment collection mismatch".into());
            }
        }
        Ok(identity)
    }

    fn visible_addresses(&self, collection: CollectionId) -> Result<Vec<MergedRecordSlot>, String> {
        let collection_index = collection.0 as usize;
        let state = self
            .repo
            .collections
            .get(collection_index)
            .ok_or("reference collection missing")?;
        let mut addresses = Vec::new();
        let base_count = state.generation.records.len();
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
            for slot in 0..published.index.record_count() {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EvaluationMode {
    Indexed,
    Reference,
}

/// Physical work caused by relation traversal, separate from the source scan.
#[derive(Clone, Default)]
pub(crate) struct RelationWork {
    pub source_reads: u64,
    pub target_reads: u64,
    pub target_cache_hits: u64,
    pub edge_visits: u64,
    pub backlink_probe_visits: u64,
    pub bytes_read: u64,
    execution: Option<crate::streaming::ExecutionContext>,
    active: Option<Arc<Vec<ActiveRecords>>>,
}

fn charge(counter: &mut u64, amount: u64, limit: u64, name: &str) -> Result<(), String> {
    *counter = counter
        .checked_add(amount)
        .ok_or_else(|| format!("relation {name} overflow"))?;
    if *counter > limit {
        return Err(format!("relation query exceeds {limit} {name}"));
    }
    Ok(())
}

impl RelationWork {
    pub(crate) fn attach(&mut self, execution: Option<&crate::streaming::ExecutionContext>) {
        self.execution = execution.cloned();
        self.active = execution.and_then(|context| context.active_records_arc());
    }

    fn is_active(&self, collection: CollectionId, slot: RecordSlot) -> bool {
        self.active
            .as_ref()
            .is_none_or(|domains| domains[collection.0 as usize].contains(slot))
    }

    fn before_read(&self, bytes: u64, target: bool) -> Result<(), String> {
        if let Some(execution) = &self.execution {
            execution.check_string()?;
            let limit = if target {
                execution.limits.target_record_bytes
            } else {
                execution.limits.source_record_bytes
            };
            if bytes > limit {
                return Err(execution.fail(
                    crate::streaming::ExecutionErrorKind::Resource,
                    "relation record exceeds source read allowance",
                ));
            }
        }
        Ok(())
    }

    fn edge(&mut self) -> Result<(), String> {
        if let Some(execution) = &self.execution {
            execution.charge_edges(1)?;
        }
        charge(
            &mut self.edge_visits,
            1,
            MAX_QUERY_EDGE_VISITS,
            "edge visits",
        )
    }

    fn backlink_probe(&mut self) -> Result<(), String> {
        if let Some(execution) = &self.execution {
            execution.charge_edges(1)?;
        }
        charge(
            &mut self.backlink_probe_visits,
            1,
            MAX_QUERY_BACKLINK_PROBES,
            "backlink probes",
        )
    }

    fn target_read(&mut self, bytes: u64) -> Result<(), String> {
        if let Some(execution) = &self.execution {
            execution.charge_reads(1)?;
            execution.charge_bytes(bytes)?;
        }
        charge(
            &mut self.target_reads,
            1,
            MAX_QUERY_RECORD_READS,
            "target reads",
        )?;
        charge(
            &mut self.bytes_read,
            bytes,
            MAX_QUERY_RELATION_BYTES,
            "source bytes",
        )
    }

    fn source_read(&mut self, bytes: u64) -> Result<(), String> {
        if let Some(execution) = &self.execution {
            execution.charge_reads(1)?;
            execution.charge_bytes(bytes)?;
        }
        charge(
            &mut self.source_reads,
            1,
            MAX_QUERY_RECORD_READS,
            "source reads",
        )?;
        charge(
            &mut self.bytes_read,
            bytes,
            MAX_QUERY_RELATION_BYTES,
            "source bytes",
        )
    }
}

/// Combine bounded scanner chunks into one query-wide relation work budget.
pub(crate) fn check_query_work_budget(
    counters: &crate::measurement::WorkCounters,
) -> Result<(), String> {
    for (actual, limit, name) in [
        (
            counters.relation_edge_visits,
            MAX_QUERY_EDGE_VISITS,
            "edge visits",
        ),
        (
            counters.relation_backlink_probe_visits,
            MAX_QUERY_BACKLINK_PROBES,
            "backlink probes",
        ),
        (
            counters.relation_target_reads,
            MAX_QUERY_RECORD_READS,
            "target reads",
        ),
        (
            counters.relation_source_reads,
            MAX_QUERY_RECORD_READS,
            "source reads",
        ),
        (
            counters.relation_bytes_read,
            MAX_QUERY_RELATION_BYTES,
            "source bytes",
        ),
    ] {
        if actual > limit {
            return Err(format!("relation query exceeds {limit} {name}"));
        }
    }
    Ok(())
}

/// Immutable traversal choices shared by every hop of one predicate.
#[derive(Clone, Copy)]
struct TraverseConfig<'a> {
    repo: &'a Repository,
    hops: &'a [BoundRelationHop],
    quantifier: Quantifier,
    collect_text: bool,
    mode: EvaluationMode,
    target_values: Option<&'a TargetValueCache>,
}

/// Bind a Boolean expression with collection-specific relation declarations.
pub(crate) fn eval_filter_truth(
    repo: &Repository,
    expr: &BoundExpr,
    ctx: &RecordContext<'_>,
    evidence: &mut std::collections::HashSet<String>,
    mode: EvaluationMode,
    work: &mut RelationWork,
) -> Result<Option<bool>, String> {
    match expr {
        BoundExpr::Term(term) => {
            let result = if term.related.is_some() {
                eval_related_term_mode(repo, term, ctx, mode, work, None)?
            } else {
                Some(eval_term(term, ctx)?)
            };
            if result == Some(true)
                && term.derived == crate::query::binding::filter::Derived::None
                && !matches!(
                    term.operator,
                    crate::query::Operator::Ne | crate::query::Operator::NotContains
                )
            {
                evidence.insert(term.query_path.display.clone());
                if let Some(rhs) = &term.rhs {
                    evidence.insert(rhs.query_path.display.clone());
                }
                if let Some(related) = &term.related {
                    evidence.insert(related.evidence_path.clone());
                }
            }
            Ok(result)
        }
        BoundExpr::And(parts) => {
            let mut answer = Some(true);
            let mut fields = std::collections::HashSet::new();
            for part in parts {
                if answer == Some(false) {
                    break;
                }
                let next = eval_filter_truth(repo, part, ctx, &mut fields, mode, work)?;
                answer = match (answer, next) {
                    (Some(false), _) | (_, Some(false)) => Some(false),
                    (None, _) | (_, None) => None,
                    _ => Some(true),
                };
            }
            if answer == Some(true) {
                evidence.extend(fields);
            }
            Ok(answer)
        }
        BoundExpr::Or(parts) => {
            let mut answer = Some(false);
            for part in parts {
                let mut fields = std::collections::HashSet::new();
                let next = eval_filter_truth(repo, part, ctx, &mut fields, mode, work)?;
                answer = match (answer, next) {
                    (Some(true), _) | (_, Some(true)) => Some(true),
                    (None, _) | (_, None) => None,
                    _ => Some(false),
                };
                if next == Some(true) {
                    evidence.extend(fields);
                }
            }
            Ok(answer)
        }
        BoundExpr::Not(inner) => {
            Ok(
                eval_filter_truth(repo, inner, ctx, &mut Default::default(), mode, work)?
                    .map(|value| !value),
            )
        }
    }
}

/// Three-valued membership stops at decisive branches without evidence sets.
pub(crate) fn eval_filter_truth_membership(
    repo: &Repository,
    expr: &BoundExpr,
    ctx: &RecordContext<'_>,
    mode: EvaluationMode,
    work: &mut RelationWork,
) -> Result<Option<bool>, String> {
    match expr {
        BoundExpr::Term(term) => {
            if term.related.is_some() {
                eval_related_term_mode(repo, term, ctx, mode, work, None)
            } else {
                eval_term(term, ctx).map(Some)
            }
        }
        BoundExpr::And(parts) => {
            let mut unknown = false;
            for part in parts {
                match eval_filter_truth_membership(repo, part, ctx, mode, work)? {
                    Some(false) => return Ok(Some(false)),
                    None => unknown = true,
                    Some(true) => {}
                }
            }
            Ok(if unknown { None } else { Some(true) })
        }
        BoundExpr::Or(parts) => {
            let mut unknown = false;
            for part in parts {
                match eval_filter_truth_membership(repo, part, ctx, mode, work)? {
                    Some(true) => return Ok(Some(true)),
                    None => unknown = true,
                    Some(false) => {}
                }
            }
            Ok(if unknown { None } else { Some(false) })
        }
        BoundExpr::Not(inner) => {
            Ok(eval_filter_truth_membership(repo, inner, ctx, mode, work)?.map(|value| !value))
        }
    }
}

/// Evaluate selected local and relation FTS fields with source identity intact.
pub(crate) fn eval_fts_with_relations(
    repo: &Repository,
    fts: &BoundFts,
    segments: &[TextSegment],
    ctx: &RecordContext<'_>,
    evidence: &mut std::collections::HashSet<String>,
    mode: EvaluationMode,
    work: &mut RelationWork,
) -> Result<bool, String> {
    eval_fts_with_relations_inner(repo, fts, segments, ctx, Some(evidence), mode, work)
}

/// Membership-only route avoids constructing evidence and stops at one match.
pub(crate) fn eval_fts_with_relations_membership(
    repo: &Repository,
    fts: &BoundFts,
    segments: &[TextSegment],
    ctx: &RecordContext<'_>,
    mode: EvaluationMode,
    work: &mut RelationWork,
) -> Result<bool, String> {
    eval_fts_with_relations_inner(repo, fts, segments, ctx, None, mode, work)
}

fn eval_fts_with_relations_inner(
    repo: &Repository,
    fts: &BoundFts,
    segments: &[TextSegment],
    ctx: &RecordContext<'_>,
    mut evidence: Option<&mut std::collections::HashSet<String>>,
    mode: EvaluationMode,
    work: &mut RelationWork,
) -> Result<bool, String> {
    if fts.fields.iter().all(|field| field.related.is_none()) {
        return if let Some(evidence) = evidence {
            eval_bound_fts(fts, segments, ctx, evidence)
        } else {
            eval_bound_fts_membership(fts, segments, ctx)
        };
    }
    let borrowed_local_text = fts.fields.iter().any(|field| field.related.is_none())
        && relation_source_uses_value_text(repo, ctx);
    let mut matched = false;
    for field in &fts.fields {
        if let Some(hops) = &field.related {
            let target_collection =
                &repo.collections[hops.last().ok_or("empty FTS relation path")?.to.0 as usize];
            let borrowed_text = relation_target_uses_value_text(target_collection);
            let source_index = repo
                .collections
                .iter()
                .position(|collection| collection.definition.name == ctx.collection_name)
                .ok_or("relation FTS source collection is unknown")?;
            let mut truth = Some(false);
            let config = TraverseConfig {
                repo,
                hops,
                quantifier: Quantifier::Any,
                collect_text: !borrowed_text,
                mode,
                target_values: None,
            };
            visit_hop(
                &config,
                0,
                source_index,
                ctx.record.slot,
                &mut truth,
                work,
                &[],
                &mut |target, target_segments, _| {
                    if borrowed_text {
                        #[cfg(test)]
                        BORROWED_RELATION_TARGETS
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        debug_assert!(target_segments.is_empty());
                        eval_borrowed_value_fts_field(field, fts.case_sensitivity, target, None)
                    } else {
                        eval_bound_fts_field(
                            field,
                            fts.case_sensitivity,
                            target_segments,
                            target,
                            None,
                        )
                    }
                },
            )?;
            if truth == Some(true) {
                matched = true;
                if let Some(evidence) = evidence.as_deref_mut() {
                    evidence.insert(field.evidence_path.clone());
                } else {
                    return Ok(true);
                }
            }
        } else {
            let local_matched = if borrowed_local_text
                && matches!(field.path.root, PathRoot::Data)
                && field.query_path.is_none()
            {
                eval_borrowed_value_fts_field(
                    field,
                    fts.case_sensitivity,
                    ctx,
                    evidence.as_deref_mut(),
                )?
            } else {
                eval_bound_fts_field(
                    field,
                    fts.case_sensitivity,
                    segments,
                    ctx,
                    evidence.as_deref_mut(),
                )?
            };
            if local_matched {
                matched = true;
                if evidence.is_none() {
                    return Ok(true);
                }
            }
        }
    }
    Ok(matched)
}

/// Exact three-valued result for a relation term. Unknown means a dangling or
/// logically unavailable target, so Boolean negation must keep it unknown.
pub(crate) fn eval_related_term(
    repo: &Repository,
    term: &BoundTerm,
    source: &RecordContext<'_>,
    work: &mut RelationWork,
    target_values: Option<&TargetValueCache>,
) -> Result<Option<bool>, String> {
    eval_related_term_mode(
        repo,
        term,
        source,
        EvaluationMode::Indexed,
        work,
        target_values,
    )
}

fn eval_related_term_mode(
    repo: &Repository,
    term: &BoundTerm,
    source: &RecordContext<'_>,
    mode: EvaluationMode,
    work: &mut RelationWork,
    target_values: Option<&TargetValueCache>,
) -> Result<Option<bool>, String> {
    let related = term.related.as_ref().ok_or("relation term is not bound")?;
    let start = repo
        .collections
        .iter()
        .position(|collection| collection.definition.name == source.collection_name)
        .ok_or("relation source collection is unknown")?;
    let mut truth = if term.quantifier == Quantifier::Any {
        Some(false)
    } else {
        Some(true)
    };
    let config = TraverseConfig {
        repo,
        hops: &related.hops,
        quantifier: term.quantifier,
        collect_text: false,
        mode,
        target_values: target_values.filter(|_| {
            related.source_operand.is_some() && related.terminal.path.root == PathRoot::Data
        }),
    };
    visit_hop(
        &config,
        0,
        start,
        source.record.slot,
        &mut truth,
        work,
        &[],
        &mut |target, _, source_positions| {
            if let Some(source_operand) = &related.source_operand {
                eval_cross_record_field_term(
                    &related.terminal,
                    source_operand,
                    target,
                    source,
                    related.target_is_lhs,
                    source_positions,
                    &related.hops[0].selectors,
                )
            } else {
                eval_term(&related.terminal, target)
            }
        },
    )?;
    Ok(truth)
}

fn merge_truth(current: &mut Option<bool>, next: Option<bool>, quantifier: Quantifier) {
    *current = if quantifier == Quantifier::Any {
        match (*current, next) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (None, _) | (_, None) => None,
            _ => Some(false),
        }
    } else {
        match (*current, next) {
            (Some(false), _) | (_, Some(false)) => Some(false),
            (None, _) | (_, None) => None,
            _ => Some(true),
        }
    };
}

fn decisive(truth: Option<bool>, quantifier: Quantifier) -> bool {
    truth == Some(quantifier == Quantifier::Any)
}

fn visit_merged_hop(
    view: &MergedRelationView<'_>,
    hops: &[BoundRelationHop],
    depth: usize,
    address: MergedRecordSlot,
    quantifier: Quantifier,
    collect_text: bool,
    cache_values: bool,
    truth: &mut Option<bool>,
    work: &mut RelationWork,
    reference: bool,
    source_positions: &[u32],
    terminal: &mut impl FnMut(&RecordContext<'_>, &[TextSegment], &[u32]) -> Result<bool, String>,
) -> Result<(), String> {
    if decisive(*truth, quantifier) {
        return Ok(());
    }
    let Some(current) = view.record(address)? else {
        merge_truth(truth, None, quantifier);
        return Ok(());
    };
    if depth == hops.len() {
        if current.record.class == RecordClass::RawFallback {
            merge_truth(truth, None, quantifier);
            return Ok(());
        }
        if current.record.range.len() > MAX_QUERY_RELATION_BYTES {
            return Err(format!(
                "relation target record exceeds {MAX_QUERY_RELATION_BYTES} source bytes"
            ));
        }
        // Distinct targets can load in parallel; repeated visits to one
        // target synchronize through the same shard and decode it once.
        let shard = (address.slot.0 as usize
            ^ address.segment.unwrap_or(0).wrapping_mul(131)
            ^ (address.collection.0 as usize).wrapping_mul(17))
            % view.target_values.len();
        let mut target_cache = cache_values.then(|| view.target_values[shard].lock().unwrap());
        if let Some(value) = target_cache
            .as_ref()
            .and_then(|cache| cache.0.get(&address).cloned())
        {
            work.target_cache_hits += 1;
            drop(target_cache);
            let ctx = RecordContext {
                value: &value,
                source_bytes: None,
                record: current.record,
                file: current.file,
                collection_name: &current.collection.definition.name,
                repository_root: &view.repo.files.root,
            };
            merge_truth(
                truth,
                Some(terminal(&ctx, &[], source_positions)?),
                quantifier,
            );
            return Ok(());
        }
        work.before_read(current.record.range.len(), true)?;
        let path = view.repo.files.root.join(&current.file.relative_path);
        if let Some(execution) = work.execution.as_ref() {
            execution.check_string()?;
        }
        let bytes = match read_record_range_for_search(
            &path,
            current.file.revision,
            current.record.range,
            work.execution.as_ref(),
        ) {
            Ok(bytes) => bytes,
            Err(message) => {
                view.record_source_failure(current.file, current.record.range, message);
                merge_truth(truth, None, quantifier);
                return Ok(());
            }
        };
        work.target_read(bytes.len() as u64)?;
        let extracted = crate::record_preparation::prepare_query_record_with_execution(
            current.record.class,
            &current.collection.source,
            &current.collection.definition.schema,
            &bytes,
            &ScanProjection::full(collect_text),
            false,
            work.execution.as_ref(),
        )?;
        if cache_values {
            let value = Arc::new(extracted.value);
            if let Some(cache) = target_cache.as_mut()
                && bytes.len() <= 64 * 1024
                && cache.1.saturating_add(bytes.len()) <= 512 * 1024
            {
                cache.1 += bytes.len();
                cache.0.insert(address, value.clone());
            }
            drop(target_cache);
            let ctx = RecordContext {
                value: &value,
                source_bytes: None,
                record: current.record,
                file: current.file,
                collection_name: &current.collection.definition.name,
                repository_root: &view.repo.files.root,
            };
            merge_truth(
                truth,
                Some(terminal(&ctx, &[], source_positions)?),
                quantifier,
            );
        } else {
            let ctx = RecordContext {
                value: &extracted.value,
                source_bytes: Some(&bytes),
                record: current.record,
                file: current.file,
                collection_name: &current.collection.definition.name,
                repository_root: &view.repo.files.root,
            };
            merge_truth(
                truth,
                Some(terminal(&ctx, &extracted.text, source_positions)?),
                quantifier,
            );
        }
        return Ok(());
    }
    let hop = &hops[depth];
    if hop.from != address.collection {
        return Err("merged relation hop collection mismatch".into());
    }
    let relation_index = hop.relation.ordinal as usize;
    match hop.direction {
        HopDirection::Forward => {
            if reference {
                let Some(references) =
                    view.reference_forward_edges(address, hop.relation, Some(&mut *work))?
                else {
                    merge_truth(truth, None, quantifier);
                    return Ok(());
                };
                let mut selected = 0usize;
                for (target_id, positions) in references.values {
                    if !positions_match(&hop.selectors, &positions) {
                        continue;
                    }
                    work.edge()?;
                    selected += 1;
                    if let Some(target) = view.reference_resolve_id(hop.to, &target_id)? {
                        visit_merged_hop(
                            view,
                            hops,
                            depth + 1,
                            target,
                            quantifier,
                            collect_text,
                            cache_values,
                            truth,
                            work,
                            true,
                            if depth == 0 {
                                &positions
                            } else {
                                source_positions
                            },
                            terminal,
                        )?;
                    } else {
                        merge_truth(truth, None, quantifier);
                    }
                    if decisive(*truth, quantifier) {
                        break;
                    }
                }
                if selected == 0
                    && (!references.present
                        || references.null
                        || hop.selectors.iter().any(Option::is_some))
                {
                    merge_truth(truth, Some(false), quantifier);
                }
                return Ok(());
            }
            let (references, present, null, _, unavailable) =
                view.forward_edges(address, relation_index)?;
            if unavailable {
                merge_truth(truth, None, quantifier);
                return Ok(());
            }
            let mut selected = 0usize;
            for (target_id, positions) in references {
                if !positions_match(&hop.selectors, positions) {
                    continue;
                }
                work.edge()?;
                selected += 1;
                if let Some(target) = view.resolve_id(hop.to, target_id)? {
                    visit_merged_hop(
                        view,
                        hops,
                        depth + 1,
                        target,
                        quantifier,
                        collect_text,
                        cache_values,
                        truth,
                        work,
                        reference,
                        if depth == 0 {
                            positions
                        } else {
                            source_positions
                        },
                        terminal,
                    )?;
                } else {
                    merge_truth(truth, None, quantifier);
                }
                if decisive(*truth, quantifier) {
                    break;
                }
            }
            if selected == 0 && (!present || null || hop.selectors.iter().any(Option::is_some)) {
                merge_truth(truth, Some(false), quantifier);
            }
        }
        HopDirection::Backlink => {
            let target_id = current.record.id.0.as_ref();
            let sources = if reference {
                view.reference_backlink_sources(hop.relation, target_id, Some(&mut *work))?
            } else {
                view.backlink_sources(hop.relation, target_id)?
            };
            let mut selected = 0usize;
            for (position, source) in sources.into_iter().enumerate() {
                work.backlink_probe()?;
                work.edge()?;
                if hop.selectors.first().is_some_and(|selector| {
                    selector.is_some_and(|index| index as usize != position)
                }) {
                    continue;
                }
                selected += 1;
                visit_merged_hop(
                    view,
                    hops,
                    depth + 1,
                    source,
                    quantifier,
                    collect_text,
                    cache_values,
                    truth,
                    work,
                    reference,
                    source_positions,
                    terminal,
                )?;
                if decisive(*truth, quantifier) {
                    break;
                }
            }
            if selected == 0 && hop.selectors.iter().any(Option::is_some) {
                merge_truth(truth, Some(false), quantifier);
            }
        }
    }
    Ok(())
}

fn eval_related_term_merged(
    view: &MergedRelationView<'_>,
    term: &BoundTerm,
    source_slot: MergedRecordSlot,
    source: &RecordContext<'_>,
    work: &mut RelationWork,
    reference: bool,
) -> Result<Option<bool>, String> {
    let related = term
        .related
        .as_ref()
        .ok_or("merged relation term is not bound")?;
    let mut truth = Some(term.quantifier != Quantifier::Any);
    visit_merged_hop(
        view,
        &related.hops,
        0,
        source_slot,
        term.quantifier,
        false,
        related.source_operand.is_some() && related.terminal.path.root == PathRoot::Data,
        &mut truth,
        work,
        reference,
        &[],
        &mut |target, _, positions| {
            if let Some(source_operand) = &related.source_operand {
                eval_cross_record_field_term(
                    &related.terminal,
                    source_operand,
                    target,
                    source,
                    related.target_is_lhs,
                    positions,
                    &related.hops[0].selectors,
                )
            } else {
                eval_term(&related.terminal, target)
            }
        },
    )?;
    Ok(truth)
}

pub(crate) fn eval_filter_truth_merged(
    view: &MergedRelationView<'_>,
    expr: &BoundExpr,
    source_slot: MergedRecordSlot,
    source: &RecordContext<'_>,
    evidence: &mut std::collections::HashSet<String>,
    work: &mut RelationWork,
    reference: bool,
) -> Result<Option<bool>, String> {
    match expr {
        BoundExpr::Term(term) => {
            let result = if term.related.is_some() {
                eval_related_term_merged(view, term, source_slot, source, work, reference)?
            } else {
                Some(eval_term(term, source)?)
            };
            if result == Some(true)
                && term.derived == crate::query::binding::filter::Derived::None
                && !matches!(
                    term.operator,
                    crate::query::Operator::Ne | crate::query::Operator::NotContains
                )
            {
                evidence.insert(term.query_path.display.clone());
                if let Some(rhs) = &term.rhs {
                    evidence.insert(rhs.query_path.display.clone());
                }
                if let Some(related) = &term.related {
                    evidence.insert(related.evidence_path.clone());
                }
            }
            Ok(result)
        }
        BoundExpr::And(parts) => {
            let mut truth = Some(true);
            let mut fields = std::collections::HashSet::new();
            for part in parts {
                if truth == Some(false) {
                    break;
                }
                let next = eval_filter_truth_merged(
                    view,
                    part,
                    source_slot,
                    source,
                    &mut fields,
                    work,
                    reference,
                )?;
                truth = match (truth, next) {
                    (Some(false), _) | (_, Some(false)) => Some(false),
                    (None, _) | (_, None) => None,
                    _ => Some(true),
                };
            }
            if truth == Some(true) {
                evidence.extend(fields);
            }
            Ok(truth)
        }
        BoundExpr::Or(parts) => {
            let mut truth = Some(false);
            for part in parts {
                let mut fields = std::collections::HashSet::new();
                let next = eval_filter_truth_merged(
                    view,
                    part,
                    source_slot,
                    source,
                    &mut fields,
                    work,
                    reference,
                )?;
                truth = match (truth, next) {
                    (Some(true), _) | (_, Some(true)) => Some(true),
                    (None, _) | (_, None) => None,
                    _ => Some(false),
                };
                if next == Some(true) {
                    evidence.extend(fields);
                }
            }
            Ok(truth)
        }
        BoundExpr::Not(inner) => eval_filter_truth_merged(
            view,
            inner,
            source_slot,
            source,
            &mut Default::default(),
            work,
            reference,
        )
        .map(|truth| truth.map(|value| !value)),
    }
}

pub(crate) fn eval_fts_with_relations_merged(
    view: &MergedRelationView<'_>,
    fts: &BoundFts,
    segments: &[TextSegment],
    source_slot: MergedRecordSlot,
    source: &RecordContext<'_>,
    evidence: &mut std::collections::HashSet<String>,
    work: &mut RelationWork,
    reference: bool,
) -> Result<bool, String> {
    if fts.fields.iter().all(|field| field.related.is_none()) {
        return eval_bound_fts(fts, segments, source, evidence);
    }
    let borrowed_local_text = fts.fields.iter().any(|field| field.related.is_none())
        && relation_source_uses_value_text(view.repo, source);
    let mut matched = false;
    for field in &fts.fields {
        if let Some(hops) = &field.related {
            let target_collection =
                &view.repo.collections[hops.last().ok_or("empty FTS relation path")?.to.0 as usize];
            let borrowed_text = relation_target_uses_value_text(target_collection);
            let mut truth = Some(false);
            visit_merged_hop(
                view,
                hops,
                0,
                source_slot,
                Quantifier::Any,
                !borrowed_text,
                false,
                &mut truth,
                work,
                reference,
                &[],
                &mut |target, target_segments, _| {
                    if borrowed_text {
                        #[cfg(test)]
                        BORROWED_RELATION_TARGETS
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        debug_assert!(target_segments.is_empty());
                        eval_borrowed_value_fts_field(field, fts.case_sensitivity, target, None)
                    } else {
                        eval_bound_fts_field(
                            field,
                            fts.case_sensitivity,
                            target_segments,
                            target,
                            None,
                        )
                    }
                },
            )?;
            if truth == Some(true) {
                matched = true;
                evidence.insert(field.evidence_path.clone());
            }
        } else {
            let local_matched = if borrowed_local_text
                && matches!(field.path.root, PathRoot::Data)
                && field.query_path.is_none()
            {
                eval_borrowed_value_fts_field(field, fts.case_sensitivity, source, Some(evidence))?
            } else {
                eval_bound_fts_field(
                    field,
                    fts.case_sensitivity,
                    segments,
                    source,
                    Some(evidence),
                )?
            };
            matched |= local_matched;
        }
    }
    Ok(matched)
}

fn visit_hop(
    config: &TraverseConfig<'_>,
    depth: usize,
    collection_index: usize,
    source_slot: RecordSlot,
    truth: &mut Option<bool>,
    work: &mut RelationWork,
    source_positions: &[u32],
    terminal: &mut impl FnMut(&RecordContext<'_>, &[TextSegment], &[u32]) -> Result<bool, String>,
) -> Result<(), String> {
    let TraverseConfig {
        repo,
        hops,
        quantifier,
        collect_text,
        mode,
        target_values,
    } = *config;
    if decisive(*truth, quantifier) {
        return Ok(());
    }
    if !work.is_active(
        repo.collections[collection_index].generation.collection,
        source_slot,
    ) {
        merge_truth(truth, None, quantifier);
        return Ok(());
    }
    if depth == hops.len() {
        let collection = &repo.collections[collection_index];
        let record = &collection.generation.records[source_slot.0 as usize];
        if record.class == RecordClass::RawFallback {
            merge_truth(truth, None, quantifier);
            return Ok(());
        }
        if let Some(value) =
            target_values.and_then(|cache| cache.get(collection.generation.collection, source_slot))
        {
            work.target_cache_hits += 1;
            let file = &repo.files.files[record.file.0 as usize];
            let ctx = RecordContext {
                value,
                source_bytes: None,
                record,
                file,
                collection_name: &collection.definition.name,
                repository_root: &repo.files.root,
            };
            merge_truth(
                truth,
                Some(terminal(&ctx, &[], source_positions)?),
                quantifier,
            );
            return Ok(());
        }
        let file = &repo.files.files[record.file.0 as usize];
        let path = repo
            .files
            .native_path(record.file)
            .ok_or("missing target source path")?;
        if record.range.len() > MAX_QUERY_RELATION_BYTES {
            return Err(format!(
                "relation target record exceeds {MAX_QUERY_RELATION_BYTES} source bytes"
            ));
        }
        work.before_read(record.range.len(), true)?;
        let bytes = read_record_range_for_search(
            &path,
            record.revision,
            record.range,
            work.execution.as_ref(),
        )?;
        work.target_read(bytes.len() as u64)?;
        let extracted = crate::record_preparation::prepare_query_record_with_execution(
            record.class,
            &collection.source,
            &collection.definition.schema,
            &bytes,
            &ScanProjection::full(collect_text),
            false,
            work.execution.as_ref(),
        )?;
        let ctx = RecordContext {
            value: &extracted.value,
            source_bytes: Some(&bytes),
            record,
            file,
            collection_name: &collection.definition.name,
            repository_root: &repo.files.root,
        };
        merge_truth(
            truth,
            Some(terminal(&ctx, &extracted.text, source_positions)?),
            quantifier,
        );
        return Ok(());
    }
    let hop = &hops[depth];
    if hop.from.0 as usize != collection_index {
        return Err("relation hop collection mismatch".into());
    }
    let source_collection = &repo.collections[hop.relation.source.0 as usize];
    let relation = source_collection
        .relations
        .get(hop.relation.ordinal as usize)
        .ok_or("relation hop index is missing")?;
    if mode == EvaluationMode::Reference {
        match hop.direction {
            HopDirection::Forward => {
                let Some(value) = read_reference_value(repo, collection_index, source_slot, work)?
                else {
                    merge_truth(truth, None, quantifier);
                    return Ok(());
                };
                let references = logical_references(&value, &relation.declaration.path)?;
                let present = references.present;
                let null = references.null;
                let target_collection = &repo.collections[hop.to.0 as usize];
                let targets = &target_collection.generation.records;
                let mut selected = 0usize;
                for (id, positions) in references.values {
                    if !positions_match(&hop.selectors, &positions) {
                        continue;
                    }
                    work.edge()?;
                    selected += 1;
                    let target_slot = if work.active.is_some() {
                        target_collection
                            .id_lookup
                            .get(id.as_str())
                            .copied()
                            .filter(|slot| work.is_active(hop.to, *slot))
                    } else {
                        targets
                            .iter()
                            .find(|record| record.id.0.as_ref() == id.as_str())
                            .map(|record| record.slot)
                    };
                    if let Some(target_slot) = target_slot {
                        visit_hop(
                            config,
                            depth + 1,
                            hop.to.0 as usize,
                            target_slot,
                            truth,
                            work,
                            if depth == 0 {
                                &positions
                            } else {
                                source_positions
                            },
                            terminal,
                        )?;
                    } else {
                        merge_truth(truth, None, quantifier);
                    }
                    if decisive(*truth, quantifier) {
                        break;
                    }
                }
                if selected == 0 && (!present || null || hop.selectors.iter().any(Option::is_some))
                {
                    merge_truth(truth, Some(false), quantifier);
                }
            }
            HopDirection::Backlink => {
                let target =
                    &repo.collections[collection_index].generation.records[source_slot.0 as usize];
                let mut seen = 0usize;
                let mut selected = 0usize;
                for candidate in &source_collection.identity_order {
                    if !work.is_active(hop.to, *candidate) {
                        continue;
                    }
                    work.backlink_probe()?;
                    let Some(value) =
                        read_reference_value(repo, hop.to.0 as usize, *candidate, work)?
                    else {
                        continue;
                    };
                    let references = logical_references(&value, &relation.declaration.path)?;
                    if !references
                        .values
                        .iter()
                        .any(|(id, _)| id.as_str() == target.id.0.as_ref())
                    {
                        continue;
                    }
                    work.edge()?;
                    if hop
                        .selectors
                        .first()
                        .is_none_or(|selector| selector.is_none_or(|index| index as usize == seen))
                    {
                        selected += 1;
                        visit_hop(
                            config,
                            depth + 1,
                            hop.to.0 as usize,
                            *candidate,
                            truth,
                            work,
                            source_positions,
                            terminal,
                        )?;
                    }
                    seen += 1;
                    if decisive(*truth, quantifier) {
                        break;
                    }
                }
                if selected == 0 && hop.selectors.iter().any(Option::is_some) {
                    merge_truth(truth, Some(false), quantifier);
                }
            }
        }
        return Ok(());
    }
    match hop.direction {
        HopDirection::Forward => {
            if relation.unavailable().contains(source_slot.0) {
                merge_truth(truth, None, quantifier);
                return Ok(());
            }
            let edges = relation
                .edges(source_slot)
                .ok_or("relation source slot is missing")?;
            let mut selected = 0;
            for edge in edges {
                if !positions_match(&hop.selectors, &edge.positions) {
                    continue;
                }
                work.edge()?;
                selected += 1;
                let target = if work.active.is_some() {
                    repo.collections[hop.to.0 as usize]
                        .id_lookup
                        .get(edge.target_id())
                        .copied()
                        .filter(|slot| work.is_active(hop.to, *slot))
                } else if let RelationTarget::Resolved(target) = edge.target {
                    Some(target)
                } else {
                    None
                };
                match target {
                    Some(target) => visit_hop(
                        config,
                        depth + 1,
                        hop.to.0 as usize,
                        target,
                        truth,
                        work,
                        if depth == 0 {
                            &edge.positions
                        } else {
                            source_positions
                        },
                        terminal,
                    )?,
                    None => merge_truth(truth, None, quantifier),
                }
                if decisive(*truth, quantifier) {
                    break;
                }
            }
            if selected == 0
                && (!relation.present().contains(source_slot.0)
                    || relation.nulls().contains(source_slot.0)
                    || hop.selectors.iter().any(Option::is_some))
            {
                merge_truth(truth, Some(false), quantifier);
            }
        }
        HopDirection::Backlink => {
            let target =
                &repo.collections[collection_index].generation.records[source_slot.0 as usize];
            let sources = repo.relation_reverse_sources(relation, &target.id.0);
            let mut seen = 0usize;
            let mut selected = 0usize;
            if let Some(sources) = sources {
                if hop.selectors.iter().all(Option::is_none) {
                    // Without a numeric selector, Boolean truth is independent
                    // of identity-order rank. Visit only actual reverse hits.
                    for source in sources.iter() {
                        if !work.is_active(hop.to, RecordSlot(source)) {
                            continue;
                        }
                        work.backlink_probe()?;
                        work.edge()?;
                        selected += 1;
                        visit_hop(
                            config,
                            depth + 1,
                            hop.to.0 as usize,
                            RecordSlot(source),
                            truth,
                            work,
                            source_positions,
                            terminal,
                        )?;
                        if decisive(*truth, quantifier) {
                            break;
                        }
                    }
                } else {
                    // A positional backlink means rank among sources in
                    // identity order, so retain that ordering here.
                    for source in &source_collection.identity_order {
                        if !work.is_active(hop.to, *source) {
                            continue;
                        }
                        work.backlink_probe()?;
                        if !sources.contains(source.0) {
                            continue;
                        }
                        work.edge()?;
                        if hop.selectors.first().is_none_or(|selector| {
                            selector.is_none_or(|index| index as usize == seen)
                        }) {
                            selected += 1;
                            visit_hop(
                                config,
                                depth + 1,
                                hop.to.0 as usize,
                                *source,
                                truth,
                                work,
                                source_positions,
                                terminal,
                            )?;
                        }
                        seen += 1;
                        if decisive(*truth, quantifier) {
                            break;
                        }
                    }
                }
            }
            if selected == 0 && hop.selectors.iter().any(Option::is_some) {
                merge_truth(truth, Some(false), quantifier);
            }
        }
    }
    Ok(())
}

fn read_reference_value(
    repo: &Repository,
    collection_index: usize,
    slot: RecordSlot,
    work: &mut RelationWork,
) -> Result<Option<crate::model::Value>, String> {
    let collection = &repo.collections[collection_index];
    if !work.is_active(collection.generation.collection, slot) {
        return Ok(None);
    }
    let record = collection
        .generation
        .records
        .get(slot.0 as usize)
        .ok_or("reference relation slot is missing")?;
    if record.class == RecordClass::RawFallback {
        return Ok(None);
    }
    let path = repo
        .files
        .native_path(record.file)
        .ok_or("reference relation path is missing")?;
    if record.range.len() > MAX_QUERY_RELATION_BYTES {
        return Err(format!(
            "relation source record exceeds {MAX_QUERY_RELATION_BYTES} source bytes"
        ));
    }
    work.before_read(record.range.len(), false)?;
    let bytes = read_record_range_for_search(
        &path,
        record.revision,
        record.range,
        work.execution.as_ref(),
    )?;
    work.source_read(bytes.len() as u64)?;
    let extracted = crate::record_preparation::prepare_query_record_with_execution(
        record.class,
        &collection.source,
        &collection.definition.schema,
        &bytes,
        &ScanProjection::full(false),
        false,
        work.execution.as_ref(),
    )?;
    Ok(Some(extracted.value))
}

/// Independent logical ID occurrences and presence/null state.
pub(crate) struct LogicalReferences {
    pub(crate) present: bool,
    pub(crate) null: bool,
    pub(crate) values: Vec<(String, Vec<u32>)>,
}

/// An intentionally separate, iterative oracle for declared ID occurrences.
/// It walks logical values directly and never consults build-time edge state.
fn logical_references(value: &Value, path: &[String]) -> Result<LogicalReferences, String> {
    let mut found = false;
    let mut null = false;
    let mut references = Vec::new();
    let mut pending = vec![(value, 0usize, Vec::<u32>::new())];
    while let Some((value, offset, positions)) = pending.pop() {
        match value {
            Value::Array(array) => {
                if array.values.is_empty() {
                    found = true;
                }
                for (index, item) in array.values.iter().enumerate().rev() {
                    let mut child_positions = positions.clone();
                    child_positions
                        .push(u32::try_from(index).map_err(|_| "relation array index overflow")?);
                    pending.push((item, offset, child_positions));
                }
            }
            Value::Object(fields) if offset < path.len() => {
                if let Some(next) = fields.get(&path[offset]) {
                    pending.push((next, offset + 1, positions));
                }
            }
            Value::Id(id) if offset == path.len() => {
                found = true;
                references.push((id.clone(), positions));
            }
            Value::Null if offset == path.len() => {
                found = true;
                null = true;
            }
            _ if offset == path.len() => return Err("relation field has non-ID value".into()),
            _ => {}
        }
    }
    Ok(LogicalReferences {
        present: found,
        null,
        values: references,
    })
}

pub(crate) fn positions_match(selectors: &[Option<u32>], positions: &[u32]) -> bool {
    selectors.len() <= positions.len()
        && selectors
            .iter()
            .zip(positions)
            .all(|(selector, position)| selector.is_none_or(|index| index == *position))
}

#[cfg(test)]
#[path = "relation_query.test.rs"]
mod tests;
