//! Store marker, generation manifest, and atomic CURRENT pointer.

use std::collections::HashSet;
use std::io;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::error::{StorageError, StorageResult};
use super::format::{FORMAT_VERSION, STORE_MAGIC};

pub const STORE_MARKER_LEN: usize = 64;
pub const MANIFEST_VERSION: u32 = 1;
pub const CURRENT_VERSION: u32 = 1;
pub const MAX_MANIFEST_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_FRONTEND_METADATA_BYTES: usize = 1024 * 1024;
pub const MAX_SOURCE_REGISTRATIONS: usize = 1024;
pub const MAX_RUNS: usize = 16_384;
pub const MAX_COLLECTION_SUMMARIES: usize = 1_024;


/// Native path encoding stored with the marker and manifest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PathEncoding {
    UnixBytes = 0,
    WindowsUtf16 = 1,
}


impl PathEncoding {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::WindowsUtf16
        } else {
            Self::UnixBytes
        }
    }

    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> StorageResult<Self> {
        Ok(match code {
            0 => Self::UnixBytes,
            1 => Self::WindowsUtf16,
            other => {
                return Err(StorageError::UnsupportedFormat(format!(
                    "unknown native path encoding {other}"
                )));
            }
        })
    }
}


/// Fixed store marker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StoreMarker {
    pub format_version: u32,
    pub path_encoding: PathEncoding,
    pub store_id: Uuid,
    pub lineage_id: Uuid,
}


impl StoreMarker {
    pub fn new(lineage_id: Uuid) -> Self {
        Self {
            format_version: FORMAT_VERSION as u32,
            path_encoding: PathEncoding::current(),
            store_id: Uuid::new_v4(),
            lineage_id,
        }
    }

    pub fn encode(self) -> [u8; STORE_MARKER_LEN] {
        let mut bytes = [0u8; STORE_MARKER_LEN];
        bytes[..8].copy_from_slice(&STORE_MAGIC);
        bytes[8..12].copy_from_slice(&self.format_version.to_le_bytes());
        bytes[12] = self.path_encoding.code();
        bytes[13..16].fill(0);
        bytes[16..32].copy_from_slice(self.store_id.as_bytes());
        bytes[32..48].copy_from_slice(self.lineage_id.as_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> StorageResult<Self> {
        if bytes.len() < STORE_MARKER_LEN {
            return Err(StorageError::corrupt("truncated STORE marker"));
        }
        if bytes[..8] != STORE_MAGIC {
            return Err(StorageError::UnsupportedFormat(
                "STORE marker magic mismatch".into(),
            ));
        }
        let format_version = u32::from_le_bytes(
            bytes[8..12]
                .try_into()
                .expect("four-byte store format version"),
        );
        if format_version != FORMAT_VERSION as u32 {
            return Err(StorageError::UnsupportedFormat(format!(
                "store format version {format_version}"
            )));
        }
        let path_encoding = PathEncoding::from_code(bytes[12])?;
        if bytes[13..16].iter().any(|byte| *byte != 0) || bytes[48..].iter().any(|byte| *byte != 0)
        {
            return Err(StorageError::corrupt(
                "STORE marker reserved bytes are not zero",
            ));
        }
        let store_id = Uuid::from_slice(&bytes[16..32])
            .map_err(|_| StorageError::corrupt("invalid store UUID"))?;
        let lineage_id = Uuid::from_slice(&bytes[32..48])
            .map_err(|_| StorageError::corrupt("invalid lineage UUID"))?;
        Ok(Self {
            format_version,
            path_encoding,
            store_id,
            lineage_id,
        })
    }
}


/// Content-addressed object reference.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ObjectRef {
    /// Lower-case hexadecimal SHA-256.
    pub sha256: String,
    pub length: u64,
}


impl ObjectRef {
    pub fn new(sha256: [u8; 32], length: u64) -> Self {
        Self {
            sha256: hex(&sha256),
            length,
        }
    }

    pub fn digest(&self) -> StorageResult<[u8; 32]> {
        decode_hex_32(&self.sha256)
    }

    pub fn validate(&self) -> StorageResult<()> {
        let _ = self.digest()?;
        if self.length == 0 {
            return Err(StorageError::InvalidManifest(
                "object reference has zero length".into(),
            ));
        }
        Ok(())
    }
}


/// One persisted run reference.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RunRef {
    pub collection: u32,
    pub ordinal: u64,
    pub level: u32,
    pub object: ObjectRef,
    pub slot_count: u32,
    pub active_count: u64,
}


