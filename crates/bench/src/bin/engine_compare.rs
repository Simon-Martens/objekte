//! Compare embedded SQLite, Turso, and SurrealDB as caches of the same JSON files.
//! The input files are authoritative and are never edited by this benchmark.
use rusqlite::{Connection as Sqlite, params};
use serde_json::{Value as Json, json};
use std::error::Error;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use surrealdb::Surreal;
use surrealdb::engine::local::{Db, SurrealKv};
use turso::Builder as TursoBuilder;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct Doc {
    id: String,
    raw: String,
}

fn synthetic(root: &Path, count: usize) -> Result<PathBuf> {
    let data = root.join("data/items");
    fs::create_dir_all(&data)?;
    for n in 0..count {
        let folder = data.join(format!("{:02x}", n % 256));
        if n < 256 {
            fs::create_dir_all(&folder)?;
        }
        let mut file = fs::File::create(folder.join(format!("{n:08}.json")))?;
        write!(
            file,
            "{{\"title\":\"Record {n}\",\"year\":{},\"active\":{}}}",
            1600 + n % 500,
            n % 3 == 0
        )?;
    }
    Ok(data)
}

fn collection_path(root: &Path, name: &str) -> Result<PathBuf> {
    let repo: Json = serde_json::from_slice(&fs::read(root.join(".editor/repository.json"))?)?;
    let settings = repo["collections"][name]
        .as_str()
        .ok_or("collection is not registered")?;
    let config: Json = serde_json::from_slice(&fs::read(root.join(settings))?)?;
    if config["type"] != "data" || config["format"] != "json" || config["id"] != "filename" {
        return Err("benchmark requires a JSON collection with filename IDs".into());
    }
    Ok(root.join(
        config["path"]
            .as_str()
            .ok_or("collection path is missing")?,
    ))
}

fn load_docs(source_dir: &Path) -> Result<Vec<Doc>> {
    let mut stack = vec![source_dir.to_path_buf()];
    let mut docs = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                stack.push(entry.path());
            } else if kind.is_file() && entry.path().extension().is_some_and(|e| e == "json") {
                let id = entry
                    .path()
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or("non-UTF-8 filename")?
                    .to_owned();
                docs.push(Doc {
                    id,
                    raw: fs::read_to_string(entry.path())?,
                });
            }
        }
    }
    docs.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(docs)
}

fn sample_agent(docs: &[Doc]) -> Option<String> {
    docs.iter().find_map(|doc| {
        serde_json::from_str::<Json>(&doc.raw)
            .ok()?
            .pointer("/agents/0/agent")?
            .as_str()
            .map(str::to_owned)
    })
}

fn edited(doc: &Doc) -> Result<Json> {
    let mut value: Json = serde_json::from_str(&doc.raw)?;
    value
        .as_object_mut()
        .ok_or("source is not an object")?
        .insert("_bench_edit".into(), json!(1));
    Ok(value)
}

fn report(label: &str, duration: Duration, result: impl std::fmt::Display) {
    println!("{label:28} {:>10.6} s  {result}", duration.as_secs_f64());
}

fn total_bytes(dir: &Path) -> Result<u64> {
    let mut bytes = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            for entry in fs::read_dir(path)? {
                stack.push(entry?.path());
            }
        } else {
            bytes += fs::metadata(path)?.len();
        }
    }
    Ok(bytes)
}

fn sql_database_bytes(path: &Path) -> u64 {
    [
        path.to_path_buf(),
        PathBuf::from(format!("{}-wal", path.display())),
        PathBuf::from(format!("{}-shm", path.display())),
    ]
    .iter()
    .filter_map(|path| fs::metadata(path).ok())
    .map(|metadata| metadata.len())
    .sum()
}

fn sqlite_query(db: &Sqlite, sql: &str, parameter: Option<&str>) -> Result<Vec<Vec<Json>>> {
    let mut statement = db.prepare(sql)?;
    let count = statement.column_count();
    let mut rows = if let Some(value) = parameter {
        statement.query([value])?
    } else {
        statement.query([])?
    };
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let mut columns = Vec::new();
        for i in 0..count {
            let value = row.get_ref(i)?;
            let item = match value {
                rusqlite::types::ValueRef::Null => Json::Null,
                rusqlite::types::ValueRef::Integer(n) => json!(n),
                rusqlite::types::ValueRef::Real(n) => json!(n),
                rusqlite::types::ValueRef::Text(s) => json!(String::from_utf8_lossy(s).to_string()),
                rusqlite::types::ValueRef::Blob(_) => {
                    return Err("unexpected BLOB query result".into());
                }
            };
            columns.push(item);
        }
        out.push(columns);
    }
    Ok(out)
}

