//! Versioned metadata and value envelopes for the RocksDB prototype.

use bincode::Options;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::inventory::{ByteRange, FileEntry, RecordClass, RecordEntry, SourceRevision};
use crate::model::{CollectionId, FieldPath, FileId, LogicalType, Number, RecordId, RecordSlot};
use crate::posting::ScalarKey;
use crate::sorting::{SortKey, SortOptions};

use super::{StoreError, StoreResult};

pub const MAGIC: &str = "OBJEKTE_ROCKSDB_PROTOTYPE";
pub const FORMAT_VERSION: u32 = 1;
pub const SCALAR_CODEC_VERSION: u32 = 1;
pub const BITMAP_CODEC_VERSION: u32 = 1;
pub const CONTRIBUTION_CODEC_VERSION: u32 = 1;
pub const VALUE_FORMAT_TAG: u8 = 1;
pub const COLLATION_VERSION: &str = "icu-2.4.0";
pub const STATE_MASK_BITS: u16 = 0x03FF;
pub const INFERENCE_STATUS_NOT_APPLICABLE: u8 = 0;
pub const INFERENCE_STATUS_BUILT: u8 = 1;

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

pub const DOMAIN_LOGICAL: u8 = 0;
pub const DOMAIN_RAW_FALLBACK: u8 = 1;
pub const DOMAIN_EMPTY_ARRAY: u8 = 2;
pub const COVERAGE_SHORT_STRING: u8 = 0;
pub const GRAM_NORMAL: u8 = 0;
pub const GRAM_SKIP: u8 = 1;
pub const RELATION_PRESENT: u8 = 0;
pub const RELATION_NULLS: u8 = 1;
pub const RELATION_EMPTY_ARRAY: u8 = 2;
pub const RELATION_UNAVAILABLE: u8 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum BuildPhase {
    Building,
    Finalizing,
    Complete,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StoreHeader {
    pub magic: String,
    pub format_version: u32,
    pub scalar_codec_version: u32,
    pub bitmap_codec_version: u32,
    pub contribution_codec_version: u32,
    pub collation_version: String,
    pub configuration_digest: [u8; 32],
    pub alphabet: String,
    pub source_root: Vec<u8>,
    pub build_identity: [u8; 32],
    pub build_epoch: u64,
    pub storage_mode: String,
    pub phase: BuildPhase,
}

impl StoreHeader {
    pub fn new(
        configuration_digest: [u8; 32],
        alphabet: impl Into<String>,
        source_root: Vec<u8>,
        build_identity: [u8; 32],
        storage_mode: impl Into<String>,
    ) -> Self {
        Self {
            magic: MAGIC.into(),
            format_version: FORMAT_VERSION,
            scalar_codec_version: SCALAR_CODEC_VERSION,
            bitmap_codec_version: BITMAP_CODEC_VERSION,
            contribution_codec_version: CONTRIBUTION_CODEC_VERSION,
            collation_version: COLLATION_VERSION.into(),
            configuration_digest,
            alphabet: alphabet.into(),
            source_root,
            build_identity,
            build_epoch: 1,
            storage_mode: storage_mode.into(),
            phase: BuildPhase::Building,
        }
    }

    pub fn validate(&self) -> StoreResult<()> {
        if self.magic != MAGIC
            || self.format_version != FORMAT_VERSION
            || self.scalar_codec_version != SCALAR_CODEC_VERSION
            || self.bitmap_codec_version != BITMAP_CODEC_VERSION
            || self.contribution_codec_version != CONTRIBUTION_CODEC_VERSION
            || self.collation_version != COLLATION_VERSION
        {
            return Err(StoreError::Incompatible(
                "RocksDB prototype format identity does not match this build".into(),
            ));
        }
        Ok(())
    }
}

fn options() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .reject_trailing_bytes()
}

pub fn encode<T: Serialize>(value: &T) -> StoreResult<Vec<u8>> {
    let payload = options()
        .serialize(value)
        .map_err(|error| StoreError::CorruptValue(format!("encode failed: {error}")))?;
    let mut encoded = Vec::with_capacity(payload.len() + 1);
    encoded.push(VALUE_FORMAT_TAG);
    encoded.extend_from_slice(&payload);
    Ok(encoded)
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8], what: &str) -> StoreResult<T> {
    let (tag, payload) = bytes
        .split_first()
        .ok_or_else(|| StoreError::CorruptValue(format!("empty {what} value record")))?;
    if *tag != VALUE_FORMAT_TAG {
        return Err(StoreError::CorruptValue(format!(
            "unknown {what} value format tag {tag}"
        )));
    }
    options()
        .deserialize(payload)
        .map_err(|error| StoreError::CorruptValue(format!("malformed {what}: {error}")))
}

