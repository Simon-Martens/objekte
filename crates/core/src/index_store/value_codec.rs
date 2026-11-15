//! Versioned value records.
//!
//! Values are not used as ordered keys, so metadata uses ordinary serde with
//! bincode and an explicit leading format tag. Every decode checks the tag and
//! converts malformed payloads into `CorruptIndex`; no value record relies on
//! Rust memory layout.

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::inventory::{ByteRange, FileEntry, RecordClass, RecordEntry, SourceRevision};
use crate::model::{CollectionId, FieldPath, FileId, LogicalType, Number, RecordId, RecordSlot, Value};
use crate::posting::ScalarKey;
use crate::sorting::{SortKey, SortOptions};

use super::StoreError;
use super::tables::{
    COVERAGE_SHORT_STRING, DOMAIN_EMPTY_ARRAY, DOMAIN_LOGICAL, DOMAIN_RAW_FALLBACK, GRAM_NORMAL,
    GRAM_SKIP, RELATION_EMPTY_ARRAY, RELATION_NULLS, RELATION_PRESENT, RELATION_UNAVAILABLE,
    STATE_ANY_NULL, STATE_ARRAY, STATE_EMPTY_OBJECT, STATE_EMPTY_SELECTION, STATE_LEGACY_NULL_SET,
    STATE_NON_EMPTY, STATE_NUMBER, STATE_OBJECT, STATE_PATH_PRESENCE, STATE_STRING,
};

pub const VALUE_FORMAT_TAG: u8 = 1;
pub const CONTRIBUTION_FORMAT_TAG: u8 = 3;
pub const STATE_MASK_BITS: u16 = 0x03FF;
pub const INFERENCE_STATUS_NOT_APPLICABLE: u8 = 0;
pub const INFERENCE_STATUS_BUILT: u8 = 1;

/// Store-wide header written during initialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub struct StoreHeader {
    pub format_version: u32,
    pub scalar_codec_version: u32,
    pub fts_codec_version: u32,
    pub collation_version: String,
    pub instance_id: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub enum ConstructionState {
    Empty,
    Discovering,
    Building,
    Complete,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub struct StoreMetadata {
    pub header: StoreHeader,
    pub revision: u64,
    pub build_epoch: u64,
    pub construction: ConstructionState,
    pub catalog_complete: bool,
    pub configuration_digest: [u8; 32],
    pub source_root: Vec<u8>,
    pub next_file_id: u32,
}

impl StoreMetadata {
    pub fn empty(instance_id: u64, configuration_digest: [u8; 32], source_root: Vec<u8>) -> Self {
        Self {
            header: StoreHeader {
                format_version: super::tables::STORE_FORMAT_VERSION,
                scalar_codec_version: super::tables::SCALAR_CODEC_VERSION,
                fts_codec_version: super::tables::FTS_CODEC_VERSION,
                collation_version: super::tables::COLLATION_VERSION.into(),
                instance_id,
            },
            revision: 1,
            build_epoch: 1,
            construction: ConstructionState::Empty,
            catalog_complete: false,
            configuration_digest,
            source_root,
            next_file_id: 0,
        }
    }
}

/// One collection's committed descriptors and counters.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub struct CollectionMetadata {
    pub id: CollectionId,
    pub name: String,
    /// Advances on every complete reset/rebuild.
    pub generation: u64,
    pub source_fingerprint: [u8; 32],
    pub slot_high_water: u64,
    pub live_records: u64,
    pub discovered_files: u64,
    pub pending_files: u64,
    pub indexed_files: u64,
    pub unavailable_files: u64,
    pub processed_files: u64,
    pub next_binding_id: u32,
}

