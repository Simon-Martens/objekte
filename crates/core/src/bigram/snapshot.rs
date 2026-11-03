//! A portable filter snapshot with an explicit slot-to-file manifest.

use super::{BigramEncoding, BigramFilter};
mod compact;
use crate::{Error, Result, error::io, repository::validate_id, source::Source};
use bincode::Options;
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{SeqAccess, Visitor},
};
use serde::{Serializer, ser::SerializeSeq};
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    collections::{BTreeMap, HashSet},
    fmt, fs,
    io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
};

const MAGIC_JSON: &[u8; 8] = b"OBJSNP01";
const MAGIC_BINARY: &[u8; 8] = b"OBJSNP02";
const MAGIC_ENCODING: &[u8; 8] = b"OBJSNP03";
const MAGIC_SCALAR_POSTINGS: &[u8; 8] = b"OBJSNP04";
const MAGIC_COMPACT: &[u8; 8] = b"OBJSNP05";
const HEADER_LEN: usize = 8 + 8 + 8 + 32;
const MAX_SNAPSHOT_BYTES: u64 = 2 * 1024 * 1024 * 1024;


#[cfg(feature = "index-timings")]
#[derive(Default)]
pub(crate) struct SnapshotLoadTimings {
    pub base_checksum_nanos: u64,
    pub base_decode_checksum_nanos: u64,
    pub manifest_decode_nanos: u64,
    pub base_filter_decode_nanos: u64,
    pub manifest_validation_nanos: u64,
    pub bytes_read: u64,
    pub posting_columns: usize,
}


/// Hash and cap the payload while it is written to a temporary file.
struct DigestWriter<'a> {
    file: &'a mut fs::File,
    digest: Sha256,
    bytes: u64,
}


impl Write for DigestWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if self.bytes.saturating_add(buffer.len() as u64) > MAX_SNAPSHOT_BYTES - HEADER_LEN as u64 {
            return Err(std::io::Error::other("snapshot exceeds size limit"));
        }
        let written = self.file.write(buffer)?;
        self.digest.update(&buffer[..written]);
        self.bytes += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}


/// Hash the second decode pass too, so a file changed after verification fails.
struct DigestReader<R> {
    inner: R,
    digest: Sha256,
    #[cfg(feature = "index-timings")]
    hash_nanos: u64,
}


impl<R: Read> Read for DigestReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        #[cfg(feature = "index-timings")]
        let started = std::time::Instant::now();
        self.digest.update(&buffer[..read]);
        #[cfg(feature = "index-timings")]
        {
            self.hash_nanos += started.elapsed().as_nanos() as u64;
        }
        Ok(read)
    }
}


fn invalid(message: &str) -> Error {
    Error::Cache(format!("invalid bigram snapshot: {message}"))
}


/// The file occupying one numeric bigram slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BigramFile {
    pub id: String,
    pub filename: String,
    pub size_bytes: u64,
    /// Signed Unix nanoseconds, stored as decimal text for exactness.
    pub modified_unix_nanos: String,
    /// Device, inode, and change time where the platform provides them.
    pub identity: String,
}


#[derive(Serialize, Deserialize)]
struct Manifest {
    source_root: PathBuf,
    recursive: bool,
    git_commit: Option<String>,
    files: Vec<BigramFile>,
    #[serde(default)]
    changed_slots: Vec<usize>,
    #[serde(default)]
    changed_bigrams: BTreeMap<usize, Vec<u16>>,
    #[serde(default)]
    bigram_encoding: BigramEncoding,
}


/// Borrowed source metadata for one stable filter slot.
pub(crate) struct BigramFileRef<'a> {
    pub id: &'a str,
    pub size_bytes: u64,
    pub modified_unix_nanos: &'a str,
    pub identity: &'a str,
    pub filename_valid: bool,
}


pub(crate) trait SnapshotFiles {
    fn len(&self) -> usize;
    fn entry(&self, slot: usize) -> BigramFileRef<'_>;

