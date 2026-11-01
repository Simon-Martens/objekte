//! A portable filter snapshot with an explicit slot-to-file manifest.

use super::BigramFilter;
use crate::{Error, Result, error::io, repository::validate_id, source::Source};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

const MAGIC: &[u8; 8] = b"OBJSNP01";
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
}

#[derive(Serialize)]
struct ManifestRef<'a> {
    source_root: &'a PathBuf,
    recursive: bool,
    git_commit: &'a Option<String>,
    files: &'a [BigramFile],
    changed_slots: &'a [usize],
    changed_bigrams: &'a BTreeMap<usize, Vec<u16>>,
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
    let mut ids = BTreeSet::new();
    let mut filenames = BTreeSet::new();
    for file in files {
        validate_id(&file.id).map_err(|_| invalid("invalid file ID"))?;
        if file.filename != format!("{}.json", file.id)
            || !ids.insert(&file.id)
            || !filenames.insert(&file.filename)
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
            ManifestRef {
                source_root: &self.source_root,
                recursive: self.recursive,
                git_commit: &self.git_commit,
                files: &self.files,
                changed_slots: &self.changed_slots,
                changed_bigrams: &self.changed_bigrams,
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
            ManifestRef {
                source_root: &source_root,
                recursive,
                git_commit: &git_commit,
                files,
                changed_slots,
                changed_bigrams,
            },
            filter,
        )
    }

    fn write_parts(path: &Path, manifest: ManifestRef<'_>, filter: &BigramFilter) -> Result<()> {
        validate_files(manifest.files, filter.file_count())?;
        validate_changes(manifest.changed_slots, filter.file_count())?;
        validate_changed_bigrams(manifest.changed_slots, manifest.changed_bigrams)?;
        let manifest = serde_json::to_vec(&manifest)
            .map_err(|error| Error::Cache(format!("cannot encode bigram manifest: {error}")))?;
        let filter = filter.to_bytes()?;
        let mut bytes = Vec::with_capacity(HEADER_LEN + manifest.len() + filter.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&(manifest.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&(filter.len() as u64).to_le_bytes());
        let mut digest = Sha256::new();
        digest.update(&manifest);
        digest.update(&filter);
        bytes.extend_from_slice(&digest.finalize());
        bytes.extend_from_slice(&manifest);
        bytes.extend_from_slice(&filter);
        if bytes.len() as u64 > MAX_SNAPSHOT_BYTES {
            return Err(invalid("snapshot exceeds size limit"));
        }
        let parent = path
            .parent()
            .ok_or_else(|| invalid("snapshot needs a parent directory"))?;
        fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|error| io(parent, error))?;
        temporary
            .write_all(&bytes)
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
        let size = fs::metadata(path).map_err(|error| io(path, error))?.len();
        if size > MAX_SNAPSHOT_BYTES {
            return Err(invalid("snapshot exceeds size limit"));
        }
        let bytes = fs::read(path).map_err(|error| io(path, error))?;
        if bytes.len() < HEADER_LEN || &bytes[..8] != MAGIC {
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
        let manifest: Manifest = serde_json::from_slice(manifest_bytes)
            .map_err(|error| Error::Cache(format!("invalid bigram manifest: {error}")))?;
        let filter = BigramFilter::from_bytes(filter_bytes)?;
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
