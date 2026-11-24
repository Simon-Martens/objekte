#[cfg(not(test))]
#[path = "../../../crates/core/examples/support/musenalm.rs"]
mod musenalm;

#[cfg(test)]
mod musenalm {
    pub fn load_definitions(
        _root: &std::path::Path,
    ) -> Result<Vec<objekte_search_core::repository::configuration::CollectionDefinition>, String>
    {
        Err("Musenalm loader is exercised by the core example tests".into())
    }
    pub fn load_extended_definitions(
        root: &std::path::Path,
    ) -> Result<Vec<objekte_search_core::repository::configuration::CollectionDefinition>, String>
    {
        load_definitions(root)
    }
}

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::num::NonZeroUsize;
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
use tantivy::collector::sort_key::SortByStaticFastValue;
use tantivy::collector::{Count, DocSetCollector, MultiCollector, TopDocs};
use tantivy::directory::Directory;
use tantivy::query::{BooleanQuery, Occur, Query, RangeQuery, RegexQuery, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, JsonObjectOptions, STORED, STRING, Schema, Term, TextFieldIndexing,
    Value as TantivyValue,
};
use tantivy::{HasLen, Index, Order, TantivyDocument, doc};

#[derive(Serialize)]
struct InventoryRow {
    source_id: String,
    schema_mode: &'static str,
    original: objekte_search_core::workload::BenchmarkCase,
    predicate_family: &'static str,
    representation: &'static str,
    effective_count: EffectiveRequest,
    effective_page: EffectiveRequest,
    predicate_status: &'static str,
    page_status: &'static str,
    predicate_reason: Option<String>,
    page_reason: Option<String>,
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

fn schema_paths(root: &Path) -> Result<BTreeMap<String, BTreeSet<String>>, String> {
    fn collect(node: &serde_json::Value, parent: &str, output: &mut BTreeSet<String>) {
        if let Some(properties) = node.get("properties").and_then(|value| value.as_object()) {
            for (name, child) in properties {
                let path = if parent.is_empty() {
                    name.clone()
                } else {
                    format!("{parent}.{name}")
                };
                output.insert(path.clone());
                collect(child, &path, output);
            }
        }
        if let Some(items) = node.get("items") {
            collect(items, parent, output);
        }
    }
    let mut result = BTreeMap::new();
    for entry in
        fs::read_dir(root.join(".objekte/collections")).map_err(|error| error.to_string())?
    {
        let path = entry.map_err(|error| error.to_string())?.path();
        if !path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with(".schema.json"))
        {
            continue;
        }
        let schema: serde_json::Value =
            serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
        let name = schema
            .get("x-objekte")
            .and_then(|meta| meta.get("name"))
            .and_then(|value| value.as_str())
            .ok_or("schema missing name")?;
        let mut paths = BTreeSet::new();
        collect(&schema, "", &mut paths);
        result.insert(name.to_owned(), paths);
    }
    Ok(result)
}

fn unsupported_path(
    expr: &FilterExpr,
    collections: &[String],
    paths: &BTreeMap<String, BTreeSet<String>>,
) -> Option<String> {
    match expr {
        FilterExpr::Term(term) => {
            if term.path.display == "@record.id" || term.path.display.starts_with('@') {
                return None;
            }
            let path = term.path.field.segments.join(".");
            for collection in collections {
                if !paths
                    .get(collection)
                    .is_some_and(|fields| fields.contains(&path))
                {
                    return Some(format!(
                        "{collection}.{path} is not a declared local source path"
                    ));
                }
            }
            None
        }
        FilterExpr::And(parts) | FilterExpr::Or(parts) => parts
            .iter()
            .find_map(|part| unsupported_path(part, collections, paths)),
        FilterExpr::Not(inner) => unsupported_path(inner, collections, paths),
    }
}