fn run_sqlite(docs: &[Doc], path: &Path, agent: Option<&str>) -> Result<()> {
    let started = Instant::now();
    let mut db = Sqlite::open(path)?;
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; CREATE TABLE records(id TEXT PRIMARY KEY, data BLOB NOT NULL);")?;
    let tx = db.transaction()?;
    {
        let mut insert = tx.prepare("INSERT INTO records(id,data) VALUES (?1,jsonb(?2))")?;
        for doc in docs {
            insert.execute(params![doc.id, doc.raw])?;
        }
    }
    tx.commit()?;
    report(
        "build + commit",
        started.elapsed(),
        format!("{} records", docs.len()),
    );
    let sqls = [
        (
            "filter",
            "SELECT count(*) FROM records WHERE json_extract(data,'$.year') BETWEEN 1800 AND 1899",
        ),
        (
            "sort 100",
            "SELECT json_extract(data,'$.year') FROM records WHERE json_extract(data,'$.year') BETWEEN 1800 AND 1899 ORDER BY json_extract(data,'$.year'),id LIMIT 100",
        ),
        (
            "group",
            "SELECT json_extract(data,'$.year'),count(*) FROM records GROUP BY json_extract(data,'$.year') ORDER BY json_extract(data,'$.year')",
        ),
    ];
    for (label, sql) in sqls {
        let started = Instant::now();
        let mut answer = Vec::new();
        for _ in 0..5 {
            answer = sqlite_query(&db, sql, None)?;
        }
        report(
            label,
            started.elapsed() / 5,
            format!("{} rows, digest {}", answer.len(), digest(&answer)?),
        );
    }
    if let Some(agent) = agent {
        let sql = "SELECT count(*) FROM records WHERE EXISTS (SELECT 1 FROM json_each(json_extract(data,'$.agents')) AS a WHERE json_extract(a.value,'$.agent')=?1)";
        let started = Instant::now();
        let mut answer = Vec::new();
        for _ in 0..5 {
            answer = sqlite_query(&db, sql, Some(agent))?;
        }
        report(
            "nested agent filter",
            started.elapsed() / 5,
            format!("digest {}", digest(&answer)?),
        );
    }
    let started = Instant::now();
    db.execute_batch(
        "CREATE INDEX records_year ON records(json_extract(data,'$.year'),id); PRAGMA optimize;",
    )?;
    report("build year index", started.elapsed(), "");
    let started = Instant::now();
    let mut answer = Vec::new();
    for _ in 0..5 {
        answer = sqlite_query(&db, sqls[1].1, None)?;
    }
    report(
        "sort 100 indexed",
        started.elapsed() / 5,
        format!("digest {}", digest(&answer)?),
    );
    let first_edit = edited(&docs[0])?;
    let started = Instant::now();
    db.execute(
        "UPDATE records SET data=jsonb(?1) WHERE id=?2",
        params![serde_json::to_string(&first_edit)?, docs[0].id],
    )?;
    report("update one + commit", started.elapsed(), "");
    let started = Instant::now();
    let tx = db.transaction()?;
    {
        let mut update = tx.prepare("UPDATE records SET data=jsonb(?1) WHERE id=?2")?;
        for doc in docs.iter().skip(1).take(100) {
            update.execute(params![serde_json::to_string(&edited(doc)?)?, doc.id])?;
        }
    }
    tx.commit()?;
    report("update 100 + commit", started.elapsed(), "");
    db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    report("storage bytes", Duration::ZERO, sql_database_bytes(path));
    Ok(())
}

async fn turso_query(
    db: &turso::Connection,
    sql: &str,
    parameter: Option<&str>,
) -> Result<Vec<Vec<Json>>> {
    let mut rows = if let Some(value) = parameter {
        db.query(sql, [value]).await?
    } else {
        db.query(sql, ()).await?
    };
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        let mut columns = Vec::new();
        for i in 0..row.column_count() {
            let item = match row.get_value(i)? {
                turso::Value::Null => Json::Null,
                turso::Value::Integer(n) => json!(n),
                turso::Value::Real(n) => json!(n),
                turso::Value::Text(s) => json!(s),
                turso::Value::Blob(_) => return Err("unexpected BLOB query result".into()),
            };
            columns.push(item);
        }
        out.push(columns);
    }
    Ok(out)
}

