#[path = "../../../crates/core/examples/support/musenalm.rs"]
mod musenalm;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use objekte_search_core::query::options::ReturnMode;
use objekte_search_core::repository::configuration::RepositoryOptions;
use objekte_search_core::workload::load_extended_manifest;
use serde::Serialize;
use tantivy::collector::Count;
use tantivy::schema::{JsonObjectOptions, STORED, STRING, Schema, TextFieldIndexing};
use tantivy::{Index, doc};

#[derive(Serialize)]
struct InventoryRow {
    source_id: String,
    schema_mode: &'static str,
    original: objekte_search_core::workload::WorkloadCase,
    effective_count: EffectiveRequest,
    effective_page: EffectiveRequest,
    predicate_status: &'static str,
    page_status: &'static str,
    reason: String,
}

#[derive(Serialize)]
struct EffectiveRequest {
    collections: Vec<String>,
    fts: Option<objekte_search_core::query::FtsQuery>,
    filter: Option<objekte_search_core::query::FilterExpr>,
    sort: Vec<objekte_search_core::query::options::SortField>,
    offset: usize,
    limit: usize,
    count: bool,
    return_mode: ReturnMode,
    evidence: bool,
}

fn effective(query: &objekte_search_core::query::Query) -> EffectiveRequest {
    EffectiveRequest {
        collections: query.collections.clone(),
        fts: query.fts.clone(),
        filter: query.filter.clone(),
        sort: query.options.sort.clone(),
        offset: query.options.offset,
        limit: query.options.limit,
        count: query.options.count,
        return_mode: query.options.return_mode.clone(),
        evidence: query.options.evidence,
    }
}

