#[path = "../../../crates/core/examples/support/musenalm.rs"]
mod musenalm;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::time::Instant;

use objekte_search_core::model::Number;
use objekte_search_core::query::options::ReturnMode;
use objekte_search_core::query::{
    CaseSensitivity, FilterExpr, FtsMode, Literal, Operand, Operator,
};
use objekte_search_core::repository::configuration::RepositoryOptions;
use objekte_search_core::workload::load_extended_manifest;
use serde::Serialize;
use tantivy::collector::{Count, DocSetCollector};
use tantivy::query::{BooleanQuery, Occur, Query, RangeQuery, RegexQuery, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, JsonObjectOptions, STORED, STRING, Schema, Term, TextFieldIndexing,
    Value as TantivyValue,
};
use tantivy::{Index, TantivyDocument, doc};

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

fn index_strings(
    value: &serde_json::Value,
    path: &mut Vec<String>,
    document: &mut TantivyDocument,
    global: Field,
    scoped: Field,
) {
    match value {
        serde_json::Value::String(text) => {
            let scalars = text.chars().collect::<Vec<_>>();
            for start in (0..scalars.len()).step_by(512) {
                let end = (start + 1024).min(scalars.len());
                let part = scalars[start..end].iter().collect::<String>();
                document.add_text(global, &part);
                document.add_text(scoped, format!("{}\u{1f}{part}", path.join(".")));
            }
        }
        serde_json::Value::Object(fields) => {
            for (name, child) in fields {
                path.push(name.clone());
                index_strings(child, path, document, global, scoped);
                path.pop();
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                index_strings(child, path, document, global, scoped);
            }
        }
        _ => {}
    }
}

fn build_tantivy(root: &Path) -> Result<(BuildObservation, Index), String> {
    let start = Instant::now();
    let mut builder = Schema::builder();
    let collection = builder.add_text_field("collection", STRING | STORED);
    let record_id = builder.add_text_field("record_id", STRING | STORED);
    let relative_path = builder.add_text_field("relative_path", STORED);
    let fts_global = builder.add_text_field("fts_global", STRING);
    let fts_scoped = builder.add_text_field("fts_scoped", STRING);
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
            let mut document = doc!(collection => name, record_id => id, relative_path => relative.as_ref(), source_json => value.clone());
            index_strings(
                &value,
                &mut Vec::new(),
                &mut document,
                fts_global,
                fts_scoped,
            );
            writer
                .add_document(document)
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
    Ok((
        BuildObservation {
            engine: "tantivy",
            elapsed_ns: start.elapsed().as_nanos(),
            documents: visible,
            collection_counts: counts,
            peak_rss_kb: peak_rss_kb(),
        },
        index,
    ))
}

fn term_query(field: Field, value: &str) -> Box<dyn Query> {
    Box::new(TermQuery::new(
        Term::from_field_text(field, value),
        IndexRecordOption::Basic,
    ))
}

fn literal_fts(
    query: &objekte_search_core::query::FtsQuery,
    global: Field,
    scoped: Field,
) -> Result<Box<dyn Query>, String> {
    if query.mode != FtsMode::Literal
        || query.case_sensitivity != CaseSensitivity::Sensitive
        || query.pattern_parameter.is_some()
    {
        return Err("FTS mode requires another mapping".into());
    }
    if query.literal.is_empty() || query.literal.chars().count() > 512 {
        return Err("literal length outside chunked term representation".into());
    }
    let needle = regex::escape(&query.literal);
    if query.fields.is_empty() {
        return RegexQuery::from_pattern(&format!("(?s).*{needle}.*"), global)
            .map(|query| Box::new(query) as Box<dyn Query>)
            .map_err(|error| error.to_string());
    }
    let mut alternatives = Vec::new();
    for field in &query.fields {
        if field.binding.is_some() || field.query_path.is_some() {
            return Err("bound or positional FTS field requires another mapping".into());
        }
        let path = regex::escape(&field.path.display());
        let pattern = format!("(?s){path}\u{1f}.*{needle}.*");
        let clause =
            RegexQuery::from_pattern(&pattern, scoped).map_err(|error| error.to_string())?;
        alternatives.push((Occur::Should, Box::new(clause) as Box<dyn Query>));
    }
    Ok(Box::new(BooleanQuery::new(alternatives)))
}

