//! Pinned routing within a collection's segment directory.

use std::borrow::Cow;
use std::sync::Arc;

use crate::model::{FileId, RecordId, RecordSlot};

use super::segment::SegmentId;
use super::transfer::StorageResult;

pub(crate) type SegmentCatalogHandle = Arc<dyn SegmentCatalogReader>;

pub(crate) trait SegmentCatalogReader: Send + Sync {
    fn get(&self, id: &str) -> StorageResult<Option<(usize, RecordSlot)>>;
    fn position(&self, segment: SegmentId) -> StorageResult<Option<usize>>;
    fn segments_for_file(&self, file: FileId) -> StorageResult<Cow<'_, [SegmentId]>>;
    fn estimated_directory_bytes(&self) -> usize;
    fn edit(&self) -> StorageResult<Box<dyn SegmentCatalogEditor>>;
}

/// A sealed segment supplies route inputs without exposing its containers.
pub(crate) trait SegmentRouteSource {
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

pub(crate) fn edited_catalog(
    source: &SegmentCatalogHandle,
    delta: SegmentDirectoryDelta<'_>,
) -> StorageResult<SegmentCatalogHandle> {
    let mut editor = source.edit()?;
    editor.apply(delta)?;
    editor.seal()
}

pub(crate) fn build_catalog(
    factory: &dyn super::StorageFactory,
    segments: impl IntoIterator<Item = impl SegmentRouteSource>,
) -> StorageResult<SegmentCatalogHandle> {
    let mut editor = factory.segment_catalog()?;
    for (position, segment) in segments.into_iter().enumerate() {
        editor.apply(SegmentDirectoryDelta::Install {
            source: &segment,
            position,
        })?;
    }
    editor.seal()
}

impl<T: SegmentRouteSource + ?Sized> SegmentRouteSource for &T {
    fn segment_id(&self) -> SegmentId {
        (**self).segment_id()
    }

    fn visit_live_records(
        &self,
        visit: &mut dyn FnMut(&[(&RecordId, RecordSlot)]) -> StorageResult<()>,
    ) -> StorageResult<()> {
        (**self).visit_live_records(visit)
    }

    fn visit_files(
        &self,
        visit: &mut dyn FnMut(&[FileId]) -> StorageResult<()>,
    ) -> StorageResult<()> {
        (**self).visit_files(visit)
    }
}

#[cfg(test)]
mod tests;
