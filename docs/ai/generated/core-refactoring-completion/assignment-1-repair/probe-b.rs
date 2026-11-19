use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use objekte_search_core::execution_options::ReturnMode;
use objekte_search_core::inference::{InferenceConfig, InferenceMode, InferencePathDiagnostic};
use objekte_search_core::lifecycle::IndexView;
use objekte_search_core::model::LogicalType;
use objekte_search_core::query::{Query, QueryOptions, parse_filter};
use objekte_search_core::query_execution::restrictions::AccessPathKind;
use objekte_search_core::repository::configuration::{CollectionDefinition, RepositoryOptions};
use objekte_search_core::schema::FieldDefinition;
use objekte_search_core::source::SourceFormat;
use objekte_search_core::source::registry::SourceRegistry;
use serde_json::{Value, json};

struct Arguments {
    root: PathBuf,
    revision: String,
    tested_sha: String,
    fixture_sha256: String,
}

struct QueryCase {
    filter: &'static str,
    field: &'static str,
    rows: Vec<usize>,
}

fn parse_arguments() -> Result<Arguments, String> {
    let mut root = None;
    let mut revision = None;
    let mut tested_sha = None;
    let mut fixture_sha256 = None;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("{argument} needs a value"))
        };
        match argument.as_str() {
            "--root" => root = Some(PathBuf::from(value()?)),
            "--revision" => revision = Some(value()?),
            "--tested-sha" => tested_sha = Some(value()?),
            "--fixture-sha256" => fixture_sha256 = Some(value()?),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Arguments {
        root: root.ok_or("--root is required")?,
        revision: revision.ok_or("--revision is required")?,
        tested_sha: tested_sha.ok_or("--tested-sha is required")?,
        fixture_sha256: fixture_sha256.ok_or("--fixture-sha256 is required")?,
    })
}

fn line_starts(bytes: &[u8]) -> Vec<u64> {
    let mut starts = Vec::new();
    let mut start = 0usize;
    for (offset, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            starts.push(start as u64);
            start = offset + 1;
        }
    }
    if start < bytes.len() {
        starts.push(start as u64);
    }
    starts
}

