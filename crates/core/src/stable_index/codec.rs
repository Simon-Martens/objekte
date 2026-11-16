//! Versioned encodings, component identities and self-contained file envelopes.

use std::cmp::Ordering;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use bincode::Options;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

use crate::inventory::{FileEntry, RecordEntry};
use crate::model::{CollectionId, FileId, LogicalType, Number, RecordId, RecordSlot};
use crate::posting::ScalarKey;
use crate::sorting::{SortKey, SortOptions};

use super::memory::StoredBytes;

pub const FORMAT_MAGIC: &[u8; 8] = b"OBJICSS2";
pub const OUTER_FORMAT_VERSION: u32 = 2;
pub const KEY_CODEC_VERSION: u32 = 1;
pub const VALUE_CODEC_VERSION: u32 = 1;
pub const DESCRIPTOR_FORMAT_VERSION: u32 = 1;
pub const DESCRIPTOR_ENCODING_VERSION: u32 = 1;
pub const MAX_DESCRIPTOR_BYTES: usize = 1024 * 1024;
pub const ENVELOPE_BYTES: usize = 128;
pub const TREE_HEADER_BYTES: usize = 28;
pub const STABLE_CRATE_MAJOR: u16 = 0;
pub const STABLE_CRATE_MINOR: u16 = 7;
pub const STABLE_CRATE_PATCH: u16 = 2;
pub const ACCEPTED_TREE_LAYOUT_VERSION: u16 = 2;
pub const METADATA_FORMAT_TAG: u8 = 1;

pub const DOMAIN_ALL: u8 = 0;
pub const DOMAIN_LOGICAL: u8 = 1;
pub const DOMAIN_RAW_FALLBACK: u8 = 2;
pub const DOMAIN_EMPTY_ARRAY: u8 = 3;
pub const DOMAIN_LIVE: u8 = 4;

pub const STATE_PRESENCE: u8 = 0;
pub const STATE_EMPTY_SELECTION: u8 = 1;
pub const STATE_LEGACY_NULL: u8 = 2;
pub const STATE_ANY_NULL: u8 = 3;
pub const STATE_NON_EMPTY: u8 = 4;
pub const STATE_STRING: u8 = 5;
pub const STATE_NUMBER: u8 = 6;
pub const STATE_ARRAY: u8 = 7;
pub const STATE_OBJECT: u8 = 8;
pub const STATE_EMPTY_OBJECT: u8 = 9;

pub const RELATION_PRESENT: u8 = 0;
pub const RELATION_NULLS: u8 = 1;
pub const RELATION_EMPTY_ARRAY: u8 = 2;
pub const RELATION_UNAVAILABLE: u8 = 3;

pub(crate) fn bincode_options() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .reject_trailing_bytes()
}

pub fn encode_meta<T: Serialize>(value: &T) -> Result<StoredBytes, String> {
    let mut bytes = Vec::new();
    bytes.push(METADATA_FORMAT_TAG);
    bytes.extend_from_slice(
        &bincode_options()
            .serialize(value)
            .map_err(|error| format!("metadata encode failed: {error}"))?,
    );
    Ok(StoredBytes::new(bytes))
}

pub fn decode_meta<T: DeserializeOwned>(value: &StoredBytes) -> Result<T, String> {
    let (tag, payload) = value
        .to_bytes()
        .split_first()
        .ok_or("empty metadata value")?;
    if *tag != METADATA_FORMAT_TAG {
        return Err(format!("unsupported metadata format tag {tag}"));
    }
    bincode_options()
        .deserialize(payload)
        .map_err(|error| format!("metadata decode failed: {error}"))
}

pub fn encode_descriptor(descriptor: &ComponentDescriptor) -> Result<Vec<u8>, String> {
    let bytes = bincode_options()
        .serialize(descriptor)
        .map_err(|error| format!("descriptor encode failed: {error}"))?;
    if bytes.len() > MAX_DESCRIPTOR_BYTES {
        return Err(format!(
            "descriptor is {} bytes, above the {MAX_DESCRIPTOR_BYTES}-byte limit",
            bytes.len()
        ));
    }
    Ok(bytes)
}

