//! Append-only values text, current-record metadata, and per-shard typed order lists.
//! Source JSON remains authoritative. A versioned manifest publishes changes atomically.

use super::*;
use crate::query::{Operator, Scalar};
use fs2::FileExt;
use grep_searcher::{SearcherBuilder, sinks::Bytes};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    sync::atomic::{AtomicU64, Ordering},
};

mod sync;

const VERSION: u32 = 6;
const COMPACT_STALE_PERCENT: u64 = 25;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);


/// Current record metadata stored in one shard.
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Entry {
    pub stamp: Stamp,
    pub digest: String,
    pub revision: u64,
    pub changed_at_nanos: u128,
    pub line: usize,
    pub offset: u64,
    pub text_len: u64,
    pub keys: BTreeMap<String, Scalar>,
    pub array_fields: BTreeSet<String>,
}


/// One typed sort key and its record ID.
#[derive(Clone, Serialize, Deserialize)]
struct OrderedRow {
    key: Scalar,
    id: String,
}


/// Revision retained after a source record is deleted.
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Tombstone {
    revision: u64,
    changed_at_nanos: u128,
}


/// Values text map and current metadata for one shard.
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Shard {
    text_file: String,
    text_len: u64,
    live_bytes: u64,
    pub lines: Vec<Option<String>>,
    pub entries: BTreeMap<String, Entry>,
    pub tombstones: BTreeMap<String, Tombstone>,
    array_fields: BTreeSet<String>,
    ordered: BTreeMap<String, Vec<OrderedRow>>,
}


/// Published generation and collection compatibility metadata.
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Manifest {
    version: u32,
    pub schema_hash: String,
    pub mapping_hash: String,
    pub revision: i64,
    pub source_hash: String,
    generation: u64,
    shard_metadata: Vec<String>,
}


/// Locked shards from one published generation.
pub(super) struct Snapshot {
    pub manifest: Manifest,
    pub shards: Arc<Vec<Shard>>,
    directory: PathBuf,
    _lock: File,
}


impl Snapshot {
    pub fn entries(&self) -> impl Iterator<Item = (&String, &Entry)> {
        self.shards.iter().flat_map(|shard| shard.entries.iter())
    }

    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.shards[shard_number(id, self.shards.len())]
            .entries
            .get(id)
    }

    pub fn revision(&self, collection: &str, id: &str) -> Option<RecordRevision> {
        let shard = &self.shards[shard_number(id, self.shards.len())];
        if let Some(entry) = shard.entries.get(id) {
            return Some(RecordRevision {
                collection: collection.into(),
                id: id.into(),
                revision: entry.revision,
                changed_at_nanos: entry.changed_at_nanos,
                present: true,
            });
        }
        shard.tombstones.get(id).map(|entry| RecordRevision {
            collection: collection.into(),
            id: id.into(),
            revision: entry.revision,
            changed_at_nanos: entry.changed_at_nanos,
            present: false,
        })
    }

    pub fn count(&self) -> usize {
        self.shards.iter().map(|shard| shard.entries.len()).sum()
    }

    pub fn has_array_value(&self, field: &str) -> bool {
        self.shards
            .iter()
            .any(|shard| shard.array_fields.contains(field))
    }

    pub fn text_path(&self, shard: &Shard) -> PathBuf {
        self.directory.join(&shard.text_file)
    }

    pub fn ordered_ids(&self, field: &str, descending: bool) -> Option<Vec<String>> {
        let mut rows = self
            .shards
            .iter()
            .map(|shard| shard.ordered.get(field))
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        rows.sort_by(|a, b| a.key.cmp(&b.key).then_with(|| a.id.cmp(&b.id)));
        if descending {
            // States (valid/null/missing/invalid) retain ascending order. Only
            // values within the same state reverse, matching view ordering.
            rows.sort_by(|a, b| {
                a.key
                    .state()
                    .cmp(&b.key.state())
                    .then_with(|| b.key.cmp(&a.key).then_with(|| a.id.cmp(&b.id)))
            });
        }
        Some(rows.into_iter().map(|row| row.id.clone()).collect())
    }

    pub fn indexed_filter_ids(
        &self,
        field: &str,
        operator: Operator,
        rhs: &Scalar,
    ) -> Option<BTreeSet<String>> {
        let mut out = BTreeSet::new();
        for shard in self.shards.iter() {
            let rows = shard.ordered.get(field)?;
            if rhs.state() == 1 {
                if matches!(operator, Operator::Eq) {
                    out.extend(
                        rows.iter()
                            .filter(|row| row.key.state() == 1)
                            .map(|row| row.id.clone()),
                    );
                }
                continue;
            }
            let valid_end = rows.partition_point(|row| row.key.state() == 0);
            let rows = &rows[..valid_end];
            let lower = rows.partition_point(|row| row.key.cmp(rhs).is_lt());
            let upper = rows.partition_point(|row| !row.key.cmp(rhs).is_gt());
            let selected = match operator {
                Operator::Eq => &rows[lower..upper],
                Operator::Lt => &rows[..lower],
                Operator::Le => &rows[..upper],
                Operator::Gt => &rows[upper..],
                Operator::Ge => &rows[lower..],
            };
            out.extend(selected.iter().map(|row| row.id.clone()));
        }
        Some(out)
    }

    pub fn json_filter_ids(
        &self,
        field: &str,
        operator: Operator,
        value: &Value,
    ) -> Option<BTreeSet<String>> {
        let rhs = crate::query::scalar_value(Some(value));
        if rhs.state() != 0 {
            return None;
        }
        // Floating point ordering is monotone but can collapse nearby exact
        // decimals. Include the equal bucket for strict bounds, then verify
        // candidates using JsonFilter's exact decimal comparison.
        let widened = if value.is_number() {
            match operator {
                Operator::Lt => Operator::Le,
                Operator::Gt => Operator::Ge,
                other => other,
            }
        } else {
            operator
        };
        let mut ids = self.indexed_filter_ids(field, widened, &rhs)?;
        if value.is_number() {
            for shard in self.shards.iter() {
                ids.extend(
                    shard
                        .ordered
                        .get(field)?
                        .iter()
                        .filter(|row| row.key.state() == 3)
                        .map(|row| row.id.clone()),
                );
            }
        }
        Some(ids)
    }

    pub fn text_candidates(&self, matcher: &grep_regex::RegexMatcher) -> Result<Vec<String>> {
        let groups = self
            .shards
            .par_iter()
            .map_init(
                || SearcherBuilder::new().bom_sniffing(false).build(),
                |searcher, shard| -> Result<Vec<String>> {
                    let mut ids = Vec::new();
                    let path = self.text_path(shard);
                    searcher
                        .search_path(
                            matcher,
                            &path,
                            Bytes(|line, _| {
                                if let Some(Some(id)) = shard.lines.get((line - 1) as usize) {
                                    ids.push(id.clone());
                                }
                                Ok(true)
                            }),
                        )
                        .map_err(|e| io(&path, e))?;
                    Ok(ids)
                },
            )
            .collect::<Result<Vec<_>>>()?;
        let mut ids = groups.into_iter().flatten().collect::<Vec<_>>();
        ids.sort();
        ids.dedup();
        Ok(ids)
    }
}