async fn run_turso(docs: &[Doc], path: &Path, agent: Option<&str>) -> Result<()> {
    let started = Instant::now();
    let db = TursoBuilder::new_local(path.to_str().ok_or("bad path")?)
        .build()
        .await?;
    let conn = db.connect()?;
    // Turso opens local databases in WAL mode; journal_mode returns a row and
    // cannot be included in execute_batch, which expects row-free statements.
    conn.execute_batch("PRAGMA synchronous=NORMAL; CREATE TABLE records(id TEXT PRIMARY KEY, data BLOB NOT NULL); BEGIN TRANSACTION;").await?;
    let mut insert = conn
        .prepare("INSERT INTO records(id,data) VALUES (?1,jsonb(?2))")
        .await?;
    for doc in docs {
        insert.execute((doc.id.as_str(), doc.raw.as_str())).await?;
    }
    conn.execute_batch("COMMIT;").await?;
    report(
        "build + commit",
        started.elapsed(),
        format!("{} records", docs.len()),
    );
    let sqls = [
        (
            "filter",
            "SELECT count(*) FROM records WHERE json_extract(data,'$.year') BETWEEN 1800 AND 1899",
        ),
        (
            "sort 100",
            "SELECT json_extract(data,'$.year') FROM records WHERE json_extract(data,'$.year') BETWEEN 1800 AND 1899 ORDER BY json_extract(data,'$.year'),id LIMIT 100",
        ),
        (
            "group",
            "SELECT json_extract(data,'$.year'),count(*) FROM records GROUP BY json_extract(data,'$.year') ORDER BY json_extract(data,'$.year')",
        ),
    ];
    for (label, sql) in sqls {
        let started = Instant::now();
        let mut answer = Vec::new();
        for _ in 0..5 {
            answer = turso_query(&conn, sql, None).await?;
        }
        report(
            label,
            started.elapsed() / 5,
            format!("{} rows, digest {}", answer.len(), digest(&answer)?),
        );
    }
    if let Some(agent) = agent {
        let sql = "SELECT count(*) FROM records WHERE EXISTS (SELECT 1 FROM json_each(json_extract(data,'$.agents')) AS a WHERE json_extract(a.value,'$.agent')=?1)";
        let started = Instant::now();
        let mut answer = Vec::new();
        for _ in 0..5 {
            answer = turso_query(&conn, sql, Some(agent)).await?;
        }
        report(
            "nested agent filter",
            started.elapsed() / 5,
            format!("digest {}", digest(&answer)?),
        );
    }
    let started = Instant::now();
    conn.execute(
        "CREATE INDEX records_year ON records(json_extract(data,'$.year'),id)",
        (),
    )
    .await?;
    report("build year index", started.elapsed(), "");
    let started = Instant::now();
    let mut answer = Vec::new();
    for _ in 0..5 {
        answer = turso_query(&conn, sqls[1].1, None).await?;
    }
    report(
        "sort 100 indexed",
        started.elapsed() / 5,
        format!("digest {}", digest(&answer)?),
    );
    let first_edit = edited(&docs[0])?;
    let started = Instant::now();
    conn.execute(
        "UPDATE records SET data=jsonb(?1) WHERE id=?2",
        (serde_json::to_string(&first_edit)?, docs[0].id.clone()),
    )
    .await?;
    report("update one + commit", started.elapsed(), "");
    let started = Instant::now();
    conn.execute_batch("BEGIN TRANSACTION;").await?;
    let mut update = conn
        .prepare("UPDATE records SET data=jsonb(?1) WHERE id=?2")
        .await?;
    for doc in docs.iter().skip(1).take(100) {
        update
            .execute((serde_json::to_string(&edited(doc)?)?, doc.id.clone()))
            .await?;
    }
    conn.execute_batch("COMMIT;").await?;
    report("update 100 + commit", started.elapsed(), "");
    report("storage bytes", Duration::ZERO, sql_database_bytes(path));
    Ok(())
}

async fn surreal_query(db: &Surreal<Db>, sql: &str, agent: Option<&str>) -> Result<Vec<Vec<Json>>> {
    let query = db.query(sql);
    let answer = if let Some(agent) = agent {
        query.bind(("agent", agent)).await?
    } else {
        query.await?
    };
    let mut answer = answer.check()?;
    let rows: Vec<Json> = answer.take(0)?;
    Ok(rows
        .into_iter()
        .map(|row| match row {
            Json::Array(columns) => columns,
            Json::Object(mut map) => {
                if map.contains_key("year") {
                    vec![
                        map.remove("year").unwrap(),
                        map.remove("count").unwrap_or(Json::Null),
                    ]
                } else if let Some(count) = map.remove("count") {
                    vec![count]
                } else {
                    map.into_values().collect()
                }
            }
            other => vec![other],
        })
        .collect())
}