/// Stable identity of one stored indexed representation.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, serde::Deserialize)]
pub enum BindingIdentity {
    /// The schema-declared typed representation of one field.
    Declared { path: Vec<String> },
    /// A named custom field operation.
    Named { path: Vec<String>, name: String },
    /// Raw text of one record collection.
    RawText,
    /// Raw fallback text for malformed records.
    FallbackText,
    /// The logical full-record text representation.
    LogicalText,
    /// A named custom text representation.
    CustomText { path: Vec<String>, name: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub enum BindingRole {
    Structured,
    Text,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub struct BindingDescriptor {
    pub identity: BindingIdentity,
    pub role: BindingRole,
    pub kind: LogicalType,
    pub multi: bool,
    /// Alphabet normalization identity for text bindings.
    pub text_alphabet: Option<String>,
    /// Raw fallback/logical text separation.
    pub text_segment: Option<String>,
    pub fingerprint: [u8; 32],
}

pub type BindingId = u32;
pub type SortDefinitionId = u32;

/// Stored `bindings` row: the assigned identifier plus its descriptor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub struct StoredBinding {
    pub id: BindingId,
    pub descriptor: BindingDescriptor,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub struct FieldDescriptor {
    pub path: FieldPath,
    pub kind: LogicalType,
    pub multi: bool,
    pub indexed: bool,
    pub values_indexed: bool,
    pub value_complete: bool,
    pub length_complete: bool,
    pub empty_string_complete: bool,
    pub covered_records: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub struct SortConstituent {
    pub key: FieldPath,
    pub direction: SortDirection,
    pub options: SortOptions,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub struct SortDefinition {
    pub id: SortDefinitionId,
    pub constituents: Vec<SortConstituent>,
    pub locale: String,
    pub collation_version: String,
}

/// Source-file ownership, revision, diagnostics and file incarnation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub struct FileMetadata {
    pub entry: FileEntry,
    pub file_version: u64,
    pub fingerprint: Option<[u8; 32]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub enum BuildFileState {
    Pending,
    Indexed,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub struct FileBuildState {
    pub state: BuildFileState,
    pub expected_file_version: u64,
    pub build_epoch: u64,
    pub last_admission_revision: u64,
    pub error: Option<String>,
}

/// Record identity and locator metadata. The incarnation is stored inside the
/// record as well so a stale handle can be rejected without reading the
/// allocator table.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub struct RecordMetadata {
    pub id: RecordId,
    pub file: FileId,
    pub range: ByteRange,
    pub revision: SourceRevision,
    pub class: RecordClass,
    pub diagnostic_ref: u32,
    pub incarnation: u64,
}

impl RecordMetadata {
    pub fn from_entry(entry: &RecordEntry, incarnation: u64) -> Self {
        Self {
            id: entry.id.clone(),
            file: entry.file,
            range: entry.range,
            revision: entry.revision,
            class: entry.class,
            diagnostic_ref: entry.diagnostic_ref.0,
            incarnation,
        }
    }

    pub fn to_entry(&self, slot: RecordSlot) -> RecordEntry {
        RecordEntry {
            slot,
            id: self.id.clone(),
            file: self.file,
            range: self.range,
            revision: self.revision,
            class: self.class,
            diagnostic_ref: crate::inventory::DiagnosticRef(self.diagnostic_ref),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, serde::Deserialize)]
pub enum StateKind {
    PathPresence,
    EmptySelection,
    LegacyNullSet,
    AnyNull,
    NonEmpty,
    String,
    Number,
    Array,
    Object,
    EmptyObject,
}

impl StateKind {
    pub fn from_tag(tag: u8) -> Option<Self> {
        Some(match tag {
            STATE_PATH_PRESENCE => Self::PathPresence,
            STATE_EMPTY_SELECTION => Self::EmptySelection,
            STATE_LEGACY_NULL_SET => Self::LegacyNullSet,
            STATE_ANY_NULL => Self::AnyNull,
            STATE_NON_EMPTY => Self::NonEmpty,
            STATE_STRING => Self::String,
            STATE_NUMBER => Self::Number,
            STATE_ARRAY => Self::Array,
            STATE_OBJECT => Self::Object,
            STATE_EMPTY_OBJECT => Self::EmptyObject,
            _ => return None,
        })
    }

    pub fn tag(self) -> u8 {
        match self {
            Self::PathPresence => STATE_PATH_PRESENCE,
            Self::EmptySelection => STATE_EMPTY_SELECTION,
            Self::LegacyNullSet => STATE_LEGACY_NULL_SET,
            Self::AnyNull => STATE_ANY_NULL,
            Self::NonEmpty => STATE_NON_EMPTY,
            Self::String => STATE_STRING,
            Self::Number => STATE_NUMBER,
            Self::Array => STATE_ARRAY,
            Self::Object => STATE_OBJECT,
            Self::EmptyObject => STATE_EMPTY_OBJECT,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub enum DomainKind {
    Logical,
    RawFallback,
    EmptyArray,
}

impl DomainKind {
    pub fn tag(self) -> u8 {
        match self {
            Self::Logical => DOMAIN_LOGICAL,
            Self::RawFallback => DOMAIN_RAW_FALLBACK,
            Self::EmptyArray => DOMAIN_EMPTY_ARRAY,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub enum CoverageKind {
    ShortString,
}

impl CoverageKind {
    pub fn tag(self) -> u8 {
        match self {
            Self::ShortString => COVERAGE_SHORT_STRING,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, serde::Deserialize)]
pub enum GramKind {
    Normal,
    Skip,
}

impl GramKind {
    pub fn tag(self) -> u8 {
        match self {
            Self::Normal => GRAM_NORMAL,
            Self::Skip => GRAM_SKIP,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub enum RelationStateKind {
    Present,
    Nulls,
    EmptyArray,
    Unavailable,
}

impl RelationStateKind {
    pub fn tag(self) -> u8 {
        match self {
            Self::Present => RELATION_PRESENT,
            Self::Nulls => RELATION_NULLS,
            Self::EmptyArray => RELATION_EMPTY_ARRAY,
            Self::Unavailable => RELATION_UNAVAILABLE,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub struct RelationOccurrence {
    pub declaration_ordinal: u32,
    pub occurrence_ordinal: u32,
    pub target_collection: CollectionId,
    pub target_id: String,
    pub positions: Vec<u32>,
}

/// One record's exact sort projection. The encoded key is regenerated from
/// the retained key material when a mutation removes the entry.
#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct SortProjection {
    pub definition: SortDefinitionId,
    pub key: Vec<SortKey>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub struct GramProjection {
    pub binding: BindingId,
    pub kind: GramKind,
    pub grams: Vec<u64>,
}

/// Stored reverse-contribution payload. Gram lists are deliberately absent:
/// they live only in `record_grams`, the single authoritative reverse FTS
/// representation.
#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct ContributionPayload {
    pub record: RecordMetadata,
    pub values: Vec<(BindingId, ScalarKey)>,
    pub lengths: Vec<(BindingId, Number)>,
    pub state_masks: Vec<(BindingId, u16)>,
    pub coverage: Vec<(BindingId, CoverageKind)>,
    pub domains: Vec<DomainKind>,
    pub sorts: Vec<SortProjection>,
    pub relations: Vec<RelationOccurrence>,
}

/// Complete preparation/mutation bundle for one record. The stored payload
/// excludes `grams`; readers join `record_grams` in the same transaction.
#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct StoredContributions {
    pub record: RecordMetadata,
    pub values: Vec<(BindingId, ScalarKey)>,
    pub lengths: Vec<(BindingId, Number)>,
    pub state_masks: Vec<(BindingId, u16)>,
    pub coverage: Vec<(BindingId, CoverageKind)>,
    pub domains: Vec<DomainKind>,
    pub sorts: Vec<SortProjection>,
    pub relations: Vec<RelationOccurrence>,
    pub grams: Vec<GramProjection>,
}

impl StoredContributions {
    pub fn normalize(&mut self) {
        self.values.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        self.values.dedup();
        self.lengths.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        self.lengths.dedup();
        self.state_masks.sort_by(|a, b| a.0.cmp(&b.0));
        self.state_masks.dedup();
        self.state_masks.retain(|(_, mask)| *mask != 0);
        self.coverage
            .sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.tag().cmp(&b.1.tag())));
        self.coverage.dedup();
        self.domains.sort_by_key(|domain| domain.tag());
        self.domains.dedup();
        self.sorts.sort_by_key(|projection| projection.definition);
        self.sorts.dedup_by_key(|projection| projection.definition);
        self.relations.sort_by(|a, b| {
            a.declaration_ordinal
                .cmp(&b.declaration_ordinal)
                .then_with(|| a.occurrence_ordinal.cmp(&b.occurrence_ordinal))
        });
        self.grams
            .sort_by(|a, b| a.binding.cmp(&b.binding).then_with(|| a.kind.tag().cmp(&b.kind.tag())));
        for gram in &mut self.grams {
            gram.grams.sort_unstable();
            gram.grams.dedup();
        }
        self.grams
            .dedup_by(|a, b| a.binding == b.binding && a.kind == b.kind);
    }
}

impl ContributionPayload {
    pub fn from_contributions(contributions: &StoredContributions) -> Self {
        Self {
            record: contributions.record.clone(),
            values: contributions.values.clone(),
            lengths: contributions.lengths.clone(),
            state_masks: contributions.state_masks.clone(),
            coverage: contributions.coverage.clone(),
            domains: contributions.domains.clone(),
            sorts: contributions.sorts.clone(),
            relations: contributions.relations.clone(),
        }
    }

    pub fn into_contributions(self, grams: Vec<GramProjection>) -> StoredContributions {
        StoredContributions {
            record: self.record,
            values: self.values,
            lengths: self.lengths,
            state_masks: self.state_masks,
            coverage: self.coverage,
            domains: self.domains,
            sorts: self.sorts,
            relations: self.relations,
            grams,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub struct ValueDirectoryEntry {
    pub cardinality: u64,
    pub representative: ScalarKey,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub struct LengthDirectoryEntry {
    pub cardinality: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
pub struct FieldCoverage {
    pub kind: LogicalType,
    pub multi: bool,
    pub values_indexed: bool,
    pub value_complete: bool,
    pub length_complete: bool,
    pub empty_string_complete: bool,
    pub covered_records: u32,
    pub inference_status: u8,
    pub value_cardinality: u64,
    pub length_cardinality: u64,
}

/// Validate compact state masks: known bits only, sorted unique field IDs,
/// zero masks omitted.
pub fn validate_state_masks(masks: &[(BindingId, u16)]) -> Result<(), StoreError> {
    let mut previous = None;
    for (binding, mask) in masks {
        if *mask == 0 || *mask & !STATE_MASK_BITS != 0 {
            return Err(StoreError::CorruptIndex(format!(
                "invalid state mask {mask:#06x} for binding {binding}"
            )));
        }
        if previous.is_some_and(|previous| *binding <= previous) {
            return Err(StoreError::CorruptIndex(
                "state masks are not sorted by unique binding".into(),
            ));
        }
        previous = Some(*binding);
    }
    Ok(())
}


pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, StoreError> {
    let payload = bincode::serialize(value)
        .map_err(|error| StoreError::CorruptIndex(format!("value encode failed: {error}")))?;
    let mut bytes = Vec::with_capacity(payload.len() + 1);
    bytes.push(VALUE_FORMAT_TAG);
    bytes.extend_from_slice(&payload);
    Ok(bytes)
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8], what: &str) -> Result<T, StoreError> {
    let (tag, payload) = bytes.split_first().ok_or_else(|| {
        StoreError::CorruptIndex(format!("empty {what} value record"))
    })?;
    if *tag != VALUE_FORMAT_TAG {
        return Err(StoreError::CorruptIndex(format!(
            "unknown {what} value format tag {tag}"
        )));
    }
    bincode::deserialize(payload).map_err(|error| {
        StoreError::CorruptIndex(format!("malformed {what} value record: {error}"))
    })
}

/// Varint list used by `record_grams`. The first value is absolute, every
/// following delta is positive. Trailing bytes are rejected.
pub fn encode_gram_list(grams: &[u64]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(4 + grams.len() * 2);
    let count = u32::try_from(grams.len()).expect("gram count fits u32");
    bytes.extend_from_slice(&count.to_le_bytes());
    let mut previous = 0u64;
    for (index, gram) in grams.iter().copied().enumerate() {
        let value = if index == 0 {
            gram
        } else {
            gram.checked_sub(previous).expect("sorted distinct grams")
        };
        previous = gram;
        write_varint(&mut bytes, value);
    }
    bytes
}

fn write_varint(bytes: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        bytes.push((value as u8 & 0x7F) | 0x80);
        value >>= 7;
    }
    bytes.push(value as u8);
}

pub fn decode_gram_list(bytes: &[u8]) -> Result<Vec<u64>, StoreError> {
    if bytes.len() < 4 {
        return Err(StoreError::CorruptIndex(
            "truncated record gram count".into(),
        ));
    }
    let count = u32::from_le_bytes(bytes[..4].try_into().expect("four-byte count"));
    let mut position = 4;
    let mut grams = Vec::with_capacity(count.min(1 << 20) as usize);
    let mut previous = 0u64;
    for index in 0..count {
        let (value, used) = read_varint(&bytes[position..])?;
        position += used;
        let gram = if index == 0 {
            value
        } else {
            if value == 0 {
                return Err(StoreError::CorruptIndex(
                    "zero delta in record gram list".into(),
                ));
            }
            previous.checked_add(value).ok_or_else(|| {
                StoreError::CorruptIndex("record gram overflow".into())
            })?
        };
        grams.push(gram);
        previous = gram;
    }
    if position != bytes.len() {
        return Err(StoreError::CorruptIndex(
            "trailing bytes in record gram list".into(),
        ));
    }
    Ok(grams)
}

fn read_varint(bytes: &[u8]) -> Result<(u64, usize), StoreError> {
    let mut value = 0u64;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if index >= 10 {
            return Err(StoreError::CorruptIndex("varint too long".into()));
        }
        let part = u64::from(byte & 0x7F);
        value |= part
            .checked_shl((index as u32) * 7)
            .ok_or_else(|| StoreError::CorruptIndex("varint overflow".into()))?;
        if byte & 0x80 == 0 {
            return Ok((value, index + 1));
        }
    }
    Err(StoreError::CorruptIndex("truncated varint".into()))
}

/// Small display helper for metadata panels: the original scalar value for a
/// representative directory payload.
pub fn representative_value(representative: &ScalarKey) -> Value {
    match representative {
        ScalarKey::String(text) => Value::String(text.clone()),
        ScalarKey::Id(text) => Value::Id(text.clone()),
        ScalarKey::Number(number) => Value::Number(number.clone()),
        ScalarKey::Bool(value) => Value::Bool(*value),
        ScalarKey::Date(value) => Value::Date(*value),
        ScalarKey::Datetime(value) => Value::Datetime(*value),
        ScalarKey::Unixtime(value) => Value::Unixtime(*value),
    }
}