fn inventory(root: &Path, core: &Path, output: &Path) -> Result<(), String> {
    let baseline = core.join("benches/musenalm_queries.json");
    let extended = core.join("benches/musenalm_queries_extended.json");
    let cases = load_extended_manifest(&baseline, &extended)?;
    let paths = schema_paths(root)?;
    let mut schema = Schema::builder();
    let json_field = schema.add_json_field("source", JsonObjectOptions::default());
    let id_field = schema.add_text_field("record_id", STRING);
    let global = schema.add_text_field("global", STRING);
    let scoped = schema.add_text_field("scoped", STRING);
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
        let mut page = source.clone();
        page.options.count = true;
        page.options.return_mode = ReturnMode::Locator;
        page.options.evidence = false;
        let predicate_reason = source
            .filter
            .as_ref()
            .and_then(|filter| unsupported_path(filter, &source.collections, &paths))
            .or_else(|| {
                source
                    .filter
                    .as_ref()
                    .and_then(|filter| translate(filter, json_field, id_field).err())
                    .or_else(|| {
                        source
                            .fts
                            .as_ref()
                            .and_then(|fts| literal_fts(fts, global, scoped).err())
                    })
            });
        let page_reason = predicate_reason.clone().or_else(|| {
            if page.options.sort.is_empty()
                || (page.options.sort.len() == 1 && page.options.sort[0].field == "numbering")
            {
                None
            } else {
                Some(format!(
                    "sort {:?} requires another mapping",
                    page.options.sort
                ))
            }
        });
        let (family, representation) = match (source.fts.is_some(), source.filter.is_some()) {
            (true, true) => ("mixed", "derived_literal_and_typed_json"),
            (true, false) => ("literal_fts", "derived_literal_term_dictionary"),
            (false, true) => ("structured", "typed_json"),
            (false, false) => ("none", "none"),
        };
        rows.push(InventoryRow {
            source_id: case.id.clone(),
            schema_mode: if case.id.as_str() <= "Q150" {
                "historical"
            } else {
                "extended"
            },
            original: case.clone(),
            predicate_family: family,
            representation,
            effective_count: effective(&count),
            effective_page: effective(&page),
            predicate_status: if predicate_reason.is_some() {
                "unsupported"
            } else {
                "pending_verification"
            },
            page_status: if page_reason.is_some() {
                "unsupported"
            } else {
                "pending_verification"
            },
            predicate_reason,
            page_reason,
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
    resident_ready_kb: Option<usize>,
    index_bytes: Option<u64>,
    segments: Option<usize>,
    writer_memory_bytes: Option<usize>,
}

fn memory_kb(name: &str) -> Option<usize> {
    fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix(name)?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
}

fn peak_rss_kb() -> Option<usize> {
    memory_kb("VmHWM:")
}
fn resident_rss_kb() -> Option<usize> {
    memory_kb("VmRSS:")
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

fn build_tantivy(
    root: &Path,
    build_workers: usize,
    query_workers: usize,
) -> Result<(BuildObservation, Index), String> {
    let start = Instant::now();
    let mut builder = Schema::builder();
    let collection = builder.add_text_field("collection", STRING | STORED);
    let record_id = builder.add_text_field("record_id", STRING | STORED);
    let relative_path = builder.add_text_field("relative_path", STORED);
    let byte_end = builder.add_u64_field("byte_end", STORED);
    let identity_ordinal = builder.add_u64_field("identity_ordinal", tantivy::schema::FAST);
    let numbering_sort = builder.add_f64_field("numbering_sort", tantivy::schema::FAST);
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
    let mut index = Index::create_in_ram(schema);
    if query_workers > 1 {
        index
            .set_multithread_executor(query_workers)
            .map_err(|error| error.to_string())?;
    }
    let mut writer = index
        .writer_with_num_threads(build_workers, 100_000_000)
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
            let mut document = doc!(collection => name, record_id => id, relative_path => relative.as_ref(), byte_end => bytes.len() as u64, identity_ordinal => identities.len() as u64, source_json => value.clone());
            if let Some(number) = value.get("numbering").and_then(|value| value.as_f64()) {
                document.add_f64(numbering_sort, number);
            }
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
    let index_bytes =
        index
            .directory()
            .list_managed_files()
            .iter()
            .try_fold(0u64, |total, path| {
                if path
                    .extension()
                    .is_some_and(|extension| extension == "json")
                {
                    return Ok(total);
                }
                index
                    .directory()
                    .open_read(path)
                    .map(|file| total + file.len() as u64)
                    .map_err(|error| error.to_string())
            })?;
    let segments = index
        .searchable_segments()
        .map_err(|error| error.to_string())?
        .len();
    Ok((
        BuildObservation {
            engine: "tantivy",
            elapsed_ns: start.elapsed().as_nanos(),
            documents: visible,
            collection_counts: counts,
            peak_rss_kb: peak_rss_kb(),
            resident_ready_kb: resident_rss_kb(),
            index_bytes: Some(index_bytes),
            segments: Some(segments),
            writer_memory_bytes: Some(100_000_000),
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

#[derive(Serialize)]
struct QuerySample {
    id: String,
    contract: &'static str,
    repetition: usize,
    elapsed_ns: u128,
    count: usize,
    page_len: usize,
}

#[derive(Serialize)]
struct MeasurementRun {
    build: BuildObservation,
    samples: Vec<QuerySample>,
}

fn selected_cases(
    selected: &str,
) -> Result<Vec<objekte_search_core::workload::BenchmarkCase>, String> {
    let core_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../crates/core/benches");
    let cases = load_extended_manifest(
        &core_path.join("musenalm_queries.json"),
        &core_path.join("musenalm_queries_extended.json"),
    )?;
    let mut seen = BTreeSet::new();
    selected
        .split(',')
        .map(|selector| selector.split('@').next().unwrap_or(selector))
        .filter(|id| seen.insert((*id).to_owned()))
        .map(|id| {
            cases
                .iter()
                .find(|case| case.id == id)
                .cloned()
                .ok_or_else(|| format!("unknown case {id}"))
        })
        .collect()
}

fn definitions_for_cases(
    root: &Path,
    cases: &[objekte_search_core::workload::BenchmarkCase],
) -> Result<Vec<objekte_search_core::repository::configuration::CollectionDefinition>, String> {
    let extended = cases.first().ok_or("no selected cases")?.id.as_str() > "Q150";
    if cases
        .iter()
        .any(|case| (case.id.as_str() > "Q150") != extended)
    {
        return Err("historical and extended cases require separate schema-mode processes".into());
    }
    if extended {
        musenalm::load_extended_definitions(root)
    } else {
        musenalm::load_definitions(root)
    }
}

fn requested_contracts(selected: &str, id: &str) -> (bool, bool) {
    let mut count = false;
    let mut page = false;
    for selector in selected.split(',') {
        if selector == id {
            count = true;
            page = true;
        }
        if selector == format!("{id}@count") {
            count = true;
        }
        if selector == format!("{id}@page") {
            page = true;
        }
    }
    (count, page)
}

fn measure_objekte(
    root: &Path,
    selected: &str,
    repetitions: usize,
    build_workers: usize,
    query_workers: usize,
) -> Result<MeasurementRun, String> {
    let cases = selected_cases(selected)?;
    let start = Instant::now();
    let index = objekte_search_core::lifecycle::Index::start_build(
        root,
        definitions_for_cases(root, &cases)?,
        RepositoryOptions {
            verified_cache_bytes: 0,
            query_workers,
            build_workers: NonZeroUsize::new(build_workers),
            ..RepositoryOptions::default()
        },
        objekte_search_core::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .map_err(|error| error.to_string())?;
    let view = index.view().map_err(|error| error.to_string())?;
    let build = BuildObservation {
        engine: "objekte",
        elapsed_ns: start.elapsed().as_nanos(),
        documents: view.index_observation().records,
        collection_counts: view
            .index_observation()
            .collection_parts
            .iter()
            .map(|part| (part.collection.clone(), part.records))
            .collect(),
        peak_rss_kb: peak_rss_kb(),
        resident_ready_kb: resident_rss_kb(),
        index_bytes: None,
        segments: Some(view.segment_summaries().len()),
        writer_memory_bytes: None,
    };
    let prepared = cases
        .iter()
        .map(|case| {
            let mut count = case.query()?;
            count.options.sort.clear();
            count.options.offset = 0;
            count.options.limit = 0;
            count.options.count = true;
            count.options.return_mode = ReturnMode::Locator;
            count.options.evidence = false;
            let mut page = case.query()?;
            page.options.count = true;
            page.options.return_mode = ReturnMode::Locator;
            page.options.evidence = false;
            let page_supported = page.options.sort.is_empty()
                || (page.options.sort.len() == 1 && page.options.sort[0].field == "numbering");
            let (count_requested, page_requested) = requested_contracts(selected, &case.id);
            Ok((
                case.id.clone(),
                count,
                page,
                count_requested,
                page_requested && page_supported,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut samples = Vec::new();
    for repetition in 0..=repetitions {
        for index in 0..prepared.len() {
            let (id, count_query, page_query, count_requested, page_requested) =
                &prepared[(index + repetition) % prepared.len()];
            for (contract, query) in [
                ("predicate_count", count_query),
                ("ordered_locator_page", page_query),
            ] {
                if (contract == "predicate_count" && !count_requested)
                    || (contract == "ordered_locator_page" && !page_requested)
                {
                    continue;
                }
                let start = Instant::now();
                let result = view.search(query)?;
                let elapsed_ns = start.elapsed().as_nanos();
                let count = result.count.ok_or("Objekte returned no count")?;
                let page_len = result.hits.len();
                std::hint::black_box(&result);
                if repetition > 0 {
                    samples.push(QuerySample {
                        id: id.clone(),
                        contract,
                        repetition: repetition - 1,
                        elapsed_ns,
                        count,
                        page_len,
                    });
                }
            }
        }
    }
    Ok(MeasurementRun { build, samples })
}

fn measure_tantivy(
    root: &Path,
    selected: &str,
    repetitions: usize,
    build_workers: usize,
    query_workers: usize,
) -> Result<MeasurementRun, String> {
    let cases = selected_cases(selected)?;
    let (build, index) = build_tantivy(root, build_workers, query_workers)?;
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
    let global = schema
        .get_field("fts_global")
        .map_err(|error| error.to_string())?;
    let scoped = schema
        .get_field("fts_scoped")
        .map_err(|error| error.to_string())?;
    let prepared = cases
        .iter()
        .map(|case| {
            let bound = case.query()?;
            let mut clauses = Vec::new();
            if let Some(filter) = &bound.filter {
                clauses.push((Occur::Must, translate(filter, source, record_id)?));
            }
            if let Some(fts) = &bound.fts {
                clauses.push((Occur::Must, literal_fts(fts, global, scoped)?));
            }
            if case.collection != "*" {
                clauses.push((Occur::Must, term_query(collection, &case.collection)));
            }
            let page_supported = bound.options.sort.is_empty()
                || (bound.options.sort.len() == 1 && bound.options.sort[0].field == "numbering");
            Ok((
                case.id.clone(),
                Box::new(BooleanQuery::new(clauses)) as Box<dyn Query>,
                bound.options.offset,
                bound.options.limit,
                !bound.options.sort.is_empty(),
                page_supported,
                requested_contracts(selected, &case.id),
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut samples = Vec::new();
    for repetition in 0..=repetitions {
        for index in 0..prepared.len() {
            let (
                id,
                query,
                offset,
                limit,
                numeric,
                page_supported,
                (count_requested, page_requested),
            ) = &prepared[(index + repetition) % prepared.len()];
            if *count_requested {
                let start = Instant::now();
                let count = searcher
                    .search(query, &Count)
                    .map_err(|error| error.to_string())?;
                let elapsed_ns = start.elapsed().as_nanos();
                std::hint::black_box(count);
                if repetition > 0 {
                    samples.push(QuerySample {
                        id: id.clone(),
                        contract: "predicate_count",
                        repetition: repetition - 1,
                        elapsed_ns,
                        count,
                        page_len: 0,
                    });
                }
            }
            if !page_supported || !page_requested {
                continue;
            }
            let mut collectors = MultiCollector::new();
            let count_handle = collectors.add_collector(Count);
            if *numeric {
                let page_handle =
                    collectors.add_collector(TopDocs::with_limit(offset + limit).order_by((
                        (
                            SortByStaticFastValue::<f64>::for_field("numbering_sort"),
                            Order::Asc,
                        ),
                        (
                            SortByStaticFastValue::<u64>::for_field("identity_ordinal"),
                            Order::Asc,
                        ),
                    )));
                let start = Instant::now();
                let mut fruit = searcher
                    .search(query, &collectors)
                    .map_err(|error| error.to_string())?;
                let count = count_handle.extract(&mut fruit);
                let winners = page_handle
                    .extract(&mut fruit)
                    .into_iter()
                    .skip(*offset)
                    .collect::<Vec<_>>();
                for (_, address) in &winners {
                    std::hint::black_box(
                        searcher
                            .doc::<TantivyDocument>(*address)
                            .map_err(|error| error.to_string())?,
                    );
                }
                let elapsed_ns = start.elapsed().as_nanos();
                if repetition > 0 {
                    samples.push(QuerySample {
                        id: id.clone(),
                        contract: "ordered_locator_page",
                        repetition: repetition - 1,
                        elapsed_ns,
                        count,
                        page_len: winners.len(),
                    });
                }
            } else {
                let page_handle = collectors.add_collector(
                    TopDocs::with_limit(offset + limit)
                        .order_by_fast_field::<u64>("identity_ordinal", Order::Asc),
                );
                let start = Instant::now();
                let mut fruit = searcher
                    .search(query, &collectors)
                    .map_err(|error| error.to_string())?;
                let count = count_handle.extract(&mut fruit);
                let winners = page_handle
                    .extract(&mut fruit)
                    .into_iter()
                    .skip(*offset)
                    .collect::<Vec<_>>();
                for (_, address) in &winners {
                    std::hint::black_box(
                        searcher
                            .doc::<TantivyDocument>(*address)
                            .map_err(|error| error.to_string())?,
                    );
                }
                let elapsed_ns = start.elapsed().as_nanos();
                if repetition > 0 {
                    samples.push(QuerySample {
                        id: id.clone(),
                        contract: "ordered_locator_page",
                        repetition: repetition - 1,
                        elapsed_ns,
                        count,
                        page_len: winners.len(),
                    });
                }
            }
        }
    }
    Ok(MeasurementRun { build, samples })
}

fn verify_counts(root: &Path, selected: &str) -> Result<Vec<VerifiedCount>, String> {
    let selected_cases = selected_cases(selected)?;
    let (_, index) = build_tantivy(root, 1, 1)?;
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
    let byte_end = schema
        .get_field("byte_end")
        .map_err(|error| error.to_string())?;
    let relative_path = schema
        .get_field("relative_path")
        .map_err(|error| error.to_string())?;
    let definitions = definitions_for_cases(root, &selected_cases)?;
    let core = objekte_search_core::lifecycle::Index::start_build(
        root,
        definitions,
        RepositoryOptions {
            verified_cache_bytes: 0,
            query_workers: 1,
            build_workers: NonZeroUsize::new(1),
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
        let original_sort = request.options.sort.clone();
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
        let mut numeric_page = Vec::new();
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
            if original_sort.len() == 1 && original_sort[0].field == "numbering" {
                let source_value = document.get_first(source).ok_or("missing source JSON")?;
                let number = source_value
                    .as_object()
                    .ok_or("source is not an object")?
                    .find(|(name, _)| *name == "numbering")
                    .and_then(|(_, value)| {
                        value
                            .as_f64()
                            .or_else(|| value.as_i64().map(|number| number as f64))
                    })
                    .ok_or("matching record lacks numeric numbering")?;
                let path = document
                    .get_first(relative_path)
                    .and_then(|value| value.as_str())
                    .ok_or("missing stored path")?;
                let end = document
                    .get_first(byte_end)
                    .and_then(|value| value.as_u64())
                    .ok_or("missing stored byte end")?;
                numeric_page.push((
                    number,
                    collection_value.to_owned(),
                    id_value.to_owned(),
                    path.to_owned(),
                    end,
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
        if !numeric_page.is_empty() {
            numeric_page.sort_by(|left, right| {
                left.0
                    .total_cmp(&right.0)
                    .then_with(|| (&left.1, &left.2).cmp(&(&right.1, &right.2)))
            });
            let expected = numeric_page
                .iter()
                .skip(case.offset)
                .take(case.limit)
                .map(|(_, collection, id, path, end)| {
                    (collection.clone(), id.clone(), path.clone(), 0, *end)
                })
                .collect::<Vec<_>>();
            let ordered_hits = searcher
                .search(
                    &query,
                    &TopDocs::with_limit(case.offset + case.limit).order_by((
                        (
                            SortByStaticFastValue::<f64>::for_field("numbering_sort"),
                            Order::Asc,
                        ),
                        (
                            SortByStaticFastValue::<u64>::for_field("identity_ordinal"),
                            Order::Asc,
                        ),
                    )),
                )
                .map_err(|error| error.to_string())?;
            let fast_page = ordered_hits
                .into_iter()
                .skip(case.offset)
                .map(|(_, address)| {
                    let document = searcher
                        .doc::<TantivyDocument>(address)
                        .map_err(|error| error.to_string())?;
                    let collection_value = document
                        .get_first(collection)
                        .and_then(|value| value.as_str())
                        .ok_or("missing collection")?;
                    let id_value = document
                        .get_first(record_id)
                        .and_then(|value| value.as_str())
                        .ok_or("missing record ID")?;
                    let path = document
                        .get_first(relative_path)
                        .and_then(|value| value.as_str())
                        .ok_or("missing path")?;
                    let end = document
                        .get_first(byte_end)
                        .and_then(|value| value.as_u64())
                        .ok_or("missing byte end")?;
                    Ok((
                        collection_value.to_owned(),
                        id_value.to_owned(),
                        path.to_owned(),
                        0,
                        end,
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?;
            if fast_page != expected {
                return Err(format!(
                    "{id}: bounded numeric collector differs from full reference page"
                ));
            }
            let mut page_request = case.query()?;
            page_request.options.count = true;
            page_request.options.return_mode = ReturnMode::Locator;
            page_request.options.evidence = false;
            let page = view.search(&page_request)?;
            let actual = page
                .hits
                .into_iter()
                .map(|hit| {
                    let path = hit.file_path.strip_prefix(root).unwrap_or(&hit.file_path);
                    Ok((
                        hit.collection,
                        hit.record_id,
                        path.to_string_lossy().to_string(),
                        hit.byte_start,
                        hit.byte_end,
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?;
            if actual != expected {
                let rank = actual
                    .iter()
                    .zip(&expected)
                    .position(|(left, right)| left != right)
                    .unwrap_or(actual.len().min(expected.len()));
                return Err(format!(
                    "{id}: numeric page differs at rank {rank}: Objekte {:?}, Tantivy {:?}",
                    actual.get(rank),
                    expected.get(rank)
                ));
            }
        }
        if original_sort.is_empty() {
            let ordered_hits = searcher
                .search(
                    &query,
                    &TopDocs::with_limit(case.offset + case.limit)
                        .order_by_fast_field::<u64>("identity_ordinal", Order::Asc),
                )
                .map_err(|error| error.to_string())?;
            let expected = ordered_hits
                .into_iter()
                .skip(case.offset)
                .map(|(_, address)| {
                    let document = searcher
                        .doc::<TantivyDocument>(address)
                        .map_err(|error| error.to_string())?;
                    let collection_value = document
                        .get_first(collection)
                        .and_then(|value| value.as_str())
                        .ok_or("missing collection")?;
                    let id_value = document
                        .get_first(record_id)
                        .and_then(|value| value.as_str())
                        .ok_or("missing record ID")?;
                    let path = document
                        .get_first(relative_path)
                        .and_then(|value| value.as_str())
                        .ok_or("missing path")?;
                    let end = document
                        .get_first(byte_end)
                        .and_then(|value| value.as_u64())
                        .ok_or("missing byte end")?;
                    Ok((
                        collection_value.to_owned(),
                        id_value.to_owned(),
                        path.to_owned(),
                        0,
                        end,
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?;
            let mut page_request = case.query()?;
            page_request.options.count = true;
            page_request.options.return_mode = ReturnMode::Locator;
            page_request.options.evidence = false;
            let page = view.search(&page_request)?;
            let actual = page
                .hits
                .into_iter()
                .map(|hit| {
                    let path = hit.file_path.strip_prefix(root).unwrap_or(&hit.file_path);
                    (
                        hit.collection,
                        hit.record_id,
                        path.to_string_lossy().to_string(),
                        hit.byte_start,
                        hit.byte_end,
                    )
                })
                .collect::<Vec<_>>();
            if actual != expected {
                let rank = actual
                    .iter()
                    .zip(&expected)
                    .position(|(left, right)| left != right)
                    .unwrap_or(actual.len().min(expected.len()));
                return Err(format!(
                    "{id}: identity page differs at rank {rank}: Objekte {:?}, Tantivy {:?}",
                    actual.get(rank),
                    expected.get(rank)
                ));
            }
        }
        results.push(VerifiedCount {
            id: id.into(),
            count: native_count,
            elapsed_ns: elapsed,
        });
    }
    Ok(results)
}

fn build_objekte(
    root: &Path,
    build_workers: usize,
    query_workers: usize,
) -> Result<BuildObservation, String> {
    let start = Instant::now();
    let definitions = musenalm::load_definitions(root)?;
    let index = objekte_search_core::lifecycle::Index::start_build(
        root,
        definitions,
        RepositoryOptions {
            verified_cache_bytes: 0,
            query_workers,
            build_workers: NonZeroUsize::new(build_workers),
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
        collection_counts: view
            .index_observation()
            .collection_parts
            .iter()
            .map(|part| (part.collection.clone(), part.records))
            .collect(),
        peak_rss_kb: peak_rss_kb(),
        resident_ready_kb: resident_rss_kb(),
        index_bytes: None,
        segments: Some(view.segment_summaries().len()),
        writer_memory_bytes: None,
    })
}

fn run() -> Result<(), String> {
    let args = std::env::args().collect::<Vec<_>>();
    if !(4..=8).contains(&args.len()) {
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
            &root,
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
        "measure-objekte" | "measure-tantivy" => {
            let selected = args.get(4).ok_or("measure requires comma-separated IDs")?;
            let repetitions = args
                .get(5)
                .ok_or("measure requires a repetition count")?
                .parse::<usize>()
                .map_err(|error| error.to_string())?;
            if repetitions == 0 {
                return Err("repetitions must be positive".into());
            }
            let build_workers = args
                .get(6)
                .ok_or("measure requires build worker count")?
                .parse::<usize>()
                .map_err(|error| error.to_string())?;
            let query_workers = args
                .get(7)
                .ok_or("measure requires query worker count")?
                .parse::<usize>()
                .map_err(|error| error.to_string())?;
            if build_workers == 0 || query_workers == 0 {
                return Err("worker counts must be positive".into());
            }
            let result = if args[1] == "measure-objekte" {
                measure_objekte(&root, selected, repetitions, build_workers, query_workers)?
            } else {
                measure_tantivy(&root, selected, repetitions, build_workers, query_workers)?
            };
            fs::write(
                output,
                serde_json::to_vec_pretty(&result).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())
        }
        "build-objekte" | "build-tantivy" => {
            let result = if args[1] == "build-objekte" {
                build_objekte(&root, 1, 1)?
            } else {
                build_tantivy(&root, 1, 1)?.0
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_translation_preserves_case_scope_and_value_boundaries() {
        let mut builder = Schema::builder();
        let global = builder.add_text_field("global", STRING);
        let scoped = builder.add_text_field("scoped", STRING);
        let index = Index::create_in_ram(builder.build());
        let mut writer = index.writer(50_000_000).unwrap();
        for value in [
            serde_json::json!({"title": "Almanachband", "url": "https://example.org", "preferred_title": "Ä"}),
            serde_json::json!({"title": "almanachband", "preferred_title": "Taschenbuch"}),
            serde_json::json!({"title": ["Alma", "nach"], "preferred_title": "text"}),
            serde_json::json!({"title": "Taschenbuch", "preferred_title": "text"}),
        ] {
            let mut document = TantivyDocument::default();
            index_strings(&value, &mut Vec::new(), &mut document, global, scoped);
            writer.add_document(document).unwrap();
        }
        writer.commit().unwrap();
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        let request = |literal: &str, path: Option<&str>| objekte_search_core::query::FtsQuery {
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Literal,
            literal: literal.into(),
            pattern_parameter: None,
            fields: path
                .into_iter()
                .map(|path| objekte_search_core::query::FtsField {
                    path: objekte_search_core::model::FieldPath::parse(path).unwrap(),
                    binding: None,
                    query_path: None,
                })
                .collect(),
        };
        for (needle, path, expected) in [
            ("Almanach", None, 1),
            ("almanach", None, 1),
            ("https://", None, 1),
            ("Ä", None, 1),
            ("Almanach", Some("title"), 1),
            ("Taschenbuch", Some("preferred_title"), 1),
        ] {
            let query = literal_fts(&request(needle, path), global, scoped).unwrap();
            assert_eq!(searcher.search(&query, &Count).unwrap(), expected);
        }
        assert_eq!(
            searcher
                .search(
                    &literal_fts(
                        &request("Almanach", Some("preferred_title")),
                        global,
                        scoped
                    )
                    .unwrap(),
                    &Count
                )
                .unwrap(),
            0
        );
    }
}
