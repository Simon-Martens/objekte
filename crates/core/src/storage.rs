//! Versioned, checksummed sections for complete immutable search generations.

use roaring::RoaringBitmap;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use bincode::Options;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::bigram::{BIGRAM_VERSION, BigramIndex, FOLDED_REPRESENTATION};
use crate::discovery::SymlinkPolicy;
use crate::inference::{InferenceMode, InferenceState};
use crate::inventory::{
    CollectionGeneration, FileEntry, FileInventory, FileStatus, FileVersion, SourceDiagnostic,
    SourceRevision,
};
use crate::lifecycle::PublishedIndex;
use crate::model::{CollectionId, FileId, RecordId, RecordSlot};
use crate::posting::{CustomPostingIndex, PostingCollection};
use crate::relation::RelationIndex;
use crate::repository::{
    CollectionDefinition, CollectionState, CollectionStats, GlobalInferenceSummary, Repository,
    RepositoryOptions, StartupStats,
};
use crate::sorting::{COLLATION_VERSION, SortCollection};
use crate::source::SourceRegistry;
use crate::workload::BigramAlphabet;

const MAGIC: &[u8; 8] = b"OBJSNAP1";
pub const SNAPSHOT_VERSION: u32 = 17;
pub(crate) const LIFECYCLE_SNAPSHOT_VERSION: u32 = 18;
const MAX_SECTION_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const SECTION_HEADER_BYTES: usize = 44;


/// Phase timings for one explicit, durable snapshot save. Opening does not
/// modify these values or perform a save.
#[derive(Clone, Debug, Default)]
pub struct SnapshotSaveStats {
    pub total_ns: u64,
    pub directory_ns: u64,
    pub header_ns: u64,
    pub inventory_ns: u64,
    pub write_ns: u64,
    pub sync_ns: u64,
    pub rename_ns: u64,
    /// Parent-directory fsync time on Linux; zero where it is not attempted.
    pub directory_sync_ns: u64,
    pub bytes_written: u64,
    pub sections: Vec<SnapshotSectionSaveStats>,
}


#[derive(Clone, Debug)]
pub struct SnapshotSectionSaveStats {
    pub id: u32,
    pub serialize_ns: u64,
    pub checksum_ns: u64,
    pub write_ns: u64,
    pub payload_bytes: u64,
}


/// Restore failures distinguish unreadable storage from invalid snapshot data.
#[derive(Debug)]
pub(crate) enum SnapshotLoadError {
    Access(String),
    Invalid(String),
}


impl std::fmt::Display for SnapshotLoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Access(message) | Self::Invalid(message) => formatter.write_str(message),
        }
    }
}


impl From<String> for SnapshotLoadError {
    fn from(message: String) -> Self {
        Self::Invalid(message)
    }
}


impl From<&str> for SnapshotLoadError {
    fn from(message: &str) -> Self {
        Self::Invalid(message.into())
    }
}


fn snapshot_read_error(context: &str, error: std::io::Error) -> SnapshotLoadError {
    if error.kind() == std::io::ErrorKind::UnexpectedEof {
        SnapshotLoadError::Invalid(format!("truncated {context}: {error}"))
    } else {
        SnapshotLoadError::Access(format!("snapshot access while reading {context}: {error}"))
    }
}


/// Snapshot representation of one source-file revision.
#[derive(Clone, Serialize, Deserialize)]
struct EncodedFile {
    id: FileId,
    relative_path: Vec<u8>,
    revision: SourceRevision,
    created_ns: Option<i128>,
    collections: Vec<CollectionId>,
    status: FileStatus,
    diagnostics: Vec<SourceDiagnostic>,
    omitted_diagnostics: u32,
    /// Present for complete fresh-build reads in the base inventory section.
    /// Lifecycle catalog entries carry their own current fingerprint below.
    fingerprint: Option<[u8; 32]>,
}


/// One active catalog entry. A `None` slot is retired; IDs are never reused.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct LifecycleCatalogFile {
    file: EncodedFile,
    pub(crate) fingerprint: Option<[u8; 32]>,
    pub(crate) configuration: Option<[u8; 32]>,
}


impl LifecycleCatalogFile {
    pub(crate) fn decode_entry(&self) -> Result<FileEntry, String> {
        Ok(FileEntry {
            id: self.file.id,
            relative_path: path_from_bytes(self.file.relative_path.clone())?,
            revision: self.file.revision,
            created_ns: self.file.created_ns,
            collections: self.file.collections.clone(),
            status: self.file.status,
            diagnostics: self.file.diagnostics.clone(),
            omitted_diagnostics: self.file.omitted_diagnostics,
        })
    }
}


/// Encoded indexed segment supplied by the segment owner. Storage checks
/// envelope identity and active domain; the caller must validate the decoded
/// index families and relation contributions before publication.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct LifecycleSegmentCache {
    pub(crate) ordinal: u64,
    pub(crate) collection: CollectionId,
    pub(crate) record_count: u32,
    pub(crate) active: RoaringBitmap,
    pub(crate) index: Vec<u8>,
}


/// Mutation state appended after a fully validated base repository snapshot.
/// The segment payload remains opaque until the caller's structural decoder
/// validates and restores the segment's private relation representation.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct LifecycleCacheState {
    pub(crate) revision: u64,
    pub(crate) configuration_identity: [u8; 32],
    pub(crate) base_active: Vec<RoaringBitmap>,
    pub(crate) catalog: Vec<Option<LifecycleCatalogFile>>,
    pub(crate) file_versions: Vec<FileVersion>,
    pub(crate) segments: Vec<Vec<LifecycleSegmentCache>>,
    pub(crate) next_segment_ordinal: u64,
}


