//! Immutable generation files and an atomic, human-readable TOML manifest.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
#[cfg(feature = "detailed-profiling")]
use std::time::Instant;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::index_storage::ResolvedIndexStorage;
use crate::inventory::ActiveRecords;
use crate::model::CollectionId;
use crate::segment_format::{
    REQUIRED_SECTIONS, SectionKind, SegmentDescriptor, SegmentOpenError, SegmentReader,
    SegmentTrust, SegmentWriteMetadata,
};

const MANIFEST_NAME: &str = "manifest.toml";
const LEGACY_MANIFEST_NAME: &str = "manifest.bin";
const LOCK_NAME: &str = "store.lock";
const GENERATIONS_DIR: &str = "generations";
const CATALOG_NAME: &str = "catalog.bin";
/// Keep the current generation and seven predecessors even during fast flushes.
const RETAINED_GENERATIONS: u64 = 8;
/// Also retain young generations while a slow reader may still be opening one.
const MIN_RETENTION_AGE: Duration = Duration::from_secs(120);
const MAX_OPEN_RETRIES: usize = 3;
const MAX_GC_ENTRIES: usize = 64;
const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
const MAX_CATALOG_BYTES: usize = 512 * 1024 * 1024;
const MAX_SEGMENTS: usize = 16_384;
#[cfg(test)]
const DEPRECATED_MAGIC: &[u8; 8] = b"OBJSNAP1";
#[cfg(test)]
const DEPRECATED_VERSIONS: [u32; 2] = [27, 28];
#[cfg(test)]
const DEPRECATED_NAMES: [&str; 2] = ["search-core-ascii.snapshot", "search-core-unicode.snapshot"];
const SEGMENT_MAGIC: &[u8; 8] = b"OBJHSEG1";
const CATALOG_MAGIC: &[u8; 8] = b"OBJHCAT1";
const LEGACY_MANIFEST_MAGIC: &[u8; 8] = b"OBJHMAN1";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    generation: u64,
    repository: String,
    configuration: String,
    segments: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    catalog: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    verify: BTreeMap<String, VerifyRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct VerifyRecord {
    length: u64,
    sha256: String,
}

/// A segment encoded by the current publish path. The writer-validation flag
/// records a successful construction outcome for the unified program version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SegmentOrigin {
    Validated,
    #[cfg_attr(not(test), allow(dead_code))]
    Unvalidated,
}

pub(crate) enum SegmentInput<'a> {
    /// Encode directly into one owned immutable file while holding the writer
    /// lock. The callback is invoked once; its segment bytes are not retained
    /// alongside other pending new segments.
    NewStream {
        collection: CollectionId,
        ordinal: u64,
        active: &'a ActiveRecords,
        origin: SegmentOrigin,
        write: Box<dyn Fn(&mut File, &SegmentWriteMetadata<'_>) -> Result<(), String> + 'a>,
    },
}

pub(crate) struct OpenedSegment {
    pub name: String,
    pub length: u64,
    pub reader: Arc<SegmentReader>,
    pub descriptor: SegmentDescriptor,
}

impl OpenedSegment {
    pub(crate) fn trust(&self) -> SegmentTrust {
        if self.descriptor.write_validated {
            SegmentTrust::WriteValidated
        } else {
            SegmentTrust::Untrusted
        }
    }
}

pub(crate) struct OpenedGeneration {
    pub generation: u64,
    pub catalog: Option<Arc<[u8]>>,
    pub segments: Vec<OpenedSegment>,
    #[cfg(test)]
    pub generation_directory: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CommitStats {
    pub segment_names: Vec<String>,
    pub segment_bytes_written: u64,
    pub orphan_files_reclaimed: usize,
    pub reclaim_errors: Vec<String>,
    pub generations_removed: usize,
    pub generation_bytes_reclaimed: u64,
    pub orphan_generations_removed: usize,
}

#[derive(Debug)]
pub(crate) enum StoreReadError {
    Incompatible(String),
    Corrupt(String),
    Io { path: PathBuf, source: io::Error },
    Conflict(String),
    Resource(String),
}

impl fmt::Display for StoreReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Incompatible(message) => write!(formatter, "incompatible store: {message}"),
            Self::Corrupt(message) => write!(formatter, "corrupt store: {message}"),
            Self::Io { path, source } => {
                write!(formatter, "store I/O {}: {source}", path.display())
            }
            Self::Conflict(message) => write!(formatter, "store conflict: {message}"),
            Self::Resource(message) => write!(formatter, "store resource limit: {message}"),
        }
    }
}

fn store_write_io(path: &Path, error: impl fmt::Display) -> StoreReadError {
    StoreReadError::Io {
        path: path.to_path_buf(),
        source: io::Error::other(error.to_string()),
    }
}

fn segment_open_store_error(path: &Path, error: SegmentOpenError) -> StoreReadError {
    match error {
        SegmentOpenError::Io(source) => StoreReadError::Io {
            path: path.to_path_buf(),
            source,
        },
        SegmentOpenError::Incompatible(message) => {
            StoreReadError::Incompatible(format!("{}: {message}", path.display()))
        }
        SegmentOpenError::Invalid(message) => {
            StoreReadError::Corrupt(format!("{}: {message}", path.display()))
        }
    }
}

enum OpenGenerationError {
    Vanished(PathBuf),
    Store(StoreReadError),
}

impl From<StoreReadError> for OpenGenerationError {
    fn from(error: StoreReadError) -> Self {
        Self::Store(error)
    }
}

fn check_generation_directory(path: &Path) -> Result<(), OpenGenerationError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(OpenGenerationError::Vanished(path.to_owned()));
        }
        Err(source) => {
            return Err(StoreReadError::Io {
                path: path.to_owned(),
                source,
            }
            .into());
        }
    };
    check_owned_directory(path, &metadata)
        .map_err(|message| StoreReadError::Corrupt(message).into())
}

fn open_generation_file(path: &Path) -> Result<File, OpenGenerationError> {
    match open_regular_nofollow_read(path) {
        Ok(file) => Ok(file),
        Err(StoreReadError::Corrupt(message))
            if message.starts_with("missing referenced store file:") =>
        {
            Err(OpenGenerationError::Vanished(path.to_owned()))
        }
        Err(error) => Err(OpenGenerationError::Store(error)),
    }
}

/// All paths are beneath one application-owned directory. A store exists only
/// after an explicit disk operation opens it; memory construction never does.
pub(crate) struct ManagedStore {
    directory: PathBuf,
    repository_identity: [u8; 32],
    configuration_identity: [u8; 32],
    #[cfg(test)]
    deprecated_directories: Vec<PathBuf>,
}