pub fn decode_descriptor(bytes: &[u8]) -> Result<ComponentDescriptor, String> {
    if bytes.len() > MAX_DESCRIPTOR_BYTES {
        return Err("descriptor exceeds the encoded length limit".into());
    }
    let descriptor: ComponentDescriptor = bincode_options()
        .deserialize(bytes)
        .map_err(|error| format!("descriptor decode failed: {error}"))?;
    if descriptor.descriptor_version != DESCRIPTOR_FORMAT_VERSION {
        return Err(format!(
            "unsupported descriptor version {}",
            descriptor.descriptor_version
        ));
    }
    Ok(descriptor)
}

pub fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub fn hex_digest(bytes: &[u8; 32]) -> String {
    let mut output = String::with_capacity(64);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

/// Stable component family identifier.  The numeric code is written to disk;
/// Rust enum discriminants are never persisted.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ComponentKind {
    Files,
    Paths,
    Records,
    Identities,
    Domains,
    Values,
    Lengths,
    States,
    NormalGrams,
    SkipGrams,
    SortAscending,
    SortDescending,
    RelationOccurrences,
    RelationTargets,
    RelationStates,
}

impl ComponentKind {
    pub const ALL: [Self; 15] = [
        Self::Files,
        Self::Paths,
        Self::Records,
        Self::Identities,
        Self::Domains,
        Self::Values,
        Self::Lengths,
        Self::States,
        Self::NormalGrams,
        Self::SkipGrams,
        Self::SortAscending,
        Self::SortDescending,
        Self::RelationOccurrences,
        Self::RelationTargets,
        Self::RelationStates,
    ];

    pub fn code(self) -> u32 {
        match self {
            Self::Files => 1,
            Self::Paths => 2,
            Self::Records => 3,
            Self::Identities => 4,
            Self::Domains => 5,
            Self::Values => 6,
            Self::Lengths => 7,
            Self::States => 8,
            Self::NormalGrams => 9,
            Self::SkipGrams => 10,
            Self::SortAscending => 11,
            Self::SortDescending => 12,
            Self::RelationOccurrences => 13,
            Self::RelationTargets => 14,
            Self::RelationStates => 15,
        }
    }

    pub fn from_code(code: u32) -> Option<Self> {
        Some(match code {
            1 => Self::Files,
            2 => Self::Paths,
            3 => Self::Records,
            4 => Self::Identities,
            5 => Self::Domains,
            6 => Self::Values,
            7 => Self::Lengths,
            8 => Self::States,
            9 => Self::NormalGrams,
            10 => Self::SkipGrams,
            11 => Self::SortAscending,
            12 => Self::SortDescending,
            13 => Self::RelationOccurrences,
            14 => Self::RelationTargets,
            15 => Self::RelationStates,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Paths => "paths",
            Self::Records => "records",
            Self::Identities => "identities",
            Self::Domains => "domains",
            Self::Values => "values",
            Self::Lengths => "lengths",
            Self::States => "states",
            Self::NormalGrams => "normal",
            Self::SkipGrams => "skip",
            Self::SortAscending => "sort-asc",
            Self::SortDescending => "sort-desc",
            Self::RelationOccurrences => "relation-occurrences",
            Self::RelationTargets => "relation-targets",
            Self::RelationStates => "relation-states",
        }
    }

    pub fn file_suffix(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Paths => "paths",
            Self::Records => "records",
            Self::Identities => "identities",
            Self::Domains => "domains",
            Self::Values => "values",
            Self::Lengths => "lengths",
            Self::States => "states",
            Self::NormalGrams => "normal",
            Self::SkipGrams => "skip",
            Self::SortAscending => "sort-asc",
            Self::SortDescending => "sort-desc",
            Self::RelationOccurrences => "relation-occurrences",
            Self::RelationTargets => "relation-targets",
            Self::RelationStates => "relation-states",
        }
    }
}

impl Serialize for ComponentKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(self.code())
    }
}

