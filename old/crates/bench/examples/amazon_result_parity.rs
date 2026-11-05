//! Write every matching Amazon review ID for comparison with an external JSON evaluator.

use objekte_core::{Error, JsonQuery, JsonQueryBackend, Options, Repository};
use serde_json::json;
use std::{
    env,
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
};


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
    if args.next().is_some() {
        return Err("expected REPOSITORY BACKEND FILTER OUTPUT.ids".into());
    }

    let mut options = Options::new();
    options.query_cache_bytes = 0;
    options.total_limit = 1_000_001;
    let mut repo = Repository::load_repository(&root, options)?;
    repo.warm_all_bigram_indexes()?;

    let request = JsonQuery {
        filter: filter.clone(),
        collections: Some(vec!["reviews".into()]),
        count: true,
        limit: 1_000_000,
        backend,
        ..Default::default()
    };
    let mut writer = BufWriter::new(File::create(&output)?);
    let result = repo.query_json_stream(&request, |_, record| {
        writeln!(writer, "{}", record.id).map_err(|source| Error::Io {
            path: output.clone(),
            source,
        })
    })?;
    writer.flush()?;
    if result.truncated || result.total != Some(result.returned) {
        return Err(format!(
            "incomplete result: returned {}, total {:?}, truncated {}",
            result.returned, result.total, result.truncated
        )
        .into());
    }
    println!(
        "{}",
        json!({
            "repository": root,
            "filter": filter,
            "backend": if backend == JsonQueryBackend::Bigram { "bigram" } else { "direct" },
            "returned": result.returned,
            "total": result.total,
            "truncated": result.truncated,
            "scans": result.metrics.grep_scans,
            "source_reads": result.metrics.source_reads,
            "parses": result.metrics.parses,
            "predicate_cache_hits": result.metrics.predicate_cache_hits,
            "ordered_result_hits": result.metrics.ordered_result_hits,
            "query_plan": result.query_plan,
            "output": output,
        })
    );
    Ok(())
}
