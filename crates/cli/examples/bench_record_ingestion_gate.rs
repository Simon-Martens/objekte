//! Measurement-only before/after probe for the record ingestion task.

#[path = "support/musenalm.rs"]
mod musenalm;

use std::collections::BTreeMap;
use std::error::Error;
use std::hint::black_box;
use std::path::Path;
use std::time::Instant;

use objekte_search_core::index::bigram::BigramAlphabet;
use objekte_search_core::lifecycle::Index;
use objekte_search_core::model::LogicalType;
use objekte_search_core::query::options::ReturnMode;
use objekte_search_core::query::{CaseSensitivity, FtsMode, Query};
use objekte_search_core::query_results::Payload;
use objekte_search_core::repository::configuration::{
    CollectionDefinition, IdentityPolicy, RepositoryOptions,
};
use objekte_search_core::schema::FieldDefinition;
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::{WorkloadCase, load_extended_manifest, load_manifest};
use serde_json::json;
use sha2::{Digest, Sha256};

fn query(filter: &str) -> Result<Query, String> {
    Query::from_workload(&WorkloadCase {
        id: "probe".into(),
        collection: "records".into(),
        fts: None,
        fts_case_sensitivity: CaseSensitivity::Sensitive,
        fts_mode: FtsMode::Literal,
        filter: Some(filter.into()),
        fts_fields: Vec::new(),
        sort: Vec::new(),
        limit: 0,
        offset: 0,
        count: true,
        return_mode: ReturnMode::Locator,
        evidence: false,
    })
}

fn direct_probe(root: &Path) -> Result<(), Box<dyn Error>> {
    let mut definition = CollectionDefinition::folder("records", "data", SourceFormat::Json);
    definition.identity = IdentityPolicy::Filename;
    for (name, kind, indexed) in [
        ("year", LogicalType::Number, true),
        ("note", LogicalType::String, false),
    ] {
        definition.schema.register_field(FieldDefinition {
            path: vec![name.into()],
            kind,
            required: true,
            nullable: false,
            indexed,
            multi: false,
            element_nullable: Vec::new(),
        })?;
    }
    let index = Index::start_build(
        root,
        vec![definition],
        RepositoryOptions::default(),
        objekte_cli::loader::installed_sources(),
    )?
    .wait_complete()?;
    let view = index.view()?;
    let exact = view.search(&query("year = 1808")?)?;
    if exact.count != Some(1024)
        || exact.counters.files_opened != 0
        || exact.counters.bytes_read != 0
    {
        return Err(format!("exact count violated: {:?}", exact.counters).into());
    }
    let residual = query("year = 1808 && note ~ 'Leipzig'")?;
    let mut lookup_samples_ns = Vec::new();
    let mut lookup_hits = 0usize;
    for _ in 0..7 {
        let started = Instant::now();
        for i in 0..10_000usize {
            let id = if i % 2 == 0 {
                format!("r{:05}", (i / 2) % 16_384)
            } else {
                format!("missing{:05}", i / 2)
            };
            let hit = view.get_record("records", &id, ReturnMode::Locator)?;
            lookup_hits += usize::from(hit.is_some());
            black_box(hit);
        }
        lookup_samples_ns.push(started.elapsed().as_nanos() as u64);
    }
    if lookup_hits != 35_000 {
        return Err("lookup hit count mismatch".into());
    }
    let mut residual_samples_ns = Vec::new();
    let mut last = None;
    for _ in 0..7 {
        let started = Instant::now();
        let result = view.search(&residual)?;
        residual_samples_ns.push(started.elapsed().as_nanos() as u64);
        if result.count != Some(1024) {
            return Err("residual count mismatch".into());
        }
        black_box(&result.count);
        last = Some(result.counters);
    }
    println!(
        "{}",
        json!({
            "records": 16384, "lookup_operations_per_sample": 10000,
            "lookup_hits": lookup_hits, "lookup_samples_ns": lookup_samples_ns,
            "residual_samples_ns": residual_samples_ns,
            "residual_count": 1024, "residual_counters": last,
            "exact_count": exact.count, "exact_counters": exact.counters,
        })
    );
    Ok(())
}

fn bytes_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 15) as usize] as char);
    }
    output
}

fn canonical(root: &Path, alphabet: BigramAlphabet, extended: bool) -> Result<(), Box<dyn Error>> {
    let historical = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/musenalm_queries.json");
    let additional =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/musenalm_queries_extended.json");
    let cases = if extended {
        load_extended_manifest(&historical, &additional)?
    } else {
        load_manifest(&historical)?
            .into_iter()
            .map(|request| objekte_search_core::workload::BenchmarkCase {
                request,
                parameters: BTreeMap::new(),
                clock_unix_seconds: None,
            })
            .collect()
    };
    let (definitions, _) = if extended {
        musenalm::load_extended_definitions_with_schemas(root)?
    } else {
        musenalm::load_definitions_with_schemas(root)?
    };
    let index = Index::start_build(
        root,
        definitions,
        RepositoryOptions {
            bigram_alphabet: alphabet,
            query_workers: 1,
            ..RepositoryOptions::default()
        },
        objekte_cli::loader::installed_sources(),
    )?
    .wait_complete()?;
    let view = index.view()?;
    let mut output = Vec::new();
    for case in cases {
        if extended
            && !matches!(
                case.id.as_str(),
                "Q155" | "Q163" | "Q166" | "Q170" | "Q195" | "Q219"
            )
        {
            continue;
        }
        let result = view.search(&case.query()?)?;
        let mut hits = Vec::new();
        for hit in result.hits {
            let path = hit
                .file
                .as_ref()
                .expect("file origin")
                .path
                .to_string_lossy()
                .replace('\\', "/");
            let range = hit.source_range().expect("file range");
            let mut fields = hit.matched_fields;
            fields.sort();
            let payload = match hit.payload {
                None => serde_json::Value::Null,
                Some(Payload::Logical(value)) => serde_json::to_value(value)?,
                Some(Payload::Bytes(bytes)) => json!({"original_bytes_hex": bytes_hex(&bytes)}),
                Some(Payload::LogicalUnavailable { .. }) => json!({"logical_unavailable": true}),
                Some(Payload::OriginalBytesUnavailable) => {
                    json!({"original_bytes_unavailable": true})
                }
            };
            hits.push(json!({
                "collection": hit.collection.clone(), "id": hit.record_id.clone(),
                "path": path,
                "range": [range.start, range.end],
                "matched_fields": fields, "payload": payload,
            }));
        }
        output.push(json!({"id": case.id, "count": result.count, "hits": hits}));
    }
    let encoded = serde_json::to_vec(&output)?;
    let digest = Sha256::digest(&encoded);
    println!(
        "{}",
        json!({"alphabet": format!("{alphabet:?}"),
                             "extended": extended, "semantic_sha256": format!("{digest:x}"),
                             "cases": output})
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().ok_or("expected probe|canonical")?;
    let root = args.next().ok_or("expected root")?;
    let root = Path::new(&root);
    match mode.as_str() {
        "probe" => direct_probe(root),
        "canonical" => {
            let alphabet = match args.next().as_deref() {
                Some("ascii") => BigramAlphabet::Ascii,
                Some("unicode") => BigramAlphabet::Unicode,
                _ => return Err("expected ascii|unicode".into()),
            };
            let extended = matches!(args.next().as_deref(), Some("extended"));
            canonical(root, alphabet, extended)
        }
        _ => Err("expected probe|canonical".into()),
    }
}
