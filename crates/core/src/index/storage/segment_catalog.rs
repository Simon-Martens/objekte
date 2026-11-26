//! Pinned routing within a collection's segment directory.

use std::borrow::Cow;
use std::num::NonZeroUsize;
use std::sync::Arc;

use smallvec::SmallVec;

use crate::model::{FileId, RecordId, RecordSlot};

use super::liveness::ActiveRecords;
use super::publication::{PublishedSegment, SegmentGroup};
use super::segment::{
    NativeSegmentSummary, SegmentDescriptor, SegmentId, SegmentReader, SegmentResourceUsage,
    visit_record_headers, visit_segment_files,
};
use super::transfer::{BatchLimit, BatchStatus, StorageError, StorageResult};

pub(crate) type SegmentCatalogHandle = Arc<dyn SegmentCatalogReader>;

/// Stable local address. Group positions may change at any publication.
#[derive(Clone, Copy)]
pub(crate) struct RoutedSlot {
    pub(crate) segment: SegmentId,
    pub(crate) slot: RecordSlot,
}

pub(crate) struct LocatedSlot {
    pub(crate) route: RoutedSlot,
    pub(crate) position: usize,
}

pub(crate) trait SegmentCatalogReader: Send + Sync {
    fn route(&self, id: &str) -> StorageResult<Option<RoutedSlot>>;

    fn locate(&self, id: &str) -> StorageResult<Option<LocatedSlot>> {
        let Some(route) = self.route(id)? else {
            return Ok(None);
        };
        let position = self.position(route.segment)?.ok_or_else(|| {
            StorageError::invalid(
                "resolve segment route",
                "route points to an unlisted segment",
            )
        })?;
        Ok(Some(LocatedSlot { route, position }))
    }

    fn segment_count(&self) -> usize;

    /// Read descriptors by directory position, in placement order.
    fn read_descriptors(
        &self,
        start: usize,
        limit: BatchLimit,
    ) -> StorageResult<(Cow<'_, [SegmentDescriptor]>, BatchStatus)>;

    fn position(&self, segment: SegmentId) -> StorageResult<Option<usize>>;
    fn placement_at(
        &self,
        position: usize,
    ) -> StorageResult<Option<Cow<'_, Arc<PublishedSegment>>>>;

    /// A resident backend may share its sealed placement order with one request.
    fn shared_placements(&self) -> Option<SegmentGroup> {
        None
    }

    fn read_placements(
        &self,
        start: usize,
        limit: BatchLimit,
    ) -> StorageResult<(Cow<'_, [Arc<PublishedSegment>]>, BatchStatus)> {
        const OP: &str = "read segment placements";
        let total = self.segment_count();
        if start > total {
            return Err(StorageError::invalid(OP, "start beyond last placement"));
        }
        let end = start
            .saturating_add(limit.fixed_batch_len(OP, size_of::<Arc<PublishedSegment>>())?)
            .min(total);
        let mut placements = Vec::with_capacity(end - start);
        for position in start..end {
            let placement = self
                .placement_at(position)?
                .ok_or_else(|| StorageError::invalid(OP, "missing placement in batch"))?;
            placements.push(placement.into_owned());
        }
        Ok((
            Cow::Owned(placements),
            if end == total {
                BatchStatus::Done
            } else {
                BatchStatus::More
            },
        ))
    }

    /// Observations from metadata captured in this immutable revision.
    fn record_counts(&self) -> SegmentRecordCounts;
    fn active_fallback_count(&self) -> StorageResult<u64>;
    fn estimates(&self) -> DirectoryEstimates;
    fn visit_summaries(&self, visit: &mut dyn FnMut(NativeSegmentSummary));
    fn visit_index_families(
        &self,
        visit: &mut dyn FnMut(u64, crate::lifecycle::IndexFamilyEstimate),
    );
    fn observe_backing(&self, usage: &mut SegmentResourceUsage);

    fn segments_for_file(&self, file: FileId) -> StorageResult<Cow<'_, [SegmentId]>>;
    fn estimated_directory_bytes(&self) -> usize;
    fn edit(&self) -> StorageResult<Box<dyn SegmentCatalogEditor>>;

    /// Attempt an infallible install when the candidate has unique ownership.
    /// A backend that cannot adopt the contribution leaves this handle intact.
    fn try_adopt(&mut self, _source: &Arc<PublishedSegment>, _position: usize) -> bool {
        false
    }
}

#[derive(Default)]
pub(crate) struct SegmentRecordCounts {
    pub slots: u64,
    pub live: u64,
    pub live_segments: u64,
}

impl SegmentRecordCounts {
    pub(crate) fn for_segments<'a>(
        segments: impl Iterator<Item = &'a Arc<PublishedSegment>>,
    ) -> Self {
        let mut counts = Self::default();
        for segment in segments {
            counts.slots += u64::from(segment.active.slot_count());
            counts.live += segment.active.len();
            counts.live_segments += u64::from(segment.active.len() != 0);
        }
        counts
    }
}