impl<'de> Deserialize<'de> for ComponentKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = u32::deserialize(deserializer)?;
        Self::from_code(code)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown component kind {code}")))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ComponentId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum BindingRole {
    DeclaredField,
    NamedBinding,
    RawText,
    FallbackText,
    LogicalText,
    CustomText,
    CompositeSort,
    RepositoryMetadata,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct CollectionIdentity {
    pub id: CollectionId,
    pub name: String,
    pub generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FieldIdentity {
    pub path: Vec<String>,
    pub binding: Option<String>,
    pub role: BindingRole,
    pub logical_type: Option<LogicalType>,
    pub multi: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OrderingDescriptor {
    pub version: u32,
    pub locale: String,
    pub collation_version: String,
    pub options: SortOptions,
    pub descending: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CoverageDescriptor {
    pub values_indexed: bool,
    pub value_complete: bool,
    pub length_complete: bool,
    pub empty_string_complete: bool,
    pub covered_records: u32,
    pub complete_for_field: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ComponentDescriptor {
    pub descriptor_version: u32,
    pub id: ComponentId,
    pub kind: ComponentKind,
    pub key_codec_version: u32,
    pub value_codec_version: u32,
    pub key_bound: String,
    pub collection: Option<CollectionIdentity>,
    pub field: Option<FieldIdentity>,
    pub ordering: Option<OrderingDescriptor>,
    pub target_collection: Option<CollectionId>,
    pub coverage: CoverageDescriptor,
    pub repository_identity: [u8; 32],
    pub configuration_identity: [u8; 32],
    pub schema_fingerprint: Option<[u8; 32]>,
    pub slot_namespace: String,
    pub slot_high_water: u32,
    pub origin_revision: Option<u64>,
    pub origin_path: Option<PathBuf>,
    pub dependency_ids: Vec<ComponentId>,
    pub detached: bool,
}

impl ComponentDescriptor {
    pub fn sort_key(&self) -> String {
        let collection = self
            .collection
            .as_ref()
            .map_or_else(|| "-".to_string(), |value| format!("{:010}", value.id.0));
        let field = self.field.as_ref().map_or_else(
            || "-".to_string(),
            |value| {
                format!(
                    "{}/{:?}/{}",
                    value.path.join("."),
                    value.role,
                    value.binding.as_deref().unwrap_or("")
                )
            },
        );
        let ordering = self.ordering.as_ref().map_or_else(
            || "-".to_string(),
            |value| {
                format!(
                    "{:?}/{:?}/{}",
                    value.locale, value.options, value.descending
                )
            },
        );
        format!(
            "{:03}/{:010}/{field}/{ordering}",
            self.kind.code(),
            collection
        )
    }

    pub fn identity_key(&self) -> String {
        format!(
            "{}/{:?}/{:?}/{:?}",
            self.sort_key(),
            self.field,
            self.collection,
            self.ordering
        )
    }
}

/// One logical path as a lossless, platform-tagged byte sequence.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct PathKey {
    pub platform: u8,
    pub bytes: Vec<u8>,
}

pub const PATH_PLATFORM_UNIX: u8 = 1;
#[allow(dead_code)]
pub const PATH_PLATFORM_WINDOWS: u8 = 2;

impl PathKey {
    pub fn from_path(path: &Path) -> Result<Self, String> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            Ok(Self {
                platform: PATH_PLATFORM_UNIX,
                bytes: path.as_os_str().as_bytes().to_vec(),
            })
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            let mut bytes = Vec::new();
            for unit in path.as_os_str().encode_wide() {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
            Ok(Self {
                platform: PATH_PLATFORM_WINDOWS,
                bytes,
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = path;
            Err("unsupported path platform".into())
        }
    }

    pub fn to_path(&self) -> Result<PathBuf, String> {
        #[cfg(unix)]
        {
            use std::ffi::OsStr;
            use std::os::unix::ffi::OsStrExt;
            if self.platform != PATH_PLATFORM_UNIX {
                return Err("path snapshot was written for another platform".into());
            }
            Ok(PathBuf::from(OsStr::from_bytes(&self.bytes)))
        }
        #[cfg(windows)]
        {
            use std::ffi::OsString;
            use std::os::windows::ffi::OsStringExt;
            if self.platform != PATH_PLATFORM_WINDOWS {
                return Err("path snapshot was written for another platform".into());
            }
            if self.bytes.len() % 2 != 0 {
                return Err("Windows path key has an incomplete UTF-16 unit".into());
            }
            let units = self
                .bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>();
            Ok(PathBuf::from(OsString::from_wide(&units)))
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = self;
            Err("unsupported path platform".into())
        }
    }

    pub fn display(&self) -> String {
        self.to_path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| format!("<foreign-path:{}>", self.bytes.len()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct RecordIdentityKey(pub String);

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct RelationOccurrenceKey {
    pub source: RecordSlot,
    pub ordinal: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct RelationTargetKey {
    pub target_collection: CollectionId,
    pub target_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SortEntryKey {
    pub options: SortOptions,
    pub descending: bool,
    pub key: SortKey,
    pub slot: RecordSlot,
}

fn strength_code(value: crate::sorting::CollationStrength) -> u8 {
    use crate::sorting::CollationStrength;
    match value {
        CollationStrength::LocaleDefault => 0,
        CollationStrength::Primary => 1,
        CollationStrength::Secondary => 2,
        CollationStrength::Tertiary => 3,
        CollationStrength::Quaternary => 4,
        CollationStrength::Identical => 5,
    }
}

fn placement_code(value: crate::sorting::Placement) -> u8 {
    use crate::sorting::Placement;
    match value {
        Placement::First => 0,
        Placement::Last => 1,
    }
}

impl PartialEq for SortEntryKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for SortEntryKey {}

impl PartialOrd for SortEntryKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SortEntryKey {
    fn cmp(&self, other: &Self) -> Ordering {
        let left_options = (
            strength_code(self.options.strength),
            self.options.numeric,
            placement_code(self.options.nulls),
            placement_code(self.options.missing),
            self.descending,
        );
        let right_options = (
            strength_code(other.options.strength),
            other.options.numeric,
            placement_code(other.options.nulls),
            placement_code(other.options.missing),
            other.descending,
        );
        left_options
            .cmp(&right_options)
            .then_with(|| {
                self.key
                    .compare_with(&other.key, self.descending, self.options)
            })
            .then_with(|| self.slot.cmp(&other.slot))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum ComponentKey {
    FileId(u32),
    Path(PathKey),
    RecordSlot(u32),
    Identity(RecordIdentityKey),
    Domain(u8),
    Scalar(ScalarKey),
    Length(Number),
    State(u8),
    Gram(u64),
    Sort(SortEntryKey),
    Occurrence(RelationOccurrenceKey),
    RelationTarget(RelationTargetKey),
    RelationState(u8),
}

impl ComponentKey {
    pub fn tag(&self) -> u8 {
        match self {
            Self::FileId(_) => 0,
            Self::Path(_) => 1,
            Self::RecordSlot(_) => 2,
            Self::Identity(_) => 3,
            Self::Domain(_) => 4,
            Self::Scalar(_) => 5,
            Self::Length(_) => 6,
            Self::State(_) => 7,
            Self::Gram(_) => 8,
            Self::Sort(_) => 9,
            Self::Occurrence(_) => 10,
            Self::RelationTarget(_) => 11,
            Self::RelationState(_) => 12,
        }
    }
}

impl ic_stable_structures::Storable for ComponentKey {
    fn to_bytes(&self) -> std::borrow::Cow<'_, [u8]> {
        let bytes = bincode_options()
            .serialize(self)
            .expect("component key serialization");
        std::borrow::Cow::Owned(bytes)
    }

    fn into_bytes(self) -> Vec<u8> {
        bincode_options()
            .serialize(&self)
            .expect("component key serialization")
    }

    fn from_bytes(bytes: std::borrow::Cow<[u8]>) -> Self {
        bincode_options()
            .deserialize(bytes.as_ref())
            .expect("component key decoded from a verified snapshot")
    }

    const BOUND: ic_stable_structures::storable::Bound =
        ic_stable_structures::storable::Bound::Unbounded;
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileCatalogRecord {
    pub entry: FileEntry,
    pub live: bool,
    pub incarnation: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordCatalogRecord {
    pub entry: RecordEntry,
    pub live: bool,
    pub incarnation: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct RecordRef {
    pub collection: CollectionId,
    pub slot: RecordSlot,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PathCatalogEntry {
    pub file: FileId,
    pub refs: Vec<RecordRef>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelationOccurrenceValue {
    pub target_collection: Option<CollectionId>,
    pub target_id: String,
    pub positions: Vec<u32>,
    pub target_resolved: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub outer_version: u32,
    pub kind: ComponentKind,
    pub component_id: ComponentId,
    pub descriptor_digest: [u8; 32],
    pub key_codec_version: u32,
    pub value_codec_version: u32,
    pub crate_major: u16,
    pub crate_minor: u16,
    pub crate_patch: u16,
    pub tree_layout_version: u16,
    pub payload_len: u64,
    pub entry_count: u64,
    pub payload_digest: [u8; 32],
    pub descriptor_len: u32,
    pub descriptor_encoding_version: u32,
}

impl Envelope {
    pub fn encode(&self) -> [u8; ENVELOPE_BYTES] {
        let mut bytes = [0u8; ENVELOPE_BYTES];
        bytes[0..8].copy_from_slice(FORMAT_MAGIC);
        bytes[8..12].copy_from_slice(&self.outer_version.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.kind.code().to_le_bytes());
        bytes[16..24].copy_from_slice(&(self.component_id.0 as u64).to_le_bytes());
        bytes[24..56].copy_from_slice(&self.descriptor_digest);
        bytes[56..60].copy_from_slice(&self.key_codec_version.to_le_bytes());
        bytes[60..64].copy_from_slice(&self.value_codec_version.to_le_bytes());
        bytes[64..66].copy_from_slice(&self.crate_major.to_le_bytes());
        bytes[66..68].copy_from_slice(&self.crate_minor.to_le_bytes());
        bytes[68..70].copy_from_slice(&self.crate_patch.to_le_bytes());
        bytes[70..72].copy_from_slice(&self.tree_layout_version.to_le_bytes());
        bytes[72..80].copy_from_slice(&self.payload_len.to_le_bytes());
        bytes[80..88].copy_from_slice(&self.entry_count.to_le_bytes());
        bytes[88..120].copy_from_slice(&self.payload_digest);
        bytes[120..124].copy_from_slice(&self.descriptor_len.to_le_bytes());
        bytes[124..128].copy_from_slice(&self.descriptor_encoding_version.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != ENVELOPE_BYTES {
            return Err("component envelope is not 128 bytes".into());
        }
        if &bytes[0..8] != FORMAT_MAGIC {
            return Err("component file has the wrong format magic".into());
        }
        let outer_version = u32_at(bytes, 8)?;
        if outer_version != OUTER_FORMAT_VERSION {
            return Err(format!(
                "unsupported component file format {outer_version}; expected {OUTER_FORMAT_VERSION}"
            ));
        }
        let kind_code = u32_at(bytes, 12)?;
        let kind = ComponentKind::from_code(kind_code)
            .ok_or_else(|| format!("unknown component kind code {kind_code}"))?;
        let mut descriptor_digest = [0u8; 32];
        descriptor_digest.copy_from_slice(&bytes[24..56]);
        let mut payload_digest = [0u8; 32];
        payload_digest.copy_from_slice(&bytes[88..120]);
        let envelope = Self {
            outer_version,
            kind,
            component_id: ComponentId(u64_at(bytes, 16)? as u32),
            descriptor_digest,
            key_codec_version: u32_at(bytes, 56)?,
            value_codec_version: u32_at(bytes, 60)?,
            crate_major: u16_at(bytes, 64)?,
            crate_minor: u16_at(bytes, 66)?,
            crate_patch: u16_at(bytes, 68)?,
            tree_layout_version: u16_at(bytes, 70)?,
            payload_len: u64_at(bytes, 72)?,
            entry_count: u64_at(bytes, 80)?,
            payload_digest,
            descriptor_len: u32_at(bytes, 120)?,
            descriptor_encoding_version: u32_at(bytes, 124)?,
        };
        if envelope.descriptor_encoding_version != DESCRIPTOR_ENCODING_VERSION {
            return Err(format!(
                "unsupported descriptor encoding version {}",
                envelope.descriptor_encoding_version
            ));
        }
        if envelope.descriptor_len as usize > MAX_DESCRIPTOR_BYTES {
            return Err("component descriptor length exceeds the configured limit".into());
        }
        if envelope.key_codec_version != KEY_CODEC_VERSION
            || envelope.value_codec_version != VALUE_CODEC_VERSION
        {
            return Err("component key/value codec versions are not supported".into());
        }
        if (
            envelope.crate_major,
            envelope.crate_minor,
            envelope.crate_patch,
        ) != (STABLE_CRATE_MAJOR, STABLE_CRATE_MINOR, STABLE_CRATE_PATCH)
        {
            return Err("component was written for another ic-stable-structures version".into());
        }
        if envelope.tree_layout_version != ACCEPTED_TREE_LAYOUT_VERSION {
            return Err(format!(
                "unsupported stable tree layout {}",
                envelope.tree_layout_version
            ));
        }
        Ok(envelope)
    }
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let slice = bytes
        .get(offset..offset + 2)
        .ok_or("truncated component envelope")?;
    Ok(u16::from_le_bytes(slice.try_into().expect("two bytes")))
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or("truncated component envelope")?;
    Ok(u32::from_le_bytes(slice.try_into().expect("four bytes")))
}

fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, String> {
    let slice = bytes
        .get(offset..offset + 8)
        .ok_or("truncated component envelope")?;
    Ok(u64::from_le_bytes(slice.try_into().expect("eight bytes")))
}

#[derive(Clone, Debug)]
pub struct ComponentFileInfo {
    pub path: PathBuf,
    pub envelope: Envelope,
    pub descriptor: ComponentDescriptor,
    pub descriptor_bytes: Vec<u8>,
    #[allow(dead_code)]
    pub file_len: u64,
}

#[derive(Clone, Debug)]
pub struct ComponentFile {
    pub info: ComponentFileInfo,
    pub payload: Vec<u8>,
}

pub fn write_component_file(
    path: &Path,
    descriptor: &ComponentDescriptor,
    payload: &[u8],
    entry_count: u64,
) -> Result<ComponentFileInfo, String> {
    if payload.len() % super::memory::MEMORY_PAGE_SIZE != 0 {
        return Err("stable payload is not page aligned".into());
    }
    if payload.len() > u64::MAX as usize {
        return Err("stable payload length overflows the file envelope".into());
    }
    let descriptor_bytes = encode_descriptor(descriptor)?;
    let descriptor_len =
        u32::try_from(descriptor_bytes.len()).map_err(|_| "descriptor length overflow")?;
    let payload_digest = digest(payload);
    let descriptor_digest = digest(&descriptor_bytes);
    let envelope = Envelope {
        outer_version: OUTER_FORMAT_VERSION,
        kind: descriptor.kind,
        component_id: descriptor.id,
        descriptor_digest,
        key_codec_version: descriptor.key_codec_version,
        value_codec_version: descriptor.value_codec_version,
        crate_major: STABLE_CRATE_MAJOR,
        crate_minor: STABLE_CRATE_MINOR,
        crate_patch: STABLE_CRATE_PATCH,
        tree_layout_version: ACCEPTED_TREE_LAYOUT_VERSION,
        payload_len: payload.len() as u64,
        entry_count,
        payload_digest,
        descriptor_len,
        descriptor_encoding_version: DESCRIPTOR_ENCODING_VERSION,
    };
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    file.write_all(&[0u8; ENVELOPE_BYTES])
        .map_err(|error| format!("write placeholder envelope: {error}"))?;
    file.write_all(&descriptor_bytes)
        .map_err(|error| format!("write descriptor: {error}"))?;
    let mut hasher = Sha256::new();
    for chunk in payload.chunks(256 * 1024) {
        hasher.update(chunk);
        file.write_all(chunk)
            .map_err(|error| format!("write payload: {error}"))?;
    }
    let written: [u8; 32] = hasher.finalize().into();
    if written != payload_digest {
        return Err("payload changed while it was being written".into());
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|error| format!("seek envelope: {error}"))?;
    file.write_all(&envelope.encode())
        .map_err(|error| format!("rewrite envelope: {error}"))?;
    file.flush()
        .map_err(|error| format!("flush component file: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("sync component file: {error}"))?;
    let file_len = ENVELOPE_BYTES as u64 + descriptor_bytes.len() as u64 + payload.len() as u64;
    Ok(ComponentFileInfo {
        path: path.to_path_buf(),
        envelope,
        descriptor: descriptor.clone(),
        descriptor_bytes,
        file_len,
    })
}

pub fn read_component_header(path: &Path) -> Result<ComponentFileInfo, String> {
    let mut file = File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let file_len = file
        .metadata()
        .map_err(|error| format!("stat {}: {error}", path.display()))?
        .len();
    if file_len < ENVELOPE_BYTES as u64 {
        return Err("component file is shorter than its envelope".into());
    }
    let mut envelope_bytes = [0u8; ENVELOPE_BYTES];
    file.read_exact(&mut envelope_bytes)
        .map_err(|error| format!("read envelope: {error}"))?;
    let envelope = Envelope::decode(&envelope_bytes)?;
    let expected_len = (ENVELOPE_BYTES as u64)
        .checked_add(envelope.descriptor_len as u64)
        .and_then(|value| value.checked_add(envelope.payload_len))
        .ok_or("component file length arithmetic overflow")?;
    if file_len != expected_len {
        return Err(format!(
            "component file length {file_len} does not match envelope length {expected_len}"
        ));
    }
    if envelope.payload_len % super::memory::MEMORY_PAGE_SIZE as u64 != 0 {
        return Err("component payload length is not page aligned".into());
    }
    let mut descriptor_bytes = vec![0u8; envelope.descriptor_len as usize];
    file.read_exact(&mut descriptor_bytes)
        .map_err(|error| format!("read descriptor: {error}"))?;
    if digest(&descriptor_bytes) != envelope.descriptor_digest {
        return Err("component descriptor hash mismatch".into());
    }
    let descriptor = decode_descriptor(&descriptor_bytes)?;
    if descriptor.id != envelope.component_id
        || descriptor.kind != envelope.kind
        || descriptor.key_codec_version != envelope.key_codec_version
        || descriptor.value_codec_version != envelope.value_codec_version
    {
        return Err("embedded descriptor does not match the envelope identity".into());
    }
    Ok(ComponentFileInfo {
        path: path.to_path_buf(),
        envelope,
        descriptor,
        descriptor_bytes,
        file_len,
    })
}

pub fn read_component_file(path: &Path) -> Result<ComponentFile, String> {
    let info = read_component_header(path)?;
    let mut file = File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    file.seek(SeekFrom::Start(
        ENVELOPE_BYTES as u64 + info.envelope.descriptor_len as u64,
    ))
    .map_err(|error| format!("seek payload: {error}"))?;
    let payload_len = usize::try_from(info.envelope.payload_len)
        .map_err(|_| "payload does not fit in this process address space")?;
    let mut payload = vec![0u8; payload_len];
    file.read_exact(&mut payload)
        .map_err(|error| format!("read payload: {error}"))?;
    if digest(&payload) != info.envelope.payload_digest {
        return Err("component payload hash mismatch".into());
    }
    Ok(ComponentFile { info, payload })
}

pub(crate) fn file_id_key(file: FileId) -> ComponentKey {
    ComponentKey::FileId(file.0)
}

pub(crate) fn record_slot_key(slot: RecordSlot) -> ComponentKey {
    ComponentKey::RecordSlot(slot.0)
}

pub(crate) fn identity_key(id: &RecordId) -> ComponentKey {
    ComponentKey::Identity(RecordIdentityKey(id.0.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_kinds_have_explicit_stable_codes() {
        for kind in ComponentKind::ALL {
            assert_eq!(ComponentKind::from_code(kind.code()), Some(kind));
        }
        assert_eq!(FORMAT_MAGIC, b"OBJICSS2");
        assert_eq!(ENVELOPE_BYTES, 128);
    }

    #[test]
    fn envelope_round_trips_all_fields() {
        let envelope = Envelope {
            outer_version: OUTER_FORMAT_VERSION,
            kind: ComponentKind::Values,
            component_id: ComponentId(42),
            descriptor_digest: [7; 32],
            key_codec_version: KEY_CODEC_VERSION,
            value_codec_version: VALUE_CODEC_VERSION,
            crate_major: STABLE_CRATE_MAJOR,
            crate_minor: STABLE_CRATE_MINOR,
            crate_patch: STABLE_CRATE_PATCH,
            tree_layout_version: ACCEPTED_TREE_LAYOUT_VERSION,
            payload_len: 65_536,
            entry_count: 9,
            payload_digest: [9; 32],
            descriptor_len: 100,
            descriptor_encoding_version: DESCRIPTOR_ENCODING_VERSION,
        };
        let decoded = Envelope::decode(&envelope.encode()).expect("envelope");
        assert_eq!(decoded.kind, ComponentKind::Values);
        assert_eq!(decoded.component_id, ComponentId(42));
        assert_eq!(decoded.payload_len, 65_536);
        assert_eq!(decoded.entry_count, 9);
    }

    #[test]
    fn path_keys_are_lossless_on_this_platform() {
        let path = Path::new("directory/\u{00e9}/file name");
        let key = PathKey::from_path(path).expect("path key");
        assert_eq!(key.to_path().expect("path"), path);
    }

    #[test]
    fn sort_key_equality_matches_native_missing_equivalence() {
        let options = SortOptions::default();
        let a = SortEntryKey {
            options,
            descending: false,
            key: SortKey::Missing,
            slot: RecordSlot(1),
        };
        let b = SortEntryKey {
            options,
            descending: false,
            key: SortKey::Unavailable,
            slot: RecordSlot(1),
        };
        assert_eq!(a, b);
    }
}
