//! Focused ordinary text-definition benchmark. Emits NDJSON, one row per call.

use objekte_core::{
    Direction, JsonQuery, JsonQueryBackend, Options, Repository, Result, Sort, TextFieldSelector,
    TextQuery,
    extensions::{
        BindingContext, DataKind, Dependency, Descriptor, SemanticVersion, TextComparison,
        TextPreparation, TextRepresentation, TextSearchDefinition,
    },
};
use objekte_extension_tests::{MarkdownDecoder, MarkdownProvider};
use serde_json::{Value, json};
use std::{
    env, fs,
    path::Path,
    time::{Duration, Instant, SystemTime},
};


struct Expanded(Descriptor);


impl Expanded {
    fn new() -> Self {
        Self(Descriptor::new(
            "example:benchmark-expanded-values",
            SemanticVersion {
                major: 1,
                minor: 0,
                patch: 0,
            },
            &(),
            DataKind::Text,
            DataKind::Text,
            vec![Dependency::SourceBytes],
        ))
    }
}


impl TextRepresentation for Expanded {
    fn descriptor(&self) -> &Descriptor {
        &self.0
    }

    fn binds(&self, context: &BindingContext<'_>) -> bool {
        context.path == "body"
    }

    fn preparation(&self) -> TextPreparation {
        TextPreparation::Stored
    }

    fn render<'a>(&self, value: &'a str, output: &'a mut String) -> Result<Option<&'a str>> {
        if !value.contains("MS") {
            return Ok(Some(value));
        }
        output.clear();
        let mut rest = value;
        while let Some(index) = rest.find("MS") {
            output.push_str(&rest[..index]);
            output.push_str("manuscript");
            rest = &rest[index + 2..];
        }
        output.push_str(rest);
        Ok(Some(output))
    }
}


fn open(root: &Path, budget_mib: usize, indexed: bool) -> Result<Repository> {
    let mut options = Options::new();
    options.query_cache_bytes = budget_mib * 1024 * 1024;
    if !indexed {
        options.cache = objekte_core::CacheMode::Disabled;
    }
    Repository::builder(root)
        .options(options)
        .register_source(MarkdownProvider::new(root.join("data/docs")))?
        .register_format(MarkdownDecoder::default())?
        .register_representation(Expanded::new())?
        .register_text_definition(
            TextSearchDefinition::new(
                Descriptor::new(
                    "example:benchmark-expanded",
                    SemanticVersion {
                        major: 1,
                        minor: 0,
                        patch: 0,
                    },
                    &(),
                    DataKind::Text,
                    DataKind::Predicate,
                    vec![Dependency::SourceBytes],
                ),
                "example:benchmark-expanded-values",
            )
            .alias("expanded")
            .operator(TextComparison::Contains, "objekte:word-raw")
            .operator(TextComparison::Equal, "objekte:word-raw"),
        )?
        .open()
}

fn fixture(root: &Path) -> std::io::Result<()> {
    fn write_source(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        fs::write(path, bytes)?;
        let fixed = SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        fs::File::options()
            .write(true)
            .open(path)?
            .set_times(fs::FileTimes::new().set_modified(fixed))
    }
    for name in ["docs", "native"] {
        fs::create_dir_all(root.join(format!("data/{name}")))?;
        fs::create_dir_all(root.join(".objekte/collections"))?;
        let search = if name == "docs" {
            r#", "x-text-search":{"default":"example:benchmark-expanded","definitions":{"example:benchmark-expanded":{"index":true},"objekte:markdown":{"index":true}}}"#
        } else {
            r#", "x-full-text":"plain""#
        };
        let schema = format!(
            r#"{{"type":"object","properties":{{"body":{{"type":"string"{search}}},"year":{{"type":"integer","index":true}}}},"x-objekte":{{"name":"{name}","path":"data/{name}","type":"data","format":"json","id":"filename"}}}}"#
        );
        fs::write(
            root.join(format!(".objekte/collections/{name}.schema.json")),
            schema,
        )?;
        for index in 0..512 {
            let body = if index == 7 {
                "a rare MS witness"
            } else if index == 397 {
                "a rare manuscript witness"
            } else if index % 4 == 0 {
                "a broad MS catalog"
            } else {
                "ordinary catalog entry"
            };
            let record = json!({"body":body,"year":if index == 7 {1848} else {1850}});
            write_source(
                &root.join(format!("data/{name}/{index:04}.json")),
                &serde_json::to_vec(&record)?,
            )?;
        }
    }
    write_source(&root.join("data/docs/extra.md"), b"a rare MS witness")?;
    Ok(())
}