/// One collection's committed descriptors and progress counters.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CollectionMetadata {
    pub id: CollectionId,
    pub name: String,
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

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum BindingIdentity {
    Declared { path: Vec<String> },
    Named { path: Vec<String>, name: String },
    RawText,
    FallbackText,
    LogicalText,
    CustomText { path: Vec<String>, name: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum BindingRole {
    Structured,
    Text,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BindingDescriptor {
    pub identity: BindingIdentity,
    pub role: BindingRole,
    pub kind: LogicalType,
    pub multi: bool,
    pub text_alphabet: Option<String>,
    pub text_segment: Option<String>,
    pub fingerprint: [u8; 32],
}

pub type BindingId = u32;
pub type SortDefinitionId = u32;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StoredBinding {
    pub id: BindingId,
    pub descriptor: BindingDescriptor,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SortConstituent {
    pub key: FieldPath,
    pub direction: SortDirection,
    pub options: SortOptions,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SortDefinition {
    pub id: SortDefinitionId,
    pub constituents: Vec<SortConstituent>,
    pub locale: String,
    pub collation_version: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileMetadata {
    pub entry: FileEntry,
    pub file_version: u64,
    pub fingerprint: Option<[u8; 32]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum BuildFileState {
    Pending,
    Indexed,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileBuildState {
    pub state: BuildFileState,
    pub expected_file_version: u64,
    pub build_epoch: u64,
    pub last_admission_revision: u64,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelationOccurrence {
    pub declaration_ordinal: u32,
    pub occurrence_ordinal: u32,
    pub target_collection: CollectionId,
    pub target_id: String,
    pub positions: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SortProjection {
    pub definition: SortDefinitionId,
    pub key: Vec<SortKey>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GramProjection {
    pub binding: BindingId,
    pub kind: GramKind,
    pub grams: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
        self.values
            .sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        self.values.dedup();
        self.lengths
            .sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        self.lengths.dedup();
        self.state_masks.sort_by_key(|entry| entry.0);
        self.state_masks.dedup_by_key(|entry| entry.0);
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
        self.grams.sort_by(|a, b| {
            a.binding
                .cmp(&b.binding)
                .then_with(|| a.kind.tag().cmp(&b.kind.tag()))
        });
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValueDirectoryEntry {
    pub cardinality: u64,
    pub representative: ScalarKey,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LengthDirectoryEntry {
    pub cardinality: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BuildReceipt {
    pub build_identity: [u8; 32],
    pub build_epoch: u64,
    pub batch_id: u64,
    pub digest: [u8; 32],
    pub slot_ranges: Vec<(CollectionId, u32, u32)>,
    pub file_memberships: u32,
    pub records: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FieldCoverageDelta {
    pub collection: CollectionId,
    pub binding: BindingId,
    pub batch_id: u64,
    pub coverage: FieldCoverage,
}

pub fn validate_state_masks(masks: &[(BindingId, u16)]) -> StoreResult<()> {
    let mut previous = None;
    for (binding, mask) in masks {
        if *mask == 0 || *mask & !STATE_MASK_BITS != 0 {
            return Err(StoreError::CorruptValue(format!(
                "invalid state mask {mask:#06x} for binding {binding}"
            )));
        }
        if previous.is_some_and(|previous| *binding <= previous) {
            return Err(StoreError::CorruptValue(
                "state masks are not sorted by unique binding".into(),
            ));
        }
        previous = Some(*binding);
    }
    Ok(())
}

pub fn encode_gram_list(grams: &[u64]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(4 + grams.len().saturating_mul(2));
    bytes.extend_from_slice(&(grams.len() as u32).to_le_bytes());
    let mut previous = 0u64;
    for gram in grams {
        let delta = gram.wrapping_sub(previous);
        previous = *gram;
        let mut value = delta;
        while value >= 0x80 {
            bytes.push((value as u8 & 0x7F) | 0x80);
            value >>= 7;
        }
        bytes.push(value as u8);
    }
    bytes
}

pub fn decode_gram_list(bytes: &[u8]) -> StoreResult<Vec<u64>> {
    if bytes.len() < 4 {
        return Err(StoreError::CorruptValue("truncated gram list".into()));
    }
    let count = u32::from_le_bytes(bytes[..4].try_into().expect("four bytes")) as usize;
    if count > bytes.len() - 4 {
        return Err(StoreError::CorruptValue(
            "impossible gram list count".into(),
        ));
    }
    let mut position = 4;
    let mut grams = Vec::with_capacity(count);
    let mut previous = 0u64;
    for index in 0..count {
        let mut value = 0u64;
        let mut shift = 0u32;
        loop {
            let byte = *bytes
                .get(position)
                .ok_or_else(|| StoreError::CorruptValue("truncated gram varint".into()))?;
            position += 1;
            value |= u64::from(byte & 0x7F)
                .checked_shl(shift)
                .ok_or_else(|| StoreError::CorruptValue("gram varint overflow".into()))?;
            if byte & 0x80 == 0 {
                break;
            }
            shift = shift
                .checked_add(7)
                .ok_or_else(|| StoreError::CorruptValue("gram varint overflow".into()))?;
            if shift >= 64 {
                return Err(StoreError::CorruptValue("gram varint overflow".into()));
            }
        }
        let gram = if index == 0 {
            value
        } else {
            if value == 0 {
                return Err(StoreError::CorruptValue("zero gram delta".into()));
            }
            previous
                .checked_add(value)
                .ok_or_else(|| StoreError::CorruptValue("gram value overflow".into()))?
        };
        grams.push(gram);
        previous = gram;
    }
    if position != bytes.len() {
        return Err(StoreError::CorruptValue(
            "trailing bytes in gram list".into(),
        ));
    }
    Ok(grams)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trip_and_format_rejection() {
        let header = StoreHeader::new([1; 32], "ascii", b"/source".to_vec(), [2; 32], "memory");
        let encoded = encode(&header).unwrap();
        assert_eq!(decode::<StoreHeader>(&encoded, "header").unwrap(), header);

        let mut wrong = header;
        wrong.format_version += 1;
        assert!(matches!(wrong.validate(), Err(StoreError::Incompatible(_))));
    }

    #[test]
    fn malformed_envelopes_are_rejected() {
        assert!(decode::<StoreHeader>(&[], "header").is_err());
        assert!(decode::<StoreHeader>(&[9, 0], "header").is_err());
        assert!(decode::<StoreHeader>(&[VALUE_FORMAT_TAG], "header").is_err());
    }
}
