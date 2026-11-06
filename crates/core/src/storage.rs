//! Versioned, checksummed sections for complete immutable search generations.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use bincode::Options;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::bigram::{BIGRAM_VERSION, BigramIndex};
use crate::inventory::{CollectionGeneration, FileInventory, SourceRevision};
use crate::model::{CollectionId, FileId, RecordId, RecordSlot};
use crate::posting::PostingCollection;
use crate::repository::{
    CollectionDefinition, CollectionState, CollectionStats, Repository, RepositoryOptions,
    StartupStats,
};
use crate::sorting::{COLLATION_VERSION, SortCollection};
use crate::workload::BigramAlphabet;

const MAGIC: &[u8; 8] = b"OBJSNAP1";
pub const SNAPSHOT_VERSION: u32 = 1;
const MAX_SECTION_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct EncodedFile {
    id: FileId,
    relative_path: Vec<u8>,
    revision: SourceRevision,
    created_ns: Option<i128>,
    collections: Vec<CollectionId>,
}

#[derive(Serialize, Deserialize)]
struct EncodedCollection {
    generation: CollectionGeneration,
    postings: PostingCollection,
    raw: BigramIndex,
    custom_text: HashMap<(Vec<String>, String), BigramIndex>,
    sorting: SortCollection,
    stats: CollectionStats,
}

#[derive(Serialize)]
struct EncodedCollectionRef<'a> {
    generation: &'a CollectionGeneration,
    postings: &'a PostingCollection,
    raw: &'a BigramIndex,
    custom_text: &'a HashMap<(Vec<String>, String), BigramIndex>,
    sorting: &'a SortCollection,
    stats: CollectionStats,
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

pub fn snapshot_path(root: &Path, alphabet: BigramAlphabet) -> PathBuf {
    root.join(".objekte").join(match alphabet {
        BigramAlphabet::Ascii => "search-core-ascii.snapshot",
        BigramAlphabet::Unicode => "search-core-unicode.snapshot",
    })
}