fn translate(expr: &FilterExpr, source: Field, record_id: Field) -> Result<Box<dyn Query>, String> {
    match expr {
        FilterExpr::And(parts) | FilterExpr::Or(parts) => {
            let occur = if matches!(expr, FilterExpr::And(_)) {
                Occur::Must
            } else {
                Occur::Should
            };
            let clauses = parts
                .iter()
                .map(|part| translate(part, source, record_id).map(|query| (occur, query)))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Box::new(BooleanQuery::new(clauses)))
        }
        FilterExpr::Not(inner) => Ok(Box::new(BooleanQuery::new(vec![
            (Occur::Must, Box::new(tantivy::query::AllQuery)),
            (Occur::MustNot, translate(inner, source, record_id)?),
        ]))),
        FilterExpr::Term(predicate) => {
            if !predicate.modifiers.is_empty()
                || predicate.path.has_position
                || predicate.path.has_explicit_array_step
            {
                return Err("field modifiers and positional paths require another mapping".into());
            }
            let path = predicate.path.display.as_str();
            let (field, mut term) = if path == "@record.id" {
                (record_id, Term::from_field_text(record_id, ""))
            } else if path.starts_with('@') {
                return Err(format!("metadata path {path} requires another mapping"));
            } else {
                (source, Term::from_field_json_path(source, path, false))
            };
            let value = match &predicate.operand {
                Operand::Literal(value) => value,
                _ => {
                    return Err(
                        "field, parameter and clock operands require another mapping".into(),
                    );
                }
            };
            match value {
                Literal::String(value) if field == record_id => {
                    term = Term::from_field_text(record_id, value)
                }
                Literal::String(value) => term.append_type_and_str(value),
                Literal::Bool(value) if field == source => term.append_type_and_fast_value(*value),
                Literal::Number(Number::Integer(value)) if field == source => {
                    let integer =
                        i64::try_from(*value).map_err(|_| "number is outside i64 domain")?;
                    term.append_type_and_fast_value(integer);
                }
                Literal::Number(Number::Float(value)) if field == source => {
                    term.append_type_and_fast_value(*value)
                }
                _ => return Err("literal type requires another mapping".into()),
            }
            let query: Box<dyn Query> = match predicate.operator {
                Operator::Eq => Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
                Operator::Gt => Box::new(RangeQuery::new(Bound::Excluded(term), Bound::Unbounded)),
                Operator::Ge => Box::new(RangeQuery::new(Bound::Included(term), Bound::Unbounded)),
                Operator::Lt => Box::new(RangeQuery::new(Bound::Unbounded, Bound::Excluded(term))),
                Operator::Le => Box::new(RangeQuery::new(Bound::Unbounded, Bound::Included(term))),
                _ => return Err("operator requires another mapping".into()),
            };
            Ok(query)
        }
    }
}

#[derive(Serialize)]
struct VerifiedCount {
    id: String,
    count: usize,
    elapsed_ns: u128,
}

