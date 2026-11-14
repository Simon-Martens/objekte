//! Matched local predicate benchmark over a caller-supplied fixed fixture.

use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::model::LogicalType;
use objekte_search_core::query::{Query, QueryOptions, parse_filter};
use objekte_search_core::reference::FieldDefinition;
use objekte_search_core::repository::{
    CollectionDefinition, Repository, RepositoryOptions,
};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::ReturnMode;

fn main() {
    if let Err(error) = run() {
        eprintln!("predicate benchmark: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().ok_or("fixture root required")?);
    let repetitions: usize = args
        .next()
        .unwrap_or_else(|| "11".into())
        .parse()
        .map_err(|error| format!("invalid repetition count: {error}"))?;
    if repetitions < 7 || args.next().is_some() {
        return Err("expected fixture root and at least seven repetitions".into());
    }
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    for (name, indexed) in [
        ("kind", true),
        ("body", false),
        ("tag", true),
        ("memo", false),
    ] {
        definition.schema.register_field(FieldDefinition {
            path: vec![name.into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed,
            multi: false,
            element_nullable: Vec::new(),
        })?;
    }
    let repo = Repository::open_with_options(
        &root,
        vec![definition],
        RepositoryOptions {
            verified_cache_bytes: 0,
            query_workers: 4,
            ..RepositoryOptions::default()
        },
    )?;
    let cases = [
        ("and-selective", "kind = 'hot' && body ~ 'needle'", false),
        ("and-reversed", "body ~ 'needle' && kind = 'hot'", false),
        ("or-broad", "body ~ 'needle' || kind = 'hot'", false),
        (
            "or-expensive-exact",
            "memo ~ 'needle' || tag ~ 'hot'",
            false,
        ),
        ("or-broad-evidence", "body ~ 'needle' || kind = 'hot'", true),
        ("or-overlap", "body ~ 'needle' || body ~ 'need'", false),
        (
            "fold-shared",
            "body:lower ~ 'strasse' && body:lower ~ 'asse'",
            false,
        ),
        ("index-control", "kind = 'hot'", false),
    ];
    for (name, filter, evidence) in cases {
        let query = Query {
            collections: vec!["data".into()],
            fts: None,
            filter: Some(parse_filter(filter)?),
            options: QueryOptions {
                sort: vec![],
                limit: 1000,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence,
            },
        };
        let reference = repo.search_reference(&query)?;
        let warmup = repo.search(&query)?;
        if warmup.hits != reference.hits || warmup.count != reference.count {
            return Err(format!("reference mismatch in {name}"));
        }
        let mut samples = Vec::with_capacity(repetitions);
        let mut last = warmup;
        for _ in 0..repetitions {
            let started = Instant::now();
            let result = repo.search(&query)?;
            let elapsed = started.elapsed().as_nanos() as u64;
            if result.hits != reference.hits || result.count != reference.count {
                return Err(format!("timed result mismatch in {name}"));
            }
            samples.push(elapsed);
            last = result;
        }
        samples.sort_unstable();
        let (_, traces) = repo.explain_analyze(&query)?;
        let order = traces
            .iter()
            .flat_map(|trace| trace.terms.iter())
            .map(|choice| choice.field.as_str())
            .collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::json!({
                "case": name,
                "filter": filter,
                "evidence": evidence,
                "repetitions": repetitions,
                "min_ns": samples[0],
                "median_ns": samples[repetitions / 2],
                "max_ns": samples[repetitions - 1],
                "count": last.count,
                "returned": last.hits.len(),
                "counters": last.counters,
                "predicate_order": order,
            })
        );
    }
    Ok(())
}
