//! Copy-on-write record and file routes for the native segment directory.

use smallvec::SmallVec;
use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use super::segment::SegmentId;
use crate::index::traits::publication::{PublishedSegment, SegmentGroup};
use crate::index::traits::segment::{
    NativeSegmentSummary, SegmentDescriptor, SegmentReader, SegmentResourceUsage,
};
use crate::index::traits::segment_catalog::{
    DirectoryEstimates, LocatedSlot, RoutedSlot, SegmentCatalogEditor, SegmentCatalogHandle,
    SegmentCatalogReader, SegmentDirectoryDelta, SegmentRecordCounts,
};
use crate::index::traits::transfer::{BatchLimit, BatchStatus, StorageError, StorageResult};
use crate::model::{FileId, RecordId, RecordSlot};

const RECORD_ROUTE_SHARDS: usize = 64;

/// Delta ID routes share all untouched shards with older pinned roots.
#[derive(Clone)]
pub(super) struct RecordRoutes {
    shards: Arc<Vec<Arc<HashMap<RecordId, RoutedSlot>>>>,
    file_shards: Arc<Vec<Arc<HashMap<FileId, SmallVec<[SegmentId; 1]>>>>>,
    positions: Arc<HashMap<SegmentId, usize>>,
    descriptors: Arc<Vec<SegmentDescriptor>>,
    placements: Arc<Vec<Arc<PublishedSegment>>>,
}

impl RecordRoutes {
    /// Resident directory capacity for one published root. Record IDs share
    /// payload allocations with headers and are therefore excluded here.
    fn estimated_bytes(&self) -> usize {
        let id_routes = self.shards.iter().fold(0usize, |bytes, shard| {
            bytes.saturating_add(
                shard
                    .capacity()
                    .saturating_mul(std::mem::size_of::<(RecordId, RoutedSlot)>() + 16),
            )
        });
        let file_routes =
            self.file_shards.iter().fold(0usize, |bytes, shard| {
                bytes
                    .saturating_add(shard.capacity().saturating_mul(
                        std::mem::size_of::<(FileId, SmallVec<[SegmentId; 1]>)>() + 16,
                    ))
                    .saturating_add(
                        shard
                            .values()
                            .map(|segments| {
                                segments
                                    .capacity()
                                    .saturating_sub(1)
                                    .saturating_mul(std::mem::size_of::<SegmentId>())
                            })
                            .sum::<usize>(),
                    )
            });
        id_routes
            .saturating_add(file_routes)
            .saturating_add(
                self.descriptors
                    .capacity()
                    .saturating_mul(size_of::<SegmentDescriptor>()),
            )
            .saturating_add(
                self.placements
                    .capacity()
                    .saturating_mul(size_of::<Arc<PublishedSegment>>()),
            )
            .saturating_add(
                self.positions
                    .capacity()
                    .saturating_mul(std::mem::size_of::<(SegmentId, usize)>() + 16),
            )
            .saturating_add(
                2 * RECORD_ROUTE_SHARDS * std::mem::size_of::<Arc<HashMap<RecordId, RoutedSlot>>>(),
            )
    }

    pub(super) fn empty() -> Self {
        Self {
            shards: Arc::new(
                (0..RECORD_ROUTE_SHARDS)
                    .map(|_| Arc::new(HashMap::new()))
                    .collect(),
            ),
            file_shards: Arc::new(
                (0..RECORD_ROUTE_SHARDS)
                    .map(|_| Arc::new(HashMap::new()))
                    .collect(),
            ),
            positions: Arc::new(HashMap::new()),
            descriptors: Arc::new(Vec::new()),
            placements: Arc::new(Vec::new()),
        }
    }

    fn shard(id: &str) -> usize {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        id.hash(&mut hasher);
        (hasher.finish() as usize) % RECORD_ROUTE_SHARDS
    }

    fn record_route(&self, id: &str) -> Option<RoutedSlot> {
        self.shards[Self::shard(id)].get(id).copied()
    }

