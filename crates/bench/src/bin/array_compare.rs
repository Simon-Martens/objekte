//! Compare qj's in-process array, Node.js's in-process array, and SQLite JSONB.
//! Source files remain authoritative; the shared NDJSON input is temporary.
use qj::io::{InputReader, ReaderOptions};
use qj::jq::lang::execute::Jq;
use qj::jq::lang::{CompileOptions, jq_compile_args};
use qj::jq::value::Value as QjValue;
use qj::jq::value::object::Object as QjObject;
use rusqlite::{Connection, params};
use serde_json::{Value as Json, json};
use std::error::Error;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn elapsed(start: Instant) -> f64 {
    start.elapsed().as_secs_f64()
}

fn emit(metric: &str, seconds: f64, result: Json) {
    println!(
        "{}",
        json!({"metric":metric,"seconds":seconds,"result":result})
    );
}

fn median(mut times: Vec<f64>) -> f64 {
    times.sort_by(f64::total_cmp);
    times[times.len() / 2]
}

fn measured(mut f: impl FnMut() -> Result<Json>, runs: usize) -> Result<(f64, Json)> {
    let _ = f()?; // Warm the engine and the operating-system page cache.
    let mut times = Vec::with_capacity(runs);
    let mut result = Json::Null;
    for _ in 0..runs {
        let start = Instant::now();
        result = f()?;
        times.push(elapsed(start));
    }
    Ok((median(times), result))
}