impl ManagedStore {
    pub(crate) fn open(resolved: &ResolvedIndexStorage) -> Result<Self, String> {
        let directory = resolved
            .managed_directory
            .as_ref()
            .ok_or("memory-only policy has no managed store")?;
        let namespace = resolved
            .owned_namespace_root
            .as_ref()
            .ok_or("managed store namespace missing")?;
        if !directory.starts_with(namespace) {
            return Err("managed store escaped its owned namespace".into());
        }
        ensure_directories_without_symlinks(directory, namespace)?;
        if resolved.private_namespace {
            check_private_namespace(namespace)?;
        }
        Ok(Self {
            directory: directory.clone(),
            repository_identity: resolved.repository_identity,
            configuration_identity: resolved.configuration_identity,
            #[cfg(test)]
            deprecated_directories: resolved.deprecated_directories.clone(),
        })
    }

    /// Locate an existing store without creating a namespace, lock, or sidecar.
    pub(crate) fn open_existing(
        resolved: &ResolvedIndexStorage,
    ) -> Result<Option<Self>, StoreReadError> {
        let directory = resolved.managed_directory.as_ref().ok_or_else(|| {
            StoreReadError::Incompatible("explicit load needs a disk target".into())
        })?;
        let namespace = resolved.owned_namespace_root.as_ref().ok_or_else(|| {
            StoreReadError::Incompatible("managed store namespace missing".into())
        })?;
        if !directory.starts_with(namespace) {
            return Err(StoreReadError::Incompatible(
                "managed store escaped its owned namespace".into(),
            ));
        }
        let mut current = PathBuf::new();
        for component in directory.components() {
            match component {
                Component::Prefix(prefix) => current.push(prefix.as_os_str()),
                Component::RootDir => current.push(component.as_os_str()),
                Component::Normal(part) => {
                    current.push(part);
                    let metadata = match fs::symlink_metadata(&current) {
                        Ok(metadata) => metadata,
                        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                        Err(source) => {
                            return Err(StoreReadError::Io {
                                path: current,
                                source,
                            });
                        }
                    };
                    if metadata.file_type().is_symlink() || !metadata.is_dir() {
                        return Err(StoreReadError::Corrupt(format!(
                            "unsafe managed store directory: {}",
                            current.display()
                        )));
                    }
                    if current.starts_with(namespace) {
                        check_owned_directory(&current, &metadata)
                            .map_err(StoreReadError::Corrupt)?;
                    }
                }
                Component::CurDir | Component::ParentDir => {
                    return Err(StoreReadError::Incompatible(
                        "managed store path is not normalized".into(),
                    ));
                }
            }
        }
        if resolved.private_namespace {
            check_private_namespace(namespace).map_err(StoreReadError::Corrupt)?;
        }
        Ok(Some(Self {
            directory: directory.clone(),
            repository_identity: resolved.repository_identity,
            configuration_identity: resolved.configuration_identity,
            #[cfg(test)]
            deprecated_directories: resolved.deprecated_directories.clone(),
        }))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.directory
    }

