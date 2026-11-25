//! Copy-on-write record and file routes for the native segment directory.

use smallvec::SmallVec;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use super::segment::{PublishedSegment, SegmentId};
use crate::model::{FileId, RecordId, RecordSlot};

const RECORD_ROUTE_SHARDS: usize = 64;

#[derive(Clone, Copy)]
struct RoutedRecord {
    pub(crate) segment: SegmentId,
    pub(crate) slot: RecordSlot,
}

/// Delta ID routes share all untouched shards with older pinned roots.
#[derive(Clone)]
pub(crate) struct RecordRoutes {
    shards: Arc<Vec<Arc<HashMap<RecordId, RoutedRecord>>>>,
    file_shards: Arc<Vec<Arc<HashMap<FileId, SmallVec<[SegmentId; 1]>>>>>,
    positions: Arc<HashMap<SegmentId, usize>>,
}

impl RecordRoutes {
    /// Resident directory capacity for one published root. Record IDs share
    /// payload allocations with headers and are therefore excluded here.
    pub(crate) fn estimated_directory_bytes(&self) -> usize {
        let id_routes = self.shards.iter().fold(0usize, |bytes, shard| {
            bytes.saturating_add(
                shard
                    .capacity()
                    .saturating_mul(std::mem::size_of::<(RecordId, RoutedRecord)>() + 16),
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
                self.positions
                    .capacity()
                    .saturating_mul(std::mem::size_of::<(SegmentId, usize)>() + 16),
            )
            .saturating_add(
                2 * RECORD_ROUTE_SHARDS
                    * std::mem::size_of::<Arc<HashMap<RecordId, RoutedRecord>>>(),
            )
    }

    pub(crate) fn empty() -> Self {
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
        }
    }

    fn shard(id: &str) -> usize {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        id.hash(&mut hasher);
        (hasher.finish() as usize) % RECORD_ROUTE_SHARDS
    }

    pub(crate) fn get(&self, id: &str) -> Option<(usize, RecordSlot)> {
        let address = self.shards[Self::shard(id)].get(id)?;
        Some((*self.positions.get(&address.segment)?, address.slot))
    }

    pub(crate) fn remove(&mut self, id: &str) {
        let shards = Arc::make_mut(&mut self.shards);
        Arc::make_mut(&mut shards[Self::shard(id)]).remove(id);
    }

    pub(crate) fn remove_if_matches(&mut self, id: &str, segment: SegmentId, slot: RecordSlot) {
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

    pub(crate) fn segments_for_file(&self, file: FileId) -> &[SegmentId] {
        self.file_shards[file.0 as usize % RECORD_ROUTE_SHARDS]
            .get(&file)
            .map(SmallVec::as_slice)
            .unwrap_or(&[])
    }

    pub(crate) fn remove_file_segment(&mut self, file: FileId, segment: SegmentId) {
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

    pub(crate) fn position(&self, segment: SegmentId) -> Option<usize> {
        self.positions.get(&segment).copied()
    }

    pub(crate) fn add_segment(&mut self, segment: &PublishedSegment, position: usize) {
        Arc::make_mut(&mut self.positions).insert(segment.id, position);
        let shards = Arc::make_mut(&mut self.shards);
        for (id, &slot) in &segment.index.id_lookup {
            if segment.active.contains(slot) {
                Arc::make_mut(&mut shards[Self::shard(id.0.as_ref())]).insert(
                    id.clone(),
                    RoutedRecord {
                        segment: segment.id,
                        slot,
                    },
                );
            }
        }
        let file_shards = Arc::make_mut(&mut self.file_shards);
        for file in &segment.index.files {
            Arc::make_mut(&mut file_shards[file.entry.id.0 as usize % RECORD_ROUTE_SHARDS])
                .entry(file.entry.id)
                .or_default()
                .push(segment.id);
        }
    }

    /// Replace only routes owned by changed segments. Rebuilding every ID
    /// route after each sparse consolidation makes single-record admission
    /// quadratic in the size of the collection.
    pub(crate) fn replace_segments(
        &mut self,
        removed: &[Arc<PublishedSegment>],
        added: &PublishedSegment,
        group: &[Arc<PublishedSegment>],
    ) {
        let positions = Arc::make_mut(&mut self.positions);
        for segment in removed {
            positions.remove(&segment.id);
        }
        for (position, segment) in group.iter().enumerate() {
            positions.insert(segment.id, position);
        }
        let shards = Arc::make_mut(&mut self.shards);
        for segment in removed {
            for (id, &slot) in &segment.index.id_lookup {
                if segment.active.contains(slot) {
                    let shard = Arc::make_mut(&mut shards[Self::shard(id.0.as_ref())]);
                    if shard
                        .get(id)
                        .is_some_and(|route| route.segment == segment.id)
                    {
                        shard.remove(id);
                    }
                }
            }
        }
        let file_shards = Arc::make_mut(&mut self.file_shards);
        for segment in removed {
            for file in &segment.index.files {
                let shard =
                    Arc::make_mut(&mut file_shards[file.entry.id.0 as usize % RECORD_ROUTE_SHARDS]);
                if let Some(routes) = shard.get_mut(&file.entry.id) {
                    routes.retain(|id| *id != segment.id);
                    if routes.is_empty() {
                        shard.remove(&file.entry.id);
                    }
                }
            }
        }
        self.add_segment(added, group.len() - 1);
    }

    pub(crate) fn from_group(group: &[Arc<PublishedSegment>]) -> Self {
        let mut routes = Self::empty();
        for (position, segment) in group.iter().enumerate() {
            routes.add_segment(segment, position);
        }
        routes
    }
}