async fn run_surreal(docs: &[Doc], path: &Path, agent: Option<&str>) -> Result<()> {
    let started = Instant::now();
    let db = Surreal::new::<SurrealKv>(path).await?;
    db.use_ns("bench").use_db("bench").await?;
    db.query("DEFINE TABLE records SCHEMALESS;")
        .await?
        .check()?;
    for chunk in docs.chunks(500) {
        let mut batch = Vec::with_capacity(chunk.len());
        for doc in chunk {
            batch.push(json!({"id": doc.id, "data": serde_json::from_str::<Json>(&doc.raw)?}));
        }
        db.query("INSERT INTO records $batch RETURN NONE;")
            .bind(("batch", batch))
            .await?
            .check()?;
    }
    report(
        "build + commit",
        started.elapsed(),
        format!("{} records", docs.len()),
    );
    let sqls = [
        (
            "filter",
            "SELECT count() AS count FROM records WHERE data.year >= 1800 AND data.year <= 1899 GROUP ALL",
        ),
        (
            "sort 100",
            "SELECT VALUE data.year FROM records WHERE data.year >= 1800 AND data.year <= 1899 ORDER BY data.year,id LIMIT 100",
        ),
        (
            "group",
            "SELECT data.year AS year, count() AS count FROM records GROUP BY data.year ORDER BY year",
        ),
    ];
    for (label, sql) in sqls {
        let started = Instant::now();
        let mut answer = Vec::new();
        for _ in 0..5 {
            answer = surreal_query(&db, sql, None).await?;
        }
        report(
            label,
            started.elapsed() / 5,
            format!("{} rows, digest {}", answer.len(), digest(&answer)?),
        );
    }
    if let Some(agent) = agent {
        let sql = "SELECT count() AS count FROM records WHERE data.agents.*.agent CONTAINS $agent GROUP ALL";
        let started = Instant::now();
        let mut answer = Vec::new();
        for _ in 0..5 {
            answer = surreal_query(&db, sql, Some(agent)).await?;
        }
        report(
            "nested agent filter",
            started.elapsed() / 5,
            format!("digest {}", digest(&answer)?),
        );
    }
    let started = Instant::now();
    db.query("DEFINE INDEX records_year ON records FIELDS data.year,id;")
        .await?
        .check()?;
    report("build year index", started.elapsed(), "");
    let mut explanation = db.query(format!("{} EXPLAIN", sqls[1].1)).await?.check()?;
    let plan: Vec<Json> = explanation.take(0)?;
    println!("indexed sort plan: {}", serde_json::to_string(&plan)?);
    let started = Instant::now();
    let mut answer = Vec::new();
    for _ in 0..5 {
        answer = surreal_query(&db, sqls[1].1, None).await?;
    }
    report(
        "sort 100 indexed",
        started.elapsed() / 5,
        format!("digest {}", digest(&answer)?),
    );
    let id_only_sql = sqls[1]
        .1
        .replace("SELECT VALUE data.year", "SELECT VALUE id");
    let started = Instant::now();
    for _ in 0..5 {
        answer = surreal_query(&db, &id_only_sql, None).await?;
    }
    report(
        "indexed sort IDs only",
        started.elapsed() / 5,
        format!("{} rows", answer.len()),
    );
    let first_edit = edited(&docs[0])?;
    let started = Instant::now();
    db.query("UPDATE type::record('records', $id) SET data = $data RETURN NONE;")
        .bind(("id", docs[0].id.as_str()))
        .bind(("data", first_edit))
        .await?
        .check()?;
    report("update one + commit", started.elapsed(), "");
    let started = Instant::now();
    let tx = db.clone().begin().await?;
    for doc in docs.iter().skip(1).take(100) {
        tx.query("UPDATE type::record('records', $id) SET data = $data RETURN NONE;")
            .bind(("id", doc.id.as_str()))
            .bind(("data", edited(doc)?))
            .await?
            .check()?;
    }
    tx.commit().await?;
    report("update 100 + commit", started.elapsed(), "");
    report("storage bytes", Duration::ZERO, total_bytes(path)?);
    if let Some(agent) = agent {
        let started = Instant::now();
        db.query("DEFINE INDEX records_agent ON records FIELDS data.agents.*.agent;")
            .await?
            .check()?;
        report("build nested agent index", started.elapsed(), "");
        let sql = "SELECT count() AS count FROM records WHERE data.agents.*.agent CONTAINS $agent GROUP ALL";
        let mut explanation = db
            .query(format!("{sql} EXPLAIN"))
            .bind(("agent", agent))
            .await?
            .check()?;
        let plan: Vec<Json> = explanation.take(0)?;
        println!("nested agent index plan: {}", serde_json::to_string(&plan)?);
        let started = Instant::now();
        let mut answer = Vec::new();
        for _ in 0..5 {
            answer = surreal_query(&db, sql, Some(agent)).await?;
        }
        report(
            "nested agent indexed",
            started.elapsed() / 5,
            format!("digest {}", digest(&answer)?),
        );
        let ids_sql =
            "SELECT VALUE id FROM records WHERE data.agents.*.agent CONTAINS $agent ORDER BY id";
        let started = Instant::now();
        let mut ids = Vec::new();
        for _ in 0..5 {
            ids = surreal_query(&db, ids_sql, Some(agent)).await?;
        }
        report(
            "nested indexed IDs",
            started.elapsed() / 5,
            format!("{} rows", ids.len()),
        );
        report(
            "storage with nested index",
            Duration::ZERO,
            total_bytes(path)?,
        );
        let source = docs
            .iter()
            .find(|doc| {
                serde_json::from_str::<Json>(&doc.raw)
                    .ok()
                    .and_then(|value| {
                        value["agents"]
                            .as_array()
                            .map(|agents| agents.iter().any(|item| item["agent"] == agent))
                    })
                    .unwrap_or(false)
            })
            .ok_or("sample agent has no source record")?;
        let mut changed: Json = serde_json::from_str(&source.raw)?;
        changed["agents"] = json!([]);
        let before = answer[0][0].as_i64().ok_or("nested count is not numeric")?;
        let started = Instant::now();
        db.query("UPDATE type::record('records', $id) SET data = $data RETURN NONE;")
            .bind(("id", source.id.as_str()))
            .bind(("data", changed))
            .await?
            .check()?;
        report("nested indexed update", started.elapsed(), "");
        let after = surreal_query(&db, sql, Some(agent)).await?;
        if after[0][0].as_i64() != Some(before - 1) {
            return Err("nested index did not reflect the update".into());
        }
        println!("nested index update verified: {before} -> {}", before - 1);
    }
    Ok(())
}