    fn id_arena(&self) -> Result<Cow<'_, [u8]>> {
        let length = (0..self.len()).try_fold(0usize, |sum, slot| {
            sum.checked_add(self.entry(slot).id.len())
                .ok_or_else(|| invalid("ID arena length overflow"))
        })?;
        if length > u32::MAX as usize || length as u64 > MAX_SNAPSHOT_BYTES {
            return Err(invalid("ID arena too large"));
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| invalid("cannot allocate ID arena"))?;
        for slot in 0..self.len() {
            bytes.extend_from_slice(self.entry(slot).id.as_bytes());
        }
        Ok(Cow::Owned(bytes))
    }
}


impl SnapshotFiles for [BigramFile] {
    fn len(&self) -> usize {
        <[BigramFile]>::len(self)
    }

    fn entry(&self, slot: usize) -> BigramFileRef<'_> {
        let file = &self[slot];
        BigramFileRef {
            id: &file.id,
            size_bytes: file.size_bytes,
            modified_unix_nanos: &file.modified_unix_nanos,
            identity: &file.identity,
            filename_valid: file.filename.strip_suffix(".json") == Some(file.id.as_str()),
        }
    }
}


/// V2 stores only data that cannot be reconstructed from the record ID.
struct CompactFilesRef<'a, F: SnapshotFiles + ?Sized>(&'a F);


impl<F: SnapshotFiles + ?Sized> Serialize for CompactFilesRef<'_, F> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut files = serializer.serialize_seq(Some(self.0.len()))?;
        for slot in 0..self.0.len() {
            let file = self.0.entry(slot);
            files.serialize_element(&(
                file.id,
                file.size_bytes,
                file.modified_unix_nanos,
                file.identity,
            ))?;
        }
        files.end()
    }
}


struct CompactFiles(Vec<BigramFile>);


impl<'de> Deserialize<'de> for CompactFiles {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct FilesVisitor;


        impl<'de> Visitor<'de> for FilesVisitor {
            type Value = CompactFiles;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a sequence of compact file records")
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                // The length comes from cache bytes. Limit speculative
                // allocation even when their checksum is internally valid.
                let mut files = Vec::with_capacity(sequence.size_hint().unwrap_or(0).min(65_536));
                while let Some((id, size_bytes, modified_unix_nanos, identity)) =
                    sequence.next_element::<(String, u64, String, String)>()?
                {
                    let filename = format!("{id}.json");
                    files.push(BigramFile {
                        id,
                        filename,
                        size_bytes,
                        modified_unix_nanos,
                        identity,
                    });
                }
                Ok(CompactFiles(files))
            }
        }


        deserializer.deserialize_seq(FilesVisitor)
    }
}


#[derive(Serialize)]
#[serde(bound(serialize = ""))]
struct ManifestBinaryRef<'a, F: SnapshotFiles + ?Sized> {
    source_root: &'a PathBuf,
    recursive: bool,
    git_commit: &'a Option<String>,
    files: CompactFilesRef<'a, F>,
    changed_slots: &'a [usize],
    changed_bigrams: &'a BTreeMap<usize, Vec<u16>>,
    bigram_encoding: &'static str,
}


#[derive(Deserialize)]
struct ManifestBinary {
    source_root: PathBuf,
    recursive: bool,
    git_commit: Option<String>,
    files: CompactFiles,
    changed_slots: Vec<usize>,
    changed_bigrams: BTreeMap<usize, Vec<u16>>,
}


/// V3 binary manifest with an explicit encoding identifier.
#[derive(Deserialize)]
struct ManifestBinaryV3 {
    source_root: PathBuf,
    recursive: bool,
    git_commit: Option<String>,
    files: CompactFiles,
    changed_slots: Vec<usize>,
    changed_bigrams: BTreeMap<usize, Vec<u16>>,
    bigram_encoding: String,
}


impl From<ManifestBinary> for Manifest {
    fn from(value: ManifestBinary) -> Self {
        Self {
            source_root: value.source_root,
            recursive: value.recursive,
            git_commit: value.git_commit,
            files: value.files.0,
            changed_slots: value.changed_slots,
            changed_bigrams: value.changed_bigrams,
            bigram_encoding: BigramEncoding::Ascii,
        }
    }
}


impl TryFrom<ManifestBinaryV3> for Manifest {
    type Error = Error;