    /// Remove only the two previous snapshot filenames in the selected
    /// storage scope, and only after their regular-file header proves a
    /// supported old format. This is never called for memory-only storage.
    #[cfg(test)]
    pub(crate) fn cleanup_deprecated_snapshots(&self) -> Result<Vec<PathBuf>, String> {
        let lock = self.open_lock()?;
        lock.try_lock()
            .map_err(|error| format!("store writer lock: {error}"))?;
        let mut candidates = Vec::new();
        for directory in &self.deprecated_directories {
            let metadata = match fs::symlink_metadata(directory) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(format!(
                        "deprecated index scope {}: {error}",
                        directory.display()
                    ));
                }
            };
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(format!(
                    "unsafe deprecated index scope: {}",
                    directory.display()
                ));
            }
            for name in DEPRECATED_NAMES {
                let path = directory.join(name);
                let metadata = match fs::symlink_metadata(&path) {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        return Err(format!("deprecated index {}: {error}", path.display()));
                    }
                };
                if metadata.file_type().is_symlink()
                    || !metadata.is_file()
                    || !owned_by_current_user(&metadata)
                {
                    return Err(format!(
                        "unsafe deprecated index candidate: {}",
                        path.display()
                    ));
                }
                let mut file = open_regular_nofollow(&path)?;
                let mut header = [0u8; 12];
                file.read_exact(&mut header).map_err(|error| {
                    format!("deprecated index header {}: {error}", path.display())
                })?;
                if &header[..8] != DEPRECATED_MAGIC {
                    return Err(format!("unknown index format at {}", path.display()));
                }
                let version = u32::from_le_bytes(header[8..12].try_into().unwrap());
                if !DEPRECATED_VERSIONS.contains(&version) {
                    return Err(format!(
                        "unsupported index version {version} at {}",
                        path.display()
                    ));
                }
                candidates.push(path);
            }
        }
        let mut removed = Vec::with_capacity(candidates.len());
        for path in candidates {
            fs::remove_file(&path)
                .map_err(|error| format!("remove deprecated index {}: {error}", path.display()))?;
            removed.push(path);
        }
        Ok(removed)
    }

    /// The manifest is the atomic pointer. Readers take no writer lock and
    /// retry if GC removed a generation while its files were being opened.
    #[cfg(test)]
    pub(crate) fn read_generation(&self) -> Result<Option<OpenedGeneration>, StoreReadError> {
        self.read_generation_inner()
    }

    /// Read without migration or validation writes.
    pub(crate) fn read_generation_readonly(
        &self,
    ) -> Result<Option<OpenedGeneration>, StoreReadError> {
        self.read_generation_inner()
    }

    /// Check the atomic pointer and its identity without reopening all
    /// immutable files. Used only when the same publication was already
    /// written to this destination and an unchanged result is possible.
    pub(crate) fn current_generation(&self) -> Result<Option<u64>, StoreReadError> {
        Ok(self
            .read_manifest_typed()?
            .map(|manifest| manifest.generation))
    }

    fn read_generation_inner(&self) -> Result<Option<OpenedGeneration>, StoreReadError> {
        for attempt in 0..MAX_OPEN_RETRIES {
            #[cfg(feature = "detailed-profiling")]
            let manifest_started = Instant::now();
            let Some(manifest) = self.read_manifest_typed()? else {
                return Ok(None);
            };
            #[cfg(feature = "detailed-profiling")]
            if std::env::var_os("OBJEKTE_DIRECT_OPEN_PHASES").is_some() {
                eprintln!(
                    "direct-open manifest_ns={}",
                    manifest_started.elapsed().as_nanos()
                );
            }
            match self.open_generation(&manifest) {
                Ok(opened) => return Ok(Some(opened)),
                Err(OpenGenerationError::Store(error)) => return Err(error),
                Err(OpenGenerationError::Vanished(path)) => {
                    let current = self.read_manifest_typed()?;
                    if current
                        .as_ref()
                        .is_some_and(|entry| entry.generation == manifest.generation)
                    {
                        return Err(StoreReadError::Corrupt(format!(
                            "referenced generation file vanished: {}",
                            path.display()
                        )));
                    }
                    if attempt + 1 == MAX_OPEN_RETRIES {
                        return Err(StoreReadError::Conflict(
                            "generation vanished repeatedly during open".into(),
                        ));
                    }
                }
            }
        }
        Err(StoreReadError::Conflict(
            "generation changed repeatedly during open".into(),
        ))
    }

    fn resolve_manifest_directory(
        &self,
        value: &str,
        context: &str,
    ) -> Result<PathBuf, OpenGenerationError> {
        let relative = manifest_relative_path(value, context)
            .map_err(|message| StoreReadError::Corrupt(message))?;
        let mut current = self.directory.clone();
        for component in relative.components() {
            current.push(component);
            check_generation_directory(&current)?;
        }
        Ok(current)
    }

    fn resolve_manifest_file(
        &self,
        value: &str,
        context: &str,
    ) -> Result<PathBuf, OpenGenerationError> {
        let relative = manifest_relative_path(value, context)
            .map_err(|message| StoreReadError::Corrupt(message))?;
        let mut parts = relative.components().peekable();
        let mut current = self.directory.clone();
        while let Some(component) = parts.next() {
            current.push(component);
            if parts.peek().is_none() {
                break;
            }
            check_generation_directory(&current)?;
        }
        Ok(current)
    }

    fn open_generation(
        &self,
        manifest: &Manifest,
    ) -> Result<OpenedGeneration, OpenGenerationError> {
        #[cfg(feature = "detailed-profiling")]
        let opened_at = Instant::now();
        let segment_directory = self.resolve_manifest_directory(&manifest.segments, "segments")?;
        let catalog_path = manifest
            .catalog
            .as_deref()
            .map(|value| self.resolve_manifest_file(value, "catalog"))
            .transpose()?;
        let catalog = catalog_path
            .as_deref()
            .map(|path| self.read_catalog_typed(path))
            .transpose()?;
        #[cfg(feature = "detailed-profiling")]
        let catalog_ns = opened_at.elapsed().as_nanos();
        let mut names = Vec::new();
        for entry in fs::read_dir(&segment_directory).map_err(|source| StoreReadError::Io {
            path: segment_directory.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| StoreReadError::Io {
                path: segment_directory.clone(),
                source,
            })?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| StoreReadError::Corrupt("non-UTF-8 segment filename".into()))?;
            if !name.ends_with(".idx") {
                continue;
            }
            names.push((name, entry.path()));
        }
        names.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        if names.len() > MAX_SEGMENTS {
            return Err(StoreReadError::Resource(format!(
                "generation contains more than {MAX_SEGMENTS} segments"
            ))
            .into());
        }
        #[cfg(feature = "detailed-profiling")]
        let mut segment_map_ns = 0u128;
        #[cfg(feature = "detailed-profiling")]
        let mut section_check_ns = 0u128;
        let mut segments = Vec::with_capacity(names.len());
        let mut identities = HashSet::with_capacity(names.len());
        for (name, path) in names {
            let metadata = fs::symlink_metadata(&path).map_err(|source| StoreReadError::Io {
                path: path.clone(),
                source,
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(StoreReadError::Corrupt(format!(
                    "segment is not a regular file: {}",
                    path.display()
                ))
                .into());
            }
            #[cfg(feature = "detailed-profiling")]
            let map_started = Instant::now();
            let file = open_generation_file(&path)?;
            let reader = SegmentReader::from_index_file_typed(&file)
                .map_err(|error| segment_open_store_error(&path, error))?;
            reader.require_complete_sections().map_err(|message| {
                StoreReadError::Corrupt(format!("{}: {message}", path.display()))
            })?;
            let descriptor = reader
                .descriptor()
                .ok_or_else(|| {
                    StoreReadError::Corrupt(format!("segment header missing: {}", path.display()))
                })?
                .clone();
            if descriptor.generation != manifest.generation {
                return Err(StoreReadError::Corrupt(format!(
                    "segment generation differs from manifest: {}",
                    path.display()
                ))
                .into());
            }
            if !identities.insert((descriptor.collection, descriptor.ordinal)) {
                return Err(StoreReadError::Corrupt(format!(
                    "duplicate segment identity: {}",
                    path.display()
                ))
                .into());
            }
            #[cfg(feature = "detailed-profiling")]
            {
                segment_map_ns += map_started.elapsed().as_nanos();
            }
            for kind in REQUIRED_SECTIONS {
                if kind != SectionKind::PostingPayloads {
                    #[cfg(feature = "detailed-profiling")]
                    let section_started = Instant::now();
                    reader.section(kind).map_err(|message| {
                        StoreReadError::Corrupt(format!("{}: {message}", path.display()))
                    })?;
                    #[cfg(feature = "detailed-profiling")]
                    {
                        section_check_ns += section_started.elapsed().as_nanos();
                    }
                }
            }
            let length = reader.encoded_len() as u64;
            segments.push(OpenedSegment {
                name,
                length,
                reader: Arc::new(reader),
                descriptor,
            });
        }
        segments.sort_unstable_by_key(|segment| {
            (segment.descriptor.collection, segment.descriptor.ordinal)
        });
        self.run_verification(
            manifest,
            catalog_path.as_deref(),
            catalog.as_deref(),
            &segments,
        )?;
        #[cfg(feature = "detailed-profiling")]
        if std::env::var_os("OBJEKTE_DIRECT_OPEN_PHASES").is_some() {
            eprintln!(
                "direct-open generation={} catalog_read_ns={} segment_map_parse_ns={} section_check_ns={} other_ns={} total_ns={}",
                manifest.generation,
                catalog_ns,
                segment_map_ns,
                section_check_ns,
                opened_at
                    .elapsed()
                    .as_nanos()
                    .saturating_sub(catalog_ns + segment_map_ns + section_check_ns),
                opened_at.elapsed().as_nanos(),
            );
        }
        Ok(OpenedGeneration {
            generation: manifest.generation,
            catalog,
            segments,
            #[cfg(test)]
            generation_directory: self.generation_directory(manifest.generation),
        })
    }

    fn run_verification(
        &self,
        manifest: &Manifest,
        catalog_path: Option<&Path>,
        catalog: Option<&[u8]>,
        segments: &[OpenedSegment],
    ) -> Result<(), OpenGenerationError> {
        if manifest.verify.is_empty() {
            return Ok(());
        }
        let manifest_path = self.directory.join(MANIFEST_NAME);
        let segment_directory = self.resolve_manifest_directory(&manifest.segments, "segments")?;
        for (key, record) in &manifest.verify {
            let path = self.resolve_manifest_file(key, "verify entry")?;
            if path == manifest_path {
                return Err(StoreReadError::Corrupt("manifest cannot verify itself".into()).into());
            }
            let expected = decode_sha256(&record.sha256).map_err(StoreReadError::Corrupt)?;
            if catalog_path == Some(path.as_path()) {
                let bytes = catalog.ok_or_else(|| {
                    StoreReadError::Corrupt(format!(
                        "verification entry names an unopened catalog: {key}"
                    ))
                })?;
                if bytes.len() as u64 != record.length
                    || Sha256::digest(bytes).as_slice() != expected
                {
                    return Err(StoreReadError::Corrupt(format!(
                        "catalog verification failed: {key}"
                    ))
                    .into());
                }
                continue;
            }
            if let Some(segment) = path
                .parent()
                .filter(|parent| *parent == segment_directory)
                .and_then(|_| {
                    let name = path.file_name()?.to_str()?;
                    segments.iter().find(|segment| segment.name == name)
                })
            {
                if segment.length != record.length {
                    return Err(StoreReadError::Corrupt(format!(
                        "segment verification length mismatch: {key}"
                    ))
                    .into());
                }
                let digest = segment
                    .reader
                    .whole_file_digest()
                    .map_err(|message| StoreReadError::Corrupt(format!("{key}: {message}")))?;
                if digest.as_slice() != expected {
                    return Err(StoreReadError::Corrupt(format!(
                        "segment verification digest mismatch: {key}"
                    ))
                    .into());
                }
                continue;
            }
            verify_regular_file(&path, &record.length, &expected, key)?;
        }
        Ok(())
    }

    fn generation_directory(&self, generation: u64) -> PathBuf {
        self.directory
            .join(GENERATIONS_DIR)
            .join(format!("{generation:016x}"))
    }

    #[cfg(test)]
    pub(crate) fn commit_generation(
        &self,
        expected_generation: Option<u64>,
        generation: u64,
        inputs: &[SegmentInput<'_>],
    ) -> Result<CommitStats, String> {
        self.commit_generation_inner(expected_generation, generation, inputs, None)
            .map_err(|error| error.to_string())
    }

    /// Publish an immutable catalog sidecar in the same atomic manifest step
    /// as the segment directory.
    #[cfg(test)]
    pub(crate) fn commit_generation_with_catalog(
        &self,
        expected_generation: Option<u64>,
        generation: u64,
        inputs: &[SegmentInput<'_>],
        catalog: Arc<[u8]>,
    ) -> Result<CommitStats, String> {
        self.commit_generation_with_catalog_typed(expected_generation, generation, inputs, catalog)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn commit_generation_with_catalog_typed(
        &self,
        expected_generation: Option<u64>,
        generation: u64,
        inputs: &[SegmentInput<'_>],
        catalog: Arc<[u8]>,
    ) -> Result<CommitStats, StoreReadError> {
        if !catalog.starts_with(CATALOG_MAGIC) || catalog.len() > MAX_CATALOG_BYTES {
            return Err(StoreReadError::Corrupt(
                "generation catalog byte length invalid".into(),
            ));
        }
        self.commit_generation_inner(expected_generation, generation, inputs, Some(catalog))
    }

    fn commit_generation_inner(
        &self,
        expected_generation: Option<u64>,
        generation: u64,
        inputs: &[SegmentInput<'_>],
        catalog: Option<Arc<[u8]>>,
    ) -> Result<CommitStats, StoreReadError> {
        if generation == 0 || inputs.len() > MAX_SEGMENTS {
            return Err(StoreReadError::Resource(
                "invalid generation or segment count".into(),
            ));
        }
        let lock = self.open_lock_read()?;
        lock.try_lock().map_err(|error| match error {
            fs::TryLockError::WouldBlock => {
                StoreReadError::Conflict("store writer lock is held".into())
            }
            fs::TryLockError::Error(source) => StoreReadError::Io {
                path: self.directory.join(LOCK_NAME),
                source,
            },
        })?;
        let previous = match self.read_manifest_typed() {
            Ok(previous) => previous,
            Err(StoreReadError::Incompatible(_)) => None,
            Err(error) => return Err(error),
        };
        if previous.as_ref().map(|old| old.generation) != expected_generation {
            return Err(StoreReadError::Conflict(
                "durable generation changed during publication".into(),
            ));
        }
        if previous
            .as_ref()
            .is_some_and(|old| generation <= old.generation)
        {
            return Err(StoreReadError::Conflict(
                "generation must advance monotonically".into(),
            ));
        }
        let generation_path = format!("{GENERATIONS_DIR}/{generation:016x}");
        let mut next = Manifest {
            version: crate::PROGRAM_VERSION,
            generation,
            repository: encode_hex(&self.repository_identity),
            configuration: encode_hex(&self.configuration_identity),
            segments: format!("{generation_path}/segments"),
            catalog: None,
            verify: BTreeMap::new(),
        };
        let mut stats = CommitStats::default();
        let generations = self.directory.join(GENERATIONS_DIR);
        create_private_directory(&generations)
            .map_err(|error| store_write_io(&generations, error))?;
        check_owned_directory(
            &generations,
            &fs::symlink_metadata(&generations).map_err(|source| StoreReadError::Io {
                path: generations.clone(),
                source,
            })?,
        )
        .map_err(StoreReadError::Corrupt)?;
        let target = self.generation_directory(generation);
        let target_present = match fs::symlink_metadata(&target) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(source) => {
                return Err(StoreReadError::Io {
                    path: target.clone(),
                    source,
                });
            }
        };
        if target_present {
            retire_generation_checked(&target).map_err(StoreReadError::Corrupt)?;
            stats.orphan_generations_removed += 1;
            sync_directory(&generations).map_err(|error| store_write_io(&generations, error))?;
        }
        let nonce = unique_nonce().map_err(StoreReadError::Resource)?;
        let temporary = generations.join(format!(".tmp-{generation:016x}-{nonce:016x}"));
        create_private_directory_new(&temporary)
            .map_err(|error| store_write_io(&temporary, error))?;
        let segment_directory = temporary.join("segments");
        create_private_directory_new(&segment_directory)
            .map_err(|error| store_write_io(&segment_directory, error))?;
        let build_result = (|| -> Result<(), StoreReadError> {
            if let Some(catalog) = catalog {
                let path = temporary.join(CATALOG_NAME);
                let mut file =
                    create_private_new(&path).map_err(|error| store_write_io(&path, error))?;
                file.write_all(&catalog)
                    .map_err(|source| StoreReadError::Io {
                        path: path.clone(),
                        source,
                    })?;
                file.sync_all().map_err(|source| StoreReadError::Io {
                    path: path.clone(),
                    source,
                })?;
                next.catalog = Some(format!("{generation_path}/{CATALOG_NAME}"));
            }
            let mut names = Vec::with_capacity(inputs.len());
            let mut identities = HashSet::with_capacity(inputs.len());
            for (index, input) in inputs.iter().enumerate() {
                let SegmentInput::NewStream {
                    collection,
                    ordinal,
                    active,
                    origin,
                    write,
                } = input;
                let metadata = SegmentWriteMetadata {
                    generation,
                    collection: *collection,
                    ordinal: *ordinal,
                    slot_count: active.slot_count(),
                    active: Some(active),
                    write_validated: *origin == SegmentOrigin::Validated,
                };
                if !identities.insert((*collection, *ordinal)) {
                    return Err(StoreReadError::Corrupt(
                        "duplicate segment identity in new generation".into(),
                    ));
                }
                let name = self.write_new_segment(
                    &segment_directory,
                    generation,
                    index,
                    nonce,
                    &metadata,
                    &mut stats,
                    |file| write(file, &metadata),
                )?;
                names.push(name);
            }
            sync_directory(&segment_directory)
                .map_err(|error| store_write_io(&segment_directory, error))?;
            sync_directory(&temporary).map_err(|error| store_write_io(&temporary, error))?;
            let final_directory = self.generation_directory(generation);
            fs::rename(&temporary, &final_directory).map_err(|source| StoreReadError::Io {
                path: final_directory.clone(),
                source,
            })?;
            sync_directory(&generations).map_err(|error| store_write_io(&generations, error))?;
            self.publish_manifest(&next, nonce)?;
            stats.segment_names = names;
            Ok(())
        })();
        if build_result.is_err() && temporary.exists() {
            let _ = fs::remove_dir_all(&temporary);
        }
        build_result?;
        self.reclaim_generations(generation, &mut stats);
        self.reclaim_orphans(&mut stats);
        Ok(stats)
    }

    fn write_new_segment(
        &self,
        directory: &Path,
        generation: u64,
        index: usize,
        nonce: u64,
        metadata: &SegmentWriteMetadata<'_>,
        stats: &mut CommitStats,
        write: impl FnOnce(&mut File) -> Result<(), String>,
    ) -> Result<String, StoreReadError> {
        if metadata.ordinal == 0 {
            return Err(StoreReadError::Corrupt(
                "segment ordinal must be nonzero".into(),
            ));
        }
        let name = format!("segment-{generation:016x}-{index:08x}-{nonce:016x}.idx");
        let path = directory.join(&name);
        let mut file = create_private_new(&path).map_err(|error| store_write_io(&path, error))?;
        let result = (|| {
            write(&mut file).map_err(|error| store_write_io(&path, error))?;
            file.sync_all().map_err(|source| StoreReadError::Io {
                path: path.clone(),
                source,
            })?;
            let length = file
                .metadata()
                .map_err(|source| StoreReadError::Io {
                    path: path.clone(),
                    source,
                })?
                .len();
            let reader = SegmentReader::from_index_file_typed(&open_regular_nofollow_read(&path)?)
                .map_err(|error| segment_open_store_error(&path, error))?;
            reader
                .require_complete_sections()
                .map_err(StoreReadError::Corrupt)?;
            let descriptor = reader
                .descriptor()
                .ok_or_else(|| StoreReadError::Corrupt("new segment header missing".into()))?;
            if descriptor.generation != metadata.generation
                || descriptor.collection != metadata.collection
                || descriptor.ordinal != metadata.ordinal
                || descriptor.slot_count != metadata.slot_count
                || descriptor.active.bitmap()
                    != metadata
                        .active
                        .map(ActiveRecords::bitmap)
                        .unwrap_or_else(|| {
                            ActiveRecords::all(metadata.slot_count as usize)
                                .expect("validated slot count")
                                .bitmap()
                        })
                || descriptor.write_validated != metadata.write_validated
            {
                return Err(StoreReadError::Corrupt(
                    "new segment identity differs from its writer metadata".into(),
                ));
            }
            Ok::<u64, StoreReadError>(length)
        })();
        drop(file);
        let length = match result {
            Ok(length) => length,
            Err(error) => {
                fs::remove_file(&path).map_err(|cleanup| StoreReadError::Io {
                    path: path.clone(),
                    source: io::Error::other(format!("{error}; remove failed segment: {cleanup}")),
                })?;
                return Err(error);
            }
        };
        stats.segment_bytes_written = stats.segment_bytes_written.saturating_add(length);
        Ok(name)
    }

    fn publish_manifest(&self, manifest: &Manifest, nonce: u64) -> Result<(), StoreReadError> {
        let text = toml::to_string_pretty(manifest)
            .map_err(|error| StoreReadError::Corrupt(format!("manifest encode: {error}")))?;
        if text.len() > MAX_MANIFEST_BYTES {
            return Err(StoreReadError::Corrupt(
                "manifest exceeds size limit".into(),
            ));
        }
        let temporary = self.directory.join(format!("manifest-{nonce:016x}.tmp"));
        let mut file =
            create_private_new(&temporary).map_err(|error| store_write_io(&temporary, error))?;
        file.write_all(text.as_bytes())
            .map_err(|source| StoreReadError::Io {
                path: temporary.clone(),
                source,
            })?;
        file.sync_all().map_err(|source| StoreReadError::Io {
            path: temporary.clone(),
            source,
        })?;
        drop(file);
        fs::rename(&temporary, self.directory.join(MANIFEST_NAME)).map_err(|source| {
            StoreReadError::Io {
                path: temporary,
                source,
            }
        })?;
        sync_directory(&self.directory).map_err(|error| store_write_io(&self.directory, error))
    }

    fn reclaim_generations(&self, current: u64, stats: &mut CommitStats) {
        let root = self.directory.join(GENERATIONS_DIR);
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) => {
                stats
                    .reclaim_errors
                    .push(format!("generation scan: {error}"));
                return;
            }
        };
        let mut entries = entries
            .filter_map(|entry| match entry {
                Ok(entry) => Some(entry),
                Err(error) => {
                    stats
                        .reclaim_errors
                        .push(format!("generation entry: {error}"));
                    None
                }
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| {
            let name = entry.file_name();
            let name = name.to_str().unwrap_or_default();
            let priority = if parse_generation_name(name).is_some() {
                0
            } else if valid_temporary_generation_name(name) {
                1
            } else {
                2
            };
            (priority, name.to_string())
        });
        if entries.len() > MAX_GC_ENTRIES {
            stats.reclaim_errors.push(format!(
                "generation scan limited to {MAX_GC_ENTRIES} of {} entries",
                entries.len()
            ));
        }
        let mut removed = false;
        for entry in entries.into_iter().take(MAX_GC_ENTRIES) {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                stats
                    .reclaim_errors
                    .push("non-UTF-8 generation entry skipped".into());
                continue;
            };
            let number = parse_generation_name(name);
            let temporary = name.starts_with(".tmp-") && valid_temporary_generation_name(name);
            if number.is_none() && !temporary {
                stats
                    .reclaim_errors
                    .push(format!("unknown generation entry skipped: {name}"));
                continue;
            }
            let path = entry.path();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    stats
                        .reclaim_errors
                        .push(format!("{}: {error}", path.display()));
                    continue;
                }
            };
            let age = SystemTime::now()
                .duration_since(metadata.modified().unwrap_or(SystemTime::now()))
                .unwrap_or_default();
            if age < MIN_RETENTION_AGE {
                continue;
            }
            if let Some(number) = number {
                if number == current
                    || (number <= current && current - number < RETAINED_GENERATIONS)
                {
                    continue;
                }
            }
            match remove_generation_checked(&path) {
                Ok(bytes) => {
                    removed = true;
                    stats.generations_removed += 1;
                    stats.generation_bytes_reclaimed =
                        stats.generation_bytes_reclaimed.saturating_add(bytes);
                    if temporary || number.is_some_and(|number| number > current) {
                        stats.orphan_generations_removed += 1;
                    }
                }
                Err(error) => stats.reclaim_errors.push(error),
            }
        }
        if removed {
            if let Err(error) = sync_directory(&root) {
                stats
                    .reclaim_errors
                    .push(format!("generation directory sync: {error}"));
            }
        }
    }

    /// Remove only recognized temporary manifests and obsolete flat-format
    /// artifacts directly inside the managed store. Unknown files are kept.
    fn reclaim_orphans(&self, stats: &mut CommitStats) {
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) => {
                stats.reclaim_errors.push(format!("orphan scan: {error}"));
                return;
            }
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let temporary_manifest = valid_temporary_manifest_name(name);
            let expected_magic: Option<&[u8; 8]> = if name == LEGACY_MANIFEST_NAME {
                Some(LEGACY_MANIFEST_MAGIC)
            } else if valid_segment_name(name) {
                Some(SEGMENT_MAGIC)
            } else if name.starts_with("catalog-") && name.ends_with(".bin") {
                Some(CATALOG_MAGIC)
            } else if temporary_manifest {
                None
            } else {
                continue;
            };
            let path = entry.path();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    stats
                        .reclaim_errors
                        .push(format!("{}: {error}", path.display()));
                    continue;
                }
            };
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || !owned_by_current_user(&metadata)
            {
                continue;
            }
            if let Some(expected_magic) = expected_magic {
                let mut file = match open_regular_nofollow(&path) {
                    Ok(file) => file,
                    Err(error) => {
                        stats.reclaim_errors.push(error);
                        continue;
                    }
                };
                let mut magic = [0u8; 8];
                if file.read_exact(&mut magic).is_err() || &magic != expected_magic {
                    continue;
                }
            }
            match fs::remove_file(&path) {
                Ok(()) => stats.orphan_files_reclaimed += 1,
                Err(error) => stats
                    .reclaim_errors
                    .push(format!("{}: {error}", path.display())),
            }
        }
    }

    #[cfg(test)]
    fn open_lock(&self) -> Result<File, String> {
        self.open_lock_read().map_err(|error| error.to_string())
    }

    fn open_lock_read(&self) -> Result<File, StoreReadError> {
        let path = self.directory.join(LOCK_NAME);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        private_nofollow_options(&mut options);
        let file = options.open(&path).map_err(|source| StoreReadError::Io {
            path: path.clone(),
            source,
        })?;
        let metadata = file.metadata().map_err(|source| StoreReadError::Io {
            path: path.clone(),
            source,
        })?;
        if !metadata.is_file()
            || !owned_by_current_user(&metadata)
            || writable_by_other_users(&metadata)
        {
            return Err(StoreReadError::Corrupt(
                "store lock is not a safe owned regular file".into(),
            ));
        }
        Ok(file)
    }

    #[cfg(test)]
    fn read_manifest(&self) -> Result<Option<Manifest>, String> {
        self.read_manifest_typed()
            .map_err(|error| error.to_string())
    }

    fn read_manifest_typed(&self) -> Result<Option<Manifest>, StoreReadError> {
        let path = self.directory.join(MANIFEST_NAME);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let legacy = self.directory.join(LEGACY_MANIFEST_NAME);
                match fs::symlink_metadata(&legacy) {
                    Ok(_) => {
                        return Err(StoreReadError::Incompatible(
                            "obsolete binary manifest requires an index rebuild".into(),
                        ));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(source) => {
                        return Err(StoreReadError::Io {
                            path: legacy,
                            source,
                        });
                    }
                }
            }
            Err(source) => return Err(StoreReadError::Io { path, source }),
            Ok(_) => {}
        }
        let file = open_regular_nofollow_read(&path)?;
        let mut bytes = Vec::new();
        file.take((MAX_MANIFEST_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|source| StoreReadError::Io {
                path: path.clone(),
                source,
            })?;
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(StoreReadError::Corrupt(
                "manifest exceeds size limit".into(),
            ));
        }
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| StoreReadError::Corrupt("manifest is not UTF-8".into()))?;
        let manifest: Manifest = toml::from_str(text)
            .map_err(|error| StoreReadError::Corrupt(format!("manifest parse: {error}")))?;
        if manifest.version != crate::PROGRAM_VERSION {
            return Err(StoreReadError::Incompatible(format!(
                "unsupported program version {}",
                manifest.version
            )));
        }
        if manifest.generation == 0 {
            return Err(StoreReadError::Corrupt(
                "manifest generation must be nonzero".into(),
            ));
        }
        let repository = decode_sha256(&manifest.repository).map_err(|message| {
            StoreReadError::Corrupt(format!("repository identity: {message}"))
        })?;
        let configuration = decode_sha256(&manifest.configuration).map_err(|message| {
            StoreReadError::Corrupt(format!("configuration identity: {message}"))
        })?;
        if repository != self.repository_identity || configuration != self.configuration_identity {
            return Err(StoreReadError::Incompatible(
                "manifest repository or configuration identity mismatch".into(),
            ));
        }
        manifest_relative_path(&manifest.segments, "segments").map_err(StoreReadError::Corrupt)?;
        if let Some(catalog) = &manifest.catalog {
            manifest_relative_path(catalog, "catalog").map_err(StoreReadError::Corrupt)?;
        }
        for (key, record) in &manifest.verify {
            manifest_relative_path(key, "verify entry").map_err(StoreReadError::Corrupt)?;
            decode_sha256(&record.sha256).map_err(|message| {
                StoreReadError::Corrupt(format!("verify entry {key}: {message}"))
            })?;
        }
        Ok(Some(manifest))
    }

    fn read_catalog_typed(&self, path: &Path) -> Result<Arc<[u8]>, OpenGenerationError> {
        let file = open_generation_file(path)?;
        let length = file
            .metadata()
            .map_err(|source| StoreReadError::Io {
                path: path.to_path_buf(),
                source,
            })?
            .len();
        let length = usize::try_from(length)
            .map_err(|_| StoreReadError::Corrupt("catalog too large".into()))?;
        if length < 8 || length > MAX_CATALOG_BYTES {
            return Err(
                StoreReadError::Corrupt("generation catalog byte length invalid".into()).into(),
            );
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|error| StoreReadError::Resource(format!("catalog allocation: {error}")))?;
        file.take((length + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|source| StoreReadError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        if bytes.len() != length {
            return Err(
                StoreReadError::Corrupt("generation catalog length mismatch".into()).into(),
            );
        }
        if !bytes.starts_with(CATALOG_MAGIC) {
            return Err(StoreReadError::Corrupt(
                "generation catalog file type marker mismatch".into(),
            )
            .into());
        }
        Ok(Arc::from(bytes))
    }
}

fn verify_regular_file(
    path: &Path,
    expected_length: &u64,
    expected_digest: &[u8; 32],
    key: &str,
) -> Result<(), OpenGenerationError> {
    let mut file = match open_regular_nofollow_read(path) {
        Ok(file) => file,
        Err(StoreReadError::Corrupt(message))
            if message.starts_with("missing referenced store file:") =>
        {
            return Err(
                StoreReadError::Corrupt(format!("verification target missing: {key}")).into(),
            );
        }
        Err(error) => return Err(error.into()),
    };
    let length = file
        .metadata()
        .map_err(|source| StoreReadError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    if length != *expected_length {
        return Err(StoreReadError::Corrupt(format!("verification length mismatch: {key}")).into());
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| StoreReadError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > *expected_length {
            return Err(
                StoreReadError::Corrupt(format!("verification length changed: {key}")).into(),
            );
        }
        hash.update(&buffer[..read]);
    }
    if total != *expected_length || hash.finalize().as_slice() != expected_digest {
        return Err(StoreReadError::Corrupt(format!("verification digest mismatch: {key}")).into());
    }
    Ok(())
}

fn manifest_relative_path(value: &str, context: &str) -> Result<PathBuf, String> {
    if value.is_empty() {
        return Err(format!("manifest {context} path is empty"));
    }
    let path = Path::new(value);
    if path.is_absolute() {
        return Err(format!("manifest {context} path is absolute"));
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(format!(
                    "manifest {context} path is not normalized: {value}"
                ));
            }
        }
    }
    if normalized.as_os_str().is_empty() || path != normalized.as_path() {
        return Err(format!(
            "manifest {context} path is not normalized: {value}"
        ));
    }
    Ok(normalized)
}