#[derive(Default)]
pub(crate) struct DirectoryEstimates {
    pub index_bytes: usize,
    pub construction_observation_bytes: usize,
}

impl DirectoryEstimates {
    pub(crate) fn for_segments<'a>(
        segments: impl Iterator<Item = &'a Arc<PublishedSegment>>,
    ) -> Self {
        let mut estimates = Self::default();
        for segment in segments {
            estimates.index_bytes = estimates
                .index_bytes
                .saturating_add(segment.estimates().index_family_bytes);
            estimates.construction_observation_bytes = estimates
                .construction_observation_bytes
                .saturating_add(segment.construction_observation_bytes());
        }
        estimates
    }
}

pub(crate) fn pin_segment_group(
    directory: &dyn SegmentCatalogReader,
) -> StorageResult<SegmentGroup> {
    if let Some(group) = directory.shared_placements() {
        return Ok(group);
    }
    const OP: &str = "pin segment placement group";
    let total = directory.segment_count();
    let max = DESCRIPTOR_LIMIT.fixed_batch_len(OP, size_of::<Arc<PublishedSegment>>())?;
    let mut placements = Vec::with_capacity(total.min(max));
    loop {
        let (page, status) = directory.read_placements(placements.len(), DESCRIPTOR_LIMIT)?;
        let end = placements
            .len()
            .checked_add(page.len())
            .ok_or_else(|| StorageError::invalid(OP, "placement count overflow"))?;
        if page.len() > max
            || end > total
            || (status == BatchStatus::Done) != (end == total)
            || (status == BatchStatus::More && page.is_empty())
        {
            return Err(StorageError::invalid(OP, "invalid placement batch"));
        }
        match page {
            Cow::Borrowed(page) => placements.extend_from_slice(page),
            Cow::Owned(page) => placements.extend(page),
        }
        if status == BatchStatus::Done {
            return Ok(Arc::new(placements));
        }
    }
}

const DESCRIPTOR_LIMIT: BatchLimit = BatchLimit::new(
    NonZeroUsize::new(128).unwrap(),
    NonZeroUsize::new(16 * 1024).unwrap(),
);

pub(crate) fn visit_segment_descriptors(
    directory: &dyn SegmentCatalogReader,
    mut visit: impl FnMut(usize, &SegmentDescriptor) -> StorageResult<()>,
) -> StorageResult<()> {
    const OP: &str = "read segment directory descriptors";
    let total = directory.segment_count();
    let maximum = DESCRIPTOR_LIMIT.fixed_batch_len(OP, size_of::<SegmentDescriptor>())?;
    let mut start = 0usize;
    loop {
        let (rows, status) = directory.read_descriptors(start, DESCRIPTOR_LIMIT)?;
        let end = start
            .checked_add(rows.len())
            .ok_or_else(|| StorageError::invalid(OP, "directory position overflow"))?;
        if rows.len() > maximum
            || end > total
            || (status == BatchStatus::Done) != (end == total)
            || (status == BatchStatus::More && rows.is_empty())
        {
            return Err(StorageError::invalid(OP, "invalid descriptor batch"));
        }
        for (offset, row) in rows.iter().enumerate() {
            visit(start + offset, row)?;
        }
        if status == BatchStatus::Done {
            return Ok(());
        }
        start = end;
    }
}

