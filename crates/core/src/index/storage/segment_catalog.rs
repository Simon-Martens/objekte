//! Pinned routing within a collection's segment directory.

use std::any::Any;
use std::borrow::Cow;
use std::sync::Arc;

use smallvec::SmallVec;

use crate::model::{FileId, RecordId, RecordSlot};

use super::liveness::ActiveRecords;
use super::segment::{SegmentId, SegmentReader, visit_record_headers, visit_segment_files};
use super::transfer::{StorageError, StorageResult};

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

    fn position(&self, segment: SegmentId) -> StorageResult<Option<usize>>;
    fn segments_for_file(&self, file: FileId) -> StorageResult<Cow<'_, [SegmentId]>>;
    fn estimated_directory_bytes(&self) -> usize;
    fn edit(&self) -> StorageResult<Box<dyn SegmentCatalogEditor>>;

    /// Attempt an infallible install when the candidate has unique ownership.
    /// A backend that cannot adopt the contribution leaves this handle intact.
    fn try_adopt(&mut self, _source: &dyn SegmentRouteSource, _position: usize) -> bool {
        false
    }
}

/// A sealed segment supplies route inputs without exposing its containers.
pub(crate) trait SegmentRouteSource: Any + Send + Sync {
    fn segment_id(&self) -> SegmentId;

    fn visit_live_records(
        &self,
        visit: &mut dyn FnMut(&[(&RecordId, RecordSlot)]) -> StorageResult<()>,
    ) -> StorageResult<()>;

    fn visit_files(
        &self,
        visit: &mut dyn FnMut(&[FileId]) -> StorageResult<()>,
    ) -> StorageResult<()>;
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
            if live.contains(header.slot) {
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
        source: &'a dyn SegmentRouteSource,
        position: usize,
    },
    Replace {
        removed: &'a [&'a dyn SegmentRouteSource],
        replacement: &'a dyn SegmentRouteSource,
        positions: &'a [SegmentId],
    },
    RetireFile {
        file: FileId,
        segment: SegmentId,
        records: &'a [(RecordId, RecordSlot)],
    },
    RetireRecord(&'a str),
}

pub(crate) trait SegmentCatalogEditor: Send {
    fn apply(&mut self, delta: SegmentDirectoryDelta<'_>) -> StorageResult<()>;
    fn seal(self: Box<Self>) -> StorageResult<SegmentCatalogHandle>;
}

pub(crate) fn install_segment(
    handle: &mut SegmentCatalogHandle,
    source: &dyn SegmentRouteSource,
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

pub(crate) fn build_catalog<'a, S: SegmentRouteSource>(
    factory: &dyn super::StorageFactory,
    segments: impl IntoIterator<Item = &'a S>,
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
