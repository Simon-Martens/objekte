#[path = "support/musenalm.rs"]
mod musenalm;

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::lifecycle::IndexView;
use objekte_search_core::native_segment::SegmentPolicy;
use objekte_search_core::repository::RepositoryOptions;
use objekte_search_core::workload::BigramAlphabet;
use serde::Serialize;
use sha2::{Digest, Sha256};

fn hash_value<T: Serialize>(hash: &mut Sha256, value: &T) -> Result<(), String> {
    let bytes = bincode::serialize(value).map_err(|error| error.to_string())?;
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
    Ok(())
}

fn index_signature(repo: &IndexView) -> Result<String, String> {
    let mut hash = Sha256::new();
    for file in &repo.files().files {
        hash_value(&mut hash, &file.id)?;
        hash_value(&mut hash, &file.relative_path.to_path_buf())?;
        hash_value(&mut hash, &file.revision)?;
        hash_value(&mut hash, &file.collections)?;
        hash_value(&mut hash, &file.status)?;
    }
    for collection in repo.collections() {
        hash_value(&mut hash, &collection.definition.name)?;
    }
    hash_value(&mut hash, &repo.native_index_signature())?;
    Ok(format!("{:x}", hash.finalize()))
}

fn memory_bytes(name: &str) -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix(name)?
                .trim()
                .strip_suffix(" kB")?
                .trim()
                .parse::<u64>()
                .ok()
                .map(|value| value * 1024)
        })
}