    fn try_from(value: ManifestBinaryV3) -> Result<Self> {
        Ok(Self {
            source_root: value.source_root,
            recursive: value.recursive,
            git_commit: value.git_commit,
            files: value.files.0,
            changed_slots: value.changed_slots,
            changed_bigrams: value.changed_bigrams,
            bigram_encoding: BigramEncoding::parse(&value.bigram_encoding)
                .ok_or_else(|| invalid("unknown encoding"))?,
        })
    }
}


/// A restored compressed filter and the filename/change metadata for each slot.
pub struct BigramSnapshot {
    pub source_root: PathBuf,
    pub recursive: bool,
    pub git_commit: Option<String>,
    pub files: Vec<BigramFile>,
    /// Slots whose source content has changed since the compressed base was built.
    pub changed_slots: Vec<usize>,
    /// Bigram postings for slots updated after the compressed base was built.
    pub changed_bigrams: BTreeMap<usize, Vec<u16>>,
    pub filter: BigramFilter,
}


/// Validated internal form used by early restoration without public file objects.
pub(crate) struct CompactSnapshot {
    pub source_root: PathBuf,
    pub recursive: bool,
    pub git_commit: Option<String>,
    pub id_bytes: Vec<u8>,
    pub entries: Vec<Option<crate::file_picker::FileEntry>>,
    pub changed_slots: Vec<usize>,
    pub changed_bigrams: BTreeMap<usize, Vec<u16>>,
    pub filter: BigramFilter,
}


impl CompactSnapshot {
    pub(crate) fn uses_scalar_postings(&self) -> bool {
        self.filter.scalar_postings
    }

    fn from_manifest(manifest: compact::DecodedManifest, filter: BigramFilter) -> Self {
        Self {
            source_root: manifest.source_root,
            recursive: manifest.recursive,
            git_commit: manifest.git_commit,
            id_bytes: manifest.id_bytes,
            entries: manifest.entries.0,
            changed_slots: manifest.changed_slots,
            changed_bigrams: manifest.changed_bigrams,
            filter,
        }
    }

    fn from_public(snapshot: BigramSnapshot) -> Result<Self> {
        let BigramSnapshot {
            source_root,
            recursive,
            git_commit,
            files,
            changed_slots,
            changed_bigrams,
            filter,
        } = snapshot;
        if files.windows(2).any(|pair| pair[0].id >= pair[1].id) {
            return Err(invalid("unordered cached file IDs"));
        }
        let arena = files.as_slice().id_arena()?.into_owned();
        let mut entries = Vec::with_capacity(files.len());
        let mut start = 0_u32;
        let changed_at_nanos = crate::file_picker::now_nanos();
        for file in files {
            let len = u32::try_from(file.id.len()).map_err(|_| invalid("ID too long"))?;
            entries.push(Some(crate::file_picker::FileEntry {
                id_start: start,
                id_len: len,
                stamp: crate::source::Stamp {
                    size: file.size_bytes,
                    modified: file.modified_unix_nanos,
                    identity: file.identity,
                },
                revision: 1,
                changed_at_nanos,
            }));
            start = start
                .checked_add(len)
                .ok_or_else(|| invalid("ID arena too large"))?;
        }
        Ok(Self {
            source_root,
            recursive,
            git_commit,
            id_bytes: arena,
            entries,
            changed_slots,
            changed_bigrams,
            filter,
        })
    }

    fn into_public(self) -> BigramSnapshot {
        let files = self
            .entries
            .into_iter()
            .map(|entry| {
                let entry = entry.expect("validated compact entry");
                let start = entry.id_start as usize;
                let end = start + entry.id_len as usize;
                let id = std::str::from_utf8(&self.id_bytes[start..end])
                    .expect("validated ID")
                    .to_owned();
                BigramFile {
                    filename: format!("{id}.json"),
                    id,
                    size_bytes: entry.stamp.size,
                    modified_unix_nanos: entry.stamp.modified,
                    identity: entry.stamp.identity,
                }
            })
            .collect();
        BigramSnapshot {
            source_root: self.source_root,
            recursive: self.recursive,
            git_commit: self.git_commit,
            files,
            changed_slots: self.changed_slots,
            changed_bigrams: self.changed_bigrams,
            filter: self.filter,
        }
    }
}


enum LoadedSnapshot {
    Public(BigramSnapshot),
    Compact(CompactSnapshot),
}


enum DecodedManifest {
    Legacy(Manifest),
    Compact(compact::DecodedManifest),
}