impl LifecycleCacheState {
    /// Capture one coherent publication without reading the source filesystem.
    /// The callback owns the private segment encoding contract.
    pub(crate) fn from_publication(
        publication: &PublishedIndex,
        mut encode_segment: impl FnMut(
            &crate::build::IndexedCollectionSegment,
        ) -> Result<Vec<u8>, String>,
    ) -> Result<Self, String> {
        let catalog = (0..publication.catalog.allocated_files())
            .map(|number| {
                let id = FileId(number as u32);
                publication
                    .catalog
                    .entry(id)
                    .map(|entry| LifecycleCatalogFile {
                        file: EncodedFile {
                            id,
                            relative_path: path_bytes(&entry.relative_path),
                            revision: entry.revision,
                            created_ns: entry.created_ns,
                            collections: entry.collections.clone(),
                            status: entry.status,
                            diagnostics: entry.diagnostics.clone(),
                            omitted_diagnostics: entry.omitted_diagnostics,
                            fingerprint: None,
                        },
                        fingerprint: publication.catalog.fingerprint(id),
                        configuration: publication.catalog.configuration(id),
                    })
            })
            .collect();
        let file_versions = (0..publication.file_versions.len())
            .map(|number| {
                publication
                    .file_versions
                    .get(FileId(number as u32))
                    .ok_or("file version missing".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let segments = publication
            .segments
            .iter()
            .enumerate()
            .map(|(number, group)| {
                group
                    .iter()
                    .map(|segment| {
                        Ok(LifecycleSegmentCache {
                            ordinal: segment.ordinal,
                            collection: CollectionId(number as u32),
                            record_count: u32::try_from(segment.index.records.len())
                                .map_err(|_| "too many segment records")?,
                            active: segment.active.bitmap(),
                            index: encode_segment(&segment.index)?,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            revision: publication.revision.0,
            configuration_identity: publication.configuration_identity,
            base_active: publication
                .active
                .iter()
                .map(|active| active.bitmap())
                .collect(),
            catalog,
            file_versions,
            segments,
            next_segment_ordinal: publication.next_segment_ordinal,
        })
    }

    /// Validate all storage-owned domains. `validate_segment` must decode and
    /// validate every index family, file mapping, and relation contribution.
    pub(crate) fn validate(
        &self,
        repository: &Repository,
        mut validate_segment: impl FnMut(&LifecycleSegmentCache) -> Result<(), String>,
    ) -> Result<(), String> {
        let definitions = repository
            .collections()
            .iter()
            .map(|state| state.definition.clone())
            .collect::<Vec<_>>();
        if self.revision == 0
            || self.configuration_identity
                != configuration_hash(&definitions, repository.options(), &repository.sources)?
        {
            return Err("invalid lifecycle revision or configuration".into());
        }
        if self.base_active.len() != repository.collections().len()
            || self.segments.len() != repository.collections().len()
            || self.catalog.len() != self.file_versions.len()
            || self.catalog.len() < repository.files().files.len()
            || self.catalog.len() > u32::MAX as usize
            || self.next_segment_ordinal == 0
            || self
                .file_versions
                .iter()
                .any(|version| version.0 == 0 || version.0 > self.revision)
        {
            return Err("lifecycle cache dimensions mismatch".into());
        }
        for (number, (active, collection)) in self
            .base_active
            .iter()
            .zip(repository.collections())
            .enumerate()
        {
            if active
                .max()
                .is_some_and(|slot| slot as usize >= collection.generation.records.len())
            {
                return Err("lifecycle base active slot out of bounds".into());
            }
            for slot in active.iter() {
                let record = &collection.generation.records[slot as usize];
                if self
                    .catalog
                    .get(record.file.0 as usize)
                    .and_then(Option::as_ref)
                    .is_none_or(|file| {
                        !file.file.collections.contains(&CollectionId(number as u32))
                    })
                {
                    return Err("lifecycle active record has no active source".into());
                }
            }
        }
        let mut paths = HashSet::new();
        for (number, entry) in self.catalog.iter().enumerate() {
            let Some(entry) = entry else { continue };
            if entry.file.id.0 as usize != number || entry.file.collections.is_empty() {
                return Err("lifecycle catalog identity mismatch".into());
            }
            if entry
                .configuration
                .is_some_and(|identity| identity != self.configuration_identity)
                || entry
                    .file
                    .collections
                    .iter()
                    .copied()
                    .collect::<HashSet<_>>()
                    .len()
                    != entry.file.collections.len()
                || entry.file.diagnostics.iter().any(|diagnostic| {
                    diagnostic.file != entry.file.id
                        || !entry.file.collections.contains(&diagnostic.collection)
                })
            {
                return Err("lifecycle catalog metadata mismatch".into());
            }
            let path = path_from_bytes(entry.file.relative_path.clone())?;
            if path.as_os_str().is_empty()
                || path.is_absolute()
                || !path
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)))
                || !paths.insert(path)
                || entry
                    .file
                    .collections
                    .iter()
                    .any(|collection| collection.0 as usize >= self.segments.len())
            {
                return Err("lifecycle catalog path or collection invalid".into());
            }
        }
        let mut ordinals = HashSet::new();
        for (number, group) in self.segments.iter().enumerate() {
            // Partitioned bases need a shard count proportional to indexed
            // records. The checked section byte budget and each segment's
            // validated slot/file domains bound cache admission instead of a
            // fixed collection-wide shard count.
            for segment in group {
                if segment.collection.0 as usize != number
                    || segment.ordinal == 0
                    || !ordinals.insert(segment.ordinal)
                    || segment.ordinal >= self.next_segment_ordinal
                    || segment
                        .active
                        .max()
                        .is_some_and(|slot| slot >= segment.record_count)
                    || segment.index.is_empty()
                {
                    return Err("lifecycle segment identity or active domain invalid".into());
                }
                validate_segment(segment)?;
            }
        }
        Ok(())
    }
}


/// Encode one pinned lifecycle revision with the existing atomic temp-file
/// protocol. `before_publish` is where the caller rechecks destination safety.
pub(crate) fn save_lifecycle_snapshot_with_stats(
    publication: &PublishedIndex,
    path: &Path,
    encode_segment: impl FnMut(&crate::build::IndexedCollectionSegment) -> Result<Vec<u8>, String>,
    before_publish: impl FnOnce() -> Result<(), String>,
) -> Result<SnapshotSaveStats, String> {
    let state = LifecycleCacheState::from_publication(publication, encode_segment)?;
    state.validate(&publication.repository, |_| Ok(()))?;
    publication
        .repository
        .save_snapshot_with_lifecycle_state(path, Some(&state), before_publish)
}


/// Owned snapshot representation of a collection generation.
#[derive(Serialize, Deserialize)]
struct EncodedCollection {
    generation: CollectionGeneration,
    identity_order: Vec<RecordSlot>,
    postings: PostingCollection,
    inference: Option<InferenceState>,
    global_inference: Option<GlobalInferenceSummary>,
    custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    raw: BigramIndex,
    fallback_text: BigramIndex,
    logical_records: RoaringBitmap,
    empty_array_records: RoaringBitmap,
    custom_text: HashMap<(Vec<String>, String), BigramIndex>,
    sorting: SortCollection,
    stats: CollectionStats,
    relations: Vec<RelationIndex>,
}


/// Borrowed collection state prepared for snapshot encoding.
#[derive(Serialize)]
struct EncodedCollectionRef<'a> {
    generation: &'a CollectionGeneration,
    identity_order: &'a Vec<RecordSlot>,
    postings: &'a PostingCollection,
    inference: &'a Option<InferenceState>,
    global_inference: &'a Option<GlobalInferenceSummary>,
    custom_postings: &'a HashMap<(Vec<String>, String), CustomPostingIndex>,
    raw: &'a BigramIndex,
    fallback_text: &'a BigramIndex,
    logical_records: &'a RoaringBitmap,
    empty_array_records: &'a RoaringBitmap,
    custom_text: &'a HashMap<(Vec<String>, String), BigramIndex>,
    sorting: &'a SortCollection,
    stats: CollectionStats,
    relations: &'a Vec<RelationIndex>,
}


fn codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .reject_trailing_bytes()
}

fn path_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect()
    }
}