fn memory() -> Json {
    let status = fs::read_to_string("/proc/self/status").unwrap_or_default();
    let kb = |key: &str| {
        status
            .lines()
            .find_map(|line| {
                line.strip_prefix(key)?
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
            .unwrap_or(0)
    };
    json!({"rss_kib":kb("VmRSS:"),"peak_rss_kib":kb("VmHWM:")})
}

fn synthetic(root: &Path, count: usize) -> Result<PathBuf> {
    let dir = root.join("data/items");
    fs::create_dir_all(&dir)?;
    for n in 0..count {
        let folder = dir.join(format!("{:02x}", n % 256));
        if n < 256 {
            fs::create_dir_all(&folder)?;
        }
        let mut file = File::create(folder.join(format!("{n:08}.json")))?;
        write!(
            file,
            "{{\"title\":\"Record {n}\",\"year\":{},\"active\":{}}}",
            1600 + n % 500,
            n % 3 == 0
        )?;
    }
    Ok(dir)
}

fn collection_path(root: &Path, name: &str) -> Result<PathBuf> {
    let repo: Json = serde_json::from_slice(&fs::read(root.join(".editor/repository.json"))?)?;
    let settings = repo["collections"][name]
        .as_str()
        .ok_or("unknown collection")?;
    let config: Json = serde_json::from_slice(&fs::read(root.join(settings))?)?;
    if config["type"] != "data" || config["format"] != "json" || config["id"] != "filename" {
        return Err("benchmark needs JSON data collection with filename IDs".into());
    }
    Ok(root.join(config["path"].as_str().ok_or("missing collection path")?))
}

fn source_paths(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut stack = vec![dir.to_path_buf()];
    let mut files = Vec::new();
    while let Some(path) = stack.pop() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                stack.push(entry.path());
            } else if kind.is_file() && entry.path().extension().is_some_and(|e| e == "json") {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

fn prepare(dir: &Path, output: &Path) -> Result<(usize, String, Option<String>)> {
    let files = source_paths(dir)?;
    let mut writer = BufWriter::new(File::create(output)?);
    let mut ids = Vec::with_capacity(files.len());
    let mut agent = None;
    for path in files {
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("non-UTF-8 ID")?
            .to_owned();
        let data: Json = serde_json::from_slice(&fs::read(&path)?)?;
        if agent.is_none() {
            agent = data
                .pointer("/agents/0/agent")
                .and_then(Json::as_str)
                .map(str::to_owned);
        }
        // Keep the ID at a predictable prefix so database ingestion can avoid
        // decoding the whole envelope just to recover the filename ID.
        writer.write_all(b"{\"id\":")?;
        serde_json::to_writer(&mut writer, &id)?;
        writer.write_all(b",\"data\":")?;
        serde_json::to_writer(&mut writer, &data)?;
        writer.write_all(b"}\n")?;
        ids.push(id);
    }
    writer.flush()?;
    if ids.is_empty() {
        return Err("empty collection".into());
    }
    ids.sort();
    Ok((ids.len(), ids[ids.len() / 2].clone(), agent))
}

fn qj_load(path: &Path) -> Result<QjValue> {
    let mut input = InputReader::new(vec![OsString::from(path)], ReaderOptions::default());
    let mut rows = Vec::new();
    while let Some(value) = input.next() {
        rows.push(value?);
    }
    if input.failures() != 0 {
        return Err("qj failed to parse input".into());
    }
    Ok(rows.into())
}

fn qj_run(input: &QjValue, program: &str) -> Result<Json> {
    Ok(serde_json::from_str(&qj_value(input, program)?.to_json())?)
}

fn qj_value(input: &QjValue, program: &str) -> Result<QjValue> {
    let opts = CompileOptions::new(".");
    let code =
        jq_compile_args(program.as_bytes(), &opts).map_err(|e| format!("qj compile: {e:?}"))?;
    let mut jq = Jq::new(code);
    jq.set_jq_attrs(&opts.attrs);
    jq.start(input.clone(), 0);
    let value = jq.next().ok_or("qj returned no value")??;
    if jq.next().is_some() {
        return Err("qj returned multiple values".into());
    }
    Ok(value)
}

fn qj_query(input: &QjValue, program: &str, runs: usize) -> Result<(f64, Json)> {
    // Compilation is outside the query timer, as with a saved view.
    let opts = CompileOptions::new(".");
    let code =
        jq_compile_args(program.as_bytes(), &opts).map_err(|e| format!("qj compile: {e:?}"))?;
    let mut jq = Jq::new(code);
    jq.set_jq_attrs(&opts.attrs);
    measured(
        || {
            jq.start(input.clone(), 0);
            let value = jq.next().ok_or("qj returned no value")??;
            if jq.next().is_some() {
                return Err("qj returned multiple values".into());
            }
            Ok(serde_json::from_str(&value.to_json())?)
        },
        runs,
    )
}

fn run_qj(
    path: &Path,
    id: &str,
    agent: Option<&str>,
    related: Option<(&Path, &Path, &Path)>,
    runs: usize,
) -> Result<()> {
    let start = Instant::now();
    let mut rows = qj_load(path)?;
    emit("build", elapsed(start), json!("in-memory array"));
    emit("memory_after_build", 0.0, memory());
    let range = "(.data.year >= 1800 and .data.year <= 1899)";
    let queries = [
        ("year_count", format!("[.[] | select({range})] | length")),
        ("year_sort100", format!("[.[] | select({range})] | sort_by(.data.year,.id) | .[:100] | map([.data.year,.id])")),
        ("year_group", "[.[] | select(.data.year | type == \"number\")] | group_by(.data.year) | map([.[0].data.year,length])".into()),
        ("id_lookup", format!("[.[] | select(.id == {}) | [.id,.data.year]]", serde_json::to_string(id)?)),
    ];
    for (name, query) in queries {
        let (time, result) = qj_query(&rows, &query, runs)?;
        emit(name, time, result);
    }
    if let Some(agent) = agent {
        let agent = serde_json::to_string(agent)?;
        let selected = format!(".[] | select(any(.data.agents[]?; .agent == {agent})) | .id");
        for (name, query) in [
            ("agent_count", format!("[{selected}] | length")),
            ("agent_ids", format!("[{selected}] | sort")),
        ] {
            let (time, result) = qj_query(&rows, &query, runs)?;
            emit(name, time, result);
        }
    }
    let start = Instant::now();
    // A saved result for this specific view, not a general field index.
    let sorted: QjValue = {
        let opts = CompileOptions::new(".");
        let code = jq_compile_args(
            b"[.[] | select(.data.year >= 1800 and .data.year <= 1899)] | sort_by(.data.year,.id)",
            &opts,
        )
        .map_err(|e| format!("qj compile: {e:?}"))?;
        let mut jq = Jq::new(code);
        jq.set_jq_attrs(&opts.attrs);
        jq.start(rows.clone(), 0);
        jq.next().ok_or("qj sorted array missing")??
    };
    emit("build_year_view", elapsed(start), json!("sorted array"));
    let (time, result) = qj_query(&sorted, ".[:100] | map([.data.year,.id])", runs)?;
    emit("year_sort100_cached", time, result);
    let _ = sorted; // Keep the materialized view alive through its query.
    if let Some((agents_path, series_path, places_path)) = related {
        let start = Instant::now();
        let agents = qj_load(agents_path)?;
        let series = qj_load(series_path)?;
        let places = qj_load(places_path)?;
        let mut input = QjObject::new();
        input.insert("entries".into(), rows.clone());
        input.insert("agents".into(), agents);
        input.insert("series".into(), series);
        input.insert("places".into(), places);
        let context = qj_value(
            &input.into(),
            ". + {entryById: INDEX(.entries[]; .id), agentById: INDEX(.agents[]; .id), placeById: INDEX(.places[]; .id)}",
        )?;
        emit(
            "build_related",
            elapsed(start),
            json!("related arrays + ID maps"),
        );
        emit("memory_after_related", 0.0, memory());
        run_qj_musenalm(&context, runs)?;
    }
    let length = rows_length(&rows)?;
    let mid = length / 2;
    let batch_length = length.min(100);
    let start = Instant::now();
    let edit = format!(".[{mid}].data._bench_edit = 1");
    let opts = CompileOptions::new(".");
    let code = jq_compile_args(edit.as_bytes(), &opts).map_err(|e| format!("qj compile: {e:?}"))?;
    let mut jq = Jq::new(code);
    jq.set_jq_attrs(&opts.attrs);
    jq.start(rows, 0);
    rows = jq.next().ok_or("qj edit missing")??;
    emit("update_one", elapsed(start), json!(true));
    let start = Instant::now();
    let batch = format!("reduce range(0;{batch_length}) as $i (. ; .[$i].data._bench_edit = 1)");
    rows = {
        let opts = CompileOptions::new(".");
        let code =
            jq_compile_args(batch.as_bytes(), &opts).map_err(|e| format!("qj compile: {e:?}"))?;
        let mut jq = Jq::new(code);
        jq.set_jq_attrs(&opts.attrs);
        jq.start(rows, 0);
        jq.next().ok_or("qj batch missing")??
    };
    emit("update_100", elapsed(start), json!(true));
    drop(rows);
    let start = Instant::now();
    let _rows = qj_load(path)?;
    emit("rebuild", elapsed(start), json!("in-memory array"));
    emit("memory_peak", 0.0, memory());
    Ok(())
}

fn rows_length(rows: &QjValue) -> Result<usize> {
    Ok(qj_run(rows, "length")?.as_u64().ok_or("length missing")? as usize)
}

fn run_qj_musenalm(context: &QjValue, runs: usize) -> Result<()> {
    let matching = r#"def matching: (.data.year >= 1800 and .data.year <= 1850) and .data.edit_state == "Edited" and ((.data.language | index("ger")) != null) and ((.data.items | length) > 0) and any(.data.agents[]?; .type == "Herausgeber:in");"#;
    let queries = [
        ("musenalm_title_page", r#"[.entries[]] | sort_by(.data.preferred_title,.id) | .[100:150] | map([.id,.data.preferred_title,.data.year])"#.to_owned()),
        ("musenalm_state_facets", r#"[.entries[]] | group_by(.data.edit_state) | map([.[0].data.edit_state,length])"#.to_owned()),
        ("musenalm_multi_count", format!("{matching} [.entries[] | select(matching)] | length")),
        ("musenalm_multi_page", format!("{matching} [.entries[] | select(matching)] | sort_by(.data.year,.id) | .[100:130] | map([.id,.data.year])")),
        ("musenalm_title_search_count", r#"[.entries[] | select(.data.preferred_title | ascii_downcase | contains("almanach"))] | length"#.to_owned()),
        ("musenalm_title_search_page", r#"[.entries[] | select(.data.preferred_title | ascii_downcase | contains("almanach")) | .id] | sort | .[:50]"#.to_owned()),
        ("musenalm_content_types", r#"[.entries[] | select(.data.year >= 1800 and .data.year <= 1899 and .data.edit_state == "Edited") | .data.contents[]?.musenalm_type[]?] | group_by(.) | map([.[0],length])"#.to_owned()),
        ("musenalm_role_decades", r#"[.entries[] | select(.data.year >= 1800 and .data.year <= 1899) | ((.data.year / 10 | floor) * 10) as $decade | .data.agents[]? | [$decade,.type]] | group_by(.) | map([.[0][0],.[0][1],length]) | sort_by(.[0],.[1])"#.to_owned()),
        ("musenalm_top_agents", r#"(.agentById) as $names | [.entries[] | . as $entry | .data.agents[]? | select(.agent != null and .agent != "") | [.agent,$entry.id]] | unique | group_by(.[0]) | map([.[0][0],($names[.[0][0]].data.name // null),length]) | sort_by(-.[2],.[0]) | .[:20]"#.to_owned()),
        ("musenalm_top_places", r#"(.placeById) as $names | [.entries[] | . as $entry | .data.places[]? | [.,$entry.id]] | unique | group_by(.[0]) | map([.[0][0],($names[.[0][0]].data.name // null),length]) | sort_by(-.[2],.[0]) | .[:20]"#.to_owned()),
        ("musenalm_series_span", r#"(.entryById) as $entries | [.series[] | . as $series | [$series.data.entries[]? | $entries[.] | select(. != null and .data.year >= 1800 and .data.year <= 1899) | .data.year] as $years | select(($years | length) > 0) | [$series.id,$series.data.title,($years | length),($years | min),($years | max)]] | sort_by(-.[2],.[0]) | .[:20]"#.to_owned()),
    ];
    for (name, query) in queries {
        let (seconds, result) = qj_query(context, &query, runs)?;
        emit(name, seconds, result);
    }
    Ok(())
}

fn sqlite_insert(db: &mut Connection, path: &Path, table: &str) -> Result<()> {
    let create = format!("CREATE TABLE {table}(id TEXT PRIMARY KEY,data BLOB NOT NULL);");
    db.execute_batch(&create)?;
    let tx = db.transaction()?;
    {
        let mut insert = tx.prepare(&format!(
            "INSERT INTO {table}(id,data) VALUES (?1,jsonb(?2))"
        ))?;
        for line in BufReader::new(File::open(path)?).lines() {
            let line = line?;
            let envelope = line.strip_prefix("{\"id\":").ok_or("missing ID prefix")?;
            let split = envelope.find(",\"data\":").ok_or("missing data key")?;
            let id: String = serde_json::from_str(&envelope[..split])?;
            let data = envelope[split + 8..]
                .strip_suffix('}')
                .ok_or("missing closing brace")?;
            insert.execute(params![id, data])?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn sqlite_load(path: &Path, db_path: &Path, in_memory: bool) -> Result<Connection> {
    let mut db = if in_memory {
        Connection::open_in_memory()?
    } else {
        Connection::open(db_path)?
    };
    if !in_memory {
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
    }
    db.execute_batch("PRAGMA cache_size=-262144;")?;
    sqlite_insert(&mut db, path, "records")?;
    Ok(db)
}

fn sqlite_query(db: &Connection, sql: &str, param: Option<&str>) -> Result<Json> {
    let mut stmt = db.prepare(sql)?;
    let count = stmt.column_count();
    let mut rows = if let Some(value) = param {
        stmt.query([value])?
    } else {
        stmt.query([])?
    };
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let mut columns = Vec::new();
        for i in 0..count {
            use rusqlite::types::ValueRef;
            columns.push(match row.get_ref(i)? {
                ValueRef::Null => Json::Null,
                ValueRef::Integer(n) => json!(n),
                ValueRef::Real(n) => json!(n),
                ValueRef::Text(s) => json!(std::str::from_utf8(s)?),
                ValueRef::Blob(_) => return Err("unexpected BLOB result".into()),
            });
        }
        out.push(Json::Array(columns));
    }
    Ok(Json::Array(out))
}

fn run_sqlite_musenalm(db: &Connection, runs: usize) -> Result<()> {
    let multi = "json_extract(e.data,'$.year') BETWEEN 1800 AND 1850 AND json_extract(e.data,'$.edit_state')='Edited' AND EXISTS (SELECT 1 FROM json_each(e.data,'$.language') l WHERE l.value='ger') AND json_array_length(e.data,'$.items')>0 AND EXISTS (SELECT 1 FROM json_each(e.data,'$.agents') a WHERE json_extract(a.value,'$.type')='Herausgeber:in')";
    let title_page = "SELECT id,json_extract(data,'$.preferred_title'),json_extract(data,'$.year') FROM records ORDER BY json_extract(data,'$.preferred_title'),id LIMIT 50 OFFSET 100";
    let state_facets = "SELECT json_extract(data,'$.edit_state'),count(*) FROM records GROUP BY json_extract(data,'$.edit_state') ORDER BY json_extract(data,'$.edit_state')";
    let queries = [
        ("musenalm_title_page", title_page.to_owned()),
        ("musenalm_state_facets", state_facets.to_owned()),
        ("musenalm_multi_count", format!("SELECT count(*) FROM records e WHERE {multi}")),
        ("musenalm_multi_page", format!("SELECT e.id,json_extract(e.data,'$.year') FROM records e WHERE {multi} ORDER BY json_extract(e.data,'$.year'),e.id LIMIT 30 OFFSET 100")),
        ("musenalm_title_search_count", "SELECT count(*) FROM records WHERE instr(lower(json_extract(data,'$.preferred_title')),'almanach')>0".to_owned()),
        ("musenalm_title_search_page", "SELECT id FROM records WHERE instr(lower(json_extract(data,'$.preferred_title')),'almanach')>0 ORDER BY id LIMIT 50".to_owned()),
        ("musenalm_content_types", "SELECT t.value,count(*) FROM records e JOIN json_each(e.data,'$.contents') c JOIN json_each(c.value,'$.musenalm_type') t WHERE json_extract(e.data,'$.year') BETWEEN 1800 AND 1899 AND json_extract(e.data,'$.edit_state')='Edited' GROUP BY t.value ORDER BY t.value".to_owned()),
        ("musenalm_role_decades", "SELECT CAST(json_extract(e.data,'$.year')/10 AS INTEGER)*10,json_extract(a.value,'$.type'),count(*) FROM records e JOIN json_each(e.data,'$.agents') a WHERE json_extract(e.data,'$.year') BETWEEN 1800 AND 1899 GROUP BY 1,2 ORDER BY 1,2".to_owned()),
        ("musenalm_top_agents", "WITH links AS (SELECT e.id AS entry_id,json_extract(a.value,'$.agent') AS agent_id FROM records e JOIN json_each(e.data,'$.agents') a WHERE json_extract(a.value,'$.agent') IS NOT NULL AND json_extract(a.value,'$.agent')!='') SELECT l.agent_id,json_extract(ag.data,'$.name'),count(DISTINCT l.entry_id) FROM links l LEFT JOIN agents ag ON ag.id=l.agent_id GROUP BY l.agent_id ORDER BY 3 DESC,l.agent_id LIMIT 20".to_owned()),
        ("musenalm_top_places", "WITH links AS (SELECT e.id AS entry_id,p.value AS place_id FROM records e JOIN json_each(e.data,'$.places') p) SELECT l.place_id,json_extract(pl.data,'$.name'),count(DISTINCT l.entry_id) FROM links l LEFT JOIN places pl ON pl.id=l.place_id GROUP BY l.place_id ORDER BY 3 DESC,l.place_id LIMIT 20".to_owned()),
        ("musenalm_series_span", "SELECT s.id,json_extract(s.data,'$.title'),count(e.id),min(json_extract(e.data,'$.year')),max(json_extract(e.data,'$.year')) FROM series s JOIN json_each(s.data,'$.entries') link JOIN records e ON e.id=link.value WHERE json_extract(e.data,'$.year') BETWEEN 1800 AND 1899 GROUP BY s.id ORDER BY 3 DESC,s.id LIMIT 20".to_owned()),
    ];
    for (name, sql) in queries {
        let (seconds, result) = measured(|| sqlite_query(db, &sql, None), runs)?;
        let result = match name {
            "musenalm_multi_count" | "musenalm_title_search_count" => result[0][0].clone(),
            "musenalm_title_search_page" => Json::Array(
                result
                    .as_array()
                    .ok_or("search result missing")?
                    .iter()
                    .map(|row| row[0].clone())
                    .collect(),
            ),
            _ => result,
        };
        emit(name, seconds, result);
    }
    let start = Instant::now();
    db.execute_batch("CREATE INDEX records_title ON records(json_extract(data,'$.preferred_title'),id); CREATE INDEX records_state ON records(json_extract(data,'$.edit_state'));")?;
    emit("build_musenalm_list_indexes", elapsed(start), json!(true));
    for (name, sql, index) in [
        ("musenalm_title_page_indexed", title_page, "records_title"),
        (
            "musenalm_state_facets_indexed",
            state_facets,
            "records_state",
        ),
    ] {
        let plan = sqlite_query(db, &format!("EXPLAIN QUERY PLAN {sql}"), None)?;
        if !plan.to_string().contains(index) {
            return Err(format!("SQLite did not use {index}").into());
        }
        let (seconds, result) = measured(|| sqlite_query(db, sql, None), runs)?;
        emit(name, seconds, result);
    }
    Ok(())
}

fn run_sqlite(
    path: &Path,
    db_path: &Path,
    id: &str,
    agent: Option<&str>,
    related: Option<(&Path, &Path, &Path)>,
    in_memory: bool,
    runs: usize,
) -> Result<()> {
    let start = Instant::now();
    let mut db = sqlite_load(path, db_path, in_memory)?;
    emit(
        "build",
        elapsed(start),
        json!(if in_memory {
            "SQLite JSONB :memory:"
        } else {
            "SQLite JSONB disk"
        }),
    );
    emit("memory_after_build", 0.0, memory());
    let queries = [
        (
            "year_count",
            "SELECT count(*) FROM records WHERE json_extract(data,'$.year') BETWEEN 1800 AND 1899",
            None,
        ),
        (
            "year_sort100",
            "SELECT json_extract(data,'$.year'),id FROM records WHERE json_extract(data,'$.year') BETWEEN 1800 AND 1899 ORDER BY json_extract(data,'$.year'),id LIMIT 100",
            None,
        ),
        (
            "year_group",
            "SELECT json_extract(data,'$.year'),count(*) FROM records WHERE json_type(data,'$.year') IN ('integer','real') GROUP BY json_extract(data,'$.year') ORDER BY json_extract(data,'$.year')",
            None,
        ),
        (
            "id_lookup",
            "SELECT id,json_extract(data,'$.year') FROM records WHERE id=?1",
            Some(id),
        ),
    ];
    for (name, sql, param) in queries {
        let (time, result) = measured(|| sqlite_query(&db, sql, param), runs)?;
        let result = if name == "year_count" {
            result[0][0].clone()
        } else {
            result
        };
        emit(name, time, result);
    }
    if let Some(agent) = agent {
        let predicate = "EXISTS (SELECT 1 FROM json_each(data,'$.agents') a WHERE json_extract(a.value,'$.agent')=?1)";
        for (name, sql) in [
            (
                "agent_count",
                format!("SELECT count(*) FROM records WHERE {predicate}"),
            ),
            (
                "agent_ids",
                format!("SELECT id FROM records WHERE {predicate} ORDER BY id"),
            ),
        ] {
            let (time, result) = measured(|| sqlite_query(&db, &sql, Some(agent)), runs)?;
            let result = if name == "agent_count" {
                result[0][0].clone()
            } else {
                Json::Array(
                    result
                        .as_array()
                        .ok_or("IDs missing")?
                        .iter()
                        .map(|row| row[0].clone())
                        .collect(),
                )
            };
            emit(name, time, result);
        }
    }
    let start = Instant::now();
    db.execute_batch("CREATE INDEX records_year ON records(json_extract(data,'$.year'),id);")?;
    emit("build_year_view", elapsed(start), json!("expression index"));
    let plan = sqlite_query(&db, &format!("EXPLAIN QUERY PLAN {}", queries[1].1), None)?;
    if !plan.to_string().contains("records_year") {
        return Err("SQLite did not use the year index".into());
    }
    let (time, result) = measured(|| sqlite_query(&db, queries[1].1, None), runs)?;
    emit("year_sort100_cached", time, result);
    if let Some((agents_path, series_path, places_path)) = related {
        let start = Instant::now();
        sqlite_insert(&mut db, agents_path, "agents")?;
        sqlite_insert(&mut db, series_path, "series")?;
        sqlite_insert(&mut db, places_path, "places")?;
        emit(
            "build_related",
            elapsed(start),
            json!("related JSONB tables"),
        );
        emit("memory_after_related", 0.0, memory());
        run_sqlite_musenalm(&db, runs)?;
    }
    let start = Instant::now();
    db.execute(
        "UPDATE records SET data=jsonb_set(data,'$._bench_edit',jsonb(1)) WHERE id=?1",
        [id],
    )?;
    emit("update_one", elapsed(start), json!(true));
    let start = Instant::now();
    let tx = db.transaction()?;
    tx.execute("UPDATE records SET data=jsonb_set(data,'$._bench_edit',jsonb(1)) WHERE id IN (SELECT id FROM records ORDER BY id LIMIT 100)",[])?;
    tx.commit()?;
    emit("update_100", elapsed(start), json!(true));
    if !in_memory {
        db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        let storage: u64 = ["", "-wal", "-shm"]
            .iter()
            .filter_map(|suffix| fs::metadata(format!("{}{suffix}", db_path.display())).ok())
            .map(|metadata| metadata.len())
            .sum();
        emit("sqlite_storage_bytes", 0.0, json!(storage));
    }
    drop(db);
    if !in_memory {
        let start = Instant::now();
        let reopened = Connection::open(db_path)?;
        reopened.execute_batch("PRAGMA synchronous=NORMAL; PRAGMA cache_size=-262144;")?;
        emit("open_cached_db", elapsed(start), json!(true));
        drop(reopened);
        for suffix in ["", "-wal", "-shm"] {
            let _ = fs::remove_file(format!("{}{suffix}", db_path.display()));
        }
    }
    let start = Instant::now();
    let _db = sqlite_load(path, db_path, in_memory)?;
    emit("rebuild", elapsed(start), json!("SQLite JSONB"));
    emit("memory_peak", 0.0, memory());
    Ok(())
}

fn worker(
    engine: &str,
    input: &Path,
    db_path: &Path,
    id: &str,
    agent: Option<&str>,
    related: Option<(&Path, &Path, &Path)>,
    runs: usize,
) -> Result<()> {
    match engine {
        "qj" => run_qj(input, id, agent, related, runs),
        "sqlite" => run_sqlite(input, db_path, id, agent, related, false, runs),
        "sqlite-memory" => run_sqlite(input, db_path, id, agent, related, true, runs),
        _ => Err("unknown worker engine".into()),
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut repo = None;
    let mut collection = None;
    let mut synthetic_count = None;
    let mut work_dir = PathBuf::from("target");
    let mut worker_engine = None;
    let mut input = None;
    let mut db_path = None;
    let mut id = None;
    let mut agent = None;
    let mut agents_input = None;
    let mut series_input = None;
    let mut places_input = None;
    let mut runs = 7;
    let mut i = 0;
    while i < args.len() {
        let key = &args[i];
        if key == "--help" || key == "-h" {
            println!(
                "array-compare [--synthetic N | --repo PATH --collection NAME] [--work-dir PATH] [--runs N]"
            );
            return Ok(());
        }
        i += 1;
        let value = args.get(i).ok_or(format!("missing value for {key}"))?;
        match key.as_str() {
            "--repo" => repo = Some(PathBuf::from(value)),
            "--collection" => collection = Some(value.clone()),
            "--synthetic" => synthetic_count = Some(value.parse::<usize>()?),
            "--work-dir" => work_dir = PathBuf::from(value),
            "--worker" => worker_engine = Some(value.clone()),
            "--input" => input = Some(PathBuf::from(value)),
            "--db" => db_path = Some(PathBuf::from(value)),
            "--id" => id = Some(value.clone()),
            "--agent" => agent = Some(value.clone()),
            "--agents-input" => agents_input = Some(PathBuf::from(value)),
            "--series-input" => series_input = Some(PathBuf::from(value)),
            "--places-input" => places_input = Some(PathBuf::from(value)),
            "--runs" => runs = value.parse::<usize>()?,
            _ => return Err(format!("unknown option {key}").into()),
        }
        i += 1;
    }
    if runs == 0 {
        return Err("--runs must be positive".into());
    }
    if let Some(engine) = worker_engine {
        return worker(
            &engine,
            &input.ok_or("missing input")?,
            &db_path.ok_or("missing db")?,
            &id.ok_or("missing id")?,
            agent.as_deref(),
            agents_input
                .as_deref()
                .zip(series_input.as_deref())
                .zip(places_input.as_deref())
                .map(|((a, s), p)| (a, s, p)),
            runs,
        );
    }
    let temporary = tempfile::Builder::new()
        .prefix("objekte-array-compare-")
        .tempdir_in(&work_dir)?;
    let source = if let Some(count) = synthetic_count {
        let start = Instant::now();
        let source = synthetic(temporary.path(), count)?;
        eprintln!("generate {count} source files: {:.3} s", elapsed(start));
        source
    } else {
        collection_path(
            repo.as_ref().ok_or("--repo required")?,
            collection.as_deref().ok_or("--collection required")?,
        )?
    };
    let input = temporary.path().join("collection.ndjson");
    let start = Instant::now();
    let (count, id, agent) = prepare(&source, &input)?;
    eprintln!(
        "prepare shared NDJSON: {:.3} s; records={count}; bytes={}; agent={agent:?}",
        elapsed(start),
        fs::metadata(&input)?.len()
    );
    let related = if collection.as_deref() == Some("entries") {
        if let Some(repo) = repo.as_ref() {
            let config: Json =
                serde_json::from_slice(&fs::read(repo.join(".editor/repository.json"))?)?;
            if config["collections"]["agents"].is_string()
                && config["collections"]["series"].is_string()
                && config["collections"]["places"].is_string()
            {
                let agents_path = temporary.path().join("agents.ndjson");
                let series_path = temporary.path().join("series.ndjson");
                let places_path = temporary.path().join("places.ndjson");
                let start = Instant::now();
                let (agent_count, _, _) = prepare(&collection_path(repo, "agents")?, &agents_path)?;
                let (series_count, _, _) =
                    prepare(&collection_path(repo, "series")?, &series_path)?;
                let (places_count, _, _) =
                    prepare(&collection_path(repo, "places")?, &places_path)?;
                eprintln!(
                    "prepare related NDJSON: {:.3} s; agents={agent_count}; series={series_count}; places={places_count}",
                    elapsed(start)
                );
                Some((agents_path, series_path, places_path))
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };
    let binary = std::env::current_exe()?;
    let node_script = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bin/array_compare.mjs");
    let mut answers = std::collections::BTreeMap::<String, Json>::new();
    for engine in [
        "qj",
        "sqlite",
        "sqlite-memory",
        "node",
        "node-stream",
        "node-files",
        "node-files-async",
        "node-files-async16",
    ] {
        let mut command = if engine.starts_with("node") {
            let mut c = Command::new("node");
            c.arg("--expose-gc").arg(&node_script);
            c
        } else {
            let mut c = Command::new(&binary);
            c.arg("--worker").arg(engine);
            c
        };
        command
            .arg("--input")
            .arg(&input)
            .arg("--id")
            .arg(&id)
            .arg("--runs")
            .arg(runs.to_string());
        if let Some(agent) = &agent {
            command.arg("--agent").arg(agent);
        }
        if let Some((agents_path, series_path, places_path)) = &related {
            command.arg("--agents-input").arg(agents_path);
            command.arg("--series-input").arg(series_path);
            command.arg("--places-input").arg(places_path);
        }
        if engine.starts_with("node-files") {
            command.arg("--source-dir").arg(&source);
            if related.is_some() {
                let repo = repo.as_ref().ok_or("related source requires repository")?;
                command
                    .arg("--agents-source-dir")
                    .arg(collection_path(repo, "agents")?);
                command
                    .arg("--series-source-dir")
                    .arg(collection_path(repo, "series")?);
                command
                    .arg("--places-source-dir")
                    .arg(collection_path(repo, "places")?);
            }
        }
        if engine == "node-stream" {
            command.arg("--stream-ndjson").arg("1");
        }
        if engine.starts_with("node-files-async") {
            command.arg("--async-original").arg("1");
        }
        if engine == "node-files-async16" {
            command.env("UV_THREADPOOL_SIZE", "16");
        }
        if !engine.starts_with("node") {
            command.arg("--db").arg(temporary.path().join("sqlite.db"));
        }
        let output = command.output()?;
        if !output.status.success() {
            return Err(format!(
                "{engine} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        println!("\n{engine}:");
        for line in String::from_utf8(output.stdout)?.lines() {
            let row: Json = serde_json::from_str(line)?;
            let metric = row["metric"].as_str().ok_or("missing metric")?;
            let seconds = row["seconds"].as_f64().ok_or("missing timing")?;
            let result = &row["result"];
            println!(
                "  {metric:24} {seconds:>9.6} s  {}",
                if metric.starts_with("memory") {
                    result.to_string()
                } else if result.is_array() && result.as_array().is_some_and(|a| a.len() > 8) {
                    format!("{} rows", result.as_array().unwrap().len())
                } else {
                    result.to_string()
                }
            );
            if metric.starts_with("musenalm_")
                || [
                    "year_count",
                    "year_sort100",
                    "year_group",
                    "id_lookup",
                    "agent_count",
                    "agent_ids",
                    "year_sort100_cached",
                ]
                .contains(&metric)
            {
                if let Some(prior) = answers.get(metric) {
                    if prior != result {
                        return Err(format!("{engine} {metric} differs from prior engine").into());
                    }
                } else {
                    answers.insert(metric.to_owned(), result.clone());
                }
            }
        }
    }
    println!("\nAll comparable query results matched.");
    Ok(())
}
