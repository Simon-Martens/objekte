//! Central registry of every redb application table and its key layout.
//!
//! All tables use byte-slice keys and byte-slice values. The ordered key
//! encoding lives in [`super::key_codec`]; this module only names the tables,
//! fixes their component order, and exposes the registry used for creation,
//! validation, and complete deletion.

use redb::TableDefinition;

use super::StoreError;

pub const STORE_FORMAT_VERSION: u32 = 1;
pub const SCALAR_CODEC_VERSION: u32 = 1;
pub const FTS_CODEC_VERSION: u32 = 1;
pub const COLLATION_VERSION: &str = "icu-2.4.0";
pub const PAGE_CACHE_BYTES: usize = 64 * 1024 * 1024;

pub const FTS_BLOCK_SLOTS: u32 = 4_096;
pub const FTS_BLOCK_SHIFT: u32 = 12;
pub const FTS_BLOCK_MASK: u32 = FTS_BLOCK_SLOTS - 1;
pub const FTS_MAX_CARDINALITY: u16 = 4_096;
pub const FTS_SPARSE_LIMIT: u16 = 255;

pub type ByteTable = TableDefinition<'static, &'static [u8], &'static [u8]>;

macro_rules! byte_table {
    ($name:literal) => {
        TableDefinition::new($name)
    };
}

pub const STORE_METADATA: ByteTable = byte_table!("store_metadata");
pub const COLLECTIONS: ByteTable = byte_table!("collections");
pub const FIELDS: ByteTable = byte_table!("fields");
pub const BINDINGS: ByteTable = byte_table!("bindings");
pub const FILES: ByteTable = byte_table!("files");
pub const FILE_BUILD_STATE: ByteTable = byte_table!("file_build_state");
pub const PENDING_FILES: ByteTable = byte_table!("pending_files");
pub const PATHS: ByteTable = byte_table!("paths");
pub const QUERY_PATHS: ByteTable = byte_table!("query_paths");
pub const RECORDS: ByteTable = byte_table!("records");
pub const LOGICAL_IDS: ByteTable = byte_table!("logical_ids");
pub const FILE_RECORDS: ByteTable = byte_table!("file_records");
pub const SLOT_INCARNATIONS: ByteTable = byte_table!("slot_incarnations");
pub const FREE_SLOTS: ByteTable = byte_table!("free_slots");
pub const RECORD_CONTRIBUTIONS: ByteTable = byte_table!("record_contributions");
pub const VALUE_MEMBERS: ByteTable = byte_table!("value_members");
pub const LENGTH_MEMBERS: ByteTable = byte_table!("length_members");
pub const STATE_MEMBERS: ByteTable = byte_table!("state_members");
pub const VALUE_DIRECTORY: ByteTable = byte_table!("value_directory");
pub const LENGTH_DIRECTORY: ByteTable = byte_table!("length_directory");
pub const FIELD_COVERAGE: ByteTable = byte_table!("field_coverage");
pub const COVERAGE_MEMBERS: ByteTable = byte_table!("coverage_members");
pub const DOMAIN_MEMBERS: ByteTable = byte_table!("domain_members");
pub const SORT_DEFINITIONS: ByteTable = byte_table!("sort_definitions");
pub const SORT_ENTRIES: ByteTable = byte_table!("sort_entries");
pub const RELATION_OUTGOING: ByteTable = byte_table!("relation_outgoing");
pub const RELATION_INCOMING: ByteTable = byte_table!("relation_incoming");
pub const RELATION_STATES: ByteTable = byte_table!("relation_states");
pub const FTS_BLOCKS: ByteTable = byte_table!("fts_blocks");
pub const FTS_GRAM_STATISTICS: ByteTable = byte_table!("fts_gram_statistics");
pub const RECORD_GRAMS: ByteTable = byte_table!("record_grams");