/// One collection summary stored in the manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CollectionSummary {
    pub collection: u32,
    pub name: String,
    pub file_count: u64,
    pub record_count: u64,
    pub active_count: u64,
    pub fallback_count: u64,
    pub run_count: u32,
}


/// Source registration identity and version.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceRegistrationIdentity {
    pub id: String,
    pub version: u32,
}


/// The full generation manifest. It contains no per-record or per-term entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GenerationManifest {
    pub manifest_version: u32,
    pub format_version: u16,
    pub model_version: u32,
    pub path_encoding: u8,
    pub store_id: String,
    pub lineage_id: String,
    pub publication_id: String,
    pub generation_id: String,
    pub source_root: String,
    pub configuration_identity: String,
    pub source_registrations: Vec<SourceRegistrationIdentity>,
    pub collation_version: String,
    pub bigram_representation_version: String,
    pub file_id_high_water: u32,
    pub catalogue: ObjectRef,
    #[serde(default)]
    pub state: Option<ObjectRef>,
    pub runs: Vec<RunRef>,
    pub collections: Vec<CollectionSummary>,
    pub frontend_metadata: Option<serde_json::Value>,
}


impl GenerationManifest {
    pub fn validate(&self, marker: &StoreMarker) -> StorageResult<()> {
        if self.manifest_version != MANIFEST_VERSION {
            return Err(StorageError::UnsupportedFormat(format!(
                "manifest version {}",
                self.manifest_version
            )));
        }
        if self.format_version != FORMAT_VERSION {
            return Err(StorageError::UnsupportedFormat(format!(
                "manifest object format version {}",
                self.format_version
            )));
        }
        if self.path_encoding != marker.path_encoding.code() {
            return Err(StorageError::Incompatible(
                "manifest native path encoding differs from the store marker".into(),
            ));
        }
        if self.store_id != marker.store_id.to_string()
            || self.lineage_id != marker.lineage_id.to_string()
        {
            return Err(StorageError::Incompatible(
                "manifest belongs to another store or lineage".into(),
            ));
        }
        Uuid::parse_str(&self.publication_id)
            .map_err(|_| StorageError::InvalidManifest("invalid publication UUID".into()))?;
        Uuid::parse_str(&self.generation_id)
            .map_err(|_| StorageError::InvalidManifest("invalid generation UUID".into()))?;
        decode_hex_32(&self.configuration_identity)?;
        if self.source_registrations.len() > MAX_SOURCE_REGISTRATIONS {
            return Err(StorageError::ResourceLimit(
                "manifest has too many source registrations".into(),
            ));
        }
        if self.runs.len() > MAX_RUNS {
            return Err(StorageError::ResourceLimit(
                "manifest has too many runs".into(),
            ));
        }
        if self.collections.len() > MAX_COLLECTION_SUMMARIES {
            return Err(StorageError::ResourceLimit(
                "manifest has too many collection summaries".into(),
            ));
        }
        self.catalogue.validate()?;
        if let Some(state) = &self.state {
            state.validate()?;
        }
        let mut ordinals = HashSet::new();
        for run in &self.runs {
            run.object.validate()?;
            if !ordinals.insert((run.collection, run.ordinal)) {
                return Err(StorageError::InvalidManifest(
                    "duplicate run ordinal".into(),
                ));
            }
            if run.active_count > run.slot_count as u64 {
                return Err(StorageError::InvalidManifest(
                    "run active count exceeds its slot domain".into(),
                ));
            }
        }
        if self
            .collections
            .iter()
            .any(|collection| collection.run_count as usize > self.runs.len())
        {
            return Err(StorageError::InvalidManifest(
                "collection summary run count is invalid".into(),
            ));
        }
        Ok(())
    }

    pub fn encode(&self) -> StorageResult<Vec<u8>> {
        let bytes = serde_json::to_vec(self)
            .map_err(|error| StorageError::InvalidManifest(error.to_string()))?;
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(StorageError::ResourceLimit(
                "manifest exceeds the maximum size".into(),
            ));
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> StorageResult<Self> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(StorageError::ResourceLimit(
                "manifest exceeds the maximum size".into(),
            ));
        }
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|error| StorageError::InvalidManifest(error.to_string()))?;
        Ok(manifest)
    }
}


