//! Ranking, ordering, cache and delivery result units.

use crate::files::RecordFileInfo;
use crate::inventory::{DiagnosticRef, RecordClass};
use crate::measurement::WorkCounters;

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
