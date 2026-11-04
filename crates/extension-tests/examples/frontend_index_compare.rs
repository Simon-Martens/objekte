//! Paired scan and explicit representation-index query over one real corpus.

use objekte_core::{
    Options, Repository, TextFormat, TextMode,
    extensions::{NativeTextOperator, RenderedRepresentation, StoredRepresentation},
};
use objekte_extension_tests::{ArchiveFrontend, PrefixOperator, PublicationYearExtractor};
use serde_json::{Value, json};
use std::{env, fs, io::Write, path::Path, time::Instant};

fn open(root: &Path, schema: &Path) -> Result<Repository, Box<dyn std::error::Error>> {
    let mut options = Options::new();
    options.query_cache_bytes = 0;
    Ok(Repository::builder(root)
        .without_default_schemas()
        .schema("contents", schema)
        .options(options)
        .register_representation(StoredRepresentation::default())?
        .register_representation(RenderedRepresentation::new(TextFormat::Markdown)?)?
        .register_operator(PrefixOperator::default())?
        .register_operator(NativeTextOperator::new(TextMode::Word, false))?
        .register_extractor(PublicationYearExtractor::default())?
        .register_frontend(ArchiveFrontend::default())?
        .open()?)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 3 {
        return Err("usage: frontend_index_compare REPOSITORY ROUNDS".into());
    }
    let root = Path::new(&args[1]);
    let rounds = args[2].parse::<usize>()?;
    let source_schema = root.join(".objekte/collections/contents.schema.json");
    let mut schema: Value = serde_json::from_slice(&fs::read(source_schema)?)?;
    schema["properties"]["annotation"]["x-full-text"] = json!("markdown");
    let mut temporary_schema = tempfile::NamedTempFile::new()?;
    temporary_schema.write_all(serde_json::to_string(&schema)?.as_bytes())?;
    temporary_schema.flush()?;
    let mut scan = open(root, temporary_schema.path())?;
    let mut indexed = open(root, temporary_schema.path())?;
    let request = json!({
        "collection":"contents", "term_path":"annotation", "term":"Goethe",
        "limit":1, "count":true,
    });
    let started = Instant::now();
    let indexed_sources =
        indexed.build_representation_index("contents", "objekte:visible-markdown", "annotation")?;
    let build_ms = started.elapsed().as_secs_f64() * 1000.0;
    scan.query_frontend("example:archive-search", &request)?;
    indexed.query_frontend("example:archive-search", &request)?;
    let mut samples = Vec::new();
    for round in 0..rounds {
        let order = if round % 2 == 0 {
            ["scan", "indexed"]
        } else {
            ["indexed", "scan"]
        };
        for label in order {
            let repository = if label == "scan" {
                &mut scan
            } else {
                &mut indexed
            };
            let started = Instant::now();
            let result = repository.query_frontend("example:archive-search", &request)?;
            samples.push(json!({
                "round": round,
                "path": label,
                "ms": started.elapsed().as_secs_f64() * 1000.0,
                "total": result.total,
                "truncated": result.truncated,
                "first_id": result.rows.first().map(|row| row.source.record_id.as_str()),
                "source_reads": result.metrics.source_reads,
                "parses": result.metrics.parses,
                "candidates": result.metrics.fts_candidates,
                "plan": result.query_plan,
            }));
        }
    }
    let complete = json!({
        "collection":"contents", "term_path":"annotation", "term":"Goethe",
        "limit":10_000, "count":true,
    });
    let direct = scan.query_frontend("example:archive-search", &complete)?;
    let accelerated = indexed.query_frontend("example:archive-search", &complete)?;
    let ordered = |result: &objekte_core::ExtensionQueryResult| {
        result
            .rows
            .iter()
            .map(|row| row.source.record_id.clone())
            .collect::<Vec<_>>()
    };
    if direct.total != accelerated.total
        || direct.truncated != accelerated.truncated
        || ordered(&direct) != ordered(&accelerated)
    {
        return Err("scan and indexed complete results differ".into());
    }
    println!(
        "{}",
        json!({
            "indexed_sources": indexed_sources,
            "index_build_ms": build_ms,
            "samples": samples,
            "total": direct.total,
            "truncated": direct.truncated,
            "ordered_ids": ordered(&direct),
        })
    );
    Ok(())
}