fn main() {
    if let Err(error) = run() {
        eprintln!("build benchmark error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let mut alphabet = BigramAlphabet::Ascii;
    let mut build_workers = None;
    let mut segment_records = None;
    let mut schema_manifest_path = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" => root = PathBuf::from(arguments.next().ok_or("--root needs a path")?),
            "--schema-manifest-path" => {
                schema_manifest_path = Some(PathBuf::from(
                    arguments
                        .next()
                        .ok_or("--schema-manifest-path needs a path")?,
                ));
            }
            "--alphabet" => {
                alphabet = match arguments.next().ok_or("--alphabet needs a value")?.as_str() {
                    "ascii" => BigramAlphabet::Ascii,
                    "unicode" => BigramAlphabet::Unicode,
                    other => return Err(format!("unknown alphabet {other}")),
                }
            }
            "--build-workers" => {
                let value = arguments.next().ok_or("--build-workers needs a value")?;
                build_workers = if value == "auto" {
                    None
                } else {
                    Some(
                        NonZeroUsize::new(
                            value.parse::<usize>().map_err(|error| error.to_string())?,
                        )
                        .ok_or("build-workers must be positive")?,
                    )
                };
            }
            "--segment-records" => {
                let value = arguments.next().ok_or("--segment-records needs a value")?;
                segment_records = Some(
                    NonZeroUsize::new(value.parse::<usize>().map_err(|error| error.to_string())?)
                        .ok_or("segment-records must be positive")?,
                );
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let (definitions, loaded_schemas) = musenalm::load_definitions_with_schemas(&root)?;
    let schema_manifest = musenalm::schema_manifest(&loaded_schemas)?;
    if let Some(path) = schema_manifest_path {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&schema_manifest).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
    }
    let query_workers = RepositoryOptions::default().query_workers;
    let options = RepositoryOptions {
        bigram_alphabet: alphabet,
        build_workers,
        segment_policy: segment_records
            .map(SegmentPolicy::with_target_records)
            .transpose()?
            .unwrap_or_default(),
        ..RepositoryOptions::default()
    };
    let started = Instant::now();
    let repo = objekte_search_core::lifecycle::Index::start_build(
        &root,
        definitions,
        options,
        objekte_search_core::source::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .and_then(|index| index.view())
    .map_err(|error| error.to_string())?;
    let wall_ns = started.elapsed().as_nanos();
    let (inventory_heap_bytes, diagnostic_heap_bytes) = repo.files().estimated_heap_bytes();
    let registrations = repo.collections().iter().map(|c| serde_json::json!({"name":c.definition.name,"source_path":c.definition.source_path,"format":c.definition.format,"identity":format!("{:?}",c.definition.identity),"files":c.generation.files.len(),"records":c.generation.records.len(),"type_hints":c.definition.schema.fields().len(),"indexed_paths":c.definition.schema.fields().iter().filter(|(_,field)|field.indexed).map(|(p,_)|p.join(".")).collect::<Vec<_>>(),"sort_fields":c.definition.sort_fields.iter().map(|p|p.display()).collect::<Vec<_>>(),"custom_bindings":c.definition.schema.custom_bindings().keys().map(|(p,n)|format!("{}:{n}",p.join("."))).collect::<Vec<_>>() })).collect::<Vec<_>>();
    let segment_summaries = repo.segment_summaries();
    let index_stores = segment_summaries
        .iter()
        .map(|segment| segment.bigram_stores)
        .sum::<usize>();
    let distinct_gram_keys = segment_summaries
        .iter()
        .map(|segment| segment.distinct_gram_keys)
        .sum::<usize>();
    let bigram_heap_bytes = segment_summaries
        .iter()
        .map(|segment| segment.bigram_heap_bytes)
        .sum::<usize>();
    let index_family_bytes = segment_summaries
        .iter()
        .map(|segment| segment.index_family_bytes)
        .sum::<usize>();
    let segment_counts =
        segment_summaries
            .iter()
            .fold(std::collections::BTreeMap::new(), |mut counts, segment| {
                let name = &repo.collections()[segment.collection.0 as usize]
                    .definition
                    .name;
                counts.entry(name.clone()).or_insert_with(Vec::new).push((
                    segment.id.get(),
                    segment.records,
                    segment.files,
                ));
                counts
            });
    let resident_bytes = memory_bytes("VmRSS:");
    let peak_resident_bytes = memory_bytes("VmHWM:");
    let index_signature_sha256 = index_signature(&repo)?;
    println!(
        "{}",
        serde_json::json!({
            "alphabet": alphabet,
            "requested_build_workers": build_workers.map(NonZeroUsize::get),
            "segment_policy": options.segment_policy,
            "resolved_build_workers": repo.startup().build_workers,
            "available_parallelism": std::thread::available_parallelism().map_or(1, NonZeroUsize::get),
            "query_workers": query_workers,
            "peak_build_admitted_bytes": repo.startup().peak_build_admitted_bytes,
            "build_weight_limit_bytes": repo.startup().build_weight_limit_bytes,
            "build_peak_active_jobs": repo.startup().build_peak_active_jobs,
            "build_peak_ready_batches": repo.startup().build_peak_ready_batches,
            "build_job_execution_ns": repo.startup().build_job_execution_ns,
            "build_receive_wait_ns": repo.startup().build_receive_wait_ns,
            "build_admission_ns": repo.startup().build_admission_ns,
            "wall_ns": wall_ns,
            "build_ns": repo.startup().build_ns,
            "discovery_ns": repo.startup().build_discovery_ns,
            "extraction_index_ns": repo.startup().build_extraction_index_ns,
            "finalization_ns": repo.startup().build_finalization_ns,
            "build_parses": repo.build_parses(),
            "failed_parses": repo.build_failed_parses(),
            "inventory_heap_bytes": inventory_heap_bytes,
            "diagnostic_heap_bytes": diagnostic_heap_bytes,
            "index_stores": index_stores,
            "distinct_gram_keys": distinct_gram_keys,
            "bigram_heap_bytes": bigram_heap_bytes,
            "index_family_bytes": index_family_bytes,
            "segment_counts": segment_counts,
            "peak_concurrent_segment_jobs": repo.startup().build_peak_segment_jobs,
            "peak_segment_window": repo.startup().build_peak_segment_window,
            "records": repo.collections().iter().map(|collection| collection.generation.records.len()).sum::<usize>(),
            "resident_bytes": resident_bytes,
            "peak_resident_bytes": peak_resident_bytes,
            "index_signature_sha256": index_signature_sha256,
            "schema_manifest_digest": schema_manifest["sha256"],
            "schema_collections": loaded_schemas.iter().map(|schema| &schema.collection).collect::<Vec<_>>(),
            "registrations": registrations,
        })
    );
    Ok(())
}
