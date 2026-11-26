//! Copy-on-write record and file routes for the native segment directory.

use smallvec::SmallVec;
use std::any::Any;
use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use super::segment::{PublishedSegment, SegmentId};
use crate::index::storage::segment::SegmentDescriptor;
use crate::index::storage::segment_catalog::{
    LocatedSlot, RoutedSlot, SegmentCatalogEditor, SegmentCatalogHandle, SegmentCatalogReader,
    SegmentDirectoryDelta, SegmentRouteSource,
};
use crate::index::storage::transfer::{BatchLimit, BatchStatus, StorageError, StorageResult};
use crate::model::{FileId, RecordId, RecordSlot};

const RECORD_ROUTE_SHARDS: usize = 64;

/// Delta ID routes share all untouched shards with older pinned roots.
#[derive(Clone)]
pub(super) struct RecordRoutes {
    shards: Arc<Vec<Arc<HashMap<RecordId, RoutedSlot>>>>,
    file_shards: Arc<Vec<Arc<HashMap<FileId, SmallVec<[SegmentId; 1]>>>>>,
    positions: Arc<HashMap<SegmentId, usize>>,
    descriptors: Arc<Vec<SegmentDescriptor>>,
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

    fn append_descriptor(&mut self, descriptor: SegmentDescriptor, position: usize) {
        Arc::make_mut(&mut self.positions).insert(descriptor.id, position);
        Arc::make_mut(&mut self.descriptors).push(descriptor);
    }

    fn add_native_routes(&mut self, source: &PublishedSegment) {
        let segment = source.id;
        let shards = Arc::make_mut(&mut self.shards);
        for (id, &slot) in &source.index.id_lookup {
            if source.active.contains(slot) {
                Arc::make_mut(&mut shards[Self::shard(id.0.as_ref())])
                    .insert(id.clone(), RoutedSlot { segment, slot });
            }
        }
        let file_shards = Arc::make_mut(&mut self.file_shards);
        for entry in &source.index.files {
            let file = entry.entry.id;
            Arc::make_mut(&mut file_shards[file.0 as usize % RECORD_ROUTE_SHARDS])
                .entry(file)
                .or_default()
                .push(segment);
        }
    }

    fn add_segment(
        &mut self,
        source: &dyn SegmentRouteSource,
        position: usize,
    ) -> StorageResult<()> {
        if self.try_adopt(source, position) {
            return Ok(());
        }
        let segment = source.segment_id();
        let descriptor = source.segment_descriptor();
        if !self.can_append(descriptor, position) {
            return Err(StorageError::invalid(
                "install segment routes",
                "invalid append position or repeated segment",
            ));
        }
        self.append_descriptor(descriptor, position);
        self.add_routes(source, segment)
    }

    fn add_routes(
        &mut self,
        source: &dyn SegmentRouteSource,
        segment: SegmentId,
    ) -> StorageResult<()> {
        if let Some(native) = (source as &dyn Any).downcast_ref::<PublishedSegment>() {
            self.add_native_routes(native);
            return Ok(());
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
        removed: &[&dyn SegmentRouteSource],
        replacement: &dyn SegmentRouteSource,
        positions: &[SegmentId],
    ) -> StorageResult<()> {
        let segment = replacement.segment_id();
        if !positions.contains(&segment) {
            return Err(crate::index::storage::transfer::StorageError::invalid(
                "replace segment routes",
                "replacement segment has no directory position",
            ));
        }
        let replacement_descriptor = replacement.segment_descriptor();
        let mut descriptors = Vec::with_capacity(positions.len());
        for &id in positions {
            if id == segment {
                descriptors.push(replacement_descriptor);
            } else {
                let old = self
                    .position(id)
                    .and_then(|position| self.descriptors.get(position))
                    .ok_or_else(|| {
                        StorageError::invalid("replace segment routes", "unknown directory segment")
                    })?;
                descriptors.push(*old);
            }
        }
        let indexed_positions = positions
            .iter()
            .copied()
            .enumerate()
            .map(|(position, id)| (id, position))
            .collect::<HashMap<_, _>>();
        if indexed_positions.len() != descriptors.len()
            || removed
                .iter()
                .any(|source| indexed_positions.contains_key(&source.segment_id()))
        {
            return Err(StorageError::invalid(
                "replace segment routes",
                "duplicate or removed segment placement",
            ));
        }
        for source in removed {
            let segment = source.segment_id();
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
        self.descriptors = Arc::new(descriptors);
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
            crate::index::storage::transfer::StorageError::invalid(
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

    fn segments_for_file(&self, file: FileId) -> StorageResult<Cow<'_, [SegmentId]>> {
        Ok(Cow::Borrowed(RecordRoutes::segments_for_file(self, file)))
    }

    fn estimated_directory_bytes(&self) -> usize {
        self.estimated_bytes()
    }

    fn edit(&self) -> StorageResult<Box<dyn SegmentCatalogEditor>> {
        Ok(Box::new(self.clone()))
    }

    fn try_adopt(&mut self, source: &dyn SegmentRouteSource, position: usize) -> bool {
        let Some(source) = (source as &dyn Any).downcast_ref::<PublishedSegment>() else {
            return false;
        };
        if !self.can_append(source.segment_descriptor(), position) {
            return false;
        }
        self.append_descriptor(source.segment_descriptor(), position);
        self.add_native_routes(source);
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
            } => {
                for (record, slot) in records {
                    self.remove_if_matches(record.0.as_ref(), segment, *slot);
                }
                self.remove_file_segment(file, segment);
            }
            SegmentDirectoryDelta::RetireRecord(id) => self.remove(id),
        }
        Ok(())
    }

    fn seal(self: Box<Self>) -> StorageResult<SegmentCatalogHandle> {
        Ok(Arc::from(self as Box<dyn SegmentCatalogReader>))
    }
}