    fn remove(&mut self, id: &str) {
        let shards = Arc::make_mut(&mut self.shards);
        Arc::make_mut(&mut shards[Self::shard(id)]).remove(id);
    }

    fn remove_if_matches(&mut self, id: &str, segment: SegmentId, slot: RecordSlot) {
        let shard_number = Self::shard(id);
        if !self.shards[shard_number]
            .get(id)
            .is_some_and(|route| route.segment == segment && route.slot == slot)
        {
            return;
        }
        let shards = Arc::make_mut(&mut self.shards);
        Arc::make_mut(&mut shards[shard_number]).remove(id);
    }

    fn segments_for_file(&self, file: FileId) -> &[SegmentId] {
        self.file_shards[file.0 as usize % RECORD_ROUTE_SHARDS]
            .get(&file)
            .map(SmallVec::as_slice)
            .unwrap_or(&[])
    }

    fn remove_file_segment(&mut self, file: FileId, segment: SegmentId) {
        let shard_number = file.0 as usize % RECORD_ROUTE_SHARDS;
        if !self.file_shards[shard_number]
            .get(&file)
            .is_some_and(|routes| routes.contains(&segment))
        {
            return;
        }
        let shards = Arc::make_mut(&mut self.file_shards);
        let shard = Arc::make_mut(&mut shards[shard_number]);
        if let Some(routes) = shard.get_mut(&file) {
            routes.retain(|id| *id != segment);
            if routes.is_empty() {
                shard.remove(&file);
            }
        }
    }

    fn position(&self, segment: SegmentId) -> Option<usize> {
        self.positions.get(&segment).copied()
    }

    fn can_append(&self, descriptor: SegmentDescriptor, position: usize) -> bool {
        position == self.descriptors.len() && !self.positions.contains_key(&descriptor.id)
    }

    fn append_placement(&mut self, placement: &Arc<PublishedSegment>, position: usize) {
        let descriptor = placement.descriptor();
        Arc::make_mut(&mut self.positions).insert(descriptor.id, position);
        Arc::make_mut(&mut self.descriptors).push(descriptor);
        Arc::make_mut(&mut self.placements).push(Arc::clone(placement));
    }

    fn refresh_placement(&mut self, placement: &Arc<PublishedSegment>) -> StorageResult<()> {
        let position = self.position(placement.id).ok_or_else(|| {
            StorageError::invalid("refresh segment placement", "segment is absent")
        })?;
        if self.descriptors[position] != placement.descriptor() {
            return Err(StorageError::invalid(
                "refresh segment placement",
                "descriptor changed",
            ));
        }
        Arc::make_mut(&mut self.placements)[position] = Arc::clone(placement);
        Ok(())
    }

    fn add_native_routes(
        &mut self,
        source: &PublishedSegment,
        payload: &super::segment::IndexedCollectionSegment,
        all_live: bool,
    ) -> StorageResult<()> {
        let segment = source.id;
        let shards = Arc::make_mut(&mut self.shards);
        for (id, &slot) in &payload.id_lookup {
            if all_live || source.active.contains(slot)? {
                Arc::make_mut(&mut shards[Self::shard(id.0.as_ref())])
                    .insert(id.clone(), RoutedSlot { segment, slot });
            }
        }
        let file_shards = Arc::make_mut(&mut self.file_shards);
        for entry in &payload.files {
            let file = entry.entry.id;
            Arc::make_mut(&mut file_shards[file.0 as usize % RECORD_ROUTE_SHARDS])
                .entry(file)
                .or_default()
                .push(segment);
        }
        Ok(())
    }

    fn add_segment(
        &mut self,
        source: &Arc<PublishedSegment>,
        position: usize,
    ) -> StorageResult<()> {
        if self.try_adopt(source, position) {
            return Ok(());
        }
        let segment = source.id;
        let descriptor = source.descriptor();
        if !self.can_append(descriptor, position) {
            return Err(StorageError::invalid(
                "install segment routes",
                "invalid append position or repeated segment",
            ));
        }
        self.append_placement(source, position);
        self.add_routes(source, segment)
    }

