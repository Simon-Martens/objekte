//! Ranking, ordering, cache and delivery result units.

use std::path::PathBuf;

use crate::files::RecordFileInfo;
use crate::inventory::{DiagnosticRef, FileStatus, RecordClass};
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
    pub file_id: u32,
    pub file_path: PathBuf,
    pub file_size: u64,
    pub file_created_ns: Option<i128>,
    pub byte_start: u64,
    pub byte_end: u64,
    pub modified_ns: i128,
    pub matched_fields: Vec<String>,
    pub file_status: FileStatus,
    pub record_class: RecordClass,
    pub diagnostic_ref: DiagnosticRef,
    pub payload: Option<Payload>,
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