fn query(filter: &str) -> Result<Query, String> {
    Ok(Query {
        collections: vec!["records".into()],
        fts: None,
        filter: Some(parse_filter(filter)?),
        options: QueryOptions {
            sort: Vec::new(),
            limit: 32,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    })
}

fn diagnostic_value(diagnostic: &InferencePathDiagnostic) -> Result<Value, String> {
    Ok(json!({
        "path": diagnostic.path,
        "status": serde_json::to_value(diagnostic.index_status)
            .map_err(|error| error.to_string())?,
        "value_keys": diagnostic.value_keys,
        "covered_records": diagnostic.covered_records,
        "value_dictionary_complete": diagnostic.value_dictionary_complete,
        "short_string_candidates": diagnostic.short_string_candidates,
        "present_records": diagnostic.present_records,
    }))
}

fn run_case(
    view: &IndexView,
    file_id: u32,
    rows_by_start: &HashMap<u64, usize>,
    case: &QueryCase,
) -> Result<Value, String> {
    let query = query(case.filter)?;
    let traces = view.explain(&query)?;
    let trace = traces
        .iter()
        .find(|trace| trace.collection == "records")
        .ok_or_else(|| format!("missing records trace for {}", case.filter))?;
    let term = trace
        .terms
        .iter()
        .find(|term| term.field == case.field)
        .ok_or_else(|| format!("missing {} term for {}", case.field, case.filter))?;
    if term.selected.kind != AccessPathKind::Posting || !term.selected.exact {
        return Err(format!(
            "{} selected {:?} exact={} instead of an exact posting",
            case.filter, term.selected.kind, term.selected.exact
        ));
    }

    let result = view.search(&query)?;
    if result.count != Some(case.rows.len()) {
        return Err(format!(
            "{} count {:?} != {}",
            case.filter,
            result.count,
            case.rows.len()
        ));
    }
    if result.hits.len() != case.rows.len() {
        return Err(format!(
            "{} page length {} != {}",
            case.filter,
            result.hits.len(),
            case.rows.len()
        ));
    }
    if result.counters.files_opened != 0 || result.counters.record_parses != 0 {
        return Err(format!(
            "{} opened {} files and parsed {} records",
            case.filter, result.counters.files_opened, result.counters.record_parses
        ));
    }

    let expected = case.rows.iter().copied().collect::<HashSet<_>>();
    let mut observed = HashSet::new();
    let mut locators = Vec::with_capacity(result.hits.len());
    for hit in &result.hits {
        if hit.collection != "records" {
            return Err(format!(
                "{} returned collection {}",
                case.filter, hit.collection
            ));
        }
        if hit.file_id != file_id {
            return Err(format!(
                "{} returned file {} instead of {}",
                case.filter, hit.file_id, file_id
            ));
        }
        let row = *rows_by_start.get(&hit.byte_start).ok_or_else(|| {
            format!(
                "{} returned unknown byte start {}",
                case.filter, hit.byte_start
            )
        })?;
        if !observed.insert(row) {
            return Err(format!("{} returned row {row} more than once", case.filter));
        }
        locators.push(json!({
            "row": row,
            "collection": hit.collection,
            "file_id": hit.file_id,
            "byte_start": hit.byte_start,
            "record_id": hit.record_id,
        }));
    }
    if observed != expected {
        return Err(format!(
            "{} rows {:?} != expected {:?}",
            case.filter, observed, expected
        ));
    }

    Ok(json!({
        "filter": case.filter,
        "field": case.field,
        "selected": {
            "kind": serde_json::to_value(term.selected.kind)
                .map_err(|error| error.to_string())?,
            "exact": term.selected.exact,
        },
        "count": result.count,
        "page_len": result.hits.len(),
        "locators": locators,
        "counters": {
            "files_opened": result.counters.files_opened,
            "record_parses": result.counters.record_parses,
            "scanned_records": result.counters.scanned_records,
            "residual_evaluations": result.counters.residual_evaluations,
        },
    }))
}

fn run() -> Result<(), String> {
    let arguments = parse_arguments()?;
    let mut definition = CollectionDefinition::folder("records", "data", SourceFormat::JsonLines);
    definition.schema.register_field(FieldDefinition {
        path: vec!["manual".into()],
        kind: LogicalType::String,
        required: true,
        nullable: false,
        indexed: true,
        multi: false,
        element_nullable: Vec::new(),
    })?;
    definition
        .schema
        .prohibit_auto_index(vec!["blocked".into()])?;
    let repository = objekte_search_core::lifecycle::Index::start_build(
        &arguments.root,
        vec![definition],
        RepositoryOptions {
            verified_cache_bytes: 0,
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
        SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .map_err(|error| error.to_string())?;
    let view = repository.view().map_err(|error| error.to_string())?;

    let fixture_path = arguments.root.join("data/records.jsonl");
    let bytes = fs::read(&fixture_path).map_err(|error| error.to_string())?;
    let starts = line_starts(&bytes);
    if starts.len() != 32 {
        return Err(format!(
            "fixture contains {} rows instead of 32",
            starts.len()
        ));
    }
    let rows_by_start = starts
        .into_iter()
        .enumerate()
        .map(|(row, start)| (start, row))
        .collect::<HashMap<_, _>>();
    let file_id = view
        .catalogue_rows()
        .find(|row| row.relative_path == Path::new("data/records.jsonl"))
        .map(|row| row.id.0)
        .ok_or("fixture file missing from catalogue")?;

    let diagnostics = view
        .inference_index_diagnostics("records")
        .ok_or("inference diagnostics missing")?;
    let mut diagnostic_values = Vec::new();
    for path in ["enabled", "group", "sequence"] {
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.path == [path])
            .ok_or_else(|| format!("missing {path} inference diagnostic"))?;
        diagnostic_value(diagnostic)?;
        if diagnostic.index_status != objekte_search_core::inference::InferenceIndexStatus::Built {
            return Err(format!("{path} inference status is not Built"));
        }
        if diagnostic.value_keys.is_none_or(|keys| keys == 0) {
            return Err(format!("{path} inference has no value keys"));
        }
        if diagnostic.covered_records != Some(32) {
            return Err(format!(
                "{path} covers {:?} records instead of 32",
                diagnostic.covered_records
            ));
        }
        diagnostic_values.push(diagnostic_value(diagnostic)?);
    }

    let even_rows = (0..32).step_by(2).collect::<Vec<_>>();
    let cases = [
        QueryCase {
            filter: "enabled = true",
            field: "enabled",
            rows: even_rows.clone(),
        },
        QueryCase {
            filter: "group = 'A'",
            field: "group",
            rows: even_rows,
        },
        QueryCase {
            filter: "sequence = 31",
            field: "sequence",
            rows: vec![31],
        },
    ];
    let queries = cases
        .iter()
        .map(|case| run_case(&view, file_id, &rows_by_start, case))
        .collect::<Result<Vec<_>, _>>()?;

    println!(
        "{}",
        json!({
            "revision": arguments.revision,
            "tested_sha": arguments.tested_sha,
            "fixture_sha256": arguments.fixture_sha256,
            "file_id": file_id,
            "diagnostics": diagnostic_values,
            "queries": queries,
        })
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("assignment-1 inference probe: {error}");
        std::process::exit(1);
    }
}
