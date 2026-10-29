//! Measure a persistent SQLite JSONB cache built directly from JSON source files.
use rusqlite::{Connection, params};
use serde_json::Value;
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Instant, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn Error>>;


// INFO: Source record ID and absolute and relative paths for cache benchmarks.
#[derive(Clone)]
struct Source {
    id: String,
    path: PathBuf,
    relative: String,
}


// INFO: Record ID and optional source for a benchmark cache update.
struct Change {
    id: String,
    source: Option<Source>,
}


fn elapsed(label: &str, started: Instant) {
    println!("{label:31} {:>10.3} s", started.elapsed().as_secs_f64());
}

fn synthetic(root: &Path, count: usize) -> Result<()> {
    fs::create_dir_all(root.join(".editor/collections/items"))?;
    fs::create_dir_all(root.join("data/items"))?;
    fs::write(
        root.join(".editor/repository.json"),
        r#"{"collections":{"items":".editor/collections/items/settings.json"}}"#,
    )?;
    fs::write(
        root.join(".editor/collections/items/settings.json"),
        r#"{"type":"data","path":"data/items","format":"json","id":"filename"}"#,
    )?;
    for n in 0..count {
        let id = format!("{n:08}");
        let folder = root.join("data/items").join(format!("{:02x}", n % 256));
        if n < 256 {
            fs::create_dir_all(&folder)?;
        }
        let mut file = fs::File::create(folder.join(format!("{id}.json")))?;
        write!(
            file,
            "{{\"title\":\"Record {n}\",\"year\":{},\"active\":{}}}",
            1600 + n % 500,
            n % 3 == 0
        )?;
    }
    Ok(())
}

fn simulated_edit(source: &Source, folder: &Path, version: i64) -> Result<Source> {
    fs::create_dir_all(folder)?;
    let mut data: Value = serde_json::from_slice(&fs::read(&source.path)?)?;
    data.as_object_mut()
        .ok_or("benchmark source must be a JSON object")?
        .insert("_objekte_benchmark_version".into(), Value::from(version));
    let path = folder.join(format!("{}.json", source.id));
    fs::write(&path, serde_json::to_vec(&data)?)?;
    Ok(Source {
        id: source.id.clone(),
        path,
        relative: source.relative.clone(),
    })
}

fn collection_path(root: &Path, name: &str) -> Result<PathBuf> {
    let repo: Value = serde_json::from_slice(&fs::read(root.join(".editor/repository.json"))?)?;
    let settings = repo["collections"][name]
        .as_str()
        .ok_or("collection is not registered")?;
    let config: Value = serde_json::from_slice(&fs::read(root.join(settings))?)?;
    if config["type"] != "data" || config["format"] != "json" || config["id"] != "filename" {
        return Err("benchmark requires a JSON data collection with filename IDs".into());
    }
    let path = config["path"]
        .as_str()
        .ok_or("collection path is missing")?;
    Ok(root.join(path))
}

fn discover(root: &Path, source_dir: &Path) -> Result<Vec<Source>> {
    let mut pending = vec![source_dir.to_path_buf()];
    let mut sources = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() && entry.path().extension().is_some_and(|e| e == "json") {
                let path = entry.path();
                let id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or("source filename is not UTF-8")?
                    .to_owned();
                let relative = path
                    .strip_prefix(root)?
                    .to_str()
                    .ok_or("source path is not UTF-8")?
                    .to_owned();
                sources.push(Source { id, path, relative });
            }
        }
    }
    sources.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(sources)
}

fn stamp(path: &Path) -> Result<(i64, i64)> {
    let metadata = fs::metadata(path)?;
    let bytes = i64::try_from(metadata.len())?;
    let modified = metadata.modified()?.duration_since(UNIX_EPOCH)?;
    let modified_ns = i64::try_from(modified.as_nanos())?;
    Ok((bytes, modified_ns))
}

fn open_database(path: &Path, cache_mib: i64) -> Result<Connection> {
    let started = Instant::now();
    let connection = Connection::open(path)?;
    elapsed("SQLite connection open", started);
    let started = Instant::now();
    connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
    elapsed("set WAL and sync mode", started);
    let started = Instant::now();
    connection.pragma_update(None, "cache_size", -(cache_mib * 1024))?;
    elapsed("configure page cache", started);
    Ok(connection)
}

