//! Disposable, indexed NDJSON snapshots of file-backed collections.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use walkdir::WalkDir;

pub const SHARDS: usize = 64;
const VERSION: u32 = 3;

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid JSON at {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid repository configuration: {0}")]
    Config(String),
    #[error("invalid record at {path}: {message}")]
    Record { path: PathBuf, message: String },
    #[error("duplicate ID {id} in {first} and {second}")]
    Duplicate {
        id: String,
        first: String,
        second: String,
    },
}
pub type Result<T> = std::result::Result<T, CacheError>;

fn io(path: &Path, source: std::io::Error) -> CacheError {
    CacheError::Io {
        path: path.into(),
        source,
    }
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let file = File::open(path).map_err(|e| io(path, e))?;
    serde_json::from_reader(BufReader::new(file)).map_err(|e| CacheError::Json {
        path: path.into(),
        source: e,
    })
}
fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let file = File::create(path).map_err(|e| io(path, e))?;
    let mut out = BufWriter::new(file);
    serde_json::to_writer(&mut out, value).map_err(|e| CacheError::Json {
        path: path.into(),
        source: e,
    })?;
    out.flush().map_err(|e| io(path, e))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceStamp {
    pub bytes: u64,
    pub modified_ns: u64,
}
fn stamp(path: &Path) -> Result<SourceStamp> {
    let meta = fs::metadata(path).map_err(|e| io(path, e))?;
    let modified = meta
        .modified()
        .map_err(|e| io(path, e))?
        .duration_since(UNIX_EPOCH)
        .map_err(|e| CacheError::Record {
            path: path.into(),
            message: e.to_string(),
        })?;
    Ok(SourceStamp {
        bytes: meta.len(),
        modified_ns: modified
            .as_nanos()
            .try_into()
            .map_err(|_| CacheError::Record {
                path: path.into(),
                message: "timestamp overflow".into(),
            })?,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordLocation {
    pub path: String,
    pub shard: u8,
    pub offset: u64,
    pub length: u64,
    pub source: SourceStamp,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionIndex {
    pub version: u32,
    pub collection: String,
    pub format: String,
    pub source_dir: String,
    pub id_rule: String,
    pub body_field: String,
    pub records: BTreeMap<String, RecordLocation>,
}
#[derive(Serialize, Deserialize)]
struct IndexManifest {
    version: u32,
    collection: String,
    format: String,
    source_dir: String,
    id_rule: String,
    body_field: String,
}
#[derive(Serialize, Deserialize)]
pub struct CacheRow {
    // First key on every line, for inspection and cheap ID extraction.
    pub id: String,
    pub path: String,
    pub data: Value,
}
#[derive(Debug, Clone)]
pub struct CollectionCache {
    pub directory: PathBuf,
    pub generation: String,
    pub index: CollectionIndex,
}
#[derive(Debug, Clone)]
pub struct CacheGeneration {
    pub directory: PathBuf,
    pub generation: String,
}
impl CacheGeneration {
    pub fn files(&self) -> Vec<PathBuf> {
        (0..SHARDS)
            .map(|shard| {
                self.directory
                    .join(&self.generation)
                    .join(format!("{shard:02x}.ndjson"))
            })
            .collect()
    }
}
impl CollectionCache {
    pub fn shard_path(&self, shard: usize) -> PathBuf {
        self.directory
            .join(&self.generation)
            .join(format!("{shard:02x}.ndjson"))
    }
    pub fn files(&self) -> Vec<PathBuf> {
        (0..SHARDS).map(|shard| self.shard_path(shard)).collect()
    }
    pub fn read_record(&self, id: &str) -> Result<Option<CacheRow>> {
        use std::io::{Read, Seek, SeekFrom};
        let Some(loc) = self.index.records.get(id) else {
            return Ok(None);
        };
        let path = self.shard_path(loc.shard as usize);
        let mut file = File::open(&path).map_err(|e| io(&path, e))?;
        file.seek(SeekFrom::Start(loc.offset))
            .map_err(|e| io(&path, e))?;
        let mut bytes = vec![0; loc.length as usize];
        file.read_exact(&mut bytes).map_err(|e| io(&path, e))?;
        let row: CacheRow = serde_json::from_slice(&bytes).map_err(|e| CacheError::Json {
            path: path.clone(),
            source: e,
        })?;
        if row.id != id {
            return Err(CacheError::Record {
                path,
                message: "cache index points to the wrong ID".into(),
            });
        }
        Ok(Some(row))
    }
}

#[derive(Deserialize)]
struct RepositoryFile {
    collections: BTreeMap<String, String>,
}
#[derive(Deserialize)]
struct CollectionFile {
    #[serde(rename = "type")]
    kind: String,
    path: String,
    format: Option<String>,
    id: Option<String>,
    content: Option<ContentMapping>,
}
#[derive(Deserialize)]
struct ContentMapping {
    body: Option<String>,
}
struct CollectionSpec {
    name: String,
    path: PathBuf,
    format: String,
    id: String,
    body: String,
}
fn spec(root: &Path, name: &str) -> Result<CollectionSpec> {
    let repo_path = root.join(".editor/repository.json");
    let repo: RepositoryFile = read_json(&repo_path)?;
    let settings = repo
        .collections
        .get(name)
        .ok_or_else(|| CacheError::Config(format!("unknown collection {name}")))?;
    let cfg: CollectionFile = read_json(&root.join(settings))?;
    if cfg.kind != "data" {
        return Err(CacheError::Config(format!(
            "{name} is not a data collection"
        )));
    }
    let format = cfg.format.unwrap_or_else(|| "json".into());
    if format != "json" && format != "markdown" {
        return Err(CacheError::Config(format!("unsupported format {format}")));
    }
    let id = cfg.id.unwrap_or_else(|| "filename".into());
    if id != "filename" && !id.starts_with("frontmatter.") && !id.starts_with("data.") {
        return Err(CacheError::Config(format!("unsupported ID mapping {id}")));
    }
    Ok(CollectionSpec {
        name: name.into(),
        path: root.join(cfg.path),
        format,
        id,
        body: cfg
            .content
            .and_then(|c| c.body)
            .unwrap_or_else(|| "body".into()),
    })
}
fn paths(spec: &CollectionSpec) -> Result<Vec<PathBuf>> {
    let extension = if spec.format == "json" { "json" } else { "md" };
    let mut files = Vec::new();
    for entry in WalkDir::new(&spec.path).follow_links(false) {
        let entry = entry.map_err(|e| CacheError::Config(e.to_string()))?;
        if entry.file_type().is_file()
            && entry.path().extension().is_some_and(|ext| ext == extension)
        {
            files.push(entry.into_path());
        }
    }
    files.sort();
    Ok(files)
}
fn parse_record(spec: &CollectionSpec, path: &Path) -> Result<(String, Value)> {
    let data: Value = if spec.format == "json" {
        read_json(path)?
    } else {
        let source = fs::read_to_string(path).map_err(|e| io(path, e))?;
        let rest = source
            .strip_prefix("---\n")
            .ok_or_else(|| CacheError::Record {
                path: path.into(),
                message: "missing YAML frontmatter".into(),
            })?;
        let (yaml, body) = rest
            .split_once("\n---\n")
            .ok_or_else(|| CacheError::Record {
                path: path.into(),
                message: "unclosed YAML frontmatter".into(),
            })?;
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(yaml).map_err(|e| CacheError::Record {
                path: path.into(),
                message: e.to_string(),
            })?;
        let mut value = serde_json::to_value(yaml).map_err(|e| CacheError::Json {
            path: path.into(),
            source: e,
        })?;
        let obj = value.as_object_mut().ok_or_else(|| CacheError::Record {
            path: path.into(),
            message: "frontmatter must be an object".into(),
        })?;
        obj.insert(spec.body.clone(), Value::String(body.to_owned()));
        value
    };
    let id = if spec.id == "filename" {
        path.file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| CacheError::Record {
                path: path.into(),
                message: "invalid filename ID".into(),
            })?
            .to_owned()
    } else {
        let field = spec.id.split_once('.').unwrap().1;
        data.get(field)
            .and_then(Value::as_str)
            .ok_or_else(|| CacheError::Record {
                path: path.into(),
                message: format!("missing string ID field {field}"),
            })?
            .to_owned()
    };
    if id.is_empty() {
        return Err(CacheError::Record {
            path: path.into(),
            message: "empty ID".into(),
        });
    }
    Ok((id, data))
}
fn parse_stable_record(spec: &CollectionSpec, path: &Path) -> Result<(String, Value, SourceStamp)> {
    let before = stamp(path)?;
    let (id, data) = parse_record(spec, path)?;
    let after = stamp(path)?;
    if before != after {
        return Err(CacheError::Record {
            path: path.into(),
            message: "source changed during cache construction; retry".into(),
        });
    }
    Ok((id, data, after))
}
fn rel(root: &Path, path: &Path) -> Result<String> {
    path.strip_prefix(root)
        .map_err(|e| CacheError::Config(e.to_string()))?
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| CacheError::Record {
            path: path.into(),
            message: "non-UTF-8 path".into(),
        })
}
fn shard(id: &str) -> usize {
    let mut h = 0xcbf29ce484222325_u64;
    for b in id.as_bytes() {
        h = (h ^ u64::from(*b)).wrapping_mul(0x100000001b3);
    }
    (h as usize) % SHARDS
}
fn generation() -> String {
    format!(
        "gen-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        std::process::id()
    )
}
fn collection_dir(root: &Path, cache_root: &Path, name: &str) -> Result<PathBuf> {
    let canonical = fs::canonicalize(root).map_err(|e| io(root, e))?;
    let mut h = 0xcbf29ce484222325_u64;
    for b in canonical.as_os_str().as_encoded_bytes() {
        h = (h ^ u64::from(*b)).wrapping_mul(0x100000001b3);
    }
    Ok(cache_root.join(format!("{h:016x}")).join(name))
}
pub fn default_cache_root() -> PathBuf {
    if let Some(path) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(path).join("objekte");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".cache/objekte");
    }
    std::env::temp_dir().join("objekte")
}
fn publish(directory: &Path, generation: &str) -> Result<()> {
    let pointer = directory.join("CURRENT");
    let temp = directory.join(format!("CURRENT.{}.tmp", std::process::id()));
    fs::write(&temp, generation).map_err(|e| io(&temp, e))?;
    fs::rename(&temp, &pointer).map_err(|e| io(&pointer, e))
}
fn shard_index_path(generation_dir: &Path, shard: usize) -> PathBuf {
    generation_dir.join(format!("{shard:02x}.index.json"))
}
fn write_shard_index(generation_dir: &Path, shard: usize, index: &CollectionIndex) -> Result<()> {
    let records: BTreeMap<&str, &RecordLocation> = index
        .records
        .iter()
        .filter(|(_, loc)| loc.shard as usize == shard)
        .map(|(id, loc)| (id.as_str(), loc))
        .collect();
    write_json(&shard_index_path(generation_dir, shard), &records)
}
fn write_manifest(generation_dir: &Path, index: &CollectionIndex) -> Result<()> {
    write_json(
        &generation_dir.join("manifest.json"),
        &IndexManifest {
            version: index.version,
            collection: index.collection.clone(),
            format: index.format.clone(),
            source_dir: index.source_dir.clone(),
            id_rule: index.id_rule.clone(),
            body_field: index.body_field.clone(),
        },
    )
}
fn open_manifest(
    root: &Path,
    name: &str,
    cache_root: &Path,
) -> Result<(CacheGeneration, IndexManifest)> {
    let spec = spec(root, name)?;
    let directory = collection_dir(root, cache_root, name)?;
    let pointer = directory.join("CURRENT");
    let generation = fs::read_to_string(&pointer).map_err(|e| io(&pointer, e))?;
    let gen_dir = directory.join(&generation);
    let manifest: IndexManifest = read_json(&gen_dir.join("manifest.json"))?;
    if manifest.version != VERSION
        || manifest.collection != name
        || manifest.format != spec.format
        || manifest.source_dir != rel(root, &spec.path)?
        || manifest.id_rule != spec.id
        || manifest.body_field != spec.body
    {
        return Err(CacheError::Config(
            "incompatible cache or changed collection configuration; rebuild required".into(),
        ));
    }
    Ok((
        CacheGeneration {
            directory,
            generation,
        },
        manifest,
    ))
}
/// Open the published files for a collection scan without loading the ID index.
pub fn open_generation(root: &Path, name: &str, cache_root: &Path) -> Result<CacheGeneration> {
    open_manifest(root, name, cache_root).map(|(generation, _)| generation)
}
/// Open the published files and load their ID index for targeted record access.
pub fn open_collection(root: &Path, name: &str, cache_root: &Path) -> Result<CollectionCache> {
    let (snapshot, manifest) = open_manifest(root, name, cache_root)?;
    let directory = snapshot.directory;
    let generation = snapshot.generation;
    let gen_dir = directory.join(&generation);
    let mut records = BTreeMap::new();
    for shard in 0..SHARDS {
        let part: BTreeMap<String, RecordLocation> = read_json(&shard_index_path(&gen_dir, shard))?;
        for (id, loc) in part {
            if loc.shard as usize != shard || records.insert(id, loc).is_some() {
                return Err(CacheError::Config("invalid cache shard index".into()));
            }
        }
    }
    let index = CollectionIndex {
        version: manifest.version,
        collection: manifest.collection,
        format: manifest.format,
        source_dir: manifest.source_dir,
        id_rule: manifest.id_rule,
        body_field: manifest.body_field,
        records,
    };
    Ok(CollectionCache {
        directory,
        generation,
        index,
    })
}
pub fn build_collection(root: &Path, name: &str, cache_root: &Path) -> Result<CollectionCache> {
    let spec = spec(root, name)?;
    let directory = collection_dir(root, cache_root, name)?;
    fs::create_dir_all(&directory).map_err(|e| io(&directory, e))?;
    let generation = generation();
    let gen_dir = directory.join(&generation);
    fs::create_dir(&gen_dir).map_err(|e| io(&gen_dir, e))?;
    let build = (|| {
        let mut writers = Vec::with_capacity(SHARDS);
        let mut positions = vec![0_u64; SHARDS];
        for n in 0..SHARDS {
            let path = gen_dir.join(format!("{n:02x}.ndjson"));
            writers.push(BufWriter::new(
                File::create(&path).map_err(|e| io(&path, e))?,
            ));
        }
        let mut records = BTreeMap::new();
        for path in paths(&spec)? {
            let (id, data, source) = parse_stable_record(&spec, &path)?;
            let relative = rel(root, &path)?;
            if let Some(first) = records
                .get(&id)
                .map(|loc: &RecordLocation| loc.path.clone())
            {
                return Err(CacheError::Duplicate {
                    id,
                    first,
                    second: relative,
                });
            }
            let row = CacheRow {
                id: id.clone(),
                path: relative.clone(),
                data,
            };
            let mut bytes = serde_json::to_vec(&row).map_err(|e| CacheError::Json {
                path: path.clone(),
                source: e,
            })?;
            bytes.push(b'\n');
            let n = shard(&id);
            writers[n].write_all(&bytes).map_err(|e| io(&gen_dir, e))?;
            records.insert(
                id,
                RecordLocation {
                    path: relative,
                    shard: n as u8,
                    offset: positions[n],
                    length: bytes.len() as u64,
                    source,
                },
            );
            positions[n] += bytes.len() as u64;
        }
        for (n, writer) in writers.iter_mut().enumerate() {
            writer
                .flush()
                .map_err(|e| io(&gen_dir.join(format!("{n:02x}.ndjson")), e))?
        }
        let index = CollectionIndex {
            version: VERSION,
            collection: spec.name,
            format: spec.format,
            source_dir: rel(root, &spec.path)?,
            id_rule: spec.id,
            body_field: spec.body,
            records,
        };
        for shard in 0..SHARDS {
            write_shard_index(&gen_dir, shard, &index)?;
        }
        write_manifest(&gen_dir, &index)?;
        Ok(index)
    })();
    match build {
        Ok(index) => {
            publish(&directory, &generation)?;
            Ok(CollectionCache {
                directory,
                generation,
                index,
            })
        }
        Err(error) => {
            let _ = fs::remove_dir_all(&gen_dir);
            Err(error)
        }
    }
}