    fn add_routes(
        &mut self,
        source: &Arc<PublishedSegment>,
        segment: SegmentId,
    ) -> StorageResult<()> {
        if let Some(payload) = super::segment::maintenance::native_segment(source.index.as_ref()) {
            return self.add_native_routes(
                source,
                payload,
                source.active.len() == u64::from(source.active.slot_count()),
            );
        }
        let shards = Arc::make_mut(&mut self.shards);
        source.visit_live_records(&mut |rows| {
            for &(id, slot) in rows {
                Arc::make_mut(&mut shards[Self::shard(id.0.as_ref())])
                    .insert(id.clone(), RoutedSlot { segment, slot });
            }
            Ok(())
        })?;
        let file_shards = Arc::make_mut(&mut self.file_shards);
        source.visit_files(&mut |files| {
            for &file in files {
                Arc::make_mut(&mut file_shards[file.0 as usize % RECORD_ROUTE_SHARDS])
                    .entry(file)
                    .or_default()
                    .push(segment);
            }
            Ok(())
        })
    }

    /// Reuse untouched route shards when the coordinator consolidates
    /// several source segments into one replacement segment.
    fn replace_segments(
        &mut self,
        removed: &[&Arc<PublishedSegment>],
        replacement: &Arc<PublishedSegment>,
        positions: &[SegmentId],
    ) -> StorageResult<()> {
        let segment = replacement.id;
        if !positions.contains(&segment) {
            return Err(crate::index::traits::transfer::StorageError::invalid(
                "replace segment routes",
                "replacement segment has no directory position",
            ));
        }
        let mut placements = Vec::with_capacity(positions.len());
        for &id in positions {
            let placement = if id == segment {
                replacement
            } else {
                self.position(id)
                    .and_then(|position| self.placements.get(position))
                    .ok_or_else(|| {
                        StorageError::invalid("replace segment routes", "unknown directory segment")
                    })?
            };
            placements.push(Arc::clone(placement));
        }
        let indexed_positions = positions
            .iter()
            .copied()
            .enumerate()
            .map(|(position, id)| (id, position))
            .collect::<HashMap<_, _>>();
        if indexed_positions.len() != placements.len()
            || removed
                .iter()
                .any(|source| indexed_positions.contains_key(&source.id))
        {
            return Err(StorageError::invalid(
                "replace segment routes",
                "duplicate or removed segment placement",
            ));
        }
        for source in removed {
            let segment = source.id;
            source.visit_live_records(&mut |rows| {
                for &(id, slot) in rows {
                    self.remove_if_matches(id.0.as_ref(), segment, slot);
                }
                Ok(())
            })?;
            source.visit_files(&mut |files| {
                for &file in files {
                    self.remove_file_segment(file, segment);
                }
                Ok(())
            })?;
        }
        self.positions = Arc::new(indexed_positions);
        self.descriptors = Arc::new(placements.iter().map(SegmentReader::descriptor).collect());
        self.placements = Arc::new(placements);
        self.add_routes(replacement, segment)
    }
}

impl SegmentCatalogReader for RecordRoutes {
    fn route(&self, id: &str) -> StorageResult<Option<RoutedSlot>> {
        Ok(self.record_route(id))
    }

    fn locate(&self, id: &str) -> StorageResult<Option<LocatedSlot>> {
        let Some(route) = self.record_route(id) else {
            return Ok(None);
        };
        let position = self.position(route.segment).ok_or_else(|| {
            crate::index::traits::transfer::StorageError::invalid(
                "resolve native segment route",
                "route points to an unlisted segment",
            )
        })?;
        Ok(Some(LocatedSlot { route, position }))
    }

    fn segment_count(&self) -> usize {
        self.descriptors.len()
    }

    fn read_descriptors(
        &self,
        start: usize,
        limit: BatchLimit,
    ) -> StorageResult<(Cow<'_, [SegmentDescriptor]>, BatchStatus)> {
        let remaining = self.descriptors.get(start..).ok_or_else(|| {
            StorageError::invalid("read native directory", "start beyond last descriptor")
        })?;
        let count = remaining
            .len()
            .min(limit.fixed_batch_len("read native directory", size_of::<SegmentDescriptor>())?);
        let status = if count == remaining.len() {
            BatchStatus::Done
        } else {
            BatchStatus::More
        };
        Ok((Cow::Borrowed(&remaining[..count]), status))
    }