fn current_git_commit(root: &Path) -> Result<Option<String>> {
    let output = match Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
    {
        Ok(output) if output.status.success() => output,
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io(root, error)),
    };
    let commit = String::from_utf8(output.stdout)
        .map_err(|_| invalid("Git commit is not UTF-8"))?
        .trim()
        .to_owned();
    if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid("Git returned an invalid commit hash"));
    }
    Ok(Some(commit))
}

fn validate_files(files: &[BigramFile], file_count: usize) -> Result<()> {
    validate_files_ref(files, file_count)
}

fn validate_files_ref<F: SnapshotFiles + ?Sized>(files: &F, file_count: usize) -> Result<()> {
    if files.len() != file_count {
        return Err(invalid("file count differs from filter slots"));
    }
    // Repository snapshots are written in ID order. Validate that common
    // layout without building millions of temporary tree nodes. Public
    // snapshots may be unordered, so retain a duplicate check for those.
    let mut unordered_ids = (1..files.len())
        .any(|slot| files.entry(slot - 1).id >= files.entry(slot).id)
        .then(|| HashSet::with_capacity(files.len()));
    for slot in 0..files.len() {
        let file = files.entry(slot);
        validate_id(file.id).map_err(|_| invalid("invalid file ID"))?;
        if !file.filename_valid
            || unordered_ids
                .as_mut()
                .is_some_and(|ids| !ids.insert(file.id))
            || file.modified_unix_nanos.parse::<i128>().is_err()
            || file.size_bytes > i64::MAX as u64
        {
            return Err(invalid("invalid file manifest"));
        }
    }
    Ok(())
}

fn validate_changes(changed_slots: &[usize], file_count: usize) -> Result<()> {
    if changed_slots.iter().any(|&slot| slot >= file_count)
        || changed_slots.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(invalid("invalid changed slots"));
    }
    Ok(())
}

fn validate_changed_bigrams(
    changed_slots: &[usize],
    changed_bigrams: &BTreeMap<usize, Vec<u16>>,
) -> Result<()> {
    if changed_bigrams.iter().any(|(slot, keys)| {
        changed_slots.binary_search(slot).is_err() || keys.windows(2).any(|pair| pair[0] >= pair[1])
    }) {
        return Err(invalid("invalid changed bigrams"));
    }
    Ok(())
}

fn check_current(
    source_root: &Path,
    recursive: bool,
    git_commit: &Option<String>,
    files: &[BigramFile],
) -> Result<()> {
    check_current_files(source_root, recursive, git_commit, files)
}

fn check_current_files<F: SnapshotFiles + ?Sized>(
    source_root: &Path,
    recursive: bool,
    git_commit: &Option<String>,
    files: &F,
) -> Result<()> {
    if &current_git_commit(source_root)? != git_commit {
        return Err(Error::Cache("bigram snapshot Git commit changed".into()));
    }
    let source = Source::open_scoped(source_root, recursive)?;
    let inventory = source.inventory()?;
    if inventory.len() != files.len() {
        return Err(Error::Cache("bigram snapshot file set changed".into()));
    }
    for slot in 0..files.len() {
        let file = files.entry(slot);
        let Some(stamp) = inventory.get(file.id) else {
            return Err(Error::Cache("bigram snapshot file set changed".into()));
        };
        if !file.filename_valid
            || file.size_bytes != stamp.size
            || file.modified_unix_nanos != stamp.modified
            || file.identity != stamp.identity
        {
            return Err(Error::Cache(format!(
                "bigram snapshot file changed: {}",
                file.id
            )));
        }
    }
    if &current_git_commit(source_root)? != git_commit {
        return Err(Error::Cache("bigram snapshot Git commit changed".into()));
    }
    Ok(())
}


impl BigramSnapshot {
    /// Pair an already built filter with the exact source file slot order.
    pub fn new(
        source_root: &Path,
        recursive: bool,
        files: Vec<BigramFile>,
        filter: BigramFilter,
    ) -> Result<Self> {
        let source_root = fs::canonicalize(source_root).map_err(|error| io(source_root, error))?;
        validate_files(&files, filter.file_count())?;
        let git_commit = current_git_commit(&source_root)?;
        check_current(&source_root, recursive, &git_commit, &files)?;
        Ok(Self {
            source_root,
            recursive,
            git_commit,
            files,
            changed_slots: Vec::new(),
            changed_bigrams: BTreeMap::new(),
            filter,
        })
    }