fn path_from_bytes(bytes: Vec<u8>) -> Result<PathBuf, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(std::ffi::OsString::from_vec(bytes).into())
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        if bytes.len() % 2 != 0 {
            return Err("invalid Windows path byte count".into());
        }
        let wide: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        Ok(std::ffi::OsString::from_wide(&wide).into())
    }
}

fn normalized_absolute_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}

fn resolved_snapshot_destination(path: &Path) -> Result<PathBuf, String> {
    match fs::canonicalize(path) {
        Ok(resolved) => Ok(resolved),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or("snapshot needs a parent directory")?;
            let name = path.file_name().ok_or("snapshot needs a filename")?;
            match fs::canonicalize(if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            }) {
                Ok(resolved_parent) => return Ok(resolved_parent.join(name)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("snapshot destination: {error}")),
            }
            let absolute = normalized_absolute_path(path)?;
            Ok(absolute)
        }
        Err(error) => Err(format!("snapshot destination: {error}")),
    }
}

pub fn snapshot_path(root: &Path, alphabet: BigramAlphabet) -> PathBuf {
    snapshot_path_in(&root.join(".objekte"), alphabet)
}

/// Resolve an alphabet-specific snapshot inside an explicit directory.
pub fn snapshot_path_in(directory: &Path, alphabet: BigramAlphabet) -> PathBuf {
    directory.join(match alphabet {
        BigramAlphabet::Ascii => "search-core-ascii.snapshot",
        BigramAlphabet::Unicode => "search-core-unicode.snapshot",
    })
}

/// Reconcile the complete selected file inventory, including additions and
/// deletions. A mismatch requires a fresh private build before publication.
pub fn sources_match(repo: &Repository) -> Result<bool, String> {
    for state in &repo.collections {
        let mut observed = Vec::new();
        for path in crate::discovery::discover(
            &repo.files.root,
            &state.definition.source_path,
            &state.definition.discovery,
            &state.source,
        )? {
            let relative = path
                .strip_prefix(&repo.files.root)
                .map_err(|e| e.to_string())?
                .to_owned();
            let revision =
                SourceRevision::from_metadata(&fs::metadata(&path).map_err(|e| e.to_string())?)?;
            observed.push((relative, revision));
        }
        observed.sort_by(|a, b| a.0.cmp(&b.0));
        let mut recorded: Vec<_> = state
            .generation
            .files
            .iter()
            .map(|id| {
                let file = &repo.files.files[id.0 as usize];
                (file.relative_path.clone(), file.revision)
            })
            .collect();
        recorded.sort_by(|a, b| a.0.cmp(&b.0));
        if observed != recorded {
            return Ok(false);
        }
    }
    Ok(true)
}

fn feed(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

pub fn configuration_hash(
    definitions: &[CollectionDefinition],
    options: RepositoryOptions,
    sources: &SourceRegistry,
) -> Result<[u8; 32], String> {
    let mut hash = Sha256::new();
    hash.update(crate::MODEL_VERSION.to_le_bytes());
    hash.update(SNAPSHOT_VERSION.to_le_bytes());
    hash.update(BIGRAM_VERSION.to_le_bytes());
    feed(&mut hash, FOLDED_REPRESENTATION.as_bytes());
    feed(&mut hash, std::env::consts::OS.as_bytes());
    feed(&mut hash, COLLATION_VERSION.as_bytes());
    hash.update([match options.bigram_alphabet {
        BigramAlphabet::Ascii => 0,
        BigramAlphabet::Unicode => 1,
    }]);
    feed(&mut hash, format!("{:?}", options.inference).as_bytes());
    for definition in definitions {
        let source = sources.resolve(&definition.format)?;
        feed(&mut hash, definition.name.as_bytes());
        feed(&mut hash, &path_bytes(&definition.source_path));
        feed(&mut hash, format!("{:?}", definition.discovery).as_bytes());
        feed(&mut hash, source.id().as_bytes());
        hash.update(source.version().to_le_bytes());
        feed(&mut hash, source.extension().as_bytes());
        feed(
            &mut hash,
            format!("{:?}/{:?}", definition.format, definition.identity).as_bytes(),
        );
        feed(&mut hash, definition.sort_locale.as_bytes());
        feed(
            &mut hash,
            format!("{:?}", definition.sort_options).as_bytes(),
        );
        let mut overrides: Vec<_> = definition.sort_overrides.iter().collect();
        overrides.sort_by(|a, b| a.0.display().cmp(&b.0.display()));
        for (path, config) in overrides {
            feed(&mut hash, path.display().as_bytes());
            feed(&mut hash, config.locale.as_bytes());
            feed(&mut hash, format!("{:?}", config.options).as_bytes());
        }
        for field in &definition.sort_fields {
            feed(&mut hash, field.display().as_bytes());
        }
        for composite in &definition.schema.composite_sorts {
            feed(&mut hash, format!("{composite:?}").as_bytes());
        }
        let mut fields: Vec<_> = definition.schema.fields.values().collect();
        fields.sort_by(|a, b| a.path.cmp(&b.path));
        for field in fields {
            feed(
                &mut hash,
                format!(
                    "{:?}|{:?}|{}|{}|{}|{}|{:?}",
                    field.path,
                    field.kind,
                    field.required,
                    field.nullable,
                    field.indexed,
                    field.multi,
                    field.element_nullable
                )
                .as_bytes(),
            );
        }
        if let Some(fingerprint) = definition.schema.source_fingerprint {
            hash.update(fingerprint);
        }
        let mut excluded: Vec<_> = definition.schema.auto_index_excluded.iter().collect();
        excluded.sort();
        for path in excluded {
            feed(
                &mut hash,
                format!("auto-index-excluded:{path:?}").as_bytes(),
            );
        }
        for relation in definition.schema.relations().values() {
            feed(&mut hash, format!("{:?}", relation).as_bytes());
        }
        let mut custom: Vec<_> = definition.schema.custom.iter().collect();
        custom.sort_by(|a, b| a.0.cmp(b.0));
        for ((path, name), binding) in custom {
            feed(
                &mut hash,
                format!(
                    "{:?}|{name}|{}|{:?}",
                    path, binding.version, binding.transform
                )
                .as_bytes(),
            );
        }
        let mut operations: Vec<_> = definition.schema.operations.iter().collect();
        operations.sort_by(|a, b| a.0.cmp(b.0));
        for ((path, name), binding) in operations {
            feed(
                &mut hash,
                format!(
                    "{:?}|{name}|{}|{}|{}|{}",
                    path,
                    binding.version,
                    binding.operation.posting(),
                    binding.operation.bigram(),
                    binding.operation.complete_coverage()
                )
                .as_bytes(),
            );
        }
        let mut defaults: Vec<_> = definition.schema.defaults.iter().collect();
        defaults.sort_by(|a, b| a.0.cmp(b.0));
        for (path, name) in defaults {
            feed(&mut hash, format!("{:?}|{name}", path).as_bytes());
        }
        if let Some(raw) = &definition.schema.raw {
            feed(
                &mut hash,
                format!(
                    "raw|{}|{}|{}",
                    raw.name,
                    raw.version,
                    raw.operation.bigram()
                )
                .as_bytes(),
            );
        }
    }
    Ok(hash.finalize().into())
}

#[cfg(test)]
fn write_section(file: &mut impl Write, id: u32, value: &impl Serialize) -> Result<(), String> {
    let mut section = std::io::Cursor::new(Vec::new());
    write_section_measured(&mut section, id, value)?;
    file.write_all(section.get_ref()).map_err(|e| e.to_string())
}


struct SectionWriter<'a, W: Write> {
    inner: &'a mut W,
    checksum: Sha256,
    len: u64,
    checksum_ns: u64,
    write_ns: u64,
}