fn decode_sha256(value: &str) -> Result<[u8; 32], String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("expected 64 hexadecimal characters".into());
    }
    let mut bytes = [0u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| "invalid hexadecimal digest".to_string())?;
    }
    Ok(bytes)
}

fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        text.push(DIGITS[(byte >> 4) as usize] as char);
        text.push(DIGITS[(byte & 15) as usize] as char);
    }
    text
}

fn valid_segment_name(name: &str) -> bool {
    if !name.starts_with("segment-") || !name.ends_with(".idx") {
        return false;
    }
    let stem = &name[8..name.len() - 4];
    let mut parts = stem.split('-');
    matches!(parts.next(), Some(part) if part.len() == 16 && part.bytes().all(|byte| byte.is_ascii_hexdigit()))
        && matches!(parts.next(), Some(part) if part.len() == 8 && part.bytes().all(|byte| byte.is_ascii_hexdigit()))
        && matches!(parts.next(), Some(part) if part.len() == 16 && part.bytes().all(|byte| byte.is_ascii_hexdigit()))
        && parts.next().is_none()
}

fn valid_temporary_manifest_name(name: &str) -> bool {
    let Some(stem) = name
        .strip_prefix("manifest-")
        .and_then(|value| value.strip_suffix(".tmp"))
    else {
        return false;
    };
    stem.len() == 16 && stem.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn parse_generation_name(name: &str) -> Option<u64> {
    (name.len() == 16 && name.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| u64::from_str_radix(name, 16).ok())
        .flatten()
}

fn valid_temporary_generation_name(name: &str) -> bool {
    let Some(stem) = name.strip_prefix(".tmp-") else {
        return false;
    };
    let mut parts = stem.split('-');
    matches!(parts.next(), Some(part) if parse_generation_name(part).is_some())
        && matches!(parts.next(), Some(part) if parse_generation_name(part).is_some())
        && parts.next().is_none()
}

/// Only known owned files under one owned generation may be removed. Listing
/// before mutation means an unknown or symlinked entry preserves the whole
/// directory for inspection rather than being swallowed by remove_dir_all.
fn checked_generation_contents(
    directory: &Path,
) -> Result<(Vec<(PathBuf, u64)>, Option<PathBuf>), String> {
    let metadata = fs::symlink_metadata(directory).map_err(|error| error.to_string())?;
    check_owned_directory(directory, &metadata)?;
    let mut files = Vec::new();
    let mut segments = None;
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        let name = name.to_str().ok_or("non-UTF-8 generation entry")?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if name == "segments" {
            check_owned_directory(&path, &metadata)?;
            segments = Some(path);
        } else if name == CATALOG_NAME && safe_owned_file(&metadata) {
            files.push((path, physical_reclaimed_bytes(&metadata)));
        } else {
            return Err(format!(
                "unsafe or unknown generation entry: {}",
                path.display()
            ));
        }
    }
    if let Some(segment_directory) = &segments {
        for entry in fs::read_dir(segment_directory).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let name = entry.file_name();
            let name = name.to_str().ok_or("non-UTF-8 segment entry")?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
            if !valid_segment_name(name) || !safe_owned_file(&metadata) {
                return Err(format!(
                    "unsafe or unknown generation segment: {}",
                    path.display()
                ));
            }
            files.push((path, physical_reclaimed_bytes(&metadata)));
        }
    }
    Ok((files, segments))
}