    /// Save the filter and manifest with a checksum and atomic file replacement.
    pub fn save(&self, path: &Path) -> Result<()> {
        Self::write_parts(
            path,
            compact::ManifestRef::new(
                &self.source_root,
                self.recursive,
                &self.git_commit,
                &self.files[..],
                &self.changed_slots,
                &self.changed_bigrams,
                self.filter.encoding().as_str(),
                self.filter.scalar_postings,
            )?,
            &self.filter,
        )
    }

    /// Save a borrowed live filter without copying its compressed columns.
    pub fn save_filter(
        path: &Path,
        source_root: &Path,
        recursive: bool,
        files: &[BigramFile],
        filter: &BigramFilter,
    ) -> Result<()> {
        Self::save_filter_with_changes(
            path,
            source_root,
            recursive,
            files,
            &[],
            &BTreeMap::new(),
            filter,
        )
    }

    /// Save a base filter plus changed slots that must bypass its postings.
    pub fn save_filter_with_changes(
        path: &Path,
        source_root: &Path,
        recursive: bool,
        files: &[BigramFile],
        changed_slots: &[usize],
        changed_bigrams: &BTreeMap<usize, Vec<u16>>,
        filter: &BigramFilter,
    ) -> Result<()> {
        Self::save_filter_with_changes_borrowed(
            path,
            source_root,
            recursive,
            files,
            changed_slots,
            changed_bigrams,
            filter,
            false,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "snapshot metadata and encoding are independent inputs"
    )]
    pub(crate) fn save_filter_with_changes_borrowed<F: SnapshotFiles + ?Sized>(
        path: &Path,
        source_root: &Path,
        recursive: bool,
        files: &F,
        changed_slots: &[usize],
        changed_bigrams: &BTreeMap<usize, Vec<u16>>,
        filter: &BigramFilter,
        scalar_postings: bool,
    ) -> Result<()> {
        let source_root = fs::canonicalize(source_root).map_err(|error| io(source_root, error))?;
        let git_commit = current_git_commit(&source_root)?;
        check_current_files(&source_root, recursive, &git_commit, files)?;
        Self::write_parts(
            path,
            compact::ManifestRef::new(
                &source_root,
                recursive,
                &git_commit,
                files,
                changed_slots,
                changed_bigrams,
                filter.encoding().as_str(),
                scalar_postings,
            )?,
            filter,
        )
    }

    pub(crate) fn uses_scalar_postings(&self) -> bool {
        self.filter.scalar_postings
    }