impl<W: Write> Write for SectionWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self
            .len
            .checked_add(bytes.len() as u64)
            .is_none_or(|len| len > MAX_SECTION_BYTES)
        {
            return Err(std::io::Error::other("snapshot section too large"));
        }
        let started = Instant::now();
        let written = self.inner.write(bytes)?;
        self.write_ns = self
            .write_ns
            .saturating_add(started.elapsed().as_nanos() as u64);
        let started = Instant::now();
        self.checksum.update(&bytes[..written]);
        self.checksum_ns = self
            .checksum_ns
            .saturating_add(started.elapsed().as_nanos() as u64);
        self.len += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let started = Instant::now();
        self.inner.flush()?;
        self.write_ns = self
            .write_ns
            .saturating_add(started.elapsed().as_nanos() as u64);
        Ok(())
    }
}


fn write_section_measured(
    file: &mut (impl Write + Seek),
    id: u32,
    value: &impl Serialize,
) -> Result<SnapshotSectionSaveStats, String> {
    let started = Instant::now();
    let header_at = file.stream_position().map_err(|e| e.to_string())?;
    file.write_all(&[0; SECTION_HEADER_BYTES])
        .map_err(|e| e.to_string())?;
    let header_write_ns = started.elapsed().as_nanos() as u64;
    let mut writer = SectionWriter {
        inner: file,
        checksum: Sha256::new(),
        len: 0,
        checksum_ns: 0,
        write_ns: 0,
    };
    {
        // Buffer small bincode writes while hashing and bounding the payload.
        let mut buffered = BufWriter::with_capacity(64 * 1024, &mut writer);
        codec()
            .serialize_into(&mut buffered, value)
            .map_err(|e| e.to_string())?;
        buffered.flush().map_err(|e| e.to_string())?;
    }
    let serialize_total_ns = started.elapsed().as_nanos() as u64;
    let checksum_ns = writer.checksum_ns;
    let mut write_ns = writer.write_ns.saturating_add(header_write_ns);
    let payload_bytes = writer.len;
    let checksum = writer.checksum.finalize();
    let file = writer.inner;
    let end_at = file.stream_position().map_err(|e| e.to_string())?;
    let started = Instant::now();
    file.seek(SeekFrom::Start(header_at))
        .map_err(|e| e.to_string())?;
    file.write_all(&id.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&payload_bytes.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&checksum).map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(end_at))
        .map_err(|e| e.to_string())?;
    write_ns = write_ns.saturating_add(started.elapsed().as_nanos() as u64);
    Ok(SnapshotSectionSaveStats {
        id,
        serialize_ns: serialize_total_ns.saturating_sub(checksum_ns.saturating_add(write_ns)),
        checksum_ns,
        write_ns,
        payload_bytes,
    })
}

fn read_section<T: for<'de> Deserialize<'de>>(
    file: &mut (impl Read + Seek),
    expected_id: u32,
) -> Result<T, SnapshotLoadError> {
    let mut id = [0; 4];
    let mut size = [0; 8];
    let mut checksum = [0; 32];
    file.read_exact(&mut id)
        .map_err(|e| snapshot_read_error("snapshot section ID", e))?;
    file.read_exact(&mut size)
        .map_err(|e| snapshot_read_error("snapshot section size", e))?;
    file.read_exact(&mut checksum)
        .map_err(|e| snapshot_read_error("snapshot checksum", e))?;
    if u32::from_le_bytes(id) != expected_id {
        return Err("snapshot section order mismatch".into());
    }
    let len = u64::from_le_bytes(size);
    if len > MAX_SECTION_BYTES {
        return Err("snapshot section too large".into());
    }
    let payload_at = file
        .stream_position()
        .map_err(|e| snapshot_read_error("snapshot section position", e))?;
    let mut hash = Sha256::new();
    let mut remaining = len;
    let mut buffer = [0; 64 * 1024];
    while remaining > 0 {
        let chunk = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..chunk])
            .map_err(|e| snapshot_read_error("snapshot section", e))?;
        hash.update(&buffer[..chunk]);
        remaining -= chunk as u64;
    }
    if hash.finalize().as_slice() != checksum {
        return Err("snapshot checksum mismatch".into());
    }
    file.seek(SeekFrom::Start(payload_at))
        .map_err(|e| snapshot_read_error("snapshot section position", e))?;
    let mut payload = BufReader::with_capacity(64 * 1024, file.take(len));
    let decoded = codec()
        .with_limit(len)
        .deserialize_from(&mut payload)
        .map_err(|e| SnapshotLoadError::Invalid(format!("invalid snapshot section: {e}")))?;
    if !payload.buffer().is_empty() || payload.get_ref().limit() != 0 {
        return Err("snapshot section has trailing bytes".into());
    }
    Ok(decoded)
}