/// Refresh one known record by rewriting only its deterministic shard.
/// A changed ID or newly added record requires a full build in this first version.
pub fn refresh_record(
    root: &Path,
    name: &str,
    cache_root: &Path,
    id: &str,
) -> Result<CacheGeneration> {
    let spec = spec(root, name)?;
    let directory = collection_dir(root, cache_root, name)?;
    let pointer = directory.join("CURRENT");
    let old_generation = fs::read_to_string(&pointer).map_err(|e| io(&pointer, e))?;
    let old_dir = directory.join(&old_generation);
    let manifest: IndexManifest = read_json(&old_dir.join("manifest.json"))?;
    if manifest.version != VERSION
        || manifest.collection != name
        || manifest.format != spec.format
        || manifest.source_dir != rel(root, &spec.path)?
        || manifest.id_rule != spec.id
        || manifest.body_field != spec.body
    {
        return Err(CacheError::Config(
            "collection configuration changed; rebuild required".into(),
        ));
    }
    let changed = shard(id);
    let mut records: BTreeMap<String, RecordLocation> =
        read_json(&shard_index_path(&old_dir, changed))?;
    let loc = records
        .get(id)
        .cloned()
        .ok_or_else(|| CacheError::Config(format!("unknown cached ID {id}; rebuild required")))?;
    if loc.shard as usize != changed {
        return Err(CacheError::Config("invalid cache shard index".into()));
    }
    let source_path = root.join(&loc.path);
    let replacement = if source_path.exists() {
        let (new_id, data, source) = parse_stable_record(&spec, &source_path)?;
        if new_id != id {
            return Err(CacheError::Config(
                "record ID changed; rebuild required".into(),
            ));
        }
        Some((data, source))
    } else {
        None
    };
    let generation = generation();
    let gen_dir = directory.join(&generation);
    fs::create_dir(&gen_dir).map_err(|e| io(&gen_dir, e))?;
    let build = (|| {
        for n in 0..SHARDS {
            if n != changed {
                let target = gen_dir.join(format!("{n:02x}.ndjson"));
                fs::hard_link(old_dir.join(format!("{n:02x}.ndjson")), &target)
                    .map_err(|e| io(&target, e))?;
                let target_index = shard_index_path(&gen_dir, n);
                fs::hard_link(shard_index_path(&old_dir, n), &target_index)
                    .map_err(|e| io(&target_index, e))?;
            }
        }
        let target_manifest = gen_dir.join("manifest.json");
        fs::hard_link(old_dir.join("manifest.json"), &target_manifest)
            .map_err(|e| io(&target_manifest, e))?;
        let input_path = old_dir.join(format!("{changed:02x}.ndjson"));
        let output_path = gen_dir.join(format!("{changed:02x}.ndjson"));
        let mut writer =
            BufWriter::new(File::create(&output_path).map_err(|e| io(&output_path, e))?);
        let mut offset = 0_u64;
        let mut found_target = false;
        for line in
            BufReader::new(File::open(&input_path).map_err(|e| io(&input_path, e))?).split(b'\n')
        {
            let mut line = line.map_err(|e| io(&input_path, e))?;
            if line.is_empty() {
                continue;
            }
            let row: CacheRow = serde_json::from_slice(&line).map_err(|e| CacheError::Json {
                path: input_path.clone(),
                source: e,
            })?;
            if row.id == id {
                found_target = true;
                if let Some((data, source)) = &replacement {
                    let updated = CacheRow {
                        id: id.into(),
                        path: loc.path.clone(),
                        data: data.clone(),
                    };
                    let mut bytes = serde_json::to_vec(&updated).map_err(|e| CacheError::Json {
                        path: output_path.clone(),
                        source: e,
                    })?;
                    bytes.push(b'\n');
                    writer.write_all(&bytes).map_err(|e| io(&output_path, e))?;
                    let entry = records.get_mut(id).expect("target is indexed");
                    entry.offset = offset;
                    entry.length = bytes.len() as u64;
                    entry.source = source.clone();
                    offset += bytes.len() as u64;
                } else {
                    records.remove(id);
                }
                continue;
            }
            line.push(b'\n');
            writer.write_all(&line).map_err(|e| io(&output_path, e))?;
            let entry = records
                .get_mut(&row.id)
                .ok_or_else(|| CacheError::Config("cache row missing from index".into()))?;
            entry.offset = offset;
            entry.length = line.len() as u64;
            offset += line.len() as u64;
        }
        if !found_target {
            return Err(CacheError::Config(
                "indexed record is absent from its shard".into(),
            ));
        }
        writer.flush().map_err(|e| io(&output_path, e))?;
        write_json(&shard_index_path(&gen_dir, changed), &records)?;
        Ok(())
    })();
    match build {
        Ok(()) => {
            publish(&directory, &generation)?;
            Ok(CacheGeneration {
                directory,
                generation,
            })
        }
        Err(error) => {
            let _ = fs::remove_dir_all(&gen_dir);
            Err(error)
        }
    }
}