/// Transfer live identity rows using the placed reader and its liveness.
pub(crate) fn visit_live_routes(
    reader: &dyn SegmentReader,
    live: &ActiveRecords,
    visit: &mut dyn FnMut(&[(&RecordId, RecordSlot)]) -> StorageResult<()>,
) -> StorageResult<()> {
    if reader.descriptor().slots != live.slot_count() as usize {
        return Err(StorageError::invalid(
            "transfer record routes",
            "invalid liveness domain",
        ));
    }
    let mut emitted = 0u64;
    visit_record_headers(reader, |headers| {
        let mut rows = SmallVec::<[_; 64]>::new();
        for header in headers {
            if live.contains(header.slot)? {
                rows.push((&header.id, header.slot));
                emitted += 1;
                if rows.len() == rows.inline_size() {
                    visit(&rows)?;
                    rows.clear();
                }
            }
        }
        if !rows.is_empty() {
            visit(&rows)?;
        }
        Ok::<_, StorageError>(())
    })?;
    if emitted != live.len() {
        return Err(StorageError::invalid(
            "transfer record routes",
            "live membership count changed",
        ));
    }
    Ok(())
}

/// File routing includes empty and path-only associations.
pub(crate) fn visit_file_routes(
    reader: &dyn SegmentReader,
    visit: &mut dyn FnMut(&[FileId]) -> StorageResult<()>,
) -> StorageResult<()> {
    let mut batch = SmallVec::<[_; 64]>::new();
    visit_segment_files(reader, |entry| {
        batch.push(entry.id);
        if batch.len() == batch.inline_size() {
            visit(&batch)?;
            batch.clear();
        }
        Ok::<_, StorageError>(())
    })?;
    if !batch.is_empty() {
        visit(&batch)?;
    }
    Ok(())
}

pub(crate) enum SegmentDirectoryDelta<'a> {
    Install {
        source: &'a Arc<PublishedSegment>,
        position: usize,
    },
    Replace {
        removed: &'a [&'a Arc<PublishedSegment>],
        replacement: &'a Arc<PublishedSegment>,
        positions: &'a [SegmentId],
    },
    RetireFile {
        file: FileId,
        segment: SegmentId,
        records: &'a [(RecordId, RecordSlot)],
        placement: &'a Arc<PublishedSegment>,
    },
    RetireRecord {
        id: &'a str,
        placement: &'a Arc<PublishedSegment>,
    },
    RefreshPlacement(&'a Arc<PublishedSegment>),
}

pub(crate) trait SegmentCatalogEditor: Send {
    fn apply(&mut self, delta: SegmentDirectoryDelta<'_>) -> StorageResult<()>;
    fn seal(self: Box<Self>) -> StorageResult<SegmentCatalogHandle>;
}

pub(crate) fn install_segment(
    handle: &mut SegmentCatalogHandle,
    source: &Arc<PublishedSegment>,
    position: usize,
) -> StorageResult<()> {
    if Arc::get_mut(handle).is_some_and(|candidate| candidate.try_adopt(source, position)) {
        return Ok(());
    }
    *handle = edited_catalog(handle, SegmentDirectoryDelta::Install { source, position })?;
    Ok(())
}

pub(crate) fn edited_catalog(
    source: &SegmentCatalogHandle,
    delta: SegmentDirectoryDelta<'_>,
) -> StorageResult<SegmentCatalogHandle> {
    let mut editor = source.edit()?;
    editor.apply(delta)?;
    editor.seal()
}

pub(crate) fn build_catalog<'a>(
    factory: &dyn super::StorageFactory,
    segments: impl IntoIterator<Item = &'a Arc<PublishedSegment>>,
) -> StorageResult<SegmentCatalogHandle> {
    let mut editor = factory.segment_catalog()?;
    for (position, segment) in segments.into_iter().enumerate() {
        editor.apply(SegmentDirectoryDelta::Install {
            source: segment,
            position,
        })?;
    }
    editor.seal()
}

#[cfg(test)]
mod tests;