/// Every application table, in registry order. The list is exhaustive:
/// [`super::regeneration::reset_tables`] refuses to run when the initialized
/// database contains any table outside it.
pub const ALL_TABLES: &[(&str, ByteTable)] = &[
    ("store_metadata", STORE_METADATA),
    ("collections", COLLECTIONS),
    ("fields", FIELDS),
    ("bindings", BINDINGS),
    ("files", FILES),
    ("file_build_state", FILE_BUILD_STATE),
    ("pending_files", PENDING_FILES),
    ("paths", PATHS),
    ("query_paths", QUERY_PATHS),
    ("records", RECORDS),
    ("logical_ids", LOGICAL_IDS),
    ("file_records", FILE_RECORDS),
    ("slot_incarnations", SLOT_INCARNATIONS),
    ("free_slots", FREE_SLOTS),
    ("record_contributions", RECORD_CONTRIBUTIONS),
    ("value_members", VALUE_MEMBERS),
    ("length_members", LENGTH_MEMBERS),
    ("state_members", STATE_MEMBERS),
    ("value_directory", VALUE_DIRECTORY),
    ("length_directory", LENGTH_DIRECTORY),
    ("field_coverage", FIELD_COVERAGE),
    ("coverage_members", COVERAGE_MEMBERS),
    ("domain_members", DOMAIN_MEMBERS),
    ("sort_definitions", SORT_DEFINITIONS),
    ("sort_entries", SORT_ENTRIES),
    ("relation_outgoing", RELATION_OUTGOING),
    ("relation_incoming", RELATION_INCOMING),
    ("relation_states", RELATION_STATES),
    ("fts_blocks", FTS_BLOCKS),
    ("fts_gram_statistics", FTS_GRAM_STATISTICS),
    ("record_grams", RECORD_GRAMS),
];

/// Metadata row names. Values are versioned records (see `value_codec`).
pub mod meta {
    pub const FORMAT: &[u8] = b"format";
    pub const REVISION: &[u8] = b"revision";
    pub const INSTANCE: &[u8] = b"instance";
    pub const BUILD_EPOCH: &[u8] = b"build_epoch";
    pub const CONSTRUCTION: &[u8] = b"construction";
    pub const CONFIGURATION_DIGEST: &[u8] = b"configuration_digest";
    pub const SOURCE_ROOT: &[u8] = b"source_root";
    pub const NEXT_FILE_ID: &[u8] = b"next_file_id";
    pub const CATALOG_COMPLETE: &[u8] = b"catalog_complete";
    pub const CODECS: &[u8] = b"codecs";
}

/// Structural predicate tags. These names describe the existing semantic
/// distinctions; the numeric values are part of the store format.
pub const STATE_PATH_PRESENCE: u8 = 0;
pub const STATE_EMPTY_SELECTION: u8 = 1;
pub const STATE_LEGACY_NULL_SET: u8 = 2;
pub const STATE_ANY_NULL: u8 = 3;
pub const STATE_NON_EMPTY: u8 = 4;
pub const STATE_STRING: u8 = 5;
pub const STATE_NUMBER: u8 = 6;
pub const STATE_ARRAY: u8 = 7;
pub const STATE_OBJECT: u8 = 8;
pub const STATE_EMPTY_OBJECT: u8 = 9;

pub const MAX_STATE_TAG: u8 = STATE_EMPTY_OBJECT;

/// Semantic domains stored in `domain_members`.
pub const DOMAIN_LOGICAL: u8 = 0;
pub const DOMAIN_RAW_FALLBACK: u8 = 1;
pub const DOMAIN_EMPTY_ARRAY: u8 = 2;

/// Gram kind tags for the FTS tables.
pub const GRAM_NORMAL: u8 = 0;
pub const GRAM_SKIP: u8 = 1;

/// Coverage kinds stored in `coverage_members`.
pub const COVERAGE_SHORT_STRING: u8 = 0;

/// Relation structural state kinds stored in `relation_states`.
pub const RELATION_PRESENT: u8 = 0;
pub const RELATION_NULLS: u8 = 1;
pub const RELATION_EMPTY_ARRAY: u8 = 2;
pub const RELATION_UNAVAILABLE: u8 = 3;

pub fn is_registered_table(name: &str) -> bool {
    ALL_TABLES.iter().any(|(registered, _)| *registered == name)
}

/// Validate a field/binding ID or relation ordinal read from a value record.
pub fn checked_component<T>(value: Option<T>, what: &str) -> Result<T, StoreError> {
    value.ok_or_else(|| StoreError::CorruptIndex(format!("missing {what}")))
}