impl Repository {
    pub fn save_snapshot(&self, path: &Path) -> Result<(), String> {
        self.save_snapshot_with_stats(path).map(|_| ())
    }

    /// Explicitly save and return section-level timing and byte diagnostics.
    pub fn save_snapshot_with_stats(&self, path: &Path) -> Result<SnapshotSaveStats, String> {
        self.save_snapshot_before_publish(path, || Ok(()))
    }

    fn save_snapshot_before_publish(
        &self,
        path: &Path,
        before_publish: impl FnOnce() -> Result<(), String>,
    ) -> Result<SnapshotSaveStats, String> {
        self.save_snapshot_with_lifecycle_state(path, None, before_publish)
    }

    fn save_snapshot_with_lifecycle_state(
        &self,
        path: &Path,
        lifecycle: Option<&LifecycleCacheState>,
        before_publish: impl FnOnce() -> Result<(), String>,
    ) -> Result<SnapshotSaveStats, String> {
        let total_started = Instant::now();
        let mut stats = SnapshotSaveStats::default();
        self.ensure_snapshot_destination_is_not_source(path)?;
        let parent = path.parent().ok_or("snapshot needs a parent directory")?;
        let started = Instant::now();
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        stats.directory_ns = started.elapsed().as_nanos() as u64;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let temporary = parent.join(format!(
            ".objekte-snapshot-{}-{nonce}.tmp",
            std::process::id()
        ));
        let outcome: Result<(), String> = (|| {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)
                .map_err(|e| e.to_string())?;
            let started = Instant::now();
            let definitions = self
                .collections
                .iter()
                .map(|c| c.definition.clone())
                .collect::<Vec<_>>();
            let configuration = configuration_hash(&definitions, self.options, &self.sources)?;
            stats.header_ns = started.elapsed().as_nanos() as u64;
            let started = Instant::now();
            file.write_all(MAGIC).map_err(|e| e.to_string())?;
            file.write_all(
                &lifecycle
                    .map_or(SNAPSHOT_VERSION, |_| LIFECYCLE_SNAPSHOT_VERSION)
                    .to_le_bytes(),
            )
            .map_err(|e| e.to_string())?;
            file.write_all(&crate::MODEL_VERSION.to_le_bytes())
                .map_err(|e| e.to_string())?;
            file.write_all(&configuration).map_err(|e| e.to_string())?;
            file.write_all(&(self.collections.len() as u32).to_le_bytes())
                .map_err(|e| e.to_string())?;
            stats.write_ns = started.elapsed().as_nanos() as u64;
            stats.bytes_written = 52;
            let started = Instant::now();
            let files: Vec<_> = self
                .files
                .files
                .iter()
                .map(|entry| EncodedFile {
                    id: entry.id,
                    relative_path: path_bytes(&entry.relative_path),
                    revision: entry.revision,
                    created_ns: entry.created_ns,
                    collections: entry.collections.clone(),
                    status: entry.status,
                    diagnostics: entry.diagnostics.clone(),
                    omitted_diagnostics: entry.omitted_diagnostics,
                    fingerprint: self.base_fingerprints[entry.id.0 as usize],
                })
                .collect();
            stats.inventory_ns = started.elapsed().as_nanos() as u64;
            let section = write_section_measured(&mut file, 0, &files)?;
            stats.write_ns = stats.write_ns.saturating_add(section.write_ns);
            stats.bytes_written = stats
                .bytes_written
                .saturating_add(section.payload_bytes + 44);
            stats.sections.push(section);
            for (index, state) in self.collections.iter().enumerate() {
                let encoded = EncodedCollectionRef {
                    generation: &state.generation,
                    identity_order: &state.identity_order,
                    postings: &state.postings,
                    inference: &state.inference,
                    global_inference: &state.global_inference,
                    custom_postings: &state.custom_postings,
                    raw: &state.raw,
                    fallback_text: &state.fallback_text,
                    logical_records: &state.logical_records,
                    empty_array_records: &state.empty_array_records,
                    custom_text: &state.custom_text,
                    sorting: &state.sorting,
                    stats: state.stats,
                    relations: &state.relations,
                };
                let section = write_section_measured(&mut file, index as u32 + 1, &encoded)?;
                stats.write_ns = stats.write_ns.saturating_add(section.write_ns);
                stats.bytes_written = stats
                    .bytes_written
                    .saturating_add(section.payload_bytes + 44);
                stats.sections.push(section);
            }
            if let Some(lifecycle) = lifecycle {
                let section = write_section_measured(
                    &mut file,
                    self.collections.len() as u32 + 1,
                    lifecycle,
                )?;
                stats.write_ns = stats.write_ns.saturating_add(section.write_ns);
                stats.bytes_written = stats
                    .bytes_written
                    .saturating_add(section.payload_bytes + 44);
                stats.sections.push(section);
            }
            let started = Instant::now();
            file.sync_all().map_err(|e| e.to_string())?;
            stats.sync_ns = started.elapsed().as_nanos() as u64;
            before_publish()?;
            self.ensure_snapshot_destination_is_not_source(path)?;
            let started = Instant::now();
            fs::rename(&temporary, path).map_err(|e| e.to_string())?;
            stats.rename_ns = started.elapsed().as_nanos() as u64;
            #[cfg(target_os = "linux")]
            {
                let started = Instant::now();
                File::open(parent)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|e| format!("snapshot directory sync: {e}"))?;
                stats.directory_sync_ns = started.elapsed().as_nanos() as u64;
            }
            Ok(())
        })();
        if outcome.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        outcome?;
        stats.total_ns = total_started.elapsed().as_nanos() as u64;
        Ok(stats)
    }

    /// Returns the number of source paths inspected individually. Ordinary
    /// destinations need only the inventory lookup and a few directory checks.
    fn ensure_snapshot_destination_is_not_source(&self, path: &Path) -> Result<usize, String> {
        let destination = resolved_snapshot_destination(path)?;
        if let Ok(relative) = destination.strip_prefix(&self.files.root) {
            if self.files.id_for(relative).is_some() {
                return Err(format!(
                    "snapshot destination aliases indexed source file: {}",
                    destination.display()
                ));
            }
        }

        #[cfg(unix)]
        let destination_metadata = fs::metadata(path).ok();
        #[cfg(unix)]
        let multiple_links = destination_metadata.as_ref().is_some_and(|metadata| {
            use std::os::unix::fs::MetadataExt;
            metadata.nlink() > 1
        });
        #[cfg(not(unix))]
        let multiple_links = false;

        let source_symlinks_possible = self.collections.iter().any(|collection| {
            if collection.definition.discovery.symlinks == SymlinkPolicy::FollowWithinRoot {
                return true;
            }
            let directory = self.files.root.join(&collection.definition.source_path);
            match fs::canonicalize(&directory) {
                Ok(resolved) => resolved != directory,
                Err(_) => true,
            }
        });
        if !multiple_links && !source_symlinks_possible {
            return Ok(0);
        }

        let mut scanned = 0;
        for entry in &self.files.files {
            scanned += 1;
            let source = self.files.root.join(&entry.relative_path);
            let resolved_source = match fs::canonicalize(&source) {
                Ok(resolved) => resolved,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if let Ok(target) = fs::read_link(&source) {
                        let parent = source.parent().ok_or("indexed source needs a parent")?;
                        resolved_snapshot_destination(&parent.join(target))?
                    } else {
                        resolved_snapshot_destination(&source)?
                    }
                }
                Err(error) => return Err(format!("indexed source path: {error}")),
            };
            if destination == resolved_source {
                return Err(format!(
                    "snapshot destination aliases indexed source file: {}",
                    source.display()
                ));
            }

            #[cfg(unix)]
            if multiple_links {
                if let (Some(destination_metadata), Ok(source_metadata)) =
                    (&destination_metadata, fs::metadata(&source))
                {
                    use std::os::unix::fs::MetadataExt;
                    if destination_metadata.dev() == source_metadata.dev()
                        && destination_metadata.ino() == source_metadata.ino()
                    {
                        return Err(format!(
                            "snapshot destination aliases indexed source file: {}",
                            source.display()
                        ));
                    }
                }
            }
        }
        Ok(scanned)
    }
}