fn inventory(core: &Path, output: &Path) -> Result<(), String> {
    let baseline = core.join("benches/musenalm_queries.json");
    let extended = core.join("benches/musenalm_queries_extended.json");
    let cases = load_extended_manifest(&baseline, &extended)?;
    let mut rows = Vec::with_capacity(cases.len());
    for case in cases {
        let source = case.query()?;
        let mut count = source.clone();
        count.options.count = true;
        count.options.limit = 0;
        count.options.offset = 0;
        count.options.sort.clear();
        count.options.return_mode = ReturnMode::Locator;
        count.options.evidence = false;
        let mut page = source;
        page.options.count = true;
        page.options.return_mode = ReturnMode::Locator;
        page.options.evidence = false;
        rows.push(InventoryRow {
            source_id: case.id.clone(),
            schema_mode: if case.id.as_str() <= "Q150" {
                "historical"
            } else {
                "extended"
            },
            original: case.request,
            effective_count: effective(&count),
            effective_page: effective(&page),
            predicate_status: "unsupported",
            page_status: "unsupported",
            reason: "Tantivy translation and parity verification pending".into(),
        });
    }
    fs::write(
        output,
        serde_json::to_vec_pretty(&rows).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

#[derive(Serialize)]
struct BuildObservation {
    engine: &'static str,
    elapsed_ns: u128,
    documents: usize,
    collection_counts: BTreeMap<String, usize>,
    peak_rss_kb: Option<usize>,
}

fn peak_rss_kb() -> Option<usize> {
    fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmHWM:")?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
}

fn build_tantivy(root: &Path) -> Result<BuildObservation, String> {
    let start = Instant::now();
    let mut builder = Schema::builder();
    let collection = builder.add_text_field("collection", STRING | STORED);
    let record_id = builder.add_text_field("record_id", STRING | STORED);
    let relative_path = builder.add_text_field("relative_path", STORED);
    let source_json = builder.add_json_field(
        "source",
        JsonObjectOptions::default()
            .set_stored()
            .set_fast("raw")
            .set_indexing_options(TextFieldIndexing::default().set_tokenizer("raw")),
    );
    let schema = builder.build();
    let index = Index::create_in_ram(schema);
    let mut writer = index
        .writer(100_000_000)
        .map_err(|error| error.to_string())?;
    let mut counts = BTreeMap::new();
    let mut identities = BTreeSet::new();
    let mut schema_paths = fs::read_dir(root.join(".objekte/collections"))
        .map_err(|error| error.to_string())?
        .map(|entry| {
            entry
                .map(|value| value.path())
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    schema_paths.sort();
    for schema_path in schema_paths {
        if !schema_path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with(".schema.json"))
        {
            continue;
        }
        let schema: serde_json::Value =
            serde_json::from_slice(&fs::read(&schema_path).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
        let metadata = schema.get("x-objekte").ok_or("schema missing x-objekte")?;
        let name = metadata
            .get("name")
            .and_then(|value| value.as_str())
            .ok_or("schema missing collection name")?;
        let source = metadata
            .get("path")
            .and_then(|value| value.as_str())
            .ok_or("schema missing source path")?;
        let mut files = fs::read_dir(root.join(source))
            .map_err(|error| error.to_string())?
            .map(|entry| {
                entry
                    .map(|value| value.path())
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        files.sort();
        let mut count = 0;
        for path in files {
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let bytes = fs::read(&path).map_err(|error| error.to_string())?;
            let value: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            let id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .ok_or("invalid filename stem")?;
            let relative = path
                .strip_prefix(root)
                .map_err(|error| error.to_string())?
                .to_string_lossy();
            if !identities.insert((name.to_owned(), id.to_owned())) {
                return Err(format!("duplicate identity {name}/{id}"));
            }
            writer.add_document(doc!(collection => name, record_id => id, relative_path => relative.as_ref(), source_json => value))
                .map_err(|error| error.to_string())?;
            count += 1;
        }
        counts.insert(name.to_owned(), count);
    }
    writer.commit().map_err(|error| error.to_string())?;
    writer
        .wait_merging_threads()
        .map_err(|error| error.to_string())?;
    let reader = index.reader().map_err(|error| error.to_string())?;
    let searcher = reader.searcher();
    let visible = searcher
        .search(&tantivy::query::AllQuery, &Count)
        .map_err(|error| error.to_string())?;
    if visible != identities.len() {
        return Err(format!(
            "Tantivy has {visible} visible records but discovered {}",
            identities.len()
        ));
    }
    Ok(BuildObservation {
        engine: "tantivy",
        elapsed_ns: start.elapsed().as_nanos(),
        documents: visible,
        collection_counts: counts,
        peak_rss_kb: peak_rss_kb(),
    })
}

fn build_objekte(root: &Path) -> Result<BuildObservation, String> {
    let start = Instant::now();
    let definitions = musenalm::load_definitions(root)?;
    let index = objekte_search_core::lifecycle::Index::start_build(
        root,
        definitions,
        RepositoryOptions {
            verified_cache_bytes: 0,
            query_workers: 1,
            ..RepositoryOptions::default()
        },
        objekte_search_core::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .map_err(|error| error.to_string())?;
    let view = index.view().map_err(|error| error.to_string())?;
    let visible = view.index_observation().records;
    Ok(BuildObservation {
        engine: "objekte",
        elapsed_ns: start.elapsed().as_nanos(),
        documents: visible,
        collection_counts: BTreeMap::new(),
        peak_rss_kb: peak_rss_kb(),
    })
}

fn run() -> Result<(), String> {
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() != 4 {
        return Err(
            "usage: objekte-tantivy-benchmark inventory|build-objekte|build-tantivy ROOT OUTPUT"
                .into(),
        );
    }
    let root = PathBuf::from(&args[2])
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let output = PathBuf::from(&args[3]);
    match args[1].as_str() {
        "inventory" => inventory(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../crates/core"),
            &output,
        ),
        "build-objekte" | "build-tantivy" => {
            let result = if args[1] == "build-objekte" {
                build_objekte(&root)?
            } else {
                build_tantivy(&root)?
            };
            fs::write(
                output,
                serde_json::to_vec_pretty(&result).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())
        }
        other => Err(format!("unknown command {other}")),
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("benchmark error: {error}");
        std::process::exit(1);
    }
}