fn directory_bytes(path: &Path) -> std::io::Result<u64> {
    let mut bytes = 0_u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        bytes += if metadata.is_dir() {
            directory_bytes(&entry.path())?
        } else {
            metadata.len()
        };
    }
    Ok(bytes)
}

fn text(pattern: &str, search: &str, sorted: bool) -> TextQuery {
    TextQuery {
        pattern: pattern.into(),
        collections: Some(vec!["docs".into()]),
        fields: Some(vec![TextFieldSelector::with_search("body", search)]),
        count: true,
        limit: 10,
        sort: if sorted {
            vec![Sort {
                field: "@id".into(),
                direction: Direction::Asc,
            }]
        } else {
            Vec::new()
        },
        ..Default::default()
    }
}

fn json_query(filter: &str, collection: &str, backend: JsonQueryBackend) -> JsonQuery {
    JsonQuery {
        filter: filter.into(),
        collections: Some(vec![collection.into()]),
        count: true,
        limit: 10,
        backend,
        ..Default::default()
    }
}

fn emit(
    (name, budget, round): (&str, usize, usize),
    ms: f64,
    total: Option<usize>,
    ids: Vec<String>,
    metrics: &objekte_core::Metrics,
    plan: Value,
) {
    println!(
        "{}",
        json!({"case":name,"cache_mib":budget,"round":round,"ms":ms,
        "total":total,"ids":ids,"source_reads":metrics.source_reads,
        "parses":metrics.parses,"candidates":metrics.selected_candidate_visits,
        "fts_candidates":metrics.fts_candidates,"predicate_cache_hits":metrics.predicate_cache_hits,
        "ordered_result_hits":metrics.ordered_result_hits,"plan":plan})
    );
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 3 {
        return Err("usage: text_search_integration_bench CACHE_MIB ROUNDS".into());
    }
    let budget = args[1].parse::<usize>()?;
    let rounds = args[2].parse::<usize>()?;
    let directory = tempfile::tempdir()?;
    fixture(directory.path())?;
    let started = Instant::now();
    let mut indexed = open(directory.path(), budget, true)?;
    indexed.warm_bigram_index("native")?;
    indexed.warm_bigram_index("docs")?;
    indexed.build_text_search_index("native", "example:benchmark-expanded", "body")?;
    let build_ms = started.elapsed().as_secs_f64() * 1000.0;
    indexed.save_bigram_cache("docs")?;
    indexed.save_bigram_cache("native")?;
    let snapshot_bytes = directory_bytes(&directory.path().join(".objekte/.cache"))?;
    drop(indexed);
    let started = Instant::now();
    let mut indexed = open(directory.path(), budget, true)?;
    indexed.warm_bigram_index("docs")?;
    indexed.warm_bigram_index("native")?;
    let restore_ms = started.elapsed().as_secs_f64() * 1000.0;
    let mut direct = open(directory.path(), 0, false)?;
    println!(
        "{}",
        json!({"case":"setup","cache_mib":budget,"build_ms":build_ms,
        "restore_ms":restore_ms,"snapshot_bytes":snapshot_bytes,
        "sources_per_collection":512,"custom_sources":1})
    );
    let text_cases = [
        (
            "custom_selective",
            text("rare manuscript", "expanded", false),
        ),
        ("custom_broad", text("manuscript", "expanded", false)),
        ("custom_absent", text("unobtainable", "expanded", false)),
        (
            "second_definition",
            text("rare manuscript", "markdown", false),
        ),
        ("custom_sorted", text("manuscript", "expanded", true)),
    ];
    let json_cases = [
        (
            "native_scalar",
            json_query("year = 1848", "native", JsonQueryBackend::Auto),
        ),
        (
            "native_fts",
            json_query("body:fts ~ 'witness'", "native", JsonQueryBackend::Auto),
        ),
        (
            "custom_and",
            json_query(
                "body:expanded ~ 'manuscript' && year = 1848",
                "docs",
                JsonQueryBackend::Auto,
            ),
        ),
        (
            "custom_or",
            json_query(
                "body:expanded ~ 'unobtainable' || year = 1848",
                "docs",
                JsonQueryBackend::Auto,
            ),
        ),
        (
            "custom_direct",
            json_query(
                "body:expanded ~ 'rare manuscript'",
                "docs",
                JsonQueryBackend::Direct,
            ),
        ),
        (
            "custom_native_index",
            json_query(
                "body:expanded ~ 'rare manuscript'",
                "native",
                JsonQueryBackend::Auto,
            ),
        ),
        (
            "custom_native_direct",
            json_query(
                "body:expanded ~ 'rare manuscript'",
                "native",
                JsonQueryBackend::Direct,
            ),
        ),
    ];
    for (name, request) in &text_cases {
        let complete = TextQuery {
            limit: 2000,
            ..request.clone()
        };
        let expected = direct.search_text(&complete)?;
        let actual = indexed.search_text(&complete)?;
        assert_eq!(expected.total, actual.total);
        assert_eq!(
            expected
                .hits
                .iter()
                .map(|hit| (
                    &hit.record.collection,
                    &hit.record.id,
                    &hit.fields,
                    &hit.matched_paths,
                    &hit.matched_definitions
                ))
                .collect::<Vec<_>>(),
            actual
                .hits
                .iter()
                .map(|hit| (
                    &hit.record.collection,
                    &hit.record.id,
                    &hit.fields,
                    &hit.matched_paths,
                    &hit.matched_definitions
                ))
                .collect::<Vec<_>>()
        );
        println!(
            "{}",
            json!({"case":"signature","query":name,"cache_mib":budget,
            "total":actual.total,"ids":actual.hits.iter()
                .map(|hit| (&hit.record.collection,&hit.record.id)).collect::<Vec<_>>()})
        );
    }
    for (name, request) in &json_cases {
        let complete = JsonQuery {
            limit: 2000,
            ..request.clone()
        };
        let expected = direct.query_json(&complete)?;
        let actual = indexed.query_json(&complete)?;
        assert_eq!(expected.total, actual.total);
        assert_eq!(
            expected
                .records
                .iter()
                .map(|record| (&record.collection, &record.id, &record.data))
                .collect::<Vec<_>>(),
            actual
                .records
                .iter()
                .map(|record| (&record.collection, &record.id, &record.data))
                .collect::<Vec<_>>()
        );
        println!(
            "{}",
            json!({"case":"signature","query":name,"cache_mib":budget,
            "total":actual.total,"ids":actual.records.iter()
                .map(|record| (&record.collection,&record.id)).collect::<Vec<_>>()})
        );
    }
    for round in 0..rounds {
        for (name, request) in &text_cases {
            let started = Instant::now();
            let result = indexed.search_text(request)?;
            emit(
                (name, budget, round),
                started.elapsed().as_secs_f64() * 1000.0,
                result.total,
                result
                    .hits
                    .iter()
                    .map(|hit| hit.record.id.clone())
                    .collect(),
                &result.metrics,
                serde_json::to_value(&result.query_plan)?,
            );
        }
        for (name, request) in &json_cases {
            let started = Instant::now();
            let result = indexed.query_json(request)?;
            emit(
                (name, budget, round),
                started.elapsed().as_secs_f64() * 1000.0,
                result.total,
                result
                    .records
                    .iter()
                    .map(|record| record.id.clone())
                    .collect(),
                &result.metrics,
                serde_json::to_value(&result.query_plan)?,
            );
        }
        let streaming = text("rare manuscript", "expanded", false);
        let mut ids = Vec::new();
        let started = Instant::now();
        let summary = indexed.search_text_stream(&streaming, |_, hit| {
            ids.push(hit.record.id.clone());
            Ok(())
        })?;
        emit(
            ("custom_stream", budget, round),
            started.elapsed().as_secs_f64() * 1000.0,
            summary.total,
            ids,
            &summary.metrics,
            serde_json::to_value(&summary.query_plan)?,
        );
    }
    Ok(())
}