#[cfg(test)]
impl Repository {
    pub(crate) fn save_default_snapshot_for_test(&self) {
        self.save_snapshot(&snapshot_path(
            &self.files.root,
            self.options.bigram_alphabet,
        ))
        .unwrap();
    }
}


pub fn load_snapshot(
    root: PathBuf,
    definitions: Vec<CollectionDefinition>,
    options: RepositoryOptions,
    sources: &SourceRegistry,
    path: &Path,
) -> Result<Repository, String> {
    load_snapshot_checked(root, definitions, options, sources, path)
        .map_err(|error| error.to_string())
}

pub(crate) fn load_snapshot_checked(
    root: PathBuf,
    definitions: Vec<CollectionDefinition>,
    options: RepositoryOptions,
    sources: &SourceRegistry,
    path: &Path,
) -> Result<Repository, SnapshotLoadError> {
    load_snapshot_parts(root, definitions, options, sources, path, false)
        .map(|(repository, _)| repository)
}

/// Restore the validated base and lifecycle section without enumerating
/// source files. The callback must decode and validate each private segment
/// representation before this function returns to the publication owner.
pub(crate) fn load_lifecycle_snapshot_checked(
    root: PathBuf,
    definitions: Vec<CollectionDefinition>,
    options: RepositoryOptions,
    sources: &SourceRegistry,
    path: &Path,
    mut validate_segment: impl FnMut(&LifecycleSegmentCache) -> Result<(), String>,
) -> Result<(Repository, LifecycleCacheState), SnapshotLoadError> {
    let (repository, state) = load_snapshot_parts(root, definitions, options, sources, path, true)?;
    let state = state.ok_or("lifecycle snapshot section missing")?;
    state.validate(&repository, |segment| validate_segment(segment))?;
    Ok((repository, state))
}

