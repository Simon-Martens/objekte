//! Collection-neutral warm query comparison over one shared compact NDJSON file.
//! Preparation and load are reported separately; source files remain authoritative.
use jaq_all::jaq_core::unwrap_valr;
use jaq_all::json::Val as JaqValue;
use jaq_all::{data, fmts};
use qj::io::{InputReader, ReaderOptions};
use qj::jq::lang::execute::Jq;
use qj::jq::lang::{CompileOptions, jq_compile_args};
use qj::jq::value::Value as QjValue;
use rusqlite::{Connection, params};
use serde_json::{Value as Json, json};
use std::collections::BTreeMap;
use std::error::Error;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn secs(start: Instant) -> f64 {
    start.elapsed().as_secs_f64()
}
fn emit(metric: &str, seconds: f64, result: Json) {
    println!(
        "{}",
        json!({"metric":metric,"seconds":seconds,"result":result})
    );
}
fn memory() -> Json {
    let status = fs::read_to_string("/proc/self/status").unwrap_or_default();
    let kb = |name: &str| {
        status
            .lines()
            .find_map(|line| {
                line.strip_prefix(name)?
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
            .unwrap_or(0)
    };
    json!({"rss_mib":kb("VmRSS:") as f64/1024.0,"peak_rss_mib":kb("VmHWM:") as f64/1024.0})
}
fn median(mut xs: Vec<f64>) -> f64 {
    xs.sort_by(f64::total_cmp);
    xs[xs.len() / 2]
}
fn measured(mut query: impl FnMut() -> Result<Json>, runs: usize) -> Result<(f64, Json)> {
    let _ = query()?;
    let mut times = Vec::with_capacity(runs);
    let mut out = Json::Null;
    for _ in 0..runs {
        let start = Instant::now();
        out = query()?;
        times.push(secs(start));
    }
    Ok((median(times), out))
}
fn paths(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut pending = vec![dir.to_path_buf()];
    let mut files = Vec::new();
    while let Some(folder) = pending.pop() {
        for entry in fs::read_dir(folder)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() && entry.path().extension().is_some_and(|x| x == "json") {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}
fn prepare(dir: &Path, output: &Path) -> Result<()> {
    let start = Instant::now();
    let files = paths(dir)?;
    let mut writer = BufWriter::new(File::create(output)?);
    for path in &files {
        let id = path.file_stem().ok_or("missing ID")?.to_string_lossy();
        let data: Json = serde_json::from_slice(&fs::read(path)?)?;
        writer.write_all(b"{\"id\":")?;
        serde_json::to_writer(&mut writer, &id)?;
        writer.write_all(b",\"data\":")?;
        serde_json::to_writer(&mut writer, &data)?;
        writer.write_all(b"}\n")?;
    }
    writer.flush()?;
    emit(
        "prepare",
        secs(start),
        json!({"records":files.len(),"bytes":fs::metadata(output)?.len()}),
    );
    Ok(())
}
fn scan(dir: &Path) -> Result<()> {
    let start = Instant::now();
    let files = paths(dir)?;
    let mut bytes = 0_u64;
    for path in &files {
        bytes += fs::metadata(path)?.len();
    }
    emit(
        "scan",
        secs(start),
        json!({"records":files.len(),"bytes":bytes}),
    );
    Ok(())
}
fn field_expr(field: &str) -> Result<String> {
    Ok(format!(".data[{}]", serde_json::to_string(field)?))
}
fn programs(
    number: &str,
    text: &str,
    start: i64,
    end: i64,
    needle: &str,
) -> Result<Vec<(&'static str, String)>> {
    let n = field_expr(number)?;
    let t = field_expr(text)?;
    let first = format!(".[0].data[{}]", serde_json::to_string(number)?);
    let needle = serde_json::to_string(&needle.to_ascii_lowercase())?;
    let cond = format!("({n} | type == \"number\") and ({n} >= {start} and {n} <= {end})");
    Ok(vec![
        ("range_count", format!("[.[] | select({cond})] | length")),
        (
            "range_page",
            format!("[.[] | select({cond})] | sort_by({n},.id) | .[:50] | map([{n},.id])"),
        ),
        (
            "number_group",
            format!(
                "[.[] | select({n} | type == \"number\")] | group_by({n}) | map([{first},length])"
            ),
        ),
        (
            "text_count",
            format!("[.[] | select(({t} // \"\") | ascii_downcase | contains({needle}))] | length"),
        ),
    ])
}
fn qj_load(path: &Path) -> Result<QjValue> {
    let mut reader = InputReader::new(vec![OsString::from(path)], ReaderOptions::default());
    let mut rows = Vec::new();
    while let Some(row) = reader.next() {
        rows.push(row?);
    }
    if reader.failures() != 0 {
        return Err("qj input failures".into());
    }
    Ok(rows.into())
}
fn qj_query(input: &QjValue, program: &str, runs: usize) -> Result<(f64, Json)> {
    let opts = CompileOptions::new(".");
    let code =
        jq_compile_args(program.as_bytes(), &opts).map_err(|e| format!("qj compile: {e:?}"))?;
    let mut jq = Jq::new(code);
    jq.set_jq_attrs(&opts.attrs);
    measured(
        || {
            jq.start(input.clone(), 0);
            let value = jq.next().ok_or("qj missing output")??;
            if jq.next().is_some() {
                return Err("qj multiple outputs".into());
            }
            Ok(serde_json::from_str(&value.to_json())?)
        },
        runs,
    )
}
fn jaq_load(path: &Path) -> Result<JaqValue> {
    let reader = BufReader::new(File::open(path)?);
    let mut rows = Vec::new();
    for row in fmts::read::json::read_many(reader) {
        rows.push(row?);
    }
    Ok(rows.into_iter().collect())
}
fn jaq_query(input: &JaqValue, program: &str, runs: usize) -> Result<(f64, Json)> {
    let compiled = data::compile(program).map_err(|_| "jaq compile error")?;
    let runner = data::Runner::default();
    measured(
        || {
            let mut outputs = Vec::new();
            data::run(
                &runner,
                &compiled,
                Default::default(),
                std::iter::once(Ok::<_, String>(input.clone())),
                |e| e,
                |v| {
                    let value = unwrap_valr(v).map_err(|e| e.to_string())?;
                    outputs.push(
                        serde_json::from_str::<Json>(&value.to_string())
                            .map_err(|e| e.to_string())?,
                    );
                    Ok(())
                },
            )?;
            if outputs.len() != 1 {
                return Err("jaq missing or multiple outputs".into());
            }
            Ok(outputs.remove(0))
        },
        runs,
    )
}
fn rust_load(path: &Path) -> Result<Vec<Json>> {
    BufReader::new(File::open(path)?)
        .lines()
        .map(|line| Ok(serde_json::from_str(&line?)?))
        .collect()
}
fn rust_load_source(dir: &Path) -> Result<Vec<Json>> {
    paths(dir)?
        .into_iter()
        .map(|path| {
            let id = path.file_stem().ok_or("missing ID")?.to_string_lossy();
            let data: Json = serde_json::from_slice(&fs::read(&path)?)?;
            Ok(json!({"id":id,"data":data}))
        })
        .collect()
}
struct ProjectedRow {
    id: String,
    number: Option<i64>,
    text: String,
}
fn project(id: String, data: &Json, number: &str, text: &str) -> ProjectedRow {
    ProjectedRow {
        id,
        number: data[number].as_i64(),
        text: data[text].as_str().unwrap_or("").to_owned(),
    }
}
fn rust_projected_load(
    input: Option<&Path>,
    source: Option<&Path>,
    number: &str,
    text: &str,
) -> Result<Vec<ProjectedRow>> {
    if let Some(dir) = source {
        return paths(dir)?
            .into_iter()
            .map(|path| {
                let id = path
                    .file_stem()
                    .ok_or("missing ID")?
                    .to_string_lossy()
                    .into_owned();
                let data: Json = serde_json::from_slice(&fs::read(&path)?)?;
                Ok(project(id, &data, number, text))
            })
            .collect();
    }
    BufReader::new(File::open(input.ok_or("missing input")?)?)
        .lines()
        .map(|line| {
            let row: Json = serde_json::from_str(&line?)?;
            Ok(project(
                row["id"].as_str().ok_or("missing ID")?.to_owned(),
                &row["data"],
                number,
                text,
            ))
        })
        .collect()
}
fn rust_projected_query(rows: &[ProjectedRow], name: &str, lo: i64, hi: i64, needle: &str) -> Json {
    match name {
        "range_count" => json!(
            rows.iter()
                .filter(|r| r.number.is_some_and(|n| n >= lo && n <= hi))
                .count()
        ),
        "range_page" => {
            let mut selected: Vec<_> = rows
                .iter()
                .filter_map(|r| {
                    let n = r.number?;
                    (n >= lo && n <= hi).then_some((n, r.id.as_str()))
                })
                .collect();
            selected.sort_unstable();
            json!(selected.into_iter().take(50).collect::<Vec<_>>())
        }
        "number_group" => {
            let mut counts = BTreeMap::<i64, usize>::new();
            for row in rows {
                if let Some(n) = row.number {
                    *counts.entry(n).or_default() += 1;
                }
            }
            json!(counts.into_iter().collect::<Vec<_>>())
        }
        "text_count" => json!(
            rows.iter()
                .filter(|r| r.text.to_ascii_lowercase().contains(needle))
                .count()
        ),
        _ => Json::Null,
    }
}
fn rust_query(
    rows: &[Json],
    name: &str,
    number: &str,
    text: &str,
    lo: i64,
    hi: i64,
    needle: &str,
) -> Json {
    let number_of = |row: &Json| row["data"][number].as_i64();
    match name {
        "range_count" => json!(
            rows.iter()
                .filter(|r| number_of(r).is_some_and(|n| n >= lo && n <= hi))
                .count()
        ),
        "range_page" => {
            let mut selected: Vec<_> = rows
                .iter()
                .filter_map(|r| {
                    let n = number_of(r)?;
                    (n >= lo && n <= hi).then(|| (n, r["id"].as_str().unwrap_or("")))
                })
                .collect();
            selected.sort_unstable();
            json!(selected.into_iter().take(50).collect::<Vec<_>>())
        }
        "number_group" => {
            let mut counts = BTreeMap::<i64, usize>::new();
            for row in rows {
                if let Some(n) = number_of(row) {
                    *counts.entry(n).or_default() += 1;
                }
            }
            json!(counts.into_iter().collect::<Vec<_>>())
        }
        "text_count" => json!(
            rows.iter()
                .filter(|r| r["data"][text]
                    .as_str()
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .contains(needle))
                .count()
        ),
        _ => Json::Null,
    }
}
fn sqlite_load(
    input: Option<&Path>,
    source: Option<&Path>,
    db_path: &Path,
    memory: bool,
) -> Result<Connection> {
    let mut db = if memory {
        Connection::open_in_memory()?
    } else {
        Connection::open(db_path)?
    };
    if !memory {
        db.execute_batch("PRAGMA journal_mode=WAL;PRAGMA synchronous=NORMAL;")?;
    }
    db.execute_batch(
        "PRAGMA cache_size=-262144;CREATE TABLE records(id TEXT PRIMARY KEY,data BLOB NOT NULL);",
    )?;
    let tx = db.transaction()?;
    {
        let mut stmt = tx.prepare("INSERT INTO records(id,data) VALUES(?1,jsonb(?2))")?;
        if let Some(dir) = source {
            for path in paths(dir)? {
                let id = path.file_stem().ok_or("missing ID")?.to_string_lossy();
                let data = fs::read_to_string(&path)?;
                stmt.execute(params![id.as_ref(), data])?;
            }
        } else {
            for line in BufReader::new(File::open(input.ok_or("missing input")?)?).lines() {
                let line = line?;
                let envelope = line.strip_prefix("{\"id\":").ok_or("missing ID prefix")?;
                let split = envelope.find(",\"data\":").ok_or("missing data key")?;
                let id: String = serde_json::from_str(&envelope[..split])?;
                let data = envelope[split + 8..]
                    .strip_suffix('}')
                    .ok_or("missing closing brace")?;
                stmt.execute(params![id, data])?;
            }
        }
    }
    tx.commit()?;
    Ok(db)
}
fn sqlite_query(
    db: &Connection,
    name: &str,
    number: &str,
    text: &str,
    range: (i64, i64),
    needle: &str,
    indexed: bool,
) -> Result<Json> {
    let (lo, hi) = range;
    let n = format!("$.{}", number);
    let t = format!("$.{}", text);
    let indexed_sql = if indexed {
        if !number
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err("indexed benchmark requires a simple number field".into());
        }
        let x = format!("json_extract(data,'$.{}')", number);
        Some(match name {
            "range_count" => format!(
                "SELECT count(*) FROM records WHERE typeof({x}) IN ('integer','real') AND {x} BETWEEN ?1 AND ?2"
            ),
            "range_page" => format!(
                "SELECT {x},id FROM records WHERE typeof({x}) IN ('integer','real') AND {x} BETWEEN ?1 AND ?2 ORDER BY {x},id LIMIT 50"
            ),
            "number_group" => format!(
                "SELECT {x},count(*) FROM records WHERE typeof({x}) IN ('integer','real') GROUP BY {x} ORDER BY {x}"
            ),
            _ => String::new(),
        })
    } else {
        None
    };
    let sql = match name {
        "range_count" => {
            "SELECT count(*) FROM records WHERE json_type(data,?1) IN ('integer','real') AND json_extract(data,?1) BETWEEN ?2 AND ?3"
        }
        "range_page" => {
            "SELECT json_extract(data,?1),id FROM records WHERE json_type(data,?1) IN ('integer','real') AND json_extract(data,?1) BETWEEN ?2 AND ?3 ORDER BY json_extract(data,?1),id LIMIT 50"
        }
        "number_group" => {
            "SELECT json_extract(data,?1),count(*) FROM records WHERE json_type(data,?1) IN ('integer','real') GROUP BY json_extract(data,?1) ORDER BY json_extract(data,?1)"
        }
        "text_count" => {
            "SELECT count(*) FROM records WHERE instr(lower(coalesce(json_extract(data,?1),'')),?2)>0"
        }
        _ => return Err("unknown query".into()),
    };
    let sql = indexed_sql
        .as_deref()
        .filter(|x| !x.is_empty())
        .unwrap_or(sql);
    let mut stmt = db.prepare(sql)?;
    let columns = stmt.column_count();
    let mut cursor = match name {
        "range_count" | "range_page" if indexed => stmt.query(params![lo, hi])?,
        "number_group" if indexed => stmt.query([])?,
        "range_count" | "range_page" => stmt.query(params![n, lo, hi])?,
        "number_group" => stmt.query(params![n])?,
        "text_count" => stmt.query(params![t, needle])?,
        _ => unreachable!(),
    };
    let mut out = Vec::new();
    while let Some(row) = cursor.next()? {
        let mut values = Vec::new();
        for i in 0..columns {
            use rusqlite::types::ValueRef;
            values.push(match row.get_ref(i)? {
                ValueRef::Null => Json::Null,
                ValueRef::Integer(n) => json!(n),
                ValueRef::Real(n) => json!(n),
                ValueRef::Text(s) => json!(std::str::from_utf8(s)?),
                ValueRef::Blob(_) => return Err("unexpected blob".into()),
            });
        }
        out.push(Json::Array(values));
    }
    if name.ends_with("count") {
        Ok(out[0][0].clone())
    } else {
        Ok(Json::Array(out))
    }
}
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut engine = String::new();
    let mut input = None;
    let mut source = None;
    let mut output = None;
    let mut number = String::new();
    let mut text = String::new();
    let mut lo = 0;
    let mut hi = 0;
    let mut needle = String::new();
    let mut runs = 7;
    let mut i = 0;
    while i < args.len() {
        let key = &args[i];
        i += 1;
        let value = args.get(i).ok_or("missing option value")?;
        match key.as_str() {
            "--engine" => engine = value.clone(),
            "--input" => input = Some(PathBuf::from(value)),
            "--source-dir" => source = Some(PathBuf::from(value)),
            "--output" => output = Some(PathBuf::from(value)),
            "--number-field" => number = value.clone(),
            "--text-field" => text = value.clone(),
            "--lower" => lo = value.parse()?,
            "--upper" => hi = value.parse()?,
            "--needle" => needle = value.to_ascii_lowercase(),
            "--runs" => runs = value.parse()?,
            other => return Err(format!("unknown option {other}").into()),
        }
        i += 1;
    }
    if engine == "prepare" {
        return prepare(
            &source.ok_or("missing source dir")?,
            &output.ok_or("missing output")?,
        );
    }
    if engine == "scan" {
        return scan(&source.ok_or("missing source dir")?);
    }
    if number.is_empty() || text.is_empty() || runs == 0 {
        return Err("missing query configuration".into());
    }
    let names = ["range_count", "range_page", "number_group", "text_count"];
    let programs = programs(&number, &text, lo, hi, &needle)?;
    let start = Instant::now();
    match engine.as_str() {
        "qj" => {
            let rows = qj_load(input.as_deref().ok_or("missing input")?)?;
            emit("load", secs(start), memory());
            for (name, program) in programs {
                let (time, result) = qj_query(&rows, &program, runs)?;
                emit(name, time, result);
            }
        }
        "jaq" => {
            let rows = jaq_load(input.as_deref().ok_or("missing input")?)?;
            emit("load", secs(start), memory());
            for (name, program) in programs {
                let (time, result) = jaq_query(&rows, &program, runs)?;
                emit(name, time, result);
            }
        }
        "rust" | "rust-source" => {
            let rows = if engine == "rust-source" {
                rust_load_source(source.as_deref().ok_or("missing source")?)?
            } else {
                rust_load(input.as_deref().ok_or("missing input")?)?
            };
            emit("load", secs(start), memory());
            for name in names {
                let (time, result) = measured(
                    || Ok(rust_query(&rows, name, &number, &text, lo, hi, &needle)),
                    runs,
                )?;
                emit(name, time, result);
            }
        }
        "rust-projected" | "rust-projected-source" => {
            let rows = rust_projected_load(
                input.as_deref(),
                if engine == "rust-projected-source" {
                    source.as_deref()
                } else {
                    None
                },
                &number,
                &text,
            )?;
            emit("load", secs(start), memory());
            for name in names {
                let (time, result) = measured(
                    || Ok(rust_projected_query(&rows, name, lo, hi, &needle)),
                    runs,
                )?;
                emit(name, time, result);
            }
        }
        "sqlite-memory" | "sqlite-disk" | "sqlite-memory-source" | "sqlite-disk-source" => {
            let db_path = output.unwrap_or_else(|| PathBuf::from("/tmp/objekte-shared-query.db"));
            let from_source = engine.ends_with("-source");
            let in_memory = engine.starts_with("sqlite-memory");
            let db = sqlite_load(
                input.as_deref(),
                if from_source { source.as_deref() } else { None },
                &db_path,
                in_memory,
            )?;
            emit("load", secs(start), memory());
            for name in names {
                let (time, result) = measured(
                    || sqlite_query(&db, name, &number, &text, (lo, hi), &needle, false),
                    runs,
                )?;
                emit(name, time, result);
            }
            if !in_memory {
                emit("storage_bytes", 0.0, json!(fs::metadata(&db_path)?.len()));
            }
        }
        "sqlite-open" | "sqlite-indexed" => {
            let db_path = output.ok_or("missing existing DB path")?;
            let db = Connection::open(&db_path)?;
            db.execute_batch("PRAGMA cache_size=-262144;")?;
            emit("open", secs(start), memory());
            let indexed = engine == "sqlite-indexed";
            if indexed {
                if !number
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    return Err("indexed benchmark requires a simple number field".into());
                }
                let start = Instant::now();
                db.execute_batch(&format!("CREATE INDEX IF NOT EXISTS records_number_{} ON records(json_extract(data,'$.{}'),id)", number, number))?;
                emit("index", secs(start), memory());
                let plan_sql = format!(
                    "EXPLAIN QUERY PLAN SELECT id FROM records WHERE json_extract(data,'$.{}') BETWEEN ?1 AND ?2 ORDER BY json_extract(data,'$.{}'),id LIMIT 50",
                    number, number
                );
                let mut plan_stmt = db.prepare(&plan_sql)?;
                let plan = plan_stmt
                    .query_map(params![lo, hi], |row| row.get::<_, String>(3))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                emit("plan", 0.0, json!(plan));
            }
            for name in names {
                let (time, result) = measured(
                    || sqlite_query(&db, name, &number, &text, (lo, hi), &needle, indexed),
                    runs,
                )?;
                emit(name, time, result);
            }
            emit("storage_bytes", 0.0, json!(fs::metadata(&db_path)?.len()));
        }
        other => return Err(format!("unknown engine {other}").into()),
    }
    emit("memory_end", 0.0, memory());
    Ok(())
}