fn retire_generation_checked(directory: &Path) -> Result<(), String> {
    checked_generation_contents(directory)?;
    let parent = directory
        .parent()
        .ok_or("generation directory has no parent")?;
    let number = directory
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(parse_generation_name)
        .ok_or("retired generation name invalid")?;
    let nonce = unique_nonce()?;
    let retired = parent.join(format!(".tmp-{number:016x}-{nonce:016x}"));
    if fs::symlink_metadata(&retired).is_ok() {
        return Err(format!(
            "retired generation target already exists: {}",
            retired.display()
        ));
    }
    fs::rename(directory, &retired)
        .map_err(|error| format!("retire generation {}: {error}", directory.display()))?;
    Ok(())
}

fn remove_generation_checked(directory: &Path) -> Result<u64, String> {
    let (files, segments) = checked_generation_contents(directory)?;
    let mut bytes = 0u64;
    for (path, size) in files {
        fs::remove_file(&path).map_err(|error| format!("remove {}: {error}", path.display()))?;
        bytes = bytes.saturating_add(size);
    }
    if let Some(segment_directory) = segments {
        fs::remove_dir(&segment_directory)
            .map_err(|error| format!("remove {}: {error}", segment_directory.display()))?;
    }
    fs::remove_dir(directory)
        .map_err(|error| format!("remove {}: {error}", directory.display()))?;
    Ok(bytes)
}