/// Compare the current source inventory with the published index. This does not
/// rebuild the snapshot and is intentionally measured separately from queries.
pub fn detect_changes(root: &Path, name: &str, cache_root: &Path) -> Result<Vec<String>> {
    let spec = spec(root, name)?;
    let cache = open_collection(root, name, cache_root)?;
    if cache.index.format != spec.format
        || cache.index.source_dir != rel(root, &spec.path)?
        || cache.index.id_rule != spec.id
        || cache.index.body_field != spec.body
    {
        return Err(CacheError::Config(
            "collection configuration changed; rebuild required".into(),
        ));
    }
    let by_path: std::collections::HashMap<&str, &SourceStamp> = cache
        .index
        .records
        .values()
        .map(|loc| (loc.path.as_str(), &loc.source))
        .collect();
    let mut found = HashSet::new();
    let mut changed = Vec::new();
    for path in paths(&spec)? {
        let relative = rel(root, &path)?;
        found.insert(relative.clone());
        if match by_path.get(relative.as_str()) {
            Some(old) => &stamp(&path)? != *old,
            None => true,
        } {
            changed.push(relative)
        }
    }
    for loc in cache.index.records.values() {
        if !found.contains(&loc.path) {
            changed.push(loc.path.clone())
        }
    }
    changed.sort();
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository(root: &Path, format: &str, id: &str) {
        fs::create_dir_all(root.join(".editor/collections/items")).unwrap();
        fs::create_dir_all(root.join("data/items")).unwrap();
        fs::write(
            root.join(".editor/repository.json"),
            r#"{"collections":{"items":".editor/collections/items/settings.json"}}"#,
        )
        .unwrap();
        fs::write(root.join(".editor/collections/items/settings.json"), format!(r#"{{"type":"data","path":"data/items","format":"{format}","id":"{id}","content":{{"body":"body"}}}}"#)).unwrap();
    }

    #[test]
    fn build_index_refresh_one_shard_and_detect_external_changes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repository");
        let cache_root = temp.path().join("cache");
        repository(&root, "json", "filename");
        let a = root.join("data/items/a.json");
        let b = root.join("data/items/b.json");
        fs::write(&a, r#"{"name":"A"}"#).unwrap();
        fs::write(&b, r#"{"name":"B"}"#).unwrap();
        let cache = build_collection(&root, "items", &cache_root).unwrap();
        assert_eq!(cache.index.records.len(), 2);
        let a_location = &cache.index.records["a"];
        let a_shard = fs::read_to_string(cache.shard_path(a_location.shard as usize)).unwrap();
        assert!(a_shard[a_location.offset as usize..].starts_with("{\"id\":\"a\","));
        assert_eq!(
            open_generation(&root, "items", &cache_root)
                .unwrap()
                .generation,
            cache.generation
        );
        assert_eq!(cache.read_record("a").unwrap().unwrap().data["name"], "A");
        assert!(
            detect_changes(&root, "items", &cache_root)
                .unwrap()
                .is_empty()
        );
        fs::write(&a, r#"{"name":"Changed A"}"#).unwrap();
        assert_eq!(
            detect_changes(&root, "items", &cache_root).unwrap(),
            vec!["data/items/a.json"]
        );
        refresh_record(&root, "items", &cache_root, "a").unwrap();
        let updated = open_collection(&root, "items", &cache_root).unwrap();
        assert_eq!(
            updated.read_record("a").unwrap().unwrap().data["name"],
            "Changed A"
        );
        assert_eq!(cache.read_record("a").unwrap().unwrap().data["name"], "A");
        assert!(
            detect_changes(&root, "items", &cache_root)
                .unwrap()
                .is_empty()
        );
        fs::remove_file(&b).unwrap();
        refresh_record(&root, "items", &cache_root, "b").unwrap();
        let updated = open_collection(&root, "items", &cache_root).unwrap();
        assert!(updated.read_record("b").unwrap().is_none());
    }

    #[test]
    fn markdown_frontmatter_is_queryable_json() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repository");
        repository(&root, "markdown", "frontmatter.id");
        fs::write(
            root.join("data/items/article.md"),
            "---\nid: welcome\ntitle: Hello\n---\n# Body\n",
        )
        .unwrap();
        let cache = build_collection(&root, "items", &temp.path().join("cache")).unwrap();
        let row = cache.read_record("welcome").unwrap().unwrap();
        assert_eq!(row.data["title"], "Hello");
        assert_eq!(row.data["body"], "# Body\n");
    }

    #[test]
    fn changed_row_updates_offsets_of_later_rows_in_same_shard() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repository");
        repository(&root, "json", "filename");
        let first = "a";
        let second = (0..1000)
            .map(|n| format!("item{n}"))
            .find(|id| shard(id) == shard(first))
            .unwrap();
        fs::write(root.join("data/items/a.json"), r#"{"value":"short"}"#).unwrap();
        fs::write(
            root.join(format!("data/items/{second}.json")),
            r#"{"value":"second"}"#,
        )
        .unwrap();
        let cache_root = temp.path().join("cache");
        let original = build_collection(&root, "items", &cache_root).unwrap();
        let original_second_offset = original.index.records[&second].offset;
        fs::write(
            root.join("data/items/a.json"),
            format!(r#"{{"value":"{}"}}"#, "long".repeat(100)),
        )
        .unwrap();
        refresh_record(&root, "items", &cache_root, first).unwrap();
        let updated = open_collection(&root, "items", &cache_root).unwrap();
        assert!(updated.index.records[&second].offset > original_second_offset);
        assert_eq!(
            updated.read_record(&second).unwrap().unwrap().data["value"],
            "second"
        );
    }

    #[test]
    fn failed_rebuild_keeps_last_published_generation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repository");
        repository(&root, "json", "filename");
        let path = root.join("data/items/a.json");
        fs::write(&path, r#"{"value":"valid"}"#).unwrap();
        let cache_root = temp.path().join("cache");
        let first = build_collection(&root, "items", &cache_root).unwrap();
        fs::write(&path, "{invalid").unwrap();
        assert!(build_collection(&root, "items", &cache_root).is_err());
        let published = open_collection(&root, "items", &cache_root).unwrap();
        assert_eq!(published.generation, first.generation);
        assert_eq!(
            published.read_record("a").unwrap().unwrap().data["value"],
            "valid"
        );
    }
}