    fn write_parts<F: SnapshotFiles + ?Sized>(
        path: &Path,
        manifest: compact::ManifestRef<'_, F>,
        filter: &BigramFilter,
    ) -> Result<()> {
        validate_files_ref(manifest.files(), filter.file_count())?;
        if manifest.id_bytes.len() > u32::MAX as usize {
            return Err(invalid("ID arena too large"));
        }
        validate_changes(manifest.changed_slots, filter.file_count())?;
        validate_changed_bigrams(manifest.changed_slots, manifest.changed_bigrams)?;
        if manifest.bigram_encoding != filter.encoding().as_str() {
            return Err(invalid("manifest encoding differs from filter"));
        }
        // Public snapshots may have unique but unordered slots. Keep their
        // established format, since a V5 ID arena requires ascending IDs.
        let legacy_unordered = (1..manifest.files().len())
            .any(|slot| manifest.files().entry(slot - 1).id >= manifest.files().entry(slot).id);
        let parent = path
            .parent()
            .ok_or_else(|| invalid("snapshot needs a parent directory"))?;
        fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|error| io(parent, error))?;
        temporary
            .write_all(&[0; HEADER_LEN])
            .map_err(|error| io(path, error))?;
        let (manifest_len, filter_len, digest) = {
            let writer = DigestWriter {
                file: temporary.as_file_mut(),
                digest: Sha256::new(),
                bytes: 0,
            };
            let mut buffered = BufWriter::with_capacity(64 * 1024, writer);
            if legacy_unordered {
                bincode::DefaultOptions::new()
                    .serialize_into(
                        &mut buffered,
                        &ManifestBinaryRef {
                            source_root: manifest.source_root,
                            recursive: manifest.recursive,
                            git_commit: manifest.git_commit,
                            files: CompactFilesRef(manifest.files()),
                            changed_slots: manifest.changed_slots,
                            changed_bigrams: manifest.changed_bigrams,
                            bigram_encoding: manifest.bigram_encoding,
                        },
                    )
                    .map_err(|error| {
                        Error::Cache(format!("cannot encode bigram manifest: {error}"))
                    })?;
            } else {
                bincode::DefaultOptions::new()
                    .serialize_into(&mut buffered, &manifest)
                    .map_err(|error| {
                        Error::Cache(format!("cannot encode bigram manifest: {error}"))
                    })?;
            }
            buffered.flush().map_err(|error| io(path, error))?;
            let manifest_len = buffered.get_ref().bytes;
            filter.write_to(&mut buffered)?;
            buffered.flush().map_err(|error| io(path, error))?;
            let writer = buffered
                .into_inner()
                .map_err(|error| io(path, error.into_error()))?;
            (
                manifest_len,
                writer.bytes - manifest_len,
                writer.digest.finalize(),
            )
        };
        let mut header = [0_u8; HEADER_LEN];
        header[..8].copy_from_slice(if legacy_unordered {
            if manifest.scalar_postings {
                MAGIC_SCALAR_POSTINGS
            } else {
                MAGIC_ENCODING
            }
        } else {
            MAGIC_COMPACT
        });
        header[8..16].copy_from_slice(&manifest_len.to_le_bytes());
        header[16..24].copy_from_slice(&filter_len.to_le_bytes());
        header[24..].copy_from_slice(&digest);
        temporary
            .as_file_mut()
            .seek(SeekFrom::Start(0))
            .map_err(|error| io(path, error))?;
        temporary
            .write_all(&header)
            .map_err(|error| io(path, error))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| io(path, error))?;
        temporary
            .persist(path)
            .map_err(|error| io(path, error.error))?;
        Ok(())
    }

    /// Read a snapshot for inspection. Use `load_checked` before querying files.
    pub fn load(path: &Path) -> Result<Self> {
        Ok(
            match Self::load_inner(
                path,
                #[cfg(feature = "index-timings")]
                &mut SnapshotLoadTimings::default(),
            )? {
                LoadedSnapshot::Public(snapshot) => snapshot,
                LoadedSnapshot::Compact(snapshot) => snapshot.into_public(),
            },
        )
    }

    #[cfg(feature = "index-timings")]
    pub(crate) fn load_profiled(path: &Path) -> Result<(CompactSnapshot, SnapshotLoadTimings)> {
        let mut timings = SnapshotLoadTimings::default();
        let snapshot = Self::load_inner(path, &mut timings)?;
        Ok((
            match snapshot {
                LoadedSnapshot::Public(snapshot) => CompactSnapshot::from_public(snapshot)?,
                LoadedSnapshot::Compact(snapshot) => snapshot,
            },
            timings,
        ))
    }

    pub(crate) fn load_scoped_compact(
        path: &Path,
        source_root: &Path,
        recursive: bool,
    ) -> Result<CompactSnapshot> {
        let snapshot = Self::load_inner(
            path,
            #[cfg(feature = "index-timings")]
            &mut SnapshotLoadTimings::default(),
        )?;
        let snapshot = match snapshot {
            LoadedSnapshot::Public(snapshot) => CompactSnapshot::from_public(snapshot)?,
            LoadedSnapshot::Compact(snapshot) => snapshot,
        };
        let source_root = fs::canonicalize(source_root).map_err(|error| io(source_root, error))?;
        if snapshot.source_root != source_root || snapshot.recursive != recursive {
            return Err(Error::Cache("bigram snapshot source scope changed".into()));
        }
        Ok(snapshot)
    }

    fn load_inner(
        path: &Path,
        #[cfg(feature = "index-timings")] timings: &mut SnapshotLoadTimings,
    ) -> Result<LoadedSnapshot> {
        let size = fs::metadata(path).map_err(|error| io(path, error))?.len();
        if size > MAX_SNAPSHOT_BYTES {
            return Err(invalid("snapshot exceeds size limit"));
        }
        let mut file = BufReader::with_capacity(
            64 * 1024,
            fs::File::open(path).map_err(|error| io(path, error))?,
        );
        let mut header = [0_u8; HEADER_LEN];
        file.read_exact(&mut header)
            .map_err(|error| io(path, error))?;
        if size < HEADER_LEN as u64
            || (&header[..8] != MAGIC_JSON
                && &header[..8] != MAGIC_BINARY
                && &header[..8] != MAGIC_ENCODING
                && &header[..8] != MAGIC_SCALAR_POSTINGS
                && &header[..8] != MAGIC_COMPACT)
        {
            return Err(invalid("unknown format version"));
        }
        let manifest_len = u64::from_le_bytes(header[8..16].try_into().unwrap());
        let filter_len = u64::from_le_bytes(header[16..24].try_into().unwrap());
        let payload_len = manifest_len
            .checked_add(filter_len)
            .ok_or_else(|| invalid("snapshot length overflow"))?;
        if payload_len.checked_add(HEADER_LEN as u64) != Some(size) {
            return Err(invalid("snapshot lengths do not match"));
        }
        // Check the payload before deserializing untrusted lengths. Rehash the
        // decode pass below to catch replacement between these two passes.
        #[cfg(feature = "index-timings")]
        let checksum_started = std::time::Instant::now();
        let mut digest = Sha256::new();
        let mut remaining = payload_len;
        let mut block = [0_u8; 64 * 1024];
        while remaining > 0 {
            let take = usize::try_from(remaining.min(block.len() as u64)).unwrap();
            file.read_exact(&mut block[..take])
                .map_err(|error| io(path, error))?;
            digest.update(&block[..take]);
            remaining -= take as u64;
        }
        if digest.finalize().as_slice() != &header[24..HEADER_LEN] {
            return Err(invalid("snapshot checksum differs"));
        }

        #[cfg(feature = "index-timings")]
        {
            timings.base_checksum_nanos = checksum_started.elapsed().as_nanos() as u64;
            timings.bytes_read = HEADER_LEN as u64 + payload_len * 2;
        }
        file.seek(SeekFrom::Start(HEADER_LEN as u64))
            .map_err(|error| io(path, error))?;
        // Hash buffered file reads rather than every small bincode field read.
        let mut reader = BufReader::with_capacity(
            64 * 1024,
            DigestReader {
                inner: file.into_inner(),
                digest: Sha256::new(),
                #[cfg(feature = "index-timings")]
                hash_nanos: 0,
            },
        );
        #[cfg(feature = "index-timings")]
        let manifest_started = std::time::Instant::now();
        let manifest = {
            let mut section = (&mut reader).take(manifest_len);
            let manifest = if &header[..8] == MAGIC_JSON {
                DecodedManifest::Legacy(
                    serde_json::from_reader(&mut section).map_err(|error| {
                        Error::Cache(format!("invalid bigram manifest: {error}"))
                    })?,
                )
            } else if &header[..8] == MAGIC_BINARY {
                let binary: ManifestBinary = bincode::DefaultOptions::new()
                    .with_limit(manifest_len)
                    .deserialize_from(&mut section)
                    .map_err(|error| Error::Cache(format!("invalid bigram manifest: {error}")))?;
                DecodedManifest::Legacy(binary.into())
            } else if &header[..8] == MAGIC_COMPACT {
                let binary: compact::DecodedManifest = bincode::DefaultOptions::new()
                    .with_limit(manifest_len)
                    .deserialize_from(&mut section)
                    .map_err(|error| Error::Cache(format!("invalid bigram manifest: {error}")))?;
                DecodedManifest::Compact(binary)
            } else {
                let binary: ManifestBinaryV3 = bincode::DefaultOptions::new()
                    .with_limit(manifest_len)
                    .deserialize_from(&mut section)
                    .map_err(|error| Error::Cache(format!("invalid bigram manifest: {error}")))?;
                DecodedManifest::Legacy(binary.try_into()?)
            };
            if section.limit() != 0 {
                return Err(invalid("manifest has trailing data"));
            }
            manifest
        };
        #[cfg(feature = "index-timings")]
        {
            timings.manifest_decode_nanos = manifest_started.elapsed().as_nanos() as u64;
        }

        #[cfg(feature = "index-timings")]
        let filter_started = std::time::Instant::now();
        let mut filter = {
            let mut section = (&mut reader).take(filter_len);
            BigramFilter::read_from(&mut section, filter_len)?
        };
        #[cfg(feature = "index-timings")]
        {
            timings.base_filter_decode_nanos = filter_started.elapsed().as_nanos() as u64;
            timings.posting_columns = filter.profile_total_columns();
        }
        let mut trailing = [0_u8; 1];
        if reader
            .read(&mut trailing)
            .map_err(|error| io(path, error))?
            != 0
        {
            return Err(invalid("snapshot has trailing data"));
        }
        let digest_reader = reader.into_inner();
        #[cfg(feature = "index-timings")]
        {
            timings.base_decode_checksum_nanos = digest_reader.hash_nanos;
        }
        if digest_reader.digest.finalize().as_slice() != &header[24..HEADER_LEN] {
            return Err(invalid("snapshot changed during decode"));
        }

        #[cfg(feature = "index-timings")]
        let validation_started = std::time::Instant::now();
        let snapshot = match manifest {
            DecodedManifest::Legacy(manifest) => {
                if manifest.bigram_encoding != filter.encoding() {
                    return Err(invalid("manifest encoding differs from filter"));
                }
                filter.scalar_postings = &header[..8] == MAGIC_SCALAR_POSTINGS;
                validate_files(&manifest.files, filter.file_count())?;
                validate_changes(&manifest.changed_slots, filter.file_count())?;
                validate_changed_bigrams(&manifest.changed_slots, &manifest.changed_bigrams)?;
                LoadedSnapshot::Public(Self {
                    source_root: manifest.source_root,
                    recursive: manifest.recursive,
                    git_commit: manifest.git_commit,
                    files: manifest.files,
                    changed_slots: manifest.changed_slots,
                    changed_bigrams: manifest.changed_bigrams,
                    filter,
                })
            }
            DecodedManifest::Compact(manifest) => {
                if BigramEncoding::parse(&manifest.bigram_encoding) != Some(filter.encoding()) {
                    return Err(invalid("manifest encoding differs from filter"));
                }
                filter.scalar_postings = manifest.scalar_postings;
                manifest.validate(filter.file_count())?;
                validate_changes(&manifest.changed_slots, filter.file_count())?;
                validate_changed_bigrams(&manifest.changed_slots, &manifest.changed_bigrams)?;
                LoadedSnapshot::Compact(CompactSnapshot::from_manifest(manifest, filter))
            }
        };
        #[cfg(feature = "index-timings")]
        {
            timings.manifest_validation_nanos = validation_started.elapsed().as_nanos() as u64;
        }
        Ok(snapshot)
    }

    /// Restore only when every ID, filename, stamp, and Git commit still agrees.
    pub fn load_checked(path: &Path, source_root: &Path, recursive: bool) -> Result<Self> {
        let snapshot = Self::load_scoped(path, source_root, recursive)?;
        check_current(
            &snapshot.source_root,
            recursive,
            &snapshot.git_commit,
            &snapshot.files,
        )?;
        Ok(snapshot)
    }

    /// Load the saved base for incremental source reconciliation.
    pub fn load_scoped(path: &Path, source_root: &Path, recursive: bool) -> Result<Self> {
        let snapshot = Self::load(path)?;
        let source_root = fs::canonicalize(source_root).map_err(|error| io(source_root, error))?;
        if snapshot.source_root != source_root || snapshot.recursive != recursive {
            return Err(Error::Cache("bigram snapshot source scope changed".into()));
        }
        Ok(snapshot)
    }

    /// Candidate files in their original numeric slot order.
    pub fn candidate_files(&self, pattern: &[u8]) -> Vec<&BigramFile> {
        let candidates = self.filter.query(pattern);
        self.files
            .iter()
            .enumerate()
            .filter(|(slot, _)| {
                self.changed_slots.binary_search(slot).is_ok()
                    || candidates
                        .as_ref()
                        .is_none_or(|bits| BigramFilter::is_candidate(bits, *slot))
            })
            .map(|(_, file)| file)
            .collect()
    }
}


#[cfg(test)]
#[path = "snapshot.test.rs"]
mod tests;