fn safe_owned_file(metadata: &fs::Metadata) -> bool {
    metadata.is_file()
        && !metadata.file_type().is_symlink()
        && owned_by_current_user(metadata)
        && !writable_by_other_users(metadata)
}

#[cfg(unix)]
fn physical_reclaimed_bytes(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    if metadata.nlink() == 1 {
        metadata.len()
    } else {
        0
    }
}

#[cfg(not(unix))]
fn physical_reclaimed_bytes(metadata: &fs::Metadata) -> u64 {
    metadata.len()
}

fn unique_nonce() -> Result<u64, String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos() as u64)
}

fn ensure_directories_without_symlinks(path: &Path, namespace: &Path) -> Result<(), String> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir | Component::ParentDir => {
                return Err("managed store path is not normalized".into());
            }
            Component::Normal(part) => {
                current.push(part);
                match fs::symlink_metadata(&current) {
                    Ok(metadata) => {
                        if metadata.file_type().is_symlink() || !metadata.is_dir() {
                            return Err(format!(
                                "unsafe managed store directory: {}",
                                current.display()
                            ));
                        }
                        if current.starts_with(namespace) {
                            check_owned_directory(&current, &metadata)?;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        create_private_directory(&current)?;
                        let metadata = fs::symlink_metadata(&current).map_err(|error| {
                            format!("store directory {}: {error}", current.display())
                        })?;
                        if metadata.file_type().is_symlink() || !metadata.is_dir() {
                            return Err(format!(
                                "unsafe managed store directory: {}",
                                current.display()
                            ));
                        }
                        if current.starts_with(namespace) {
                            check_owned_directory(&current, &metadata)?;
                        }
                    }
                    Err(error) => {
                        return Err(format!("store directory {}: {error}", current.display()));
                    }
                }
            }
        }
    }
    Ok(())
}

