//! A portable filter snapshot with an explicit slot-to-file manifest.

use super::{BigramEncoding, BigramFilter};
use crate::{Error, Result, error::io, repository::validate_id, source::Source};
use bincode::Options;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{SeqAccess, Visitor},
    ser::SerializeSeq,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

const MAGIC_JSON: &[u8; 8] = b"OBJSNP01";
const MAGIC_BINARY: &[u8; 8] = b"OBJSNP02";
const MAGIC_ENCODING: &[u8; 8] = b"OBJSNP03";
const HEADER_LEN: usize = 8 + 8 + 8 + 32;
const MAX_SNAPSHOT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

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


/// V2 stores only data that cannot be reconstructed from the record ID.
struct CompactFilesRef<'a>(&'a [BigramFile]);


impl Serialize for CompactFilesRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut files = serializer.serialize_seq(Some(self.0.len()))?;
        for file in self.0 {
            files.serialize_element(&(
                &file.id,
                file.size_bytes,
                &file.modified_unix_nanos,
                &file.identity,
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
struct ManifestBinaryRef<'a> {
    source_root: &'a PathBuf,
    recursive: bool,
    git_commit: &'a Option<String>,
    files: CompactFilesRef<'a>,
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
    if files.len() != file_count {
        return Err(invalid("file count differs from filter slots"));
    }
    // Repository snapshots are written in ID order. Validate that common
    // layout without building millions of temporary tree nodes. Public
    // snapshots may be unordered, so retain a duplicate check for those.
    let mut unordered_ids = files
        .windows(2)
        .any(|pair| pair[0].id >= pair[1].id)
        .then(|| HashSet::with_capacity(files.len()));
    for file in files {
        validate_id(&file.id).map_err(|_| invalid("invalid file ID"))?;
        if file.filename.strip_suffix(".json") != Some(file.id.as_str())
            || unordered_ids
                .as_mut()
                .is_some_and(|ids| !ids.insert(file.id.as_str()))
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
    if &current_git_commit(source_root)? != git_commit {
        return Err(Error::Cache("bigram snapshot Git commit changed".into()));
    }
    let source = Source::open_scoped(source_root, recursive)?;
    let inventory = source.inventory()?;
    if inventory.len() != files.len() {
        return Err(Error::Cache("bigram snapshot file set changed".into()));
    }
    for file in files {
        let Some(stamp) = inventory.get(&file.id) else {
            return Err(Error::Cache("bigram snapshot file set changed".into()));
        };
        if file.filename != format!("{}.json", file.id)
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
            ManifestBinaryRef {
                source_root: &self.source_root,
                recursive: self.recursive,
                git_commit: &self.git_commit,
                files: CompactFilesRef(&self.files),
                changed_slots: &self.changed_slots,
                changed_bigrams: &self.changed_bigrams,
                bigram_encoding: self.filter.encoding().as_str(),
            },
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
        let source_root = fs::canonicalize(source_root).map_err(|error| io(source_root, error))?;
        let git_commit = current_git_commit(&source_root)?;
        check_current(&source_root, recursive, &git_commit, files)?;
        Self::write_parts(
            path,
            ManifestBinaryRef {
                source_root: &source_root,
                recursive,
                git_commit: &git_commit,
                files: CompactFilesRef(files),
                changed_slots,
                changed_bigrams,
                bigram_encoding: filter.encoding().as_str(),
            },
            filter,
        )
    }

    fn write_parts(
        path: &Path,
        manifest: ManifestBinaryRef<'_>,
        filter: &BigramFilter,
    ) -> Result<()> {
        validate_files(manifest.files.0, filter.file_count())?;
        validate_changes(manifest.changed_slots, filter.file_count())?;
        validate_changed_bigrams(manifest.changed_slots, manifest.changed_bigrams)?;
        if manifest.bigram_encoding != filter.encoding().as_str() {
            return Err(invalid("manifest encoding differs from filter"));
        }
        let manifest = bincode::DefaultOptions::new()
            .serialize(&manifest)
            .map_err(|error| Error::Cache(format!("cannot encode bigram manifest: {error}")))?;
        let filter = filter.to_bytes()?;
        let size = HEADER_LEN
            .checked_add(manifest.len())
            .and_then(|size| size.checked_add(filter.len()))
            .ok_or_else(|| invalid("snapshot exceeds size limit"))?;
        if size as u64 > MAX_SNAPSHOT_BYTES {
            return Err(invalid("snapshot exceeds size limit"));
        }
        let mut digest = Sha256::new();
        digest.update(&manifest);
        digest.update(&filter);
        let mut header = [0_u8; HEADER_LEN];
        header[..8].copy_from_slice(MAGIC_ENCODING);
        header[8..16].copy_from_slice(&(manifest.len() as u64).to_le_bytes());
        header[16..24].copy_from_slice(&(filter.len() as u64).to_le_bytes());
        header[24..].copy_from_slice(&digest.finalize());
        let parent = path
            .parent()
            .ok_or_else(|| invalid("snapshot needs a parent directory"))?;
        fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|error| io(parent, error))?;
        for part in [&header[..], &manifest, &filter] {
            temporary.write_all(part).map_err(|error| io(path, error))?;
        }
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
        let size = fs::metadata(path).map_err(|error| io(path, error))?.len();
        if size > MAX_SNAPSHOT_BYTES {
            return Err(invalid("snapshot exceeds size limit"));
        }
        let bytes = fs::read(path).map_err(|error| io(path, error))?;
        if bytes.len() < HEADER_LEN
            || (&bytes[..8] != MAGIC_JSON
                && &bytes[..8] != MAGIC_BINARY
                && &bytes[..8] != MAGIC_ENCODING)
        {
            return Err(invalid("unknown format version"));
        }
        let manifest_len = usize::try_from(u64::from_le_bytes(bytes[8..16].try_into().unwrap()))
            .map_err(|_| invalid("manifest length overflow"))?;
        let filter_len = usize::try_from(u64::from_le_bytes(bytes[16..24].try_into().unwrap()))
            .map_err(|_| invalid("filter length overflow"))?;
        if HEADER_LEN
            .checked_add(manifest_len)
            .and_then(|size| size.checked_add(filter_len))
            != Some(bytes.len())
        {
            return Err(invalid("snapshot lengths do not match"));
        }
        let manifest_bytes = &bytes[HEADER_LEN..HEADER_LEN + manifest_len];
        let filter_bytes = &bytes[HEADER_LEN + manifest_len..];
        let mut digest = Sha256::new();
        digest.update(manifest_bytes);
        digest.update(filter_bytes);
        if digest.finalize().as_slice() != &bytes[24..HEADER_LEN] {
            return Err(invalid("snapshot checksum differs"));
        }
        let manifest: Manifest = if &bytes[..8] == MAGIC_JSON {
            serde_json::from_slice(manifest_bytes)
                .map_err(|error| Error::Cache(format!("invalid bigram manifest: {error}")))?
        } else if &bytes[..8] == MAGIC_BINARY {
            let binary: ManifestBinary = bincode::DefaultOptions::new()
                .with_limit(MAX_SNAPSHOT_BYTES)
                .deserialize(manifest_bytes)
                .map_err(|error| Error::Cache(format!("invalid bigram manifest: {error}")))?;
            binary.into()
        } else {
            let binary: ManifestBinaryV3 = bincode::DefaultOptions::new()
                .with_limit(MAX_SNAPSHOT_BYTES)
                .deserialize(manifest_bytes)
                .map_err(|error| Error::Cache(format!("invalid bigram manifest: {error}")))?;
            binary.try_into()?
        };
        let filter = BigramFilter::from_bytes(filter_bytes)?;
        if manifest.bigram_encoding != filter.encoding() {
            return Err(invalid("manifest encoding differs from filter"));
        }
        validate_files(&manifest.files, filter.file_count())?;
        validate_changes(&manifest.changed_slots, filter.file_count())?;
        validate_changed_bigrams(&manifest.changed_slots, &manifest.changed_bigrams)?;
        Ok(Self {
            source_root: manifest.source_root,
            recursive: manifest.recursive,
            git_commit: manifest.git_commit,
            files: manifest.files,
            changed_slots: manifest.changed_slots,
            changed_bigrams: manifest.changed_bigrams,
            filter,
        })
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