fn build_database(
    root: &Path,
    source_dir: &Path,
    path: &Path,
    cache_mib: i64,
) -> Result<Connection> {
    let started = Instant::now();
    let mut connection = open_database(path, cache_mib)?;
    elapsed("open and configure SQLite", started);
    let started = Instant::now();
    connection.execute_batch(
        "CREATE TABLE records(id TEXT PRIMARY KEY, data BLOB NOT NULL);
         CREATE TABLE sources(id TEXT PRIMARY KEY, path TEXT NOT NULL, bytes INTEGER NOT NULL, modified_ns INTEGER NOT NULL);",
    )?;
    elapsed("create cache tables", started);
    let started = Instant::now();
    let files = discover(root, source_dir)?;
    elapsed("discover source files", started);
    println!("source files: {}", files.len());
    let started = Instant::now();
    let transaction = connection.transaction()?;
    {
        let mut record =
            transaction.prepare("INSERT INTO records(id,data) VALUES (?1,jsonb(?2))")?;
        let mut source = transaction
            .prepare("INSERT INTO sources(id,path,bytes,modified_ns) VALUES (?1,?2,?3,?4)")?;
        for file in files {
            let before = stamp(&file.path)?;
            let json = fs::read_to_string(&file.path)?;
            let after = stamp(&file.path)?;
            if before != after {
                return Err(format!("source changed during build: {}", file.path.display()).into());
            }
            record.execute(params![file.id, json])?;
            source.execute(params![file.id, file.relative, after.0, after.1])?;
        }
    }
    transaction.commit()?;
    elapsed("insert JSONB + source stamps", started);
    Ok(connection)
}

fn checkpoint(connection: &Connection) -> Result<()> {
    let (_busy, _written, _checkpointed): (i64, i64, i64) =
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    Ok(())
}

fn size(path: &Path) -> Result<u64> {
    Ok(fs::metadata(path)?.len())
}

fn verify_record_count(connection: &Connection, expected: usize) -> Result<()> {
    let count: i64 = connection.query_row("SELECT count(*) FROM records", [], |row| row.get(0))?;
    if count != i64::try_from(expected)? {
        return Err(format!("expected {expected} cached records, found {count}").into());
    }
    Ok(())
}

fn wal_size(path: &Path) -> u64 {
    fs::metadata(format!("{}-wal", path.display()))
        .map(|m| m.len())
        .unwrap_or(0)
}

fn scan_changes(connection: &Connection, root: &Path, source_dir: &Path) -> Result<Vec<Change>> {
    let mut known = HashMap::new();
    let mut statement = connection.prepare("SELECT id,path,bytes,modified_ns FROM sources")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let path: String = row.get(1)?;
        let bytes: i64 = row.get(2)?;
        let modified_ns: i64 = row.get(3)?;
        known.insert(id, (path, bytes, modified_ns));
    }
    let mut changes = Vec::new();
    for source in discover(root, source_dir)? {
        let current = stamp(&source.path)?;
        if !matches!(
            known.remove(&source.id),
            Some((path, bytes, modified_ns))
                if path == source.relative && (bytes, modified_ns) == current
        ) {
            changes.push(Change {
                id: source.id.clone(),
                source: Some(source),
            });
        }
    }
    for (id, _) in known {
        changes.push(Change { id, source: None });
    }
    Ok(changes)
}