fn check_owned_directory(path: &Path, metadata: &fs::Metadata) -> Result<(), String> {
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || !owned_by_current_user(metadata)
        || writable_by_other_users(metadata)
    {
        return Err(format!(
            "unsafe or unowned managed store namespace: {}",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn writable_by_other_users(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.mode() & 0o022 != 0
}

#[cfg(not(unix))]
fn writable_by_other_users(_metadata: &fs::Metadata) -> bool {
    false
}

fn create_private_directory(path: &Path) -> Result<(), String> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(format!(
            "create store directory {}: {error}",
            path.display()
        )),
    }
}

fn create_private_directory_new(path: &Path) -> Result<(), String> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|error| format!("create store directory {}: {error}", path.display()))
}

#[cfg(unix)]
fn check_private_namespace(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    // SAFETY: geteuid has no pointers or preconditions.
    let user = unsafe { libc::geteuid() };
    if !metadata.is_dir() || metadata.uid() != user || metadata.mode() & 0o077 != 0 {
        return Err("temporary index namespace is not private to this user".into());
    }
    Ok(())
}

#[cfg(unix)]
fn owned_by_current_user(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid has no pointers or preconditions.
    metadata.uid() == unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
fn owned_by_current_user(_metadata: &fs::Metadata) -> bool {
    true
}

#[cfg(not(unix))]
fn check_private_namespace(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn private_nofollow_options(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
}

#[cfg(not(unix))]
fn private_nofollow_options(_options: &mut OpenOptions) {}

fn create_private_new(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    private_nofollow_options(&mut options);
    options
        .open(path)
        .map_err(|error| format!("create store file {}: {error}", path.display()))
}

fn open_regular_nofollow(path: &Path) -> Result<File, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(format!("unsafe store file: {}", path.display()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("missing regular file: {}", path.display()));
        }
        Err(error) => return Err(format!("store file {}: {error}", path.display())),
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|error| format!("store file {}: {error}", path.display()))?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file()
        || !owned_by_current_user(&metadata)
        || writable_by_other_users(&metadata)
    {
        return Err(format!(
            "store file is not an owned regular file: {}",
            path.display()
        ));
    }
    Ok(file)
}

fn open_regular_nofollow_read(path: &Path) -> Result<File, StoreReadError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(StoreReadError::Corrupt(format!(
                "unsafe store file: {}",
                path.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(StoreReadError::Corrupt(format!(
                "missing referenced store file: {}",
                path.display()
            )));
        }
        Err(source) => {
            return Err(StoreReadError::Io {
                path: path.to_owned(),
                source,
            });
        }
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            StoreReadError::Corrupt(format!("missing referenced store file: {}", path.display()))
        } else {
            StoreReadError::Io {
                path: path.to_owned(),
                source,
            }
        }
    })?;
    let metadata = file.metadata().map_err(|source| StoreReadError::Io {
        path: path.to_owned(),
        source,
    })?;
    if !metadata.is_file()
        || !owned_by_current_user(&metadata)
        || writable_by_other_users(&metadata)
    {
        return Err(StoreReadError::Corrupt(format!(
            "store file is not an owned regular file: {}",
            path.display()
        )));
    }
    Ok(file)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("store directory sync: {error}"))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
#[path = "managed_store.test.rs"]
mod tests;