fn digest(rows: &[Vec<Json>]) -> Result<u64> {
    // Stable order and value check without printing large result sets.
    let bytes = serde_json::to_vec(rows)?;
    Ok(bytes.iter().fold(14695981039346656037_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(1099511628211)
    }))
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut engine = None;
    let mut repo = None;
    let mut collection = None;
    let mut synthetic_count = None;
    let mut work_dir = PathBuf::from("target");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let key = &args[i];
        if key == "--help" {
            println!(
                "engine-compare --engine sqlite|turso|surreal [--synthetic N | --repo PATH --collection NAME] [--work-dir PATH]"
            );
            return Ok(());
        }
        i += 1;
        let value = args.get(i).ok_or(format!("missing value for {key}"))?;
        match key.as_str() {
            "--engine" => engine = Some(value.clone()),
            "--repo" => repo = Some(PathBuf::from(value)),
            "--collection" => collection = Some(value.clone()),
            "--synthetic" => synthetic_count = Some(value.parse::<usize>()?),
            "--work-dir" => work_dir = PathBuf::from(value),
            _ => return Err(format!("unknown argument {key}").into()),
        }
        i += 1;
    }
    let engine = engine.ok_or("--engine required")?;
    let temp = tempfile::Builder::new()
        .prefix("objekte-engine-compare-")
        .tempdir_in(work_dir)?;
    let source_dir = if let Some(count) = synthetic_count {
        let started = Instant::now();
        let dir = synthetic(&temp.path().join("synthetic"), count)?;
        report(
            "generate source files",
            started.elapsed(),
            format!("{count} files"),
        );
        dir
    } else {
        collection_path(
            &repo.ok_or("--repo required")?,
            &collection.ok_or("--collection required")?,
        )?
    };
    let started = Instant::now();
    let docs = load_docs(&source_dir)?;
    report(
        "read source files",
        started.elapsed(),
        format!("{} records", docs.len()),
    );
    if docs.is_empty() {
        return Err("empty collection".into());
    }
    let agent = sample_agent(&docs);
    println!("engine={engine}, sample_agent={agent:?}");
    match engine.as_str() {
        "sqlite" => run_sqlite(&docs, &temp.path().join("sqlite.db"), agent.as_deref())?,
        "turso" => run_turso(&docs, &temp.path().join("turso.db"), agent.as_deref()).await?,
        "surreal" => run_surreal(&docs, &temp.path().join("surreal"), agent.as_deref()).await?,
        _ => return Err("unknown engine".into()),
    }
    Ok(())
}