    fn position(&self, segment: SegmentId) -> StorageResult<Option<usize>> {
        Ok(RecordRoutes::position(self, segment))
    }

    fn placement_at(
        &self,
        position: usize,
    ) -> StorageResult<Option<Cow<'_, Arc<PublishedSegment>>>> {
        Ok(self.placements.get(position).map(Cow::Borrowed))
    }

    fn shared_placements(&self) -> Option<SegmentGroup> {
        Some(Arc::clone(&self.placements))
    }

    fn record_counts(&self) -> SegmentRecordCounts {
        SegmentRecordCounts::for_segments(self.placements.iter())
    }

    fn active_fallback_count(&self) -> StorageResult<u64> {
        self.placements.iter().try_fold(0, |sum, segment| {
            Ok(sum + segment.active_fallback_count(&segment.active)?)
        })
    }

    fn estimates(&self) -> DirectoryEstimates {
        DirectoryEstimates::for_segments(self.placements.iter())
    }

    fn visit_summaries(&self, visit: &mut dyn FnMut(NativeSegmentSummary)) {
        for segment in self.placements.iter() {
            visit(segment.summary(segment.active.len() as usize));
        }
    }

    fn visit_index_families(
        &self,
        visit: &mut dyn FnMut(u64, crate::lifecycle::IndexFamilyEstimate),
    ) {
        for segment in self.placements.iter() {
            visit(segment.id.get(), segment.index_family_estimate());
        }
    }

    fn observe_backing(&self, usage: &mut SegmentResourceUsage) {
        for segment in self.placements.iter() {
            segment.observe_backing(usage);
        }
    }

    fn segments_for_file(&self, file: FileId) -> StorageResult<Cow<'_, [SegmentId]>> {
        Ok(Cow::Borrowed(RecordRoutes::segments_for_file(self, file)))
    }

    fn estimated_directory_bytes(&self) -> usize {
        self.estimated_bytes()
    }

    fn edit(&self) -> StorageResult<Box<dyn SegmentCatalogEditor>> {
        Ok(Box::new(self.clone()))
    }

    fn try_adopt(&mut self, source: &Arc<PublishedSegment>, position: usize) -> bool {
        let Some(payload) = super::segment::maintenance::native_segment(source.index.as_ref())
        else {
            return false;
        };
        if !self.can_append(source.descriptor(), position)
            || source.active.len() != u64::from(source.active.slot_count())
        {
            return false;
        }
        self.append_placement(source, position);
        self.add_native_routes(source, payload, true)
            .expect("all slots live");
        true
    }
}

impl SegmentCatalogEditor for RecordRoutes {
    fn apply(&mut self, delta: SegmentDirectoryDelta<'_>) -> StorageResult<()> {
        match delta {
            SegmentDirectoryDelta::Install { source, position } => {
                self.add_segment(source, position)?
            }
            SegmentDirectoryDelta::Replace {
                removed,
                replacement,
                positions,
            } => {
                self.replace_segments(removed, replacement, positions)?;
            }
            SegmentDirectoryDelta::RetireFile {
                file,
                segment,
                records,
                placement,
            } => {
                for (record, slot) in records {
                    self.remove_if_matches(record.0.as_ref(), segment, *slot);
                }
                self.remove_file_segment(file, segment);
                self.refresh_placement(placement)?;
            }
            SegmentDirectoryDelta::RetireRecord { id, placement } => {
                self.remove(id);
                self.refresh_placement(placement)?;
            }
            SegmentDirectoryDelta::RefreshPlacement(placement) => {
                self.refresh_placement(placement)?
            }
        }
        Ok(())
    }

    fn seal(self: Box<Self>) -> StorageResult<SegmentCatalogHandle> {
        Ok(Arc::from(self as Box<dyn SegmentCatalogReader>))
    }
}
