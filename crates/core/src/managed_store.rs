//! Immutable generation files and atomic, checked manifest publication.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::index_storage::{IndexResidency, ResolvedIndexStorage};
use crate::segment_format::{FORMAT_VERSION, REQUIRED_SECTIONS, SectionKind, SegmentReader};

const MANIFEST_MAGIC: &[u8; 8] = b"OBJHMAN1";
const MANIFEST_NAME: &str = "manifest.bin";
const LOCK_NAME: &str = "store.lock";
const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
const MAX_SEGMENTS: usize = 16_384;
const DEPRECATED_MAGIC: &[u8; 8] = b"OBJSNAP1";
const DEPRECATED_VERSIONS: [u32; 2] = [27, 28];
const DEPRECATED_NAMES: [&str; 2] = ["search-core-ascii.snapshot", "search-core-unicode.snapshot"];

#[derive(Clone, Debug)]
struct SegmentRecord {
    name: String,
    length: u64,
}

#[derive(Clone, Debug)]
struct Manifest {
    generation: u64,
    segments: Vec<SegmentRecord>,
}

/// A previous immutable segment may be referenced without rewriting it.
pub(crate) enum SegmentInput<'a> {
    Existing(&'a str),
    New(Arc<[u8]>),
}

pub(crate) struct OpenedGeneration {
    pub generation: u64,
    pub segment_names: Vec<String>,
    pub segments: Vec<Arc<SegmentReader>>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CommitStats {
    pub generation: u64,
    pub segment_bytes_written: u64,
    pub segments_reused: usize,
    pub obsolete_files_reclaimed: usize,
    pub orphan_files_reclaimed: usize,
    pub reclaim_errors: Vec<String>,
}

/// All paths are beneath one application-owned directory. A store exists only
/// after a disk policy explicitly opens it; memory-only never constructs one.
pub(crate) struct ManagedStore {
    directory: PathBuf,
    repository_identity: [u8; 32],
    configuration_identity: [u8; 32],
    root_bytes: Vec<u8>,
    deprecated_directories: Vec<PathBuf>,
    residency: IndexResidency,
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
        ensure_directories_without_symlinks(directory)?;
        if resolved.private_namespace {
            check_private_namespace(namespace)?;
        }
        Ok(Self {
            directory: directory.clone(),
            repository_identity: resolved.repository_identity,
            configuration_identity: resolved.configuration_identity,
            root_bytes: path_bytes(&resolved.repository_root),
            deprecated_directories: resolved.deprecated_directories.clone(),
            residency: resolved.residency,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.directory
    }

    /// Remove only the two previous snapshot filenames in the selected
    /// storage scope, and only after their regular-file header proves a
    /// supported old format. This is never called for memory-only storage.
    pub(crate) fn cleanup_deprecated_snapshots(&self) -> Result<Vec<PathBuf>, String> {
        let lock = self.open_lock()?;
        lock.lock()
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

    /// Hold a shared cross-process lock until every immutable file is opened
    /// and mapped. Pinned mappings then survive later manifest publication.
    pub(crate) fn read_generation(&self) -> Result<Option<OpenedGeneration>, String> {
        let lock = self.open_lock()?;
        lock.lock_shared()
            .map_err(|error| format!("store reader lock: {error}"))?;
        let Some(manifest) = self.read_manifest()? else {
            return Ok(None);
        };
        let mut segments = Vec::with_capacity(manifest.segments.len());
        let mut names = Vec::with_capacity(manifest.segments.len());
        for record in &manifest.segments {
            let path = self.directory.join(&record.name);
            let file = open_regular_nofollow(&path)?;
            if file.metadata().map_err(|error| error.to_string())?.len() != record.length {
                return Err(format!("segment length changed: {}", record.name));
            }
            let reader = match self.residency {
                IndexResidency::OnDemand => SegmentReader::from_index_file(&file)?,
                IndexResidency::FullyResident => SegmentReader::from_index_file_owned(&file)?,
            };
            reader.require_complete_sections()?;
            for kind in REQUIRED_SECTIONS {
                if kind != SectionKind::PostingPayloads
                    || self.residency == IndexResidency::FullyResident
                {
                    reader.section(kind)?;
                }
            }
            names.push(record.name.clone());
            segments.push(Arc::new(reader));
        }
        Ok(Some(OpenedGeneration {
            generation: manifest.generation,
            segment_names: names,
            segments,
        }))
    }

    /// Write only new immutable segments, then atomically replace the small
    /// manifest. Existing entries must belong to the current generation.
    pub(crate) fn commit_generation(
        &self,
        generation: u64,
        inputs: &[SegmentInput<'_>],
    ) -> Result<CommitStats, String> {
        if generation == 0 || inputs.len() > MAX_SEGMENTS {
            return Err("invalid generation or segment count".into());
        }
        let lock = self.open_lock()?;
        lock.lock()
            .map_err(|error| format!("store writer lock: {error}"))?;
        let previous = self.read_manifest()?;
        if previous
            .as_ref()
            .is_some_and(|old| generation <= old.generation)
        {
            return Err("generation must advance monotonically".into());
        }
        let previous_names = previous
            .as_ref()
            .map(|old| {
                old.segments
                    .iter()
                    .map(|segment| segment.name.as_str())
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();
        let mut next = Manifest {
            generation,
            segments: Vec::with_capacity(inputs.len()),
        };
        let mut stats = CommitStats {
            generation,
            ..Default::default()
        };
        self.reclaim_orphans(previous.as_ref(), &mut stats)?;
        let nonce = unique_nonce()?;
        for (index, input) in inputs.iter().enumerate() {
            match input {
                SegmentInput::Existing(name) => {
                    if !previous_names.contains(name) {
                        return Err(format!("segment is not in durable generation: {name}"));
                    }
                    let record = previous
                        .as_ref()
                        .unwrap()
                        .segments
                        .iter()
                        .find(|segment| segment.name == *name)
                        .unwrap()
                        .clone();
                    let file = open_regular_nofollow(&self.directory.join(&record.name))?;
                    if file.metadata().map_err(|error| error.to_string())?.len() != record.length {
                        return Err("reused segment length changed".into());
                    }
                    SegmentReader::from_index_file(&file)?.require_complete_sections()?;
                    next.segments.push(record);
                    stats.segments_reused += 1;
                }
                SegmentInput::New(bytes) => {
                    let reader = SegmentReader::from_owned(Arc::clone(bytes))?;
                    reader.require_complete_sections()?;
                    let name = format!("segment-{generation:016x}-{index:08x}-{nonce:016x}.idx");
                    let path = self.directory.join(&name);
                    let mut file = create_private_new(&path)?;
                    file.write_all(bytes)
                        .map_err(|error| format!("segment write: {error}"))?;
                    file.sync_all()
                        .map_err(|error| format!("segment sync: {error}"))?;
                    next.segments.push(SegmentRecord {
                        name,
                        length: bytes.len() as u64,
                    });
                    stats.segment_bytes_written = stats
                        .segment_bytes_written
                        .saturating_add(bytes.len() as u64);
                }
            }
        }
        if next
            .segments
            .iter()
            .map(|record| &record.name)
            .collect::<HashSet<_>>()
            .len()
            != next.segments.len()
        {
            return Err("duplicate segment in new generation".into());
        }
        let bytes = self.encode_manifest(&next)?;
        let temporary = self.directory.join(format!("manifest-{nonce:016x}.tmp"));
        let mut file = create_private_new(&temporary)?;
        file.write_all(&bytes)
            .map_err(|error| format!("manifest write: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("manifest sync: {error}"))?;
        drop(file);
        fs::rename(&temporary, self.directory.join(MANIFEST_NAME))
            .map_err(|error| format!("manifest publish: {error}"))?;
        sync_directory(&self.directory)?;
        if let Some(previous) = previous {
            let next_names = next
                .segments
                .iter()
                .map(|segment| segment.name.as_str())
                .collect::<HashSet<_>>();
            for record in previous.segments {
                if next_names.contains(record.name.as_str()) {
                    continue;
                }
                let path = self.directory.join(&record.name);
                let metadata = match fs::symlink_metadata(&path) {
                    Ok(metadata)
                        if metadata.is_file()
                            && !metadata.file_type().is_symlink()
                            && owned_by_current_user(&metadata) =>
                    {
                        metadata
                    }
                    Ok(_) => {
                        stats
                            .reclaim_errors
                            .push(format!("unsafe obsolete segment: {}", record.name));
                        continue;
                    }
                    Err(error) => {
                        stats
                            .reclaim_errors
                            .push(format!("{}: {error}", record.name));
                        continue;
                    }
                };
                if metadata.len() != record.length {
                    stats
                        .reclaim_errors
                        .push(format!("obsolete segment length changed: {}", record.name));
                    continue;
                }
                match fs::remove_file(path) {
                    Ok(()) => stats.obsolete_files_reclaimed += 1,
                    Err(error) => stats
                        .reclaim_errors
                        .push(format!("{}: {error}", record.name)),
                }
            }
        }
        Ok(stats)
    }

    fn reclaim_orphans(
        &self,
        current: Option<&Manifest>,
        stats: &mut CommitStats,
    ) -> Result<(), String> {
        let referenced = current
            .map(|manifest| {
                manifest
                    .segments
                    .iter()
                    .map(|entry| entry.name.as_str())
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();
        for entry in fs::read_dir(&self.directory).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let expected_magic = if valid_segment_name(name) && !referenced.contains(name) {
                b"OBJHSEG1".as_slice()
            } else if valid_temporary_manifest_name(name) {
                MANIFEST_MAGIC.as_slice()
            } else {
                continue;
            };
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || !owned_by_current_user(&metadata)
            {
                continue;
            }
            let mut file = open_regular_nofollow(&path)?;
            let mut magic = [0u8; 8];
            if file.read_exact(&mut magic).is_err() || magic != expected_magic {
                continue;
            }
            drop(file);
            match fs::remove_file(&path) {
                Ok(()) => stats.orphan_files_reclaimed += 1,
                Err(error) => stats
                    .reclaim_errors
                    .push(format!("{}: {error}", path.display())),
            }
        }
        Ok(())
    }

    fn open_lock(&self) -> Result<File, String> {
        let path = self.directory.join(LOCK_NAME);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        private_nofollow_options(&mut options);
        let file = options
            .open(&path)
            .map_err(|error| format!("store lock {}: {error}", path.display()))?;
        if !file
            .metadata()
            .map_err(|error| error.to_string())?
            .is_file()
        {
            return Err("store lock is not a regular file".into());
        }
        Ok(file)
    }

    fn read_manifest(&self) -> Result<Option<Manifest>, String> {
        let path = self.directory.join(MANIFEST_NAME);
        let file = match open_regular_nofollow(&path) {
            Ok(file) => file,
            Err(error) if error.starts_with("missing regular file:") => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take((MAX_MANIFEST_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("manifest read: {error}"))?;
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err("manifest exceeds size limit".into());
        }
        self.decode_manifest(&bytes).map(Some)
    }

    fn encode_manifest(&self, manifest: &Manifest) -> Result<Vec<u8>, String> {
        if self.root_bytes.len() > u32::MAX as usize {
            return Err("repository root path too long".into());
        }
        let mut bytes = Vec::with_capacity(256 + manifest.segments.len() * 64);
        bytes.extend_from_slice(MANIFEST_MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&crate::MODEL_VERSION.to_le_bytes());
        bytes.extend_from_slice(&self.repository_identity);
        bytes.extend_from_slice(&self.configuration_identity);
        bytes.extend_from_slice(&(self.root_bytes.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&self.root_bytes);
        bytes.extend_from_slice(&manifest.generation.to_le_bytes());
        bytes.extend_from_slice(&(manifest.segments.len() as u32).to_le_bytes());
        for record in &manifest.segments {
            if !valid_segment_name(&record.name) {
                return Err("invalid segment filename".into());
            }
            bytes.extend_from_slice(&(record.name.len() as u16).to_le_bytes());
            bytes.extend_from_slice(record.name.as_bytes());
            bytes.extend_from_slice(&record.length.to_le_bytes());
        }
        let digest = Sha256::digest(&bytes);
        bytes.extend_from_slice(&digest);
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err("manifest exceeds size limit".into());
        }
        Ok(bytes)
    }

    fn decode_manifest(&self, bytes: &[u8]) -> Result<Manifest, String> {
        if bytes.len() < 8 + 4 + 4 + 32 + 32 + 4 + 8 + 4 + 32 {
            return Err("manifest truncated".into());
        }
        let payload_end = bytes.len() - 32;
        if Sha256::digest(&bytes[..payload_end]).as_slice() != &bytes[payload_end..] {
            return Err("manifest checksum mismatch".into());
        }
        let mut cursor = SliceCursor::new(&bytes[..payload_end]);
        if cursor.take(8)? != MANIFEST_MAGIC {
            return Err("manifest magic mismatch".into());
        }
        if cursor.u32()? != FORMAT_VERSION || cursor.u32()? != crate::MODEL_VERSION {
            return Err("unsupported manifest or model version".into());
        }
        if cursor.take(32)? != self.repository_identity
            || cursor.take(32)? != self.configuration_identity
        {
            return Err("manifest repository or configuration identity mismatch".into());
        }
        let root_len = cursor.u32()? as usize;
        if cursor.take(root_len)? != self.root_bytes {
            return Err("manifest repository root mismatch".into());
        }
        let generation = cursor.u64()?;
        let count = cursor.u32()? as usize;
        if generation == 0 || count > MAX_SEGMENTS || count > cursor.remaining() / 10 {
            return Err("manifest generation or segment count invalid".into());
        }
        let mut segments = Vec::with_capacity(count);
        let mut names = HashSet::with_capacity(count);
        for _ in 0..count {
            let name_len = cursor.u16()? as usize;
            let name = std::str::from_utf8(cursor.take(name_len)?)
                .map_err(|_| "segment name is not UTF-8")?
                .to_owned();
            let length = cursor.u64()?;
            if !valid_segment_name(&name) || length == 0 || !names.insert(name.clone()) {
                return Err("manifest segment entry invalid".into());
            }
            segments.push(SegmentRecord { name, length });
        }
        if cursor.remaining() != 0 {
            return Err("manifest trailing bytes".into());
        }
        Ok(Manifest {
            generation,
            segments,
        })
    }
}

struct SliceCursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> SliceCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        let end = self
            .position
            .checked_add(len)
            .ok_or("manifest offset overflow")?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or("manifest truncated")?;
        self.position = end;
        Ok(bytes)
    }

    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
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

fn unique_nonce() -> Result<u64, String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos() as u64)
}

fn ensure_directories_without_symlinks(path: &Path) -> Result<(), String> {
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
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        create_private_directory(&current)?;
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

fn create_private_directory(path: &Path) -> Result<(), String> {
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
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err(format!("store file is not regular: {}", path.display()));
    }
    Ok(file)
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(windows)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
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