/// Reconcile the complete selected file inventory, including additions and
/// deletions. A mismatch requires a fresh private build before publication.
pub fn sources_match(repo: &Repository) -> Result<bool, String> {
    for state in &repo.collections {
        let directory = repo.files.root.join(&state.definition.source_path);
        let extension = match state.definition.format {
            crate::source::SourceFormat::Json => "json",
            crate::source::SourceFormat::JsonLines => "jsonl",
        };
        let mut observed = Vec::new();
        for entry in
            fs::read_dir(&directory).map_err(|e| format!("{}: {e}", directory.display()))?
        {
            let entry = entry.map_err(|e| e.to_string())?;
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if !kind.is_file() || kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            if path.extension().is_none_or(|e| e != extension) {
                continue;
            }
            let relative = path
                .strip_prefix(&repo.files.root)
                .map_err(|e| e.to_string())?
                .to_owned();
            let revision =
                SourceRevision::from_metadata(&entry.metadata().map_err(|e| e.to_string())?)?;
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
) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(crate::MODEL_VERSION.to_le_bytes());
    hash.update(SNAPSHOT_VERSION.to_le_bytes());
    hash.update(BIGRAM_VERSION.to_le_bytes());
    feed(&mut hash, std::env::consts::OS.as_bytes());
    feed(&mut hash, COLLATION_VERSION.as_bytes());
    hash.update([match options.bigram_alphabet {
        BigramAlphabet::Ascii => 0,
        BigramAlphabet::Unicode => 1,
    }]);
    for definition in definitions {
        feed(&mut hash, definition.name.as_bytes());
        feed(&mut hash, &path_bytes(&definition.source_path));
        feed(
            &mut hash,
            format!("{:?}/{:?}", definition.format, definition.identity).as_bytes(),
        );
        feed(&mut hash, definition.sort_locale.as_bytes());
        for field in &definition.sort_fields {
            feed(&mut hash, field.display().as_bytes());
        }
        let mut fields: Vec<_> = definition.schema.fields.values().collect();
        fields.sort_by(|a, b| a.path.cmp(&b.path));
        for field in fields {
            feed(
                &mut hash,
                format!(
                    "{:?}|{:?}|{}|{}|{}|{}",
                    field.path,
                    field.kind,
                    field.required,
                    field.nullable,
                    field.indexed,
                    field.multi
                )
                .as_bytes(),
            );
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
    }
    hash.finalize().into()
}

fn write_section(file: &mut File, id: u32, value: &impl Serialize) -> Result<(), String> {
    let bytes = codec().serialize(value).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_SECTION_BYTES {
        return Err("snapshot section too large".into());
    }
    file.write_all(&id.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&(bytes.len() as u64).to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&Sha256::digest(&bytes))
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())
}

fn read_section<T: for<'de> Deserialize<'de>>(
    file: &mut File,
    expected_id: u32,
) -> Result<T, String> {
    let mut id = [0; 4];
    let mut size = [0; 8];
    let mut checksum = [0; 32];
    file.read_exact(&mut id)
        .map_err(|e| format!("truncated snapshot section ID: {e}"))?;
    file.read_exact(&mut size)
        .map_err(|e| format!("truncated snapshot section size: {e}"))?;
    file.read_exact(&mut checksum)
        .map_err(|e| format!("truncated snapshot checksum: {e}"))?;
    if u32::from_le_bytes(id) != expected_id {
        return Err("snapshot section order mismatch".into());
    }
    let len = u64::from_le_bytes(size);
    if len > MAX_SECTION_BYTES {
        return Err("snapshot section too large".into());
    }
    let mut bytes = vec![0; len as usize];
    file.read_exact(&mut bytes)
        .map_err(|e| format!("truncated snapshot section: {e}"))?;
    if Sha256::digest(&bytes).as_slice() != checksum {
        return Err("snapshot checksum mismatch".into());
    }
    codec()
        .with_limit(len)
        .deserialize(&bytes)
        .map_err(|e| format!("invalid snapshot section: {e}"))
}

impl Repository {
    pub fn save_snapshot(&self, path: &Path) -> Result<(), String> {
        let parent = path.parent().ok_or("snapshot needs a parent directory")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let temporary = parent.join(format!(
            ".objekte-snapshot-{}-{nonce}.tmp",
            std::process::id()
        ));
        let outcome = (|| {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)
                .map_err(|e| e.to_string())?;
            file.write_all(MAGIC).map_err(|e| e.to_string())?;
            file.write_all(&SNAPSHOT_VERSION.to_le_bytes())
                .map_err(|e| e.to_string())?;
            file.write_all(&crate::MODEL_VERSION.to_le_bytes())
                .map_err(|e| e.to_string())?;
            file.write_all(&configuration_hash(
                &self
                    .collections
                    .iter()
                    .map(|c| c.definition.clone())
                    .collect::<Vec<_>>(),
                self.options,
            ))
            .map_err(|e| e.to_string())?;
            file.write_all(&(self.collections.len() as u32).to_le_bytes())
                .map_err(|e| e.to_string())?;
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
                })
                .collect();
            write_section(&mut file, 0, &files)?;
            for (index, state) in self.collections.iter().enumerate() {
                let encoded = EncodedCollectionRef {
                    generation: &state.generation,
                    postings: &state.postings,
                    raw: &state.raw,
                    custom_text: &state.custom_text,
                    sorting: &state.sorting,
                    stats: state.stats,
                };
                write_section(&mut file, index as u32 + 1, &encoded)?;
            }
            file.sync_all().map_err(|e| e.to_string())?;
            fs::rename(&temporary, path).map_err(|e| e.to_string())?;
            Ok(())
        })();
        if outcome.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        outcome
    }
}