fn refresh(connection: &mut Connection, changes: &[Change]) -> Result<()> {
    let transaction = connection.transaction()?;
    {
        let mut record = transaction.prepare(
            "INSERT INTO records(id,data) VALUES (?1,jsonb(?2))
             ON CONFLICT(id) DO UPDATE SET data=excluded.data",
        )?;
        let mut source = transaction.prepare(
            "INSERT INTO sources(id,path,bytes,modified_ns) VALUES (?1,?2,?3,?4)
             ON CONFLICT(id) DO UPDATE SET path=excluded.path,bytes=excluded.bytes,modified_ns=excluded.modified_ns",
        )?;
        let mut delete_record = transaction.prepare("DELETE FROM records WHERE id=?1")?;
        let mut delete_source = transaction.prepare("DELETE FROM sources WHERE id=?1")?;
        for change in changes {
            if let Some(file) = &change.source {
                let before = stamp(&file.path)?;
                let json = fs::read_to_string(&file.path)?;
                let after = stamp(&file.path)?;
                if before != after {
                    return Err(
                        format!("source changed during refresh: {}", file.path.display()).into(),
                    );
                }
                record.execute(params![file.id, json])?;
                source.execute(params![file.id, file.relative, after.0, after.1])?;
            } else {
                delete_record.execute([&change.id])?;
                delete_source.execute([&change.id])?;
            }
        }
    }
    transaction.commit()?;
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut repository = None;
    let mut collection = None;
    let mut synthetic_count = None;
    let mut work_dir = PathBuf::from("target");
    let mut cache_mib = 256_i64;
    let mut batch = 1_000_usize;
    let mut i = 0;
    while i < args.len() {
        let key = &args[i];
        if key == "--help" {
            println!(
                "Usage: sqlite-cache-bench [--synthetic N | --repo PATH --collection NAME] [--work-dir PATH] [--cache-mib N] [--batch N]"
            );
            return Ok(());
        }
        i += 1;
        let value = args.get(i).ok_or(format!("missing value for {key}"))?;
        match key.as_str() {
            "--synthetic" => synthetic_count = Some(value.parse::<usize>()?),
            "--repo" => repository = Some(PathBuf::from(value)),
            "--collection" => collection = Some(value.clone()),
            "--work-dir" => work_dir = PathBuf::from(value),
            "--cache-mib" => cache_mib = value.parse::<i64>()?,
            "--batch" => batch = value.parse::<usize>()?,
            _ => return Err(format!("unknown argument {key}").into()),
        }
        i += 1;
    }
    if cache_mib <= 0 {
        return Err("--cache-mib must be positive".into());
    }
    let temp = tempfile::Builder::new()
        .prefix("objekte-sqlite-bench-")
        .tempdir_in(work_dir)?;
    let generated = synthetic_count.is_some();
    let (root, name) = if let Some(count) = synthetic_count {
        let root = temp.path().join("synthetic");
        let started = Instant::now();
        synthetic(&root, count)?;
        elapsed("generate source files", started);
        (root, "items".to_owned())
    } else {
        (
            repository.ok_or("--repo required")?,
            collection.ok_or("--collection required")?,
        )
    };
    let source_dir = collection_path(&root, &name)?;
    let files = discover(&root, &source_dir)?;
    if files.is_empty() {
        return Err("collection is empty".into());
    }
    println!("repository: {}; collection: {name}", root.display());
    println!(
        "SQLite version: {}; WAL; synchronous=NORMAL; cache={} MiB",
        rusqlite::version(),
        cache_mib
    );

    let cold_path = temp.path().join("cold.db");
    let started = Instant::now();
    let cold = build_database(&root, &source_dir, &cold_path, cache_mib)?;
    verify_record_count(&cold, files.len())?;
    let checkpoint_started = Instant::now();
    checkpoint(&cold)?;
    elapsed("cold WAL checkpoint", checkpoint_started);
    elapsed("cold build + checkpoint", started);
    println!("cold DB bytes: {}", size(&cold_path)?);
    drop(cold);

    let rebuild_path = temp.path().join("rebuild.db");
    let started = Instant::now();
    let mut connection = build_database(&root, &source_dir, &rebuild_path, cache_mib)?;
    verify_record_count(&connection, files.len())?;
    let checkpoint_started = Instant::now();
    checkpoint(&connection)?;
    elapsed("rebuild WAL checkpoint", checkpoint_started);
    elapsed("full rebuild + checkpoint", started);
    println!("rebuild DB bytes: {}", size(&rebuild_path)?);

    let started = Instant::now();
    let unchanged = scan_changes(&connection, &root, &source_dir)?;
    elapsed("scan unchanged sources", started);
    println!("changed sources: {}", unchanged.len());

    let first = &files[0];
    if generated {
        fs::write(
            &first.path,
            r#"{"title":"Changed record","year":1749,"active":true}"#,
        )?;
    }
    let started = Instant::now();
    let changed = if generated {
        scan_changes(&connection, &root, &source_dir)?
    } else {
        let source = simulated_edit(first, &temp.path().join("simulated"), 1)?;
        vec![Change {
            id: first.id.clone(),
            source: Some(source),
        }]
    };
    elapsed(
        if generated {
            "scan after one source edit"
        } else {
            "prepare simulated source edit"
        },
        started,
    );
    println!("changed sources: {}", changed.len());
    let started = Instant::now();
    refresh(&mut connection, &changed)?;
    elapsed("refresh one source + commit", started);
    let changed_year: Option<i64> = connection.query_row(
        "SELECT json_extract(data,'$.year') FROM records WHERE id=?1",
        [&first.id],
        |row| row.get(0),
    )?;
    if generated && changed_year != Some(1749) {
        return Err("one-record refresh did not publish the changed JSON".into());
    }
    println!("WAL bytes before checkpoint: {}", wal_size(&rebuild_path));
    let started = Instant::now();
    checkpoint(&connection)?;
    elapsed("one-update WAL checkpoint", started);

    let batch_files: Vec<_> = files.iter().skip(1).take(batch).cloned().collect();
    if generated {
        let started = Instant::now();
        for file in &batch_files {
            fs::write(
                &file.path,
                r#"{"title":"Changed in batch","year":1750,"active":false}"#,
            )?;
        }
        elapsed("edit batch source files", started);
    }
    let started = Instant::now();
    let batch_changes = if generated {
        scan_changes(&connection, &root, &source_dir)?
    } else {
        batch_files
            .into_iter()
            .map(|source| {
                Ok(Change {
                    id: source.id.clone(),
                    source: Some(simulated_edit(&source, &temp.path().join("simulated"), 2)?),
                })
            })
            .collect::<Result<Vec<_>>>()?
    };
    elapsed(
        if generated {
            "scan after batch edits"
        } else {
            "prepare simulated batch"
        },
        started,
    );
    println!("batch changed sources: {}", batch_changes.len());

    let started = Instant::now();
    connection.execute(
        "CREATE INDEX records_year_id ON records(json_extract(data,'$.year'), id)",
        [],
    )?;
    elapsed("build year expression index", started);
    let started = Instant::now();
    refresh(&mut connection, &batch_changes)?;
    elapsed("refresh batch + commit", started);
    println!("WAL bytes before checkpoint: {}", wal_size(&rebuild_path));
    let started = Instant::now();
    checkpoint(&connection)?;
    elapsed("batch WAL checkpoint", started);
    println!("indexed DB bytes: {}", size(&rebuild_path)?);
    Ok(())
}