fn shard_number(id: &str, count: usize) -> usize {
    let hash = id.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    (hash as usize) % count
}

fn cache_dir(repo: &Repository, c: &Collection) -> Result<PathBuf> {
    Ok(repo
        .cache_directory()?
        .join("segments")
        .join(digest(c.name.as_bytes())))
}

fn lock(directory: &Path, exclusive: bool) -> Result<File> {
    fs::create_dir_all(directory).map_err(|e| io(directory, e))?;
    let path = directory.join("LOCK");
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| io(&path, e))?;
    if exclusive {
        file.lock_exclusive().map_err(|e| io(&path, e))?;
    } else {
        FileExt::lock_shared(&file).map_err(|e| io(&path, e))?;
    }
    Ok(file)
}

fn read_json<T: for<'a> Deserialize<'a>>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).map_err(|e| io(path, e))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| Error::Cache(format!("invalid segment cache {}: {e}", path.display())))
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = File::create(&temp).map_err(|e| io(&temp, e))?;
    file.write_all(bytes).map_err(|e| io(&temp, e))?;
    file.sync_all().map_err(|e| io(&temp, e))?;
    fs::rename(&temp, path).map_err(|e| io(path, e))?;
    File::open(path.parent().unwrap())
        .map_err(|e| io(path, e))?
        .sync_all()
        .map_err(|e| io(path, e))?;
    Ok(())
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|e| Error::Cache(e.to_string()))?;
    write_bytes(path, &bytes)
}

fn read_bin<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).map_err(|e| io(path, e))?;
    let (value, consumed): (T, usize) =
        bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).map_err(|e| {
            Error::Cache(format!("invalid segment metadata {}: {e}", path.display()))
        })?;
    if consumed != bytes.len() {
        return Err(Error::Cache(format!(
            "trailing segment metadata bytes: {}",
            path.display()
        )));
    }
    Ok(value)
}

fn write_bin(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = bincode::serde::encode_to_vec(value, bincode::config::standard())
        .map_err(|e| Error::Cache(e.to_string()))?;
    write_bytes(path, &bytes)
}

fn load(directory: &Path, guard: File) -> Result<Option<Snapshot>> {
    let path = directory.join("CURRENT.json");
    if !path.exists() {
        return Ok(None);
    }
    let manifest: Manifest = read_json(&path)?;
    let shards = if manifest.version == VERSION {
        manifest
            .shard_metadata
            .par_iter()
            .map(|name| read_bin(&directory.join(name)))
            .collect::<Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    Ok(Some(Snapshot {
        manifest,
        shards: Arc::new(shards),
        directory: directory.to_owned(),
        _lock: guard,
    }))
}

fn ordered_fields(c: &Collection) -> Vec<String> {
    c.indexes.iter().map(|field| field.name.clone()).collect()
}

fn rebuild_ordered(shard: &mut Shard, fields: &[String]) {
    shard.ordered.clear();
    shard.array_fields = shard
        .entries
        .values()
        .flat_map(|entry| entry.array_fields.iter().cloned())
        .collect();
    for name in fields {
        let mut rows = shard
            .entries
            .iter()
            .map(|(id, entry)| OrderedRow {
                key: entry.keys.get(name).cloned().unwrap_or(Scalar::Missing),
                id: id.clone(),
            })
            .collect::<Vec<_>>();
        rows.sort_by(|a, b| a.key.cmp(&b.key).then_with(|| a.id.cmp(&b.id)));
        shard.ordered.insert(name.clone(), rows);
    }
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.as_nanos())
}
