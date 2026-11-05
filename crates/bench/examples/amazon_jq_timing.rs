//! Time a complete Amazon result stream before or after one query warmup.

use objekte_core::{Error, JsonQuery, JsonQueryBackend, Options, Repository};
use serde_json::{json, Value};
use std::{
    env,
    fs::File,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    time::Instant,
};

fn execute(
    repo: &mut Repository,
    request: &JsonQuery,
    output: &Path,
) -> Result<(Value, f64), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let mut writer = BufWriter::new(File::create(output)?);
    let result = repo.query_json_stream(request, |_, record| {
        writeln!(writer, "{}", record.id).map_err(|source| Error::Io {
            path: output.to_owned(),
            source,
        })
    })?;
    writer.flush()?;
    let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
    if result.truncated || result.total != Some(result.returned) {
        return Err(format!(
            "incomplete result: returned {}, total {:?}, truncated {}",
            result.returned, result.total, result.truncated
        )
        .into());
    }
    Ok((
        json!({
            "returned": result.returned,
            "total": result.total,
            "truncated": result.truncated,
            "scans": result.metrics.grep_scans,
            "source_reads": result.metrics.source_reads,
            "parses": result.metrics.parses,
            "predicate_cache_hits": result.metrics.predicate_cache_hits,
            "ordered_result_hits": result.metrics.ordered_result_hits,
        }),
        elapsed_ms,
    ))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let root = PathBuf::from(args.next().ok_or("expected REPOSITORY")?);
    let backend = match args.next().as_deref() {
        Some("bigram") => JsonQueryBackend::Bigram,
        Some("direct") => JsonQueryBackend::Direct,
        _ => return Err("backend must be bigram or direct".into()),
    };
    let filter = args.next().ok_or("expected FILTER")?;
    let output = PathBuf::from(args.next().ok_or("expected OUTPUT.ids")?);
    let warmup = match args.next().as_deref() {
        Some("warm") => true,
        Some("first") => false,
        _ => return Err("expected first or warm".into()),
    };
    if args.next().is_some() {
        return Err("expected REPOSITORY BACKEND FILTER OUTPUT.ids first|warm".into());
    }

    let mut options = Options::new();
    options.query_cache_bytes = 0;
    options.total_limit = 1_000_001;
    let started = Instant::now();
    let mut repo = Repository::load_repository(&root, options)?;
    let open_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let request = JsonQuery {
        filter: filter.clone(),
        collections: Some(vec!["reviews".into()]),
        count: true,
        limit: 1_000_000,
        backend,
        ..Default::default()
    };
    let warmup_ms = if warmup {
        let (_, elapsed_ms) = execute(&mut repo, &request, Path::new("/dev/null"))?;
        Some(elapsed_ms)
    } else {
        None
    };
    let (result, query_ms) = execute(&mut repo, &request, &output)?;
    println!(
        "{}",
        json!({
            "engine": "objekte",
            "backend": if backend == JsonQueryBackend::Bigram { "bigram" } else { "direct" },
            "filter": filter,
            "phase": if warmup { "warm" } else { "first" },
            "open_ms": open_ms,
            "warmup_ms": warmup_ms,
            "query_ms": query_ms,
            "result": result,
        })
    );
    Ok(())
}