pub fn load_snapshot(
    root: PathBuf,
    definitions: Vec<CollectionDefinition>,
    options: RepositoryOptions,
    path: &Path,
) -> Result<Repository, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut magic = [0; 8];
    let mut version = [0; 4];
    let mut model = [0; 4];
    let mut hash = [0; 32];
    let mut count = [0; 4];
    file.read_exact(&mut magic).map_err(|e| e.to_string())?;
    file.read_exact(&mut version).map_err(|e| e.to_string())?;
    file.read_exact(&mut model).map_err(|e| e.to_string())?;
    file.read_exact(&mut hash).map_err(|e| e.to_string())?;
    file.read_exact(&mut count).map_err(|e| e.to_string())?;
    if &magic != MAGIC
        || u32::from_le_bytes(version) != SNAPSHOT_VERSION
        || u32::from_le_bytes(model) != crate::MODEL_VERSION
    {
        return Err("incompatible snapshot version".into());
    }
    if hash != configuration_hash(&definitions, options) {
        return Err("incompatible snapshot configuration".into());
    }
    if u32::from_le_bytes(count) as usize != definitions.len() {
        return Err("snapshot collection count mismatch".into());
    }
    let encoded_files: Vec<EncodedFile> = read_section(&mut file, 0)?;
    let mut files = FileInventory::new(root);
    for (index, entry) in encoded_files.into_iter().enumerate() {
        if entry.id.0 as usize != index || entry.collections.is_empty() {
            return Err("invalid snapshot file identity".into());
        }
        let path = path_from_bytes(entry.relative_path)?;
        for collection in entry.collections {
            if collection.0 as usize >= definitions.len() {
                return Err("snapshot file collection out of bounds".into());
            }
            let id = files.add(path.clone(), entry.revision, entry.created_ns, collection)?;
            if id != entry.id {
                return Err("snapshot file ID mismatch".into());
            }
        }
    }
    files.rebuild_lookup()?;
    let mut collections = Vec::new();
    for (index, definition) in definitions.into_iter().enumerate() {
        let encoded: EncodedCollection = read_section(&mut file, index as u32 + 1)?;
        let generation = encoded.generation;
        if generation.collection.0 as usize != index || generation.name != definition.name {
            return Err("snapshot collection identity mismatch".into());
        }
        generation.validate(&files)?;
        encoded.postings.validate(generation.records.len())?;
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
        encoded.raw.validate(generation.records.len())?;
        if encoded.raw.alphabet != options.bigram_alphabet {
            return Err("snapshot alphabet mismatch".into());
        }
        if encoded.custom_text.keys().collect::<HashSet<_>>()
            != definition.schema.custom.keys().collect::<HashSet<_>>()
        {
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
                || column.locale != definition.sort_locale
                || column.collation_version != COLLATION_VERSION
                || column.keys.len() != generation.records.len()
                || column.ascending.len() != generation.records.len()
            {
                return Err("snapshot sort binding mismatch".into());
            }
            let mut seen = vec![false; generation.records.len()];
            for slot in &column.ascending {
                let i = slot.0 as usize;
                if i >= seen.len() || seen[i] {
                    return Err("snapshot sort permutation mismatch".into());
                }
                seen[i] = true;
            }
            if column.ascending.windows(2).any(|pair| {
                let a = pair[0].0 as usize;
                let b = pair[1].0 as usize;
                column.keys[a]
                    .compare(&column.keys[b], false)
                    .then_with(|| generation.records[a].id.cmp(&generation.records[b].id))
                    .is_gt()
            }) {
                return Err("snapshot sort order mismatch".into());
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
            generation,
            postings: encoded.postings,
            raw: encoded.raw,
            custom_text: encoded.custom_text,
            sorting: encoded.sorting,
            id_lookup,
            stats,
        });
    }
    let mut trailing = [0; 1];
    if file.read(&mut trailing).map_err(|e| e.to_string())? != 0 {
        return Err("snapshot trailing bytes".into());
    }
    Ok(Repository {
        files,
        collections,
        build_parses: 0,
        options,
        startup: StartupStats::default(),
        workers: Repository::create_workers(options.query_workers)?,
        prepared: Mutex::new(crate::prepared::PreparedCache::new(
            options.prepared_cache_bytes,
        )),
        verified: Mutex::new(crate::verified::VerifiedCache::new(
            options.verified_cache_bytes,
        )),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FieldPath, LogicalType};
    use crate::query::{Query, QueryOptions, parse_filter};
    use crate::reference::{FieldDefinition, Schema};
    use crate::repository::IdentityPolicy;
    use crate::source::SourceFormat;
    use crate::workload::ReturnMode;

    #[test]
    fn round_trip_shared_multi_record_files_and_reject_corruption() {
        let root = std::env::temp_dir().join(format!("objekte-snapshot-{}", std::process::id()));
        fs::create_dir_all(root.join("data")).unwrap();
        fs::write(
            root.join("data/multi.jsonl"),
            b"{\"x\":\"one\"}\n{\"x\":\"two\"}\n",
        )
        .unwrap();
        fs::write(root.join("data/empty.jsonl"), b"").unwrap();
        let mut schema = Schema::default();
        schema
            .register_field(FieldDefinition {
                path: vec!["x".into()],
                kind: LogicalType::String,
                required: false,
                nullable: false,
                indexed: true,
                multi: false,
            })
            .unwrap();
        let definitions: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|name| CollectionDefinition {
                name: name.into(),
                source_path: "data".into(),
                format: SourceFormat::JsonLines,
                identity: IdentityPolicy::Generated,
                schema: schema.clone(),
                sort_fields: vec![FieldPath::parse("x").unwrap()],
                sort_locale: "de".into(),
            })
            .collect();
        let query = Query {
            collections: vec!["a".into()],
            fts: None,
            filter: Some(parse_filter("x = 'one'").unwrap()),
            options: QueryOptions {
                sort: vec![],
                limit: 10,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: true,
            },
        };
        for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
            let options = RepositoryOptions {
                bigram_alphabet: alphabet,
                ..RepositoryOptions::default()
            };
            let built = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
            assert_eq!(built.build_parses, 2);
            let path = root.join(format!("{alphabet:?}.snapshot"));
            built.save_snapshot(&path).unwrap();
            let restored =
                load_snapshot(root.clone(), definitions.clone(), options, &path).unwrap();
            assert_eq!(restored.files.files.len(), 2);
            assert_eq!(restored.files.files[0].collections.len(), 2);
            assert_eq!(restored.collections[0].generation.records.len(), 2);
            assert_eq!(
                restored.collections[0].generation.file_records.slots.len(),
                2
            );
            assert_eq!(
                built.search(&query).unwrap().count,
                restored.search(&query).unwrap().count
            );
            let other = RepositoryOptions {
                bigram_alphabet: if alphabet == BigramAlphabet::Ascii {
                    BigramAlphabet::Unicode
                } else {
                    BigramAlphabet::Ascii
                },
                ..options
            };
            assert!(
                load_snapshot(root.clone(), definitions.clone(), other, &path)
                    .err()
                    .unwrap()
                    .contains("incompatible snapshot configuration")
            );
            let mut bytes = fs::read(&path).unwrap();
            let mut corrupted = bytes.clone();
            let tail = corrupted.len() - 1;
            corrupted[tail] ^= 0x01;
            fs::write(&path, &corrupted).unwrap();
            assert!(
                load_snapshot(root.clone(), definitions.clone(), options, &path)
                    .err()
                    .unwrap()
                    .contains("snapshot checksum mismatch")
            );
            bytes.truncate(bytes.len() / 2);
            fs::write(&path, &bytes).unwrap();
            assert!(load_snapshot(root.clone(), definitions.clone(), options, &path).is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn startup_restores_unchanged_and_rebuilds_after_complete_file_changes() {
        let root = std::env::temp_dir().join(format!("objekte-reopen-{}", std::process::id()));
        fs::create_dir_all(root.join("data")).unwrap();
        let multi = root.join("data/multi.jsonl");
        fs::write(&multi, b"{\"x\":\"a\"}\n{\"x\":\"b\"}\n").unwrap();
        let mut schema = Schema::default();
        schema
            .register_field(FieldDefinition {
                path: vec!["x".into()],
                kind: LogicalType::String,
                required: true,
                nullable: false,
                indexed: true,
                multi: false,
            })
            .unwrap();
        let definitions = vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::JsonLines,
            identity: IdentityPolicy::Generated,
            schema,
            sort_fields: vec![FieldPath::parse("x").unwrap()],
            sort_locale: "de".into(),
        }];
        let options = RepositoryOptions::default();
        let fresh = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
        assert!(!fresh.startup.restored);
        assert_eq!(fresh.build_parses, 2);
        let restored = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
        assert!(restored.startup.restored);
        assert_eq!(restored.build_parses, 0);
        assert_eq!(restored.collections[0].generation.records.len(), 2);
        fs::write(&multi, b"{\"x\":\"a\"}\n{\"x\":\"b\"}\n{\"x\":\"c\"}\n").unwrap();
        let changed = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
        assert!(!changed.startup.restored);
        assert_eq!(changed.collections[0].generation.generation, 2);
        assert_eq!(changed.collections[0].generation.records.len(), 3);
        assert_eq!(
            changed.collections[0].generation.file_records.slots.len(),
            3
        );
        assert!(
            Repository::open_with_options(&root, definitions.clone(), options)
                .unwrap()
                .startup
                .restored
        );
        fs::write(root.join("data/extra.jsonl"), b"{\"x\":\"d\"}\n").unwrap();
        let added = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
        assert_eq!(added.collections[0].generation.generation, 3);
        assert_eq!(added.collections[0].generation.records.len(), 4);
        fs::remove_file(multi).unwrap();
        let deleted = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
        assert_eq!(deleted.collections[0].generation.generation, 4);
        assert_eq!(deleted.collections[0].generation.records.len(), 1);
        assert_eq!(deleted.files.files.len(), 1);
        let snapshot = snapshot_path(&root, BigramAlphabet::Ascii);
        let mut bytes = fs::read(&snapshot).unwrap();
        bytes[0] ^= 1;
        fs::write(&snapshot, bytes).unwrap();
        let rebuilt = Repository::open_with_options(&root, definitions, options).unwrap();
        assert!(!rebuilt.startup.restored);
        assert_eq!(rebuilt.collections[0].generation.records.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
