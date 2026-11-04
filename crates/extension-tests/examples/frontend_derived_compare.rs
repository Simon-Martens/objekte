//! Deterministic selected-source typed-posting control with cache disabled.

use objekte_core::{
    Options, Repository, SelectedIndexRequest, TextFormat, TextMode,
    extensions::{NativeTextOperator, RenderedRepresentation, StoredRepresentation},
};
use objekte_extension_tests::{
    ArchiveFrontend, MarkdownDecoder, MarkdownProvider, PrefixOperator, PublicationYearExtractor,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{env, fs, path::Path, time::Instant};


fn open(root: &Path, directory: &Path) -> Result<Repository, Box<dyn std::error::Error>> {
    let mut options = Options::new();
    options.query_cache_bytes = 0;
    Ok(Repository::builder(root)
        .options(options)
        .register_source(MarkdownProvider::new(directory.to_path_buf()))?
        .register_format(MarkdownDecoder::default())?
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
    if !(args.len() == 2 || args.len() == 3 && args[2] == "sort") {
        return Err("usage: frontend_derived_compare ROUNDS [sort]".into());
    }
    let rounds = args[1].parse::<usize>()?;
    let sort_year = args.len() == 3;
    let temp = tempfile::tempdir()?;
    let directory = temp.path().join("data/archive");
    fs::create_dir_all(&directory)?;
    let mut fingerprint = Sha256::new();
    for index in 0..20_000 {
        let name = format!("doc-{index:05}.json");
        let date = if index % 100 == 0 {
            "circa 1852"
        } else {
            "1800"
        };
        let contents =
            format!(r#"{{"body":"Archive record {index}","publication_date":"{date}"}}"#);
        fingerprint.update(name.as_bytes());
        fingerprint.update(contents.as_bytes());
        fs::write(directory.join(name), contents)?;
    }
    for index in 0..100 {
        let name = format!("note-{index:03}.md");
        let contents = format!("# Note {index}\n");
        fingerprint.update(name.as_bytes());
        fingerprint.update(contents.as_bytes());
        fs::write(directory.join(name), contents)?;
    }
    let fixture_sha256 = format!("{:x}", fingerprint.finalize());
    let mut scan = open(temp.path(), &directory)?;
    let mut indexed = open(temp.path(), &directory)?;
    let started = Instant::now();
    let indexed_sources =
        indexed.build_selected_derived_posting("archive", "example:publication_year")?;
    let build_ms = started.elapsed().as_secs_f64() * 1000.0;
    let request = json!({
        "collection":"archive", "min_year":1850, "count":true, "limit":1,
        "sort_year":sort_year,
    });
    let started = Instant::now();
    scan.query_frontend("example:archive-search", &request)?;
    let scan_warmup_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    indexed.query_frontend("example:archive-search", &request)?;
    let indexed_warmup_ms = started.elapsed().as_secs_f64() * 1000.0;
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
                "round":round, "path":label,
                "ms":started.elapsed().as_secs_f64() * 1000.0,
                "total":result.total, "truncated":result.truncated,
                "source_reads":result.metrics.source_reads,
                "parses":result.metrics.parses,
                "plan":result.query_plan,
            }));
        }
    }
    let complete = json!({
        "collection":"archive", "min_year":1850, "count":true, "limit":1000,
        "sort_year":sort_year,
    });
    let direct = scan.query_frontend("example:archive-search", &complete)?;
    let accelerated = indexed.query_frontend("example:archive-search", &complete)?;
    let ids = |result: &objekte_core::ExtensionQueryResult| {
        result
            .rows
            .iter()
            .map(|row| row.source.record_id.clone())
            .collect::<Vec<_>>()
    };
    if direct.total != accelerated.total
        || direct.truncated != accelerated.truncated
        || ids(&direct) != ids(&accelerated)
    {
        return Err("scan and indexed derived results differ".into());
    }
    let mut fused = open(temp.path(), &directory)?;
    fused.reset_metrics();
    let started = Instant::now();
    fused.build_selected_indexes(
        "archive",
        &[
            SelectedIndexRequest::Text {
                representation: "objekte:stored",
                path: "body",
            },
            SelectedIndexRequest::Derived {
                extractor: "example:publication_year",
            },
        ],
    )?;
    let fused_build_ms = started.elapsed().as_secs_f64() * 1000.0;
    let fused_build_reads = fused.metrics().source_reads;
    let fused_build_parses = fused.metrics().parses;
    let fused_result = fused.query_frontend("example:archive-search", &complete)?;
    if direct.total != fused_result.total
        || direct.truncated != fused_result.truncated
        || ids(&direct) != ids(&fused_result)
    {
        return Err("fused-build derived results differ".into());
    }
    println!(
        "{}",
        json!({
            "fixture_sha256":fixture_sha256,
            "sort_year":sort_year,
            "indexed_sources":indexed_sources,
            "index_build_ms":build_ms,
            "scan_warmup_ms":scan_warmup_ms,
            "indexed_warmup_ms":indexed_warmup_ms,
            "fused_build_ms":fused_build_ms,
            "fused_build_reads":fused_build_reads,
            "fused_build_parses":fused_build_parses,
            "samples":samples,
            "total":direct.total,
            "truncated":direct.truncated,
            "ordered_ids":ids(&direct),
        })
    );
    Ok(())
}
