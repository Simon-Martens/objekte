//! Native and registered built-in text search over the same repository.

use objekte_core::{
    Options, Repository, TextFormat, TextHit, TextMode, TextQuery,
    extensions::{NativeTextOperator, RenderedRepresentation},
};
use serde_json::{Value, json};
use std::{env, fs, io::Write, path::Path, time::Instant};

fn ids(hits: &[TextHit]) -> Vec<(&str, &Vec<String>, &Vec<String>)> {
    hits.iter()
        .map(|hit| (hit.record.id.as_str(), &hit.fields, &hit.matched_paths))
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 4 {
        return Err("usage: text_search_compare REPOSITORY ROUNDS QUERY_CACHE_MIB".into());
    }
    let rounds = args[2].parse::<usize>()?;
    let cache_mib = args[3].parse::<usize>()?;
    let source_schema = Path::new(&args[1]).join(".objekte/collections/contents.schema.json");
    let mut schema: Value = serde_json::from_slice(&fs::read(source_schema)?)?;
    schema["properties"]["annotation"]["x-full-text"] = json!("markdown");
    schema["properties"]["preferred_title"]["x-full-text"] = json!("plain");
    schema["properties"]["title_statement"]["x-full-text"] = json!("html");
    schema["properties"]["contributors"]["items"]["properties"]["annotation"]["x-full-text"] =
        json!("xml");
    let mut temporary_schema = tempfile::NamedTempFile::new()?;
    temporary_schema.write_all(serde_json::to_string(&schema)?.as_bytes())?;
    temporary_schema.flush()?;
    let mut options = Options::new();
    options.query_cache_bytes = cache_mib * 1024 * 1024;
    let mut builder = Repository::builder(Path::new(&args[1]))
        .without_default_schemas()
        .schema("contents", temporary_schema.path())
        .options(options);
    for format in [
        TextFormat::Plain,
        TextFormat::Markdown,
        TextFormat::Html,
        TextFormat::Xml,
    ] {
        builder = builder.register_representation(RenderedRepresentation::new(format)?)?;
    }
    let mut repository = builder
        .register_operator(NativeTextOperator::new(TextMode::Word, false))?
        .open()?;

    for (field, term, representation) in [
        ("annotation", "Goethe", "objekte:visible-markdown"),
        ("preferred_title", "Goethe", "objekte:visible-plain"),
        ("title_statement", "Verlag", "objekte:visible-html"),
        ("contributors[].annotation", "Goethe", "objekte:visible-xml"),
    ] {
        let request = TextQuery {
            pattern: term.into(),
            collections: Some(vec!["contents".into()]),
            fields: Some(vec![field.into()]),
            count: true,
            limit: 1,
            ..Default::default()
        };
        // Build the index and warm each predicate independently of timing.
        repository.search_text(&request)?;
        repository.search_text_with(&request, representation, "objekte:word")?;
        let mut samples = Vec::new();
        for round in 0..rounds {
            let order = if round % 2 == 0 {
                ["native", "registered"]
            } else {
                ["registered", "native"]
            };
            for label in order {
                let started = Instant::now();
                let result = if label == "native" {
                    repository.search_text(&request)?
                } else {
                    repository.search_text_with(&request, representation, "objekte:word")?
                };
                samples.push(json!({
                    "round": round,
                    "path": label,
                    "ms": started.elapsed().as_secs_f64() * 1000.0,
                    "total": result.total,
                    "truncated": result.truncated,
                    "first_id": result.hits.first().map(|hit| hit.record.id.as_str()),
                    "first_fields": result.hits.first().map(|hit| &hit.fields),
                    "first_paths": result.hits.first().map(|hit| &hit.matched_paths),
                    "candidates": result.metrics.fts_candidates,
                    "scans": result.metrics.grep_scans,
                    "parses": result.metrics.parses,
                    "cache_hits": result.metrics.predicate_cache_hits,
                }));
            }
        }
        // A complete ordered result, including fields and concrete paths,
        // is collected after timing for semantic comparison.
        let full = TextQuery {
            limit: 10_000,
            ..request
        };
        let native = repository.search_text(&full)?;
        let registered = repository.search_text_with(&full, representation, "objekte:word")?;
        println!(
            "{}",
            json!({
                "field": field,
                "term": term,
                "representation": representation,
                "cache_mib": cache_mib,
                "samples": samples,
                "native_total": native.total,
                "registered_total": registered.total,
                "native_truncated": native.truncated,
                "registered_truncated": registered.truncated,
                "native_ordered": ids(&native.hits),
                "registered_ordered": ids(&registered.hits),
            })
        );
    }
    Ok(())
}