fn load_snapshot_parts(
    root: PathBuf,
    definitions: Vec<CollectionDefinition>,
    options: RepositoryOptions,
    sources: &SourceRegistry,
    path: &Path,
    lifecycle: bool,
) -> Result<(Repository, Option<LifecycleCacheState>), SnapshotLoadError> {
    crate::relation::validate_definitions(&definitions)?;
    let mut file = File::open(path).map_err(|e| {
        SnapshotLoadError::Access(format!("snapshot access: {}: {e}", path.display()))
    })?;
    let mut magic = [0; 8];
    let mut version = [0; 4];
    let mut model = [0; 4];
    let mut hash = [0; 32];
    let mut count = [0; 4];
    file.read_exact(&mut magic)
        .map_err(|e| snapshot_read_error("snapshot magic", e))?;
    file.read_exact(&mut version)
        .map_err(|e| snapshot_read_error("snapshot version", e))?;
    file.read_exact(&mut model)
        .map_err(|e| snapshot_read_error("snapshot model", e))?;
    file.read_exact(&mut hash)
        .map_err(|e| snapshot_read_error("snapshot configuration", e))?;
    file.read_exact(&mut count)
        .map_err(|e| snapshot_read_error("snapshot collection count", e))?;
    if &magic != MAGIC
        || u32::from_le_bytes(version)
            != if lifecycle {
                LIFECYCLE_SNAPSHOT_VERSION
            } else {
                SNAPSHOT_VERSION
            }
        || u32::from_le_bytes(model) != crate::MODEL_VERSION
    {
        return Err("incompatible snapshot version".into());
    }
    if hash != configuration_hash(&definitions, options, sources)? {
        return Err("incompatible snapshot configuration".into());
    }
    if u32::from_le_bytes(count) as usize != definitions.len() {
        return Err("snapshot collection count mismatch".into());
    }
    let encoded_files: Vec<EncodedFile> = read_section(&mut file, 0)?;
    let base_fingerprints = encoded_files
        .iter()
        .map(|file| file.fingerprint)
        .collect::<Vec<_>>();
    let mut files = FileInventory::new(root);
    for (index, entry) in encoded_files.into_iter().enumerate() {
        if entry.id.0 as usize != index || entry.collections.is_empty() {
            return Err("invalid snapshot file identity".into());
        }
        let path = path_from_bytes(entry.relative_path)?;
        for collection in &entry.collections {
            if collection.0 as usize >= definitions.len() {
                return Err("snapshot file collection out of bounds".into());
            }
            let id = files.add(path.clone(), entry.revision, entry.created_ns, *collection)?;
            if id != entry.id {
                return Err("snapshot file ID mismatch".into());
            }
        }
        let restored = &mut files.files[entry.id.0 as usize];
        restored.status = entry.status;
        restored.diagnostics = entry.diagnostics;
        restored.omitted_diagnostics = entry.omitted_diagnostics;
    }
    files.rebuild_lookup()?;
    let mut collections = Vec::new();
    for (index, definition) in definitions.into_iter().enumerate() {
        let source = sources.resolve(&definition.format)?;
        let encoded: EncodedCollection = read_section(&mut file, index as u32 + 1)?;
        let generation = encoded.generation;
        if generation.collection.0 as usize != index || generation.name != definition.name {
            return Err("snapshot collection identity mismatch".into());
        }
        generation.validate(&files)?;
        for record in &generation.records {
            if record.class == crate::inventory::RecordClass::RawFallback {
                let file_entry = &files.files[record.file.0 as usize];
                let expected_id = RecordId::fallback(
                    generation.collection,
                    source.id(),
                    source.version(),
                    &file_entry.relative_path,
                    record.file,
                    record.revision.modified_ns,
                    record.range.start,
                    record.range.end,
                );
                if record.id != expected_id {
                    return Err("snapshot fallback identity mismatch".into());
                }
                if let Some(diagnostic) = files.diagnostic(record.file, record.diagnostic_ref) {
                    if diagnostic.source_id != source.id()
                        || diagnostic.source_version != source.version()
                    {
                        return Err("snapshot fallback diagnostic registration mismatch".into());
                    }
                }
            }
        }
        encoded.fallback_text.validate(generation.records.len())?;
        let logical_count = generation
            .records
            .iter()
            .filter(|record| record.class == crate::inventory::RecordClass::Logical)
            .count();
        let all_logical = logical_count == generation.records.len();
        if all_logical {
            // A cardinality equal to the validated slot capacity, with no
            // out-of-bound maximum, is exactly the full dense slot domain.
            if encoded.logical_records.len() != logical_count as u64
                || encoded
                    .logical_records
                    .max()
                    .is_some_and(|slot| slot as usize >= logical_count)
            {
                return Err("snapshot logical eligibility mismatch".into());
            }
        } else {
            let actual_logical: RoaringBitmap = generation
                .records
                .iter()
                .filter(|record| record.class == crate::inventory::RecordClass::Logical)
                .map(|record| record.slot.0)
                .collect();
            if encoded.logical_records != actual_logical {
                return Err("snapshot logical eligibility mismatch".into());
            }
        }
        if !all_logical {
            encoded.postings.validate_domain(&encoded.logical_records)?;
            for posting in encoded.custom_postings.values() {
                posting.index.validate_domain(&encoded.logical_records)?;
            }
            encoded.raw.validate_domain(&encoded.logical_records)?;
            for index in encoded.custom_text.values() {
                index.validate_domain(&encoded.logical_records)?;
            }
        }
        if !(&encoded.empty_array_records - &encoded.logical_records).is_empty() {
            return Err("snapshot empty-array structural domain mismatch".into());
        }
        if all_logical {
            encoded
                .fallback_text
                .validate_domain(&RoaringBitmap::new())?;
        } else if logical_count != 0 {
            let fallback_slots: RoaringBitmap = generation
                .records
                .iter()
                .filter(|record| record.class == crate::inventory::RecordClass::RawFallback)
                .map(|record| record.slot.0)
                .collect();
            encoded.fallback_text.validate_domain(&fallback_slots)?;
        }
        encoded
            .postings
            .validate_eligible(generation.records.len(), logical_count)?;
        let expected_custom: HashSet<_> = definition
            .schema
            .operations
            .iter()
            .filter(|(_, binding)| binding.operation.posting())
            .map(|(key, _)| key.clone())
            .collect();
        if encoded
            .custom_postings
            .keys()
            .cloned()
            .collect::<HashSet<_>>()
            != expected_custom
        {
            return Err("snapshot custom posting coverage mismatch".into());
        }
        for (key, custom) in &encoded.custom_postings {
            custom
                .index
                .validate_eligible(generation.records.len(), logical_count)?;
            if custom.index.kind != definition.schema.fields[&key.0].kind
                || custom.index.path != key.0
            {
                return Err("snapshot custom posting binding mismatch".into());
            }
            if custom.complete
                && !definition.schema.operations[key]
                    .operation
                    .complete_coverage()
            {
                return Err("snapshot custom posting coverage mismatch".into());
            }
        }
        let expected_postings: HashSet<_> = definition
            .schema
            .fields
            .values()
            .filter(|f| f.indexed)
            .map(|f| f.path.clone())
            .collect();
        if encoded
            .postings
            .fields
            .keys()
            .cloned()
            .collect::<HashSet<_>>()
            != expected_postings
        {
            return Err("snapshot posting coverage mismatch".into());
        }
        for (path, posting) in &encoded.postings.fields {
            let field = &definition.schema.fields[path];
            if posting.kind != field.kind || posting.multi != field.multi || posting.path != *path {
                return Err("snapshot posting binding mismatch".into());
            }
        }
        match (&encoded.inference, options.inference.mode) {
            (None, InferenceMode::Disabled) => {}
            (Some(inference), InferenceMode::ReportOnly | InferenceMode::AutoIndex) => {
                inference.validate(
                    options.inference,
                    &encoded.logical_records,
                    generation.records.len(),
                    &definition.schema,
                )?;
            }
            _ => return Err("snapshot inference mode mismatch".into()),
        }
        encoded.raw.validate(generation.records.len())?;
        if encoded.raw.alphabet != options.bigram_alphabet {
            return Err("snapshot alphabet mismatch".into());
        }
        let expected_text: HashSet<_> = definition
            .schema
            .custom
            .keys()
            .chain(
                definition
                    .schema
                    .operations
                    .iter()
                    .filter(|(_, binding)| {
                        binding.operation.bigram() && binding.operation.complete_coverage()
                    })
                    .map(|(key, _)| key),
            )
            .map(|key| {
                crate::repository::canonical_custom_text_key(&definition.schema, &key.0, &key.1)
            })
            .collect();
        if encoded.custom_text.keys().cloned().collect::<HashSet<_>>() != expected_text {
            return Err("snapshot custom binding mismatch".into());
        }
        for index in encoded.custom_text.values() {
            index.validate(generation.records.len())?;
            if index.alphabet != options.bigram_alphabet {
                return Err("snapshot custom alphabet mismatch".into());
            }
        }
        if encoded.sorting.columns.len() != definition.sort_fields.len() {
            return Err("snapshot sort coverage mismatch".into());
        }
        for path in &definition.sort_fields {
            let column = encoded
                .sorting
                .columns
                .get(path)
                .ok_or("snapshot sort path missing")?;
            if column.path != *path
                || column.kind != definition.schema.field(path)?.kind
                || column.locale
                    != definition
                        .sort_overrides
                        .get(path)
                        .map_or(definition.sort_locale.as_str(), |config| {
                            config.locale.as_str()
                        })
                || column.options
                    != definition
                        .sort_overrides
                        .get(path)
                        .map_or(definition.sort_options, |config| config.options)
                || column.collation_version != COLLATION_VERSION
                || column.keys.len() != generation.records.len()
                || column.ascending.len() != generation.records.len()
                || column.descending.len() != generation.records.len()
            {
                return Err("snapshot sort binding mismatch".into());
            }
            for (ordered, descending) in [(&column.ascending, false), (&column.descending, true)] {
                let mut seen = vec![false; generation.records.len()];
                for slot in ordered {
                    let i = slot.0 as usize;
                    if i >= seen.len() || seen[i] {
                        return Err("snapshot sort permutation mismatch".into());
                    }
                    seen[i] = true;
                }
                if ordered.windows(2).any(|pair| {
                    let a = pair[0].0 as usize;
                    let b = pair[1].0 as usize;
                    column.keys[a]
                        .compare_with(&column.keys[b], descending, column.options)
                        .then_with(|| generation.records[a].id.cmp(&generation.records[b].id))
                        .is_gt()
                }) {
                    return Err("snapshot sort order mismatch".into());
                }
            }
        }
        if encoded.sorting.composites.len() != definition.schema.composite_sorts.len() {
            return Err("snapshot composite sort coverage mismatch".into());
        }
        for (index, expected) in definition.schema.composite_sorts.iter().enumerate() {
            let composite = &encoded.sorting.composites[index];
            let normalized = expected
                .iter()
                .map(crate::sorting::simple_composite_part)
                .collect::<Option<Vec<_>>>()
                .ok_or("invalid composite sort definition")?;
            if composite.fields != normalized || composite.order.len() != generation.records.len() {
                return Err("snapshot composite sort definition mismatch".into());
            }
            let columns = normalized
                .iter()
                .map(|field| {
                    encoded
                        .sorting
                        .columns
                        .get(&field.path)
                        .map(|column| (column, field.descending))
                        .ok_or("snapshot composite sort column missing")
                })
                .collect::<Result<Vec<_>, &str>>()?;
            let mut seen = vec![false; generation.records.len()];
            for slot in &composite.order {
                let i = slot.0 as usize;
                if i >= seen.len() || seen[i] {
                    return Err("snapshot composite sort permutation mismatch".into());
                }
                seen[i] = true;
            }
            if composite.order.windows(2).any(|pair| {
                crate::sorting::compare_composite(&columns, pair[0], pair[1])
                    .then_with(|| {
                        generation.records[pair[0].0 as usize]
                            .id
                            .cmp(&generation.records[pair[1].0 as usize].id)
                    })
                    .is_gt()
            }) {
                return Err("snapshot composite sort order mismatch".into());
            }
        }
        let id_lookup: HashMap<RecordId, RecordSlot> = generation
            .records
            .iter()
            .map(|r| (r.id.clone(), r.slot))
            .collect();
        if id_lookup.len() != generation.records.len() {
            return Err("snapshot duplicate record ID".into());
        }
        if encoded.identity_order.len() != generation.records.len() {
            return Err("snapshot identity order coverage mismatch".into());
        }
        let mut seen_identity = vec![false; generation.records.len()];
        for slot in &encoded.identity_order {
            let i = slot.0 as usize;
            if i >= seen_identity.len() || seen_identity[i] {
                return Err("snapshot identity order permutation mismatch".into());
            }
            seen_identity[i] = true;
        }
        if encoded.identity_order.windows(2).any(|pair| {
            generation.records[pair[0].0 as usize].id > generation.records[pair[1].0 as usize].id
        }) {
            return Err("snapshot identity order mismatch".into());
        }
        let stats = CollectionStats {
            file_count: generation.files.len(),
            record_count: generation.records.len(),
            total_file_bytes: generation
                .files
                .iter()
                .map(|file| files.files[file.0 as usize].revision.size)
                .sum(),
        };
        if stats.file_count != encoded.stats.file_count
            || stats.record_count != encoded.stats.record_count
            || stats.total_file_bytes != encoded.stats.total_file_bytes
        {
            return Err("snapshot statistics mismatch".into());
        }
        collections.push(CollectionState {
            definition,
            source,
            generation,
            postings: encoded.postings,
            inference: encoded.inference,
            global_inference: encoded.global_inference,
            custom_postings: encoded.custom_postings,
            raw: encoded.raw,
            fallback_text: encoded.fallback_text,
            logical_records: encoded.logical_records,
            empty_array_records: encoded.empty_array_records,
            custom_text: encoded.custom_text,
            sorting: encoded.sorting,
            id_lookup,
            identity_order: encoded.identity_order,
            stats,
            relations: encoded.relations,
        });
    }
    crate::relation::validate_indexes(&collections)?;
    for file in &files.files {
        for diagnostic in &file.diagnostics {
            if let Some(slot) = diagnostic.record {
                let record = collections
                    .get(diagnostic.collection.0 as usize)
                    .and_then(|collection| collection.generation.records.get(slot.0 as usize))
                    .ok_or("snapshot diagnostic record out of bounds")?;
                if record.file != file.id
                    || record.class != crate::inventory::RecordClass::RawFallback
                {
                    return Err("snapshot diagnostic record ownership mismatch".into());
                }
            }
        }
    }
    let state = if lifecycle {
        Some(read_section(&mut file, collections.len() as u32 + 1)?)
    } else {
        None
    };
    let mut trailing = [0; 1];
    if file
        .read(&mut trailing)
        .map_err(|e| snapshot_read_error("snapshot trailer", e))?
        != 0
    {
        return Err("snapshot trailing bytes".into());
    }
    Ok((
        Repository {
            files,
            base_fingerprints,
            collections,
            sources: sources.clone(),
            build_parses: 0,
            build_failed_parses: 0,
            instance_id: crate::repository::next_repository_instance(),
            options,
            startup: StartupStats::default(),
            workers: Repository::create_workers(options.query_workers)?,
            prepared: Mutex::new(crate::prepared::PreparedCache::new(
                options.prepared_cache_bytes,
            )),
            verified: Mutex::new(crate::verified::VerifiedCache::new(
                options.verified_cache_bytes,
            )),
            inferred_orders: Mutex::new(crate::sorting::InferredOrderCache::new(
                crate::sorting::INFERRED_ORDER_CACHE_BYTES,
            )),
        },
        state,
    ))
}

#[cfg(test)]
#[path = "storage.test.rs"]
mod tests;
