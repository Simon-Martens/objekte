//! Ranking, ordering, cache and delivery result units.

use crate::files::RecordFileInfo;
use crate::index::traits::file_catalog::FileCatalogReader;
use crate::index::traits::segment::SegmentReader;
use crate::index::traits::transfer::{StorageError, StorageResult};
use crate::inventory::{DiagnosticRef, RecordClass};
use crate::measurement::WorkCounters;
use crate::model::RecordSlot;

pub(crate) mod cache;
pub(crate) mod delivery;
pub mod fts_preview;
pub(crate) mod materialize;
pub(crate) mod payload;
pub(crate) mod ranking;
pub(crate) mod verified;

/// One page result with locator, evidence, and optional payload.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub collection: String,
    pub record_id: String,
    pub generation: u64,
    /// Optional origin resolved from the same pinned publication as this hit.
    pub file: Option<RecordFileInfo>,
    pub matched_fields: Vec<String>,
    pub record_class: RecordClass,
    pub diagnostic_ref: DiagnosticRef,
    pub payload: Option<Payload>,
}

impl SearchHit {
    pub(crate) fn remap_segment_diagnostic(
        &mut self,
        reader: &dyn SegmentReader,
        catalog: &dyn FileCatalogReader,
        slot: RecordSlot,
    ) -> StorageResult<()> {
        let (Some(local), Some(file)) = (
            self.diagnostic_ref.stored_index(),
            self.file.as_ref().map(|origin| origin.file_id),
        ) else {
            return Ok(());
        };
        let header = reader
            .header(slot)?
            .filter(|header| header.slot == slot && header.id.0.as_ref() == self.record_id)
            .ok_or_else(|| StorageError::invalid("map hit diagnostic", "invalid routed record"))?;
        if reader
            .origin(header.slot)?
            .and_then(|origin| origin.file())
            .is_none_or(|origin| origin.file != file)
        {
            return Ok(());
        }
        let Some(source) = reader.file_metadata(file)? else {
            return Ok(());
        };
        let Some(diagnostic) = source.diagnostics.get(local) else {
            return Ok(());
        };
        let Some(position) = catalog.file(file)?.and_then(|entry| {
            entry
                .diagnostics
                .iter()
                .position(|candidate| candidate == diagnostic)
        }) else {
            return Ok(());
        };
        let reference = DiagnosticRef((position + 1) as u32);
        self.diagnostic_ref = reference;
        if let Some(Payload::LogicalUnavailable { diagnostic_ref }) = &mut self.payload {
            *diagnostic_ref = reference;
        }
        Ok(())
    }

    pub fn source_range(&self) -> Option<crate::inventory::ByteRange> {
        self.file.as_ref()?.range
    }

    pub fn source_range_len(&self) -> u64 {
        self.source_range()
            .map_or(0, crate::inventory::ByteRange::len)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Payload {
    Bytes(Vec<u8>),
    Logical(crate::model::Value),
    LogicalUnavailable {
        diagnostic_ref: DiagnosticRef,
    },
    /// The record is live, but its registrar did not supply authoritative
    /// original bytes. Logical typed storage is never serialized to invent it.
    OriginalBytesUnavailable,
}

/// Requested exact match count, returned page, and measured work.
#[derive(Clone, Debug)]
pub struct SearchResult {
    pub count: Option<usize>,
    pub hits: Vec<SearchHit>,
    pub counters: WorkCounters,
}