fn verify_counts(root: &Path, selected: &str) -> Result<Vec<VerifiedCount>, String> {
    let (_, index) = build_tantivy(root)?;
    let reader = index.reader().map_err(|error| error.to_string())?;
    let searcher = reader.searcher();
    let schema = index.schema();
    let source = schema
        .get_field("source")
        .map_err(|error| error.to_string())?;
    let collection = schema
        .get_field("collection")
        .map_err(|error| error.to_string())?;
    let record_id = schema
        .get_field("record_id")
        .map_err(|error| error.to_string())?;
    let fts_global = schema
        .get_field("fts_global")
        .map_err(|error| error.to_string())?;
    let fts_scoped = schema
        .get_field("fts_scoped")
        .map_err(|error| error.to_string())?;
    let definitions = musenalm::load_definitions(root)?;
    let core = objekte_search_core::lifecycle::Index::start_build(
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
    let view = core.view().map_err(|error| error.to_string())?;
    let core_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../crates/core/benches");
    let cases = load_extended_manifest(
        &core_path.join("musenalm_queries.json"),
        &core_path.join("musenalm_queries_extended.json"),
    )?;
    let mut results = Vec::new();
    for id in selected.split(',') {
        let case = cases
            .iter()
            .find(|case| case.id == id)
            .ok_or_else(|| format!("unknown case {id}"))?;
        let mut request = case.query()?;
        let mut clauses = Vec::new();
        if let Some(filter) = &request.filter {
            clauses.push((Occur::Must, translate(filter, source, record_id)?));
        }
        if let Some(fts) = &request.fts {
            clauses.push((Occur::Must, literal_fts(fts, fts_global, fts_scoped)?));
        }
        if case.collection != "*" {
            clauses.push((Occur::Must, term_query(collection, &case.collection)));
        }
        let query = BooleanQuery::new(clauses);
        request.options.sort.clear();
        request.options.offset = 0;
        request.options.limit = 0;
        request.options.count = true;
        request.options.return_mode = ReturnMode::Locator;
        request.options.evidence = false;
        let native_count = view
            .search(&request)?
            .count
            .ok_or("Objekte returned no count")?;
        let start = Instant::now();
        let tantivy_count = searcher
            .search(&query, &Count)
            .map_err(|error| error.to_string())?;
        let elapsed = start.elapsed().as_nanos();
        if native_count != tantivy_count {
            return Err(format!(
                "{id}: Objekte {native_count}, Tantivy {tantivy_count}"
            ));
        }
        let addresses = searcher
            .search(&query, &DocSetCollector)
            .map_err(|error| error.to_string())?;
        if addresses.len() != tantivy_count {
            return Err(format!("{id}: duplicate Tantivy hits"));
        }
        let mut tantivy_ids = BTreeSet::new();
        for address in addresses {
            let document = searcher
                .doc::<TantivyDocument>(address)
                .map_err(|error| error.to_string())?;
            let collection_value = document
                .get_first(collection)
                .and_then(|value| value.as_str())
                .ok_or("missing stored collection")?;
            let id_value = document
                .get_first(record_id)
                .and_then(|value| value.as_str())
                .ok_or("missing stored record ID")?;
            if !tantivy_ids.insert((collection_value.to_owned(), id_value.to_owned())) {
                return Err(format!(
                    "{id}: duplicate Tantivy identity {collection_value}/{id_value}"
                ));
            }
        }
        request.options.limit = view.index_observation().records;
        let native = view.search(&request)?;
        let native_ids = native
            .hits
            .into_iter()
            .map(|hit| (hit.collection, hit.record_id))
            .collect::<BTreeSet<_>>();
        if native_ids.len() != native_count {
            return Err(format!("{id}: duplicate Objekte identity"));
        }
        if native_ids != tantivy_ids {
            let missing = native_ids.difference(&tantivy_ids).next();
            let extra = tantivy_ids.difference(&native_ids).next();
            return Err(format!(
                "{id}: membership mismatch, first missing {missing:?}, first extra {extra:?}"
            ));
        }
        results.push(VerifiedCount {
            id: id.into(),
            count: native_count,
            elapsed_ns: elapsed,
        });
    }
    Ok(results)
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
    if args.len() != 4 && args.len() != 5 {
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
        "verify-counts" => {
            let selected = args
                .get(4)
                .ok_or("verify-counts requires comma-separated IDs")?;
            let result = verify_counts(&root, selected)?;
            fs::write(
                output,
                serde_json::to_vec_pretty(&result).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())
        }
        "build-objekte" | "build-tantivy" => {
            let result = if args[1] == "build-objekte" {
                build_objekte(&root)?
            } else {
                build_tantivy(&root)?.0
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