/// The atomic CURRENT pointer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CurrentPointer {
    pub generation: Uuid,
    pub manifest_sha256: [u8; 32],
}


impl CurrentPointer {
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes =
            format!("{} {}\n", self.generation, hex(&self.manifest_sha256)).into_bytes();
        if bytes.len() > 128 {
            bytes.truncate(128);
        }
        bytes
    }

    pub fn decode(bytes: &[u8]) -> StorageResult<Self> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| StorageError::corrupt("CURRENT is not UTF-8"))?;
        let line = text
            .strip_suffix('\n')
            .ok_or_else(|| StorageError::corrupt("CURRENT lacks a trailing newline"))?;
        let mut parts = line.split_whitespace();
        let generation = parts
            .next()
            .ok_or_else(|| StorageError::corrupt("CURRENT has no generation"))?;
        let sha = parts
            .next()
            .ok_or_else(|| StorageError::corrupt("CURRENT has no manifest checksum"))?;
        if parts.next().is_some() {
            return Err(StorageError::corrupt("CURRENT has trailing fields"));
        }
        Ok(Self {
            generation: Uuid::parse_str(generation)
                .map_err(|_| StorageError::corrupt("CURRENT generation is invalid"))?,
            manifest_sha256: decode_hex_32(sha)?,
        })
    }
}


/// SHA-256 of one byte slice.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// Lower-case hexadecimal encoding.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(DIGITS[(byte >> 4) as usize] as char);
        text.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    text
}

/// Decode exactly 32 bytes from a 64-character hexadecimal string.
pub fn decode_hex_32(text: &str) -> StorageResult<[u8; 32]> {
    if text.len() != 64 {
        return Err(StorageError::InvalidManifest(
            "SHA-256 hex string must contain 64 characters".into(),
        ));
    }
    let mut bytes = [0u8; 32];
    for (index, chunk) in text.as_bytes().chunks_exact(2).enumerate() {
        bytes[index] = (hex_digit(chunk[0])? << 4) | hex_digit(chunk[1])?;
    }
    Ok(bytes)
}

fn hex_digit(byte: u8) -> StorageResult<u8> {
    Ok(match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => {
            return Err(StorageError::InvalidManifest(
                "SHA-256 hex string contains a non-hex character".into(),
            ));
        }
    })
}

/// Write a set of bytes to a synchronised file, creating parent directories.
pub fn write_synced(path: &std::path::Path, bytes: &[u8]) -> StorageResult<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| StorageError::io(parent, error))?;
    }
    let mut file = std::fs::File::create(path).map_err(|error| StorageError::io(path, error))?;
    file.write_all(bytes)
        .map_err(|error| StorageError::io(path, error))?;
    file.sync_all()
        .map_err(|error| StorageError::io(path, error))?;
    Ok(())
}

/// Synchronise one directory entry table where the platform supports it.
pub fn sync_directory(path: &std::path::Path) -> StorageResult<()> {
    #[cfg(unix)]
    {
        let file = std::fs::File::open(path).map_err(|error| StorageError::io(path, error))?;
        file.sync_all()
            .map_err(|error| StorageError::io(path, error))?;
    }

    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Atomic replacement of `CURRENT`. Never deletes the valid pointer first.
pub fn replace_current(directory: &std::path::Path, pointer: &CurrentPointer) -> StorageResult<()> {
    let temporary = directory.join("CURRENT.tmp");
    let current = directory.join("CURRENT");
    write_synced(&temporary, &pointer.encode())?;
    #[cfg(unix)]
    {
        std::fs::rename(&temporary, &current).map_err(|error| StorageError::io(&current, error))?;
    }

    #[cfg(not(unix))]
    {
        if current.exists() {
            std::fs::remove_file(&current).map_err(|error| StorageError::io(&current, error))?;
        }
        std::fs::rename(&temporary, &current).map_err(|error| StorageError::io(&current, error))?;
    }
    sync_directory(directory)
}

/// Read and validate a manifest's checksum against CURRENT.
pub fn manifest_checksum_matches(bytes: &[u8], pointer: &CurrentPointer) -> bool {
    sha256(bytes) == pointer.manifest_sha256
}

/// Convert an io error while writing a store path.
pub fn store_io(path: &std::path::Path, error: io::Error) -> StorageError {
    StorageError::io(path, error)
}
