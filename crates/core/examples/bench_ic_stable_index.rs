//! Independent stable-index prototype benchmark and fixture driver.
//!
//! Every mode operates on owned in-memory stable images.  `save-index` and
//! `snapshot` are the only modes that write application index files.

#[path = "support/musenalm.rs"]
mod musenalm;

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use objekte_search_core::model::{CollectionId, LogicalType, Number, RecordSlot};
use objekte_search_core::posting::ScalarKey;
use objekte_search_core::reference::FieldDefinition;
use objekte_search_core::repository::{CollectionDefinition, Repository, RepositoryOptions};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::stable_index::{
    BindingRole, ComponentDescriptor, ComponentId, ComponentKey, ComponentKind, FieldComponentKey,
    Restriction, SortComponentKey, StablePrototype, TextComponentKey, sort_component_key,
};
use objekte_search_core::workload::BigramAlphabet;
use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

struct Args {
    mode: Option<String>,
    root: Option<PathBuf>,
    alphabet: BigramAlphabet,
    build_workers: Option<NonZeroUsize>,
    result_dir: Option<PathBuf>,
    snapshot_output: Option<PathBuf>,
    snapshot_input: Option<PathBuf>,
    index_input: Option<PathBuf>,
    index_output: Option<PathBuf>,
    components: Vec<ComponentId>,
    reference: Option<PathBuf>,
    queries: Option<PathBuf>,
    queries_output: Option<PathBuf>,
    field: Option<String>,
    value: Option<String>,
    construction: String,
    extended_relations: bool,
    repetitions: usize,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            mode: None,
            root: None,
            alphabet: BigramAlphabet::Ascii,
            build_workers: None,
            result_dir: None,
            snapshot_output: None,
            snapshot_input: None,
            index_input: None,
            index_output: None,
            components: Vec::new(),
            reference: None,
            queries: None,
            queries_output: None,
            field: None,
            value: None,
            construction: "direct".into(),
            extended_relations: false,
            repetitions: 200,
        }
    }
}

fn worker_budget(args: &Args) -> usize {
    args.build_workers
        .map(NonZeroUsize::get)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|value| value.get())
                .unwrap_or(1)
        })
}

fn main() {
    if let Err(error) = run() {
        eprintln!("ic stable-index benchmark error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let mode = args.mode.as_deref().ok_or("--mode is required")?;
    if let Some(result_dir) = &args.result_dir {
        std::fs::create_dir_all(result_dir).map_err(|error| error.to_string())?;
    }
    match mode {
        "source-build" => source_build(&args, false),
        "snapshot" => source_build(&args, true),
        "fixture" => fixture_mode(&args),
        "validate" => validate_mode(&args),
        "open" => open_mode(&args),
        "open-index" => open_index_mode(&args),
        "save-index" => save_index_mode(&args),
        "primitives" => primitives_mode(&args),
        "primitive-plan" => primitive_plan_mode(&args),
        "native-primitives" => native_primitives_mode(&args),
        "mutations" => mutations_mode(&args),
        "lifecycle" => lifecycle_mode(&args),
        other => Err(format!("unknown --mode {other}")),
    }
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args::default();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--mode" => args.mode = Some(arguments.next().ok_or("--mode needs a value")?),
            "--root" => {
                args.root = Some(PathBuf::from(
                    arguments.next().ok_or("--root needs a path")?,
                ))
            }
            "--alphabet" => {
                args.alphabet = match arguments.next().ok_or("--alphabet needs a value")?.as_str() {
                    "ascii" => BigramAlphabet::Ascii,
                    "unicode" => BigramAlphabet::Unicode,
                    other => return Err(format!("unknown alphabet {other}")),
                }
            }
            "--build-workers" => {
                let value = arguments.next().ok_or("--build-workers needs a value")?;
                args.build_workers = if value == "auto" {
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
            "--result-dir" => {
                args.result_dir = Some(PathBuf::from(
                    arguments.next().ok_or("--result-dir needs a path")?,
                ));
            }
            "--snapshot-output" => {
                args.snapshot_output = Some(PathBuf::from(
                    arguments.next().ok_or("--snapshot-output needs a path")?,
                ));
            }
            "--snapshot-input" => {
                args.snapshot_input = Some(PathBuf::from(
                    arguments.next().ok_or("--snapshot-input needs a path")?,
                ));
            }
            "--index-input" => {
                args.index_input = Some(PathBuf::from(
                    arguments.next().ok_or("--index-input needs a path")?,
                ));
            }
            "--index-output" => {
                args.index_output = Some(PathBuf::from(
                    arguments.next().ok_or("--index-output needs a path")?,
                ));
            }
            "--component" => {
                let value = arguments.next().ok_or("--component needs a value")?;
                args.components.push(ComponentId(
                    value.parse::<u32>().map_err(|error| error.to_string())?,
                ));
            }
            "--reference" => {
                args.reference = Some(PathBuf::from(
                    arguments.next().ok_or("--reference needs a path")?,
                ));
            }
            "--queries" => {
                args.queries = Some(PathBuf::from(
                    arguments.next().ok_or("--queries needs a path")?,
                ));
            }
            "--queries-output" => {
                args.queries_output = Some(PathBuf::from(
                    arguments.next().ok_or("--queries-output needs a path")?,
                ));
            }
            "--field" => args.field = Some(arguments.next().ok_or("--field needs a value")?),
            "--value" => args.value = Some(arguments.next().ok_or("--value needs a value")?),
            "--repetitions" => {
                args.repetitions = arguments
                    .next()
                    .ok_or("--repetitions needs a value")?
                    .parse::<usize>()
                    .map_err(|error| error.to_string())?;
                if args.repetitions == 0 {
                    return Err("--repetitions must be positive".into());
                }
            }
            "--construction" => {
                let value = arguments.next().ok_or("--construction needs a value")?;
                if value != "direct" && value != "transfer" {
                    return Err(format!("unknown construction mode {value}"));
                }
                args.construction = value;
            }
            "--extended-relations" => args.extended_relations = true,
            "--inference" => {
                return Err("inference is explicitly unsupported by this prototype".into());
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(args)
}

fn load_musenalm_definitions(
    args: &Args,
    root: &Path,
) -> Result<(Vec<CollectionDefinition>, Vec<musenalm::LoadedSchema>), String> {
    if args.extended_relations {
        musenalm::load_extended_definitions_with_schemas(root)
    } else {
        musenalm::load_definitions_with_schemas(root)
    }
}

fn source_build(args: &Args, force_snapshot: bool) -> Result<(), String> {
    if args.construction == "direct" {
        return direct_source_build(args, force_snapshot);
    }
    let root = args.root.clone().ok_or("--root is required")?;
    let (definitions, schemas) = load_musenalm_definitions(args, &root)?;
    let schema_manifest = musenalm::schema_manifest(&schemas)?;
    let schema_digest = schema_manifest["sha256"]
        .as_str()
        .ok_or("schema manifest digest missing")?
        .to_string();
    let native_started = Instant::now();
    let repository = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            bigram_alphabet: args.alphabet,
            build_workers: args.build_workers,
            ..RepositoryOptions::default()
        },
    )?;
    let native_build_ns = native_started.elapsed().as_nanos() as u64;
    let readiness_rss = resident_bytes();
    let readiness_hwm = high_water_bytes();
    let source_bytes = repository
        .files()
        .files
        .iter()
        .map(|file| file.revision.size)
        .sum::<u64>();
    let source_records = repository
        .collections()
        .iter()
        .map(|collection| collection.generation.records.len())
        .sum::<usize>();
    let conversion_started = Instant::now();
    let mut prototype =
        StablePrototype::from_repository_with_workers(&repository, worker_budget(args))?;
    if force_snapshot && args.snapshot_output.is_none() {
        return Err("snapshot mode requires --snapshot-output".into());
    }
    let conversion_ns = conversion_started.elapsed().as_nanos() as u64;
    let total_build_ns = native_build_ns.saturating_add(conversion_ns);
    let mut persistence = json!({});
    if let Some(output) = &args.snapshot_output {
        let started = Instant::now();
        let generation = prototype.save_snapshot(output)?;
        let save_ns = started.elapsed().as_nanos() as u64;
        persistence = json!({
            "generation": generation,
            "snapshot_save_ns": save_ns,
            "manifest_components": prototype.manifest().map_or(0, |manifest| manifest.components.len()),
        });
    }
    let descriptors = prototype
        .component_descriptors()
        .cloned()
        .collect::<Vec<_>>();
    let build_records = prototype.build_records().to_vec();
    let overlap = field_overlap_pairs(&build_records);
    let payload_bytes = build_records
        .iter()
        .map(|record| record.payload_len)
        .sum::<u64>();
    let components_json = json!({
        "components": descriptors,
        "records": build_records,
        "overlap_pairs": overlap,
        "field_job_overlap_demonstrated": overlap > 0,
    });
    let build_json = json!({
        "construction_mode": "native_build_then_independent_component_transfer",
        "requested_build_workers": args.build_workers.map(NonZeroUsize::get),
        "resolved_build_workers": repository.startup().build_workers,
        "available_parallelism": std::thread::available_parallelism().map_or(1, NonZeroUsize::get),
        "native_upstream_build_ns": native_build_ns,
        "stable_component_conversion_ns": conversion_ns,
        "total_source_build_ns": total_build_ns,
        "source_files": repository.files().files.len(),
        "source_bytes": source_bytes,
        "source_records": source_records,
        "schema_sha256": schema_digest,
        "counters": prototype.counters(),
        "components": prototype.component_descriptors().count(),
    });
    let memory_json = json!({
        "readiness_rss_bytes": readiness_rss,
        "readiness_hwm_bytes": readiness_hwm,
        "owned_component_payload_bytes": payload_bytes,
        "native_index_stats": repository.collections().iter().map(|collection| json!({
            "collection": collection.definition.name,
            "records": collection.generation.records.len(),
            "posting_fields": collection.postings.fields.len(),
            "custom_postings": collection.custom_postings.len(),
            "custom_text": collection.custom_text.len(),
            "relations": collection.relations.len(),
        })).collect::<Vec<_>>(),
    });
    write_result(args.result_dir.as_deref(), "build.json", &build_json)?;
    write_result(args.result_dir.as_deref(), "memory.json", &memory_json)?;
    write_result(
        args.result_dir.as_deref(),
        "components.json",
        &components_json,
    )?;
    write_result(args.result_dir.as_deref(), "persistence.json", &persistence)?;
    write_result(
        args.result_dir.as_deref(),
        "campaign-manifest.json",
        &json!({
            "alphabet": args.alphabet,
            "construction": args.construction,
            "schema_sha256": schema_digest,
            "schema_collections": schemas.iter().map(|schema| &schema.collection).collect::<Vec<_>>(),
            "executable": std::env::current_exe().ok(),
            "executable_sha256": executable_digest(),
            "cargo_lock_sha256": lockfile_digest(),
            "source_files": repository.files().files.len(),
            "source_records": source_records,
            "source_bytes": source_bytes,
            "root": root,
        }),
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "build": build_json,
            "memory": memory_json,
            "persistence": persistence,
            "field_job_overlap_pairs": overlap,
        }))
        .map_err(|error| error.to_string())?
    );
    Ok(())
}

fn direct_source_build(args: &Args, force_snapshot: bool) -> Result<(), String> {
    let root = args.root.clone().ok_or("--root is required")?;
    let (definitions, schemas) = load_musenalm_definitions(args, &root)?;
    let schema_manifest = musenalm::schema_manifest(&schemas)?;
    let schema_digest = schema_manifest["sha256"]
        .as_str()
        .ok_or("schema manifest digest missing")?
        .to_string();
    let options = RepositoryOptions {
        bigram_alphabet: args.alphabet,
        build_workers: args.build_workers,
        ..RepositoryOptions::default()
    };
    let started = Instant::now();
    let (mut prototype, timings) =
        StablePrototype::from_sources_with_timings(&root, definitions, options)?;
    let total_source_build_ns = started.elapsed().as_nanos() as u64;
    let readiness_rss = resident_bytes();
    let readiness_hwm = high_water_bytes();
    if force_snapshot && args.snapshot_output.is_none() {
        return Err("snapshot mode requires --snapshot-output".into());
    }
    let mut persistence = json!({});
    if let Some(output) = &args.snapshot_output {
        let save_started = Instant::now();
        let generation = prototype.save_snapshot(output)?;
        let save_ns = save_started.elapsed().as_nanos() as u64;
        persistence = json!({
            "generation": generation,
            "snapshot_save_ns": save_ns,
            "manifest_components": prototype.manifest().map_or(0, |manifest| manifest.components.len()),
        });
    }
    let descriptors = prototype
        .component_descriptors()
        .cloned()
        .collect::<Vec<_>>();
    let build_records = prototype.build_records().to_vec();
    let overlap = field_overlap_pairs(&build_records);
    let payload_bytes = build_records
        .iter()
        .map(|record| record.payload_len)
        .sum::<u64>();
    let stage_ns = prototype.build_stage_ns().clone();
    let trace = trace_metrics(&build_records, worker_budget(args));
    let build_json = json!({
        "construction_mode": "direct_batch_contributions",
        "requested_build_workers": args.build_workers.map(NonZeroUsize::get),
        "resolved_build_workers": args.build_workers.map(NonZeroUsize::get).unwrap_or_else(|| std::thread::available_parallelism().map_or(1, NonZeroUsize::get)),
        "available_parallelism": std::thread::available_parallelism().map_or(1, NonZeroUsize::get),
        "total_source_build_ns": total_source_build_ns,
        "stage_ns": stage_ns,
        "source_files": timings.source_files,
        "source_records": timings.source_records,
        "source_bytes": timings.source_bytes,
        "source_reads": timings.source_reads,
        "source_bytes_read": timings.source_bytes_read,
        "source_parses": timings.source_parses,
        "failed_parses": timings.failed_parses,
        "peak_active_jobs": timings.peak_active_jobs,
        "peak_ready_batches": timings.peak_ready_batches,
        "preparation_waves": timings.preparation_waves,
        "oversized_preparation_jobs": timings.oversized_preparation_jobs,
        "job_execution_ns": timings.job_execution_ns,
        "receive_wait_ns": timings.receive_wait_ns,
        "admission_ns": timings.admission_ns,
        "owner_merge_ns": timings.owner_merge_ns,
        "unavailable_diagnostics": if timings.job_execution_ns.is_none() {
            json!({
                "job_execution_ns": "detailed-profiling feature disabled",
                "receive_wait_ns": "detailed-profiling feature disabled",
                "admission_ns": "detailed-profiling feature disabled",
            })
        } else {
            json!({})
        },
        "source_build_timings": {
            "discovery_ns": timings.discovery_ns,
            "setup_ns": timings.setup_ns,
            "extraction_admission_ns": timings.extraction_admission_ns,
            "owner_merge_ns": timings.owner_merge_ns,
            "finalization_ns": timings.finalization_ns,
            "stable_component_ns": timings.stable_component_ns,
        },
        "schema_sha256": schema_digest,
        "counters": prototype.counters(),
        "components": prototype.component_descriptors().count(),
        "trace": trace,
    });
    let components_json = json!({
        "components": descriptors,
        "records": build_records,
        "overlap_pairs": overlap,
        "field_job_overlap_demonstrated": overlap > 0,
        "trace": trace,
    });
    let memory_json = json!({
        "readiness_rss_bytes": readiness_rss,
        "readiness_hwm_bytes": readiness_hwm,
        "owned_component_payload_bytes": payload_bytes,
    });
    write_result(args.result_dir.as_deref(), "build.json", &build_json)?;
    write_result(args.result_dir.as_deref(), "memory.json", &memory_json)?;
    write_result(
        args.result_dir.as_deref(),
        "components.json",
        &components_json,
    )?;
    write_result(args.result_dir.as_deref(), "persistence.json", &persistence)?;
    write_result(
        args.result_dir.as_deref(),
        "campaign-manifest.json",
        &json!({
            "alphabet": args.alphabet,
            "construction": args.construction,
            "schema_sha256": schema_digest,
            "schema_collections": schemas.iter().map(|schema| &schema.collection).collect::<Vec<_>>(),
            "executable": std::env::current_exe().ok(),
            "executable_sha256": executable_digest(),
            "cargo_lock_sha256": lockfile_digest(),
            "source_files": timings.source_files,
            "source_records": timings.source_records,
            "source_bytes": timings.source_bytes,
            "root": root,
        }),
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "build": build_json,
            "memory": memory_json,
            "persistence": persistence,
            "field_job_overlap_pairs": overlap,
        }))
        .map_err(|error| error.to_string())?
    );
    Ok(())
}

fn fixture_mode(args: &Args) -> Result<(), String> {
    let root = create_fixture()?;
    let result = (|| -> Result<(), String> {
        let definitions = fixture_definitions();
        let repository = Repository::open(&root, definitions)?;
        let started = Instant::now();
        let mut prototype = StablePrototype::from_repository(&repository)?;
        let build_ns = started.elapsed().as_nanos() as u64;
        let report = prototype.validate_against_repository(&repository)?;
        let mut persistence = json!({});
        if let Some(index_output) = &args.index_output {
            let component = args
                .components
                .first()
                .copied()
                .or_else(|| {
                    prototype
                        .component_descriptors()
                        .find(|descriptor| {
                            descriptor.kind
                                == objekte_search_core::stable_index::ComponentKind::Values
                        })
                        .map(|descriptor| descriptor.id)
                })
                .ok_or("fixture has no value component")?;
            let save_started = Instant::now();
            let info = prototype.save_index(component, index_output)?;
            let save_ns = save_started.elapsed().as_nanos() as u64;
            let open_started = Instant::now();
            let standalone = StablePrototype::open_index(index_output)?;
            let open_ns = open_started.elapsed().as_nanos() as u64;
            persistence = json!({
                "standalone_save_ns": save_ns,
                "standalone_open_ns": open_ns,
                "standalone": info,
                "standalone_entry_count": standalone.entry_count(),
            });
        }
        if let Some(snapshot_output) = &args.snapshot_output {
            let save_started = Instant::now();
            prototype.save_snapshot(snapshot_output)?;
            let save_ns = save_started.elapsed().as_nanos() as u64;
            let open_started = Instant::now();
            let opened = StablePrototype::open_snapshot(snapshot_output)?;
            let open_ns = open_started.elapsed().as_nanos() as u64;
            persistence = json!({
                "snapshot_save_ns": save_ns,
                "manifest_open_ns": open_ns,
                "components": opened.component_descriptors().count(),
            });
        }
        let build = json!({
            "fixture_build_ns": build_ns,
            "components": prototype.component_descriptors().count(),
            "records": prototype.collections().map(|collection| collection.slot_high_water).sum::<u32>(),
            "counters": prototype.counters(),
        });
        write_result(args.result_dir.as_deref(), "build.json", &build)?;
        write_result(args.result_dir.as_deref(), "validation.json", &report)?;
        write_result(args.result_dir.as_deref(), "persistence.json", &persistence)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "build": build,
                "validation": report,
                "persistence": persistence,
            }))
            .map_err(|error| error.to_string())?
        );
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&root);
    result
}

fn validate_mode(args: &Args) -> Result<(), String> {
    let root = args.root.clone().ok_or("--root is required")?;
    let (definitions, schemas) = load_musenalm_definitions(args, &root)?;
    let definitions_for_stable = if args.construction == "direct" {
        Some(definitions.clone())
    } else {
        None
    };
    let repository = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            bigram_alphabet: args.alphabet,
            build_workers: args.build_workers,
            ..RepositoryOptions::default()
        },
    )?;
    let mut prototype = if let Some(definitions) = definitions_for_stable {
        StablePrototype::from_sources(
            &root,
            definitions,
            RepositoryOptions {
                bigram_alphabet: args.alphabet,
                build_workers: args.build_workers,
                ..RepositoryOptions::default()
            },
        )?
    } else {
        StablePrototype::from_repository_with_workers(&repository, worker_budget(args))?
    };
    let report = prototype.validate_against_repository(&repository)?;
    let output = json!({
        "schema_collections": schemas.iter().map(|schema| &schema.collection).collect::<Vec<_>>(),
        "validation": report,
    });
    write_result(args.result_dir.as_deref(), "validation.json", &output)?;
    if !report.is_valid() {
        return Err(format!("validation failed: {:?}", report.errors));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&output).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn open_mode(args: &Args) -> Result<(), String> {
    let input = args
        .snapshot_input
        .as_ref()
        .ok_or("open requires --snapshot-input")?;
    if args.index_input.is_some() {
        return Err("open does not accept --index-input".into());
    }
    let open_started = Instant::now();
    let mut prototype = StablePrototype::open_snapshot(input)?;
    let manifest_open_ns = open_started.elapsed().as_nanos() as u64;
    let mut loads = Vec::new();
    for component in &args.components {
        let started = Instant::now();
        prototype.load_component(*component)?;
        loads.push(json!({
            "component": component.0,
            "load_ns": started.elapsed().as_nanos() as u64,
            "statistics": prototype.component_statistics(*component)?,
        }));
    }
    let output = json!({
        "snapshot": input,
        "manifest_open_ns": manifest_open_ns,
        "loads": loads,
        "counters": prototype.counters(),
        "components": prototype.component_descriptors().count(),
    });
    write_result(args.result_dir.as_deref(), "persistence.json", &output)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&output).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn open_index_mode(args: &Args) -> Result<(), String> {
    let input = args
        .index_input
        .as_ref()
        .ok_or("open-index requires --index-input")?;
    if args.snapshot_input.is_some() {
        return Err("open-index does not accept --snapshot-input".into());
    }
    let started = Instant::now();
    let standalone = StablePrototype::open_index(input)?;
    let open_ns = started.elapsed().as_nanos() as u64;
    let output = json!({
        "index": input,
        "open_ns": open_ns,
        "entry_count": standalone.entry_count(),
        "descriptor": standalone.descriptor(),
        "counters": standalone.counters(),
    });
    write_result(args.result_dir.as_deref(), "persistence.json", &output)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&output).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn save_index_mode(args: &Args) -> Result<(), String> {
    let output = args
        .index_output
        .as_ref()
        .ok_or("save-index requires --index-output")?;
    let started = Instant::now();
    let result = match (&args.snapshot_input, &args.index_input) {
        (Some(snapshot), None) => {
            if args.components.len() != 1 {
                return Err(
                    "save-index with --snapshot-input requires exactly one --component".into(),
                );
            }
            let mut prototype = StablePrototype::open_snapshot(snapshot)?;
            json!(prototype.save_index(args.components[0], output)?)
        }
        (None, Some(index)) => {
            let mut standalone = StablePrototype::open_index(index)?;
            json!(standalone.save_as(output)?)
        }
        (Some(_), Some(_)) => {
            return Err(
                "save-index accepts either --snapshot-input or --index-input, not both".into(),
            );
        }
        (None, None) => {
            return Err(
                "save-index requires --snapshot-input plus --component or --index-input".into(),
            );
        }
    };
    let output_json = json!({
        "elapsed_ns": started.elapsed().as_nanos() as u64,
        "saved": result,
    });
    write_result(args.result_dir.as_deref(), "persistence.json", &output_json)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&output_json).map_err(|error| error.to_string())?
    );
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct PrimitiveField {
    path: Vec<String>,
    binding: Option<String>,
    role: BindingRole,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PointQuery {
    collection: String,
    field: PrimitiveField,
    key: ScalarKey,
    stable_component: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RangeQuery {
    collection: String,
    field: PrimitiveField,
    lower: Option<ScalarKey>,
    upper: Option<ScalarKey>,
    descending: bool,
    stable_component: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LengthPointQuery {
    collection: String,
    field: PrimitiveField,
    length: Number,
    stable_component: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LengthRangeQuery {
    collection: String,
    field: PrimitiveField,
    lower: Option<Number>,
    upper: Option<Number>,
    stable_component: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StateQuery {
    collection: String,
    field: PrimitiveField,
    state: u8,
    stable_component: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TextQuery {
    collection: String,
    field: PrimitiveField,
    literal: String,
    stable_components: Vec<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SortQuery {
    collection: String,
    path: Vec<String>,
    descending: bool,
    limit: usize,
    sort_key: String,
    stable_components: Vec<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RelationStateQuery {
    collection: String,
    path: Vec<String>,
    target_collection: String,
    state: u8,
    stable_states: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RelationTargetQuery {
    collection: String,
    path: Vec<String>,
    target_collection: String,
    target_collection_id: u32,
    target_id: String,
    stable_targets: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PrimitivePlan {
    alphabet: BigramAlphabet,
    collections: Vec<String>,
    point: Vec<PointQuery>,
    range: Vec<RangeQuery>,
    length_point: Vec<LengthPointQuery>,
    length_range: Vec<LengthRangeQuery>,
    state: Vec<StateQuery>,
    text: Vec<TextQuery>,
    sort: Vec<SortQuery>,
    relation_state: Vec<RelationStateQuery>,
    relation_target: Vec<RelationTargetQuery>,
}

impl PrimitivePlan {
    fn stable_components(&self) -> Vec<u32> {
        let mut components = Vec::new();
        components.extend(self.point.iter().map(|query| query.stable_component));
        components.extend(self.range.iter().map(|query| query.stable_component));
        components.extend(self.length_point.iter().map(|query| query.stable_component));
        components.extend(self.length_range.iter().map(|query| query.stable_component));
        components.extend(self.state.iter().map(|query| query.stable_component));
        for query in &self.text {
            components.extend(query.stable_components.iter().copied());
        }
        for query in &self.sort {
            components.extend(query.stable_components.iter().copied());
        }
        components.extend(self.relation_state.iter().map(|query| query.stable_states));
        components.extend(
            self.relation_target
                .iter()
                .map(|query| query.stable_targets),
        );
        let mut unique = std::collections::BTreeSet::new();
        components
            .into_iter()
            .filter(|id| unique.insert(*id))
            .collect()
    }

    fn collections(&self) -> Vec<String> {
        let mut unique = std::collections::BTreeSet::new();
        self.collections
            .iter()
            .filter(|name| unique.insert((*name).clone()))
            .cloned()
            .collect()
    }
}

fn primitive_plan_mode(args: &Args) -> Result<(), String> {
    let input = args
        .snapshot_input
        .as_ref()
        .ok_or("primitive-plan requires --snapshot-input")?;
    let mut prototype = StablePrototype::open_snapshot(input)?;
    let plan = build_primitive_plan(&mut prototype, args.alphabet)?;
    let output = if let Some(path) = &args.queries_output {
        path.clone()
    } else if let Some(directory) = &args.result_dir {
        std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        directory.join("primitives-plan.json")
    } else {
        return Err("primitive-plan requires --queries-output or --result-dir".into());
    };
    std::fs::write(
        &output,
        serde_json::to_string_pretty(&plan).map_err(|error| error.to_string())? + "\n",
    )
    .map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "plan": output,
            "collections": plan.collections(),
            "point": plan.point.len(),
            "range": plan.range.len(),
            "length_point": plan.length_point.len(),
            "length_range": plan.length_range.len(),
            "state": plan.state.len(),
            "text": plan.text.len(),
            "sort": plan.sort.len(),
            "relation_state": plan.relation_state.len(),
            "relation_target": plan.relation_target.len(),
        }))
        .map_err(|error| error.to_string())?
    );
    Ok(())
}

fn build_primitive_plan(
    prototype: &mut StablePrototype,
    alphabet: BigramAlphabet,
) -> Result<PrimitivePlan, String> {
    let mut plan = PrimitivePlan {
        alphabet,
        collections: prototype
            .collections()
            .map(|collection| collection.name.clone())
            .collect(),
        point: Vec::new(),
        range: Vec::new(),
        length_point: Vec::new(),
        length_range: Vec::new(),
        state: Vec::new(),
        text: Vec::new(),
        sort: Vec::new(),
        relation_state: Vec::new(),
        relation_target: Vec::new(),
    };
    let descriptors = prototype
        .component_descriptors()
        .cloned()
        .collect::<Vec<_>>();
    let mut string_values = BTreeMap::<Vec<String>, String>::new();
    for descriptor in &descriptors {
        if descriptor.kind != ComponentKind::Values {
            continue;
        }
        let Some(field) = &descriptor.field else {
            continue;
        };
        let keys = value_keys(prototype, descriptor.id)?;
        if let Some(text) = keys.iter().find_map(|key| match key {
            ScalarKey::String(text) | ScalarKey::Id(text) => Some(text.clone()),
            _ => None,
        }) {
            string_values.entry(field.path.clone()).or_insert(text);
        }
    }

    let collection_ids = prototype
        .collections()
        .map(|collection| (collection.id, collection.name.clone()))
        .collect::<Vec<_>>();
    for (collection_id, collection_name) in collection_ids {
        let values = descriptors
            .iter()
            .filter(|descriptor| {
                descriptor.kind == ComponentKind::Values
                    && descriptor.coverage.values_indexed
                    && descriptor.coverage.value_complete
                    && descriptor
                        .collection
                        .as_ref()
                        .is_some_and(|value| value.id == collection_id)
            })
            .take(12)
            .cloned()
            .collect::<Vec<_>>();
        for descriptor in values {
            let Some(field) = descriptor.field.as_ref() else {
                continue;
            };
            let keys = value_keys(prototype, descriptor.id)?;
            if keys.is_empty() {
                continue;
            }
            let primitive = primitive_field(field);
            for key in select_samples(&keys, None, None) {
                plan.point.push(PointQuery {
                    collection: collection_name.clone(),
                    field: primitive.clone(),
                    key,
                    stable_component: descriptor.id.0,
                });
            }
            if keys.len() > 1 {
                plan.range.push(RangeQuery {
                    collection: collection_name.clone(),
                    field: primitive,
                    lower: keys.first().cloned(),
                    upper: keys.last().cloned(),
                    descending: false,
                    stable_component: descriptor.id.0,
                });
            }
        }

        let lengths = descriptors
            .iter()
            .filter(|descriptor| {
                descriptor.kind == ComponentKind::Lengths
                    && descriptor.coverage.length_complete
                    && descriptor
                        .collection
                        .as_ref()
                        .is_some_and(|value| value.id == collection_id)
            })
            .take(8)
            .cloned()
            .collect::<Vec<_>>();
        for descriptor in lengths {
            let Some(field) = descriptor.field.as_ref() else {
                continue;
            };
            let keys = length_keys(prototype, descriptor.id)?;
            if keys.is_empty() {
                continue;
            }
            let primitive = primitive_field(field);
            for length in select_number_samples(&keys) {
                plan.length_point.push(LengthPointQuery {
                    collection: collection_name.clone(),
                    field: primitive.clone(),
                    length,
                    stable_component: descriptor.id.0,
                });
            }
            if keys.len() > 1 {
                plan.length_range.push(LengthRangeQuery {
                    collection: collection_name.clone(),
                    field: primitive,
                    lower: keys.first().cloned(),
                    upper: keys.last().cloned(),
                    stable_component: descriptor.id.0,
                });
            }
        }

        let states = descriptors
            .iter()
            .filter(|descriptor| {
                descriptor.kind == ComponentKind::States
                    && descriptor.coverage.structural_complete
                    && descriptor
                        .collection
                        .as_ref()
                        .is_some_and(|value| value.id == collection_id)
            })
            .take(8)
            .cloned()
            .collect::<Vec<_>>();
        for descriptor in states {
            let Some(field) = descriptor.field.as_ref() else {
                continue;
            };
            let keys = state_keys(prototype, descriptor.id)?;
            let primitive = primitive_field(field);
            for state in keys.into_iter().take(4) {
                plan.state.push(StateQuery {
                    collection: collection_name.clone(),
                    field: primitive.clone(),
                    state,
                    stable_component: descriptor.id.0,
                });
            }
        }

        let texts = descriptors
            .iter()
            .filter(|descriptor| {
                descriptor.kind == ComponentKind::NormalGrams
                    && descriptor
                        .collection
                        .as_ref()
                        .is_some_and(|value| value.id == collection_id)
            })
            .take(8)
            .cloned()
            .collect::<Vec<_>>();
        for descriptor in texts {
            let Some(field) = descriptor.field.as_ref() else {
                continue;
            };
            let literal = match field.role {
                BindingRole::RawText | BindingRole::FallbackText => {
                    string_values.values().next().cloned()
                }
                _ => string_values.get(&field.path).cloned(),
            };
            let Some(literal) = literal else {
                continue;
            };
            let key = TextComponentKey {
                path: field.path.clone(),
                binding: field.binding.clone(),
                role: field.role,
            };
            let Some(ids) = prototype
                .collection(collection_id)
                .and_then(|collection| collection.texts.get(&key))
            else {
                continue;
            };
            let mut stable_components = Vec::new();
            if let Some(normal) = ids.normal {
                stable_components.push(normal.0);
            }
            if let Some(skip) = ids.skip {
                stable_components.push(skip.0);
            }
            if stable_components.is_empty() {
                continue;
            }
            plan.text.push(TextQuery {
                collection: collection_name.clone(),
                field: primitive_field(field),
                literal,
                stable_components,
            });
        }

        let sorts = descriptors
            .iter()
            .filter(|descriptor| {
                descriptor.kind == ComponentKind::SortAscending
                    && descriptor
                        .collection
                        .as_ref()
                        .is_some_and(|value| value.id == collection_id)
            })
            .take(8)
            .cloned()
            .collect::<Vec<_>>();
        for descriptor in sorts {
            let Some(field) = descriptor.field.as_ref() else {
                continue;
            };
            let key = sort_component_key(&descriptor);
            let Some(ids) = prototype
                .collection(collection_id)
                .and_then(|collection| collection.sorts.get(&key))
            else {
                continue;
            };
            for descending in [false, true] {
                plan.sort.push(SortQuery {
                    collection: collection_name.clone(),
                    path: field.path.clone(),
                    descending,
                    limit: 64,
                    sort_key: key.0.clone(),
                    stable_components: vec![ids.ascending.0, ids.descending.0],
                });
            }
        }

        let relations = prototype
            .collection(collection_id)
            .map(|collection| collection.relations.clone())
            .unwrap_or_default();
        for (key, ids) in relations.iter().take(8) {
            let state_keys = relation_state_keys(prototype, ids.states)?;
            for state in state_keys.into_iter().take(4) {
                plan.relation_state.push(RelationStateQuery {
                    collection: collection_name.clone(),
                    path: key.path.clone(),
                    target_collection: key.target_collection.clone(),
                    state,
                    stable_states: ids.states.0,
                });
            }
            let mut source = None;
            prototype.for_each_raw_entry(ids.occurrences, |component_key, _| {
                if let ComponentKey::Occurrence(occurrence) = component_key {
                    source = Some(occurrence.source);
                }
                Ok(())
            })?;
            let Some(source) = source else {
                continue;
            };
            let occurrences = prototype.relation_occurrences(collection_id, key, source)?;
            let Some(target_id) = occurrences
                .first()
                .map(|occurrence| occurrence.target_id.clone())
            else {
                continue;
            };
            plan.relation_target.push(RelationTargetQuery {
                collection: collection_name.clone(),
                path: key.path.clone(),
                target_collection: key.target_collection.clone(),
                target_collection_id: ids.target_collection.0,
                target_id,
                stable_targets: ids.targets.0,
            });
        }
    }
    Ok(plan)
}

fn primitive_field(field: &objekte_search_core::stable_index::FieldIdentity) -> PrimitiveField {
    PrimitiveField {
        path: field.path.clone(),
        binding: field.binding.clone(),
        role: field.role,
    }
}

fn length_keys(
    prototype: &mut StablePrototype,
    component: ComponentId,
) -> Result<Vec<Number>, String> {
    let mut keys = Vec::new();
    prototype.for_each_raw_entry(component, |key, _value| {
        if let ComponentKey::Length(length) = key {
            keys.push(length.clone());
        }
        Ok(())
    })?;
    Ok(keys)
}

fn state_keys(prototype: &mut StablePrototype, component: ComponentId) -> Result<Vec<u8>, String> {
    let mut keys = Vec::new();
    prototype.for_each_raw_entry(component, |key, _value| {
        if let ComponentKey::State(state) = key {
            keys.push(*state);
        }
        Ok(())
    })?;
    keys.sort_unstable();
    keys.dedup();
    Ok(keys)
}

fn relation_state_keys(
    prototype: &mut StablePrototype,
    component: ComponentId,
) -> Result<Vec<u8>, String> {
    let mut keys = Vec::new();
    prototype.for_each_raw_entry(component, |key, _value| {
        if let ComponentKey::RelationState(state) = key {
            keys.push(*state);
        }
        Ok(())
    })?;
    keys.sort_unstable();
    keys.dedup();
    Ok(keys)
}

fn select_number_samples(keys: &[Number]) -> Vec<Number> {
    let mut samples = Vec::new();
    if let Some(first) = keys.first() {
        samples.push(first.clone());
    }
    if let Some(last) = keys.last() {
        if samples.first() != Some(last) {
            samples.push(last.clone());
        }
    }
    samples
}

fn native_primitives_mode(args: &Args) -> Result<(), String> {
    let root = args
        .root
        .clone()
        .ok_or("native-primitives requires --root")?;
    let queries = args
        .queries
        .as_ref()
        .ok_or("native-primitives requires --queries")?;
    let plan: PrimitivePlan =
        serde_json::from_slice(&std::fs::read(queries).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let (definitions, _schemas) = load_musenalm_definitions(args, &root)?;
    let build_started = Instant::now();
    let repository = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            bigram_alphabet: plan.alphabet,
            build_workers: args.build_workers,
            ..RepositoryOptions::default()
        },
    )?;
    let build_ns = build_started.elapsed().as_nanos() as u64;
    let mut rows = Vec::new();
    for query in &plan.point {
        rows.push(measure_distribution(
            "point",
            query,
            args.repetitions,
            || {
                let index = native_field(&repository, &query.collection, &query.field)?;
                Ok(index.values.get(&query.key).map_or(0, RoaringBitmap::len) as usize)
            },
        )?);
    }
    for query in &plan.range {
        rows.push(measure_distribution(
            "range",
            query,
            args.repetitions,
            || {
                let index = native_field(&repository, &query.collection, &query.field)?;
                let lower = query
                    .lower
                    .clone()
                    .map_or(std::ops::Bound::Unbounded, std::ops::Bound::Included);
                let upper = query
                    .upper
                    .clone()
                    .map_or(std::ops::Bound::Unbounded, std::ops::Bound::Included);
                let mut result = RoaringBitmap::new();
                if query.descending {
                    for (_, posting) in index.values.range((lower, upper)).rev() {
                        result |= posting;
                    }
                } else {
                    for (_, posting) in index.values.range((lower, upper)) {
                        result |= posting;
                    }
                }
                Ok(result.len() as usize)
            },
        )?);
    }
    for query in &plan.length_point {
        rows.push(measure_distribution(
            "length_point",
            query,
            args.repetitions,
            || {
                let index = native_field(&repository, &query.collection, &query.field)?;
                Ok(index
                    .lengths
                    .get(&query.length)
                    .map_or(0, RoaringBitmap::len) as usize)
            },
        )?);
    }
    for query in &plan.length_range {
        rows.push(measure_distribution(
            "length_range",
            query,
            args.repetitions,
            || {
                let index = native_field(&repository, &query.collection, &query.field)?;
                let lower = query
                    .lower
                    .clone()
                    .map_or(std::ops::Bound::Unbounded, std::ops::Bound::Included);
                let upper = query
                    .upper
                    .clone()
                    .map_or(std::ops::Bound::Unbounded, std::ops::Bound::Included);
                let mut result = RoaringBitmap::new();
                for (_, posting) in index.lengths.range((lower, upper)) {
                    result |= posting;
                }
                Ok(result.len() as usize)
            },
        )?);
    }
    for query in &plan.state {
        rows.push(measure_distribution(
            "state",
            query,
            args.repetitions,
            || {
                let index = native_field(&repository, &query.collection, &query.field)?;
                Ok(native_state_posting(index, query.state)?.len() as usize)
            },
        )?);
    }
    for query in &plan.text {
        rows.push(measure_distribution(
            "text_candidates",
            query,
            args.repetitions,
            || {
                let index = native_text(&repository, &query.collection, &query.field)?;
                index
                    .candidates(&query.literal)
                    .map(|bitmap| bitmap.len() as usize)
                    .ok_or_else(|| "no native text restriction".to_string())
            },
        )?);
    }
    for query in &plan.sort {
        rows.push(measure_distribution(
            "sort",
            query,
            args.repetitions,
            || {
                let collection = native_collection(&repository, &query.collection)?;
                let column = collection
                    .sorting
                    .columns
                    .values()
                    .find(|column| column.path.segments == query.path)
                    .ok_or_else(|| "native sort column missing".to_string())?;
                let order = if query.descending {
                    &column.descending
                } else {
                    &column.ascending
                };
                Ok(order.len().min(query.limit))
            },
        )?);
    }
    for query in &plan.relation_state {
        rows.push(measure_distribution(
            "relation_state",
            query,
            args.repetitions,
            || {
                let relation = native_relation(
                    &repository,
                    &query.collection,
                    &query.path,
                    &query.target_collection,
                )?;
                Ok(native_relation_state(relation, query.state)?.len() as usize)
            },
        )?);
    }
    for query in &plan.relation_target {
        rows.push(measure_distribution(
            "relation_target",
            query,
            args.repetitions,
            || {
                let relation = native_relation(
                    &repository,
                    &query.collection,
                    &query.path,
                    &query.target_collection,
                )?;
                Ok(repository
                    .relation_reverse_sources(relation, &query.target_id)
                    .map_or(0, RoaringBitmap::len) as usize)
            },
        )?);
    }
    let output = json!({
        "implementation": "native",
        "build_ns": build_ns,
        "first_load": [],
        "first_dependency_load": [],
        "first_load_unavailable": "native repository indexes are resident after the measured build",
        "resident_primitives": rows,
        "repetitions": args.repetitions,
        "plan": queries,
        "unavailable": {
            "component_first_load_ns": "not applicable to the resident native repository",
            "component_dependency_load_ns": "not applicable to the resident native repository",
        },
    });
    write_result(args.result_dir.as_deref(), "primitives.json", &output)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&output).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn native_collection<'a>(
    repository: &'a Repository,
    name: &str,
) -> Result<&'a objekte_search_core::repository::CollectionState, String> {
    repository
        .collections()
        .iter()
        .find(|collection| collection.definition.name == name)
        .ok_or_else(|| format!("native collection missing: {name}"))
}

fn native_field<'a>(
    repository: &'a Repository,
    collection: &str,
    field: &PrimitiveField,
) -> Result<&'a objekte_search_core::posting::PostingIndex, String> {
    let state = native_collection(repository, collection)?;
    match field.role {
        BindingRole::DeclaredField => state
            .postings
            .fields
            .get(&field.path)
            .ok_or_else(|| "native posting field missing".into()),
        BindingRole::NamedBinding => state
            .custom_postings
            .get(&(
                field.path.clone(),
                field
                    .binding
                    .clone()
                    .ok_or("native custom posting binding missing")?,
            ))
            .map(|custom| &custom.index)
            .ok_or_else(|| "native custom posting missing".into()),
        _ => Err("native posting role is not a typed field".into()),
    }
}

fn native_text<'a>(
    repository: &'a Repository,
    collection: &str,
    field: &PrimitiveField,
) -> Result<&'a objekte_search_core::bigram::BigramIndex, String> {
    let state = native_collection(repository, collection)?;
    match field.role {
        BindingRole::RawText => Ok(&state.raw),
        BindingRole::FallbackText => Ok(&state.fallback_text),
        BindingRole::CustomText | BindingRole::LogicalText => state
            .custom_text
            .get(&(
                field.path.clone(),
                field
                    .binding
                    .clone()
                    .ok_or("native custom text binding missing")?,
            ))
            .ok_or_else(|| "native custom text index missing".into()),
        _ => Err("native text role is not a text index".into()),
    }
}

fn native_state_posting<'a>(
    index: &'a objekte_search_core::posting::PostingIndex,
    state: u8,
) -> Result<&'a RoaringBitmap, String> {
    use objekte_search_core::stable_index::{
        STATE_ANY_NULL, STATE_ARRAY, STATE_EMPTY_OBJECT, STATE_EMPTY_SELECTION, STATE_LEGACY_NULL,
        STATE_NON_EMPTY, STATE_NUMBER, STATE_OBJECT, STATE_PRESENCE, STATE_STRING,
    };
    match state {
        STATE_PRESENCE => Ok(&index.presence),
        STATE_EMPTY_SELECTION => Ok(&index.empty_selections),
        STATE_LEGACY_NULL => Ok(&index.nulls),
        other => {
            let facts = index
                .structural
                .as_ref()
                .ok_or("native structural facts missing")?;
            let key = match other {
                STATE_ANY_NULL => objekte_search_core::posting::StructuralKey::AnyNull,
                STATE_NON_EMPTY => objekte_search_core::posting::StructuralKey::NonEmpty,
                STATE_STRING => objekte_search_core::posting::StructuralKey::String,
                STATE_NUMBER => objekte_search_core::posting::StructuralKey::Number,
                STATE_ARRAY => objekte_search_core::posting::StructuralKey::Array,
                STATE_OBJECT => objekte_search_core::posting::StructuralKey::Object,
                STATE_EMPTY_OBJECT => objekte_search_core::posting::StructuralKey::EmptyObject,
                _ => return Err(format!("unknown state code {other}")),
            };
            facts
                .postings
                .get(&key)
                .ok_or_else(|| "native structural state missing".into())
        }
    }
}

fn native_relation<'a>(
    repository: &'a Repository,
    collection: &str,
    path: &[String],
    target_collection: &str,
) -> Result<&'a objekte_search_core::relation::RelationIndex, String> {
    native_collection(repository, collection)?
        .relations
        .iter()
        .find(|relation| {
            relation.declaration.path == path
                && relation.declaration.target_collection == target_collection
        })
        .ok_or_else(|| "native relation missing".into())
}

fn native_relation_state(
    relation: &objekte_search_core::relation::RelationIndex,
    state: u8,
) -> Result<&RoaringBitmap, String> {
    use objekte_search_core::stable_index::{
        RELATION_EMPTY_ARRAY, RELATION_NULLS, RELATION_PRESENT, RELATION_UNAVAILABLE,
    };
    match state {
        RELATION_PRESENT => Ok(relation.present()),
        RELATION_NULLS => Ok(relation.nulls()),
        RELATION_EMPTY_ARRAY => Ok(relation.empty_arrays()),
        RELATION_UNAVAILABLE => Ok(relation.unavailable()),
        _ => Err(format!("unknown relation state code {state}")),
    }
}

fn measure_distribution(
    family: &str,
    query: &impl Serialize,
    repetitions: usize,
    mut run: impl FnMut() -> Result<usize, String>,
) -> Result<serde_json::Value, String> {
    let mut samples = Vec::with_capacity(repetitions);
    let mut result_members = 0usize;
    for _ in 0..repetitions {
        let started = Instant::now();
        result_members = run()?;
        samples.push(started.elapsed().as_nanos() as u64);
    }
    samples.sort_unstable();
    let median = samples[samples.len() / 2];
    let p95 = samples[(samples.len() * 95 / 100).min(samples.len() - 1)];
    Ok(json!({
        "family": family,
        "query": query,
        "repetitions": repetitions,
        "median_ns": median,
        "p95_ns": p95,
        "min_ns": samples.first().copied(),
        "max_ns": samples.last().copied(),
        "result_members": result_members,
    }))
}

fn stable_primitives_with_plan(args: &Args, queries: &Path) -> Result<(), String> {
    let input = args
        .snapshot_input
        .as_ref()
        .ok_or("primitives requires --snapshot-input")?;
    let plan: PrimitivePlan =
        serde_json::from_slice(&std::fs::read(queries).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let open_started = Instant::now();
    let mut prototype = StablePrototype::open_snapshot(input)?;
    let manifest_open_ns = open_started.elapsed().as_nanos() as u64;
    let mut first_load = Vec::new();
    for component in plan.stable_components() {
        let started = Instant::now();
        prototype.load_component(ComponentId(component))?;
        first_load.push(json!({
            "component": component,
            "load_ns": started.elapsed().as_nanos() as u64,
        }));
    }
    let mut first_dependency_load = Vec::new();
    for name in plan.collections() {
        let collection = stable_collection_id(&prototype, &name)?;
        let started = Instant::now();
        let live = prototype.live_domain_owned(collection)?;
        first_dependency_load.push(json!({
            "collection": collection.0,
            "collection_name": name,
            "load_ns": started.elapsed().as_nanos() as u64,
            "live_slots": live.len(),
        }));
    }
    let mut rows = Vec::new();
    for query in &plan.point {
        let collection = stable_collection_id(&prototype, &query.collection)?;
        let field = stable_field(&query.field);
        rows.push(measure_distribution(
            "point",
            query,
            args.repetitions,
            || match prototype.typed_posting(collection, &field, &query.key)? {
                Restriction::Complete(bitmap) => Ok(bitmap.len() as usize),
                Restriction::NoRestriction => Err("no stable typed restriction".into()),
            },
        )?);
    }
    for query in &plan.range {
        let collection = stable_collection_id(&prototype, &query.collection)?;
        let field = stable_field(&query.field);
        let lower = query
            .lower
            .clone()
            .map_or(std::ops::Bound::Unbounded, std::ops::Bound::Included);
        let upper = query
            .upper
            .clone()
            .map_or(std::ops::Bound::Unbounded, std::ops::Bound::Included);
        rows.push(measure_distribution(
            "range",
            query,
            args.repetitions,
            || match prototype.typed_range_slots(
                collection,
                &field,
                lower.clone(),
                upper.clone(),
                query.descending,
            )? {
                Restriction::Complete(bitmap) => Ok(bitmap.len() as usize),
                Restriction::NoRestriction => Err("no stable range restriction".into()),
            },
        )?);
    }
    for query in &plan.length_point {
        let collection = stable_collection_id(&prototype, &query.collection)?;
        let field = stable_field(&query.field);
        rows.push(measure_distribution(
            "length_point",
            query,
            args.repetitions,
            || match prototype.length_posting(collection, &field, &query.length)? {
                Restriction::Complete(bitmap) => Ok(bitmap.len() as usize),
                Restriction::NoRestriction => Err("no stable length restriction".into()),
            },
        )?);
    }
    for query in &plan.length_range {
        let collection = stable_collection_id(&prototype, &query.collection)?;
        let field = stable_field(&query.field);
        let lower = query
            .lower
            .clone()
            .map_or(std::ops::Bound::Unbounded, std::ops::Bound::Included);
        let upper = query
            .upper
            .clone()
            .map_or(std::ops::Bound::Unbounded, std::ops::Bound::Included);
        rows.push(measure_distribution(
            "length_range",
            query,
            args.repetitions,
            || {
                let mut result = RoaringBitmap::new();
                prototype.visit_length_range(
                    collection,
                    &field,
                    lower.clone(),
                    upper.clone(),
                    |_, posting| {
                        result |= posting;
                        Ok(())
                    },
                )?;
                Ok(result.len() as usize)
            },
        )?);
    }
    for query in &plan.state {
        let collection = stable_collection_id(&prototype, &query.collection)?;
        let field = stable_field(&query.field);
        rows.push(measure_distribution(
            "state",
            query,
            args.repetitions,
            || match prototype.state_posting(collection, &field, query.state)? {
                Restriction::Complete(bitmap) => Ok(bitmap.len() as usize),
                Restriction::NoRestriction => Err("no stable state restriction".into()),
            },
        )?);
    }
    for query in &plan.text {
        let collection = stable_collection_id(&prototype, &query.collection)?;
        let key = stable_text(&query.field);
        rows.push(measure_distribution(
            "text_candidates",
            query,
            args.repetitions,
            || {
                prototype
                    .text_candidates(collection, &key, &query.literal, plan.alphabet)?
                    .map(|bitmap| bitmap.len() as usize)
                    .ok_or_else(|| "no stable text restriction".into())
            },
        )?);
    }
    for query in &plan.sort {
        let collection = stable_collection_id(&prototype, &query.collection)?;
        let key = SortComponentKey(query.sort_key.clone());
        rows.push(measure_distribution(
            "sort",
            query,
            args.repetitions,
            || {
                Ok(prototype
                    .sort_page(collection, &key, query.descending, 0, query.limit)?
                    .len())
            },
        )?);
    }
    for query in &plan.relation_state {
        let collection = stable_collection_id(&prototype, &query.collection)?;
        let key = stable_relation(&query.path, &query.target_collection);
        rows.push(measure_distribution(
            "relation_state",
            query,
            args.repetitions,
            || {
                Ok(prototype
                    .relation_state(collection, &key, query.state)?
                    .len() as usize)
            },
        )?);
    }
    for query in &plan.relation_target {
        let collection = stable_collection_id(&prototype, &query.collection)?;
        let key = stable_relation(&query.path, &query.target_collection);
        rows.push(measure_distribution(
            "relation_target",
            query,
            args.repetitions,
            || {
                Ok(prototype
                    .relation_target_sources(
                        collection,
                        &key,
                        CollectionId(query.target_collection_id),
                        &query.target_id,
                    )?
                    .len() as usize)
            },
        )?);
    }
    let output = json!({
        "implementation": "stable",
        "manifest_open_ns": manifest_open_ns,
        "first_load": first_load,
        "first_dependency_load": first_dependency_load,
        "resident_primitives": rows,
        "repetitions": args.repetitions,
        "plan": queries,
        "counters": prototype.counters(),
        "unavailable": {
            "native_build_ns": "not applicable to an opened stable snapshot",
        },
    });
    write_result(args.result_dir.as_deref(), "primitives.json", &output)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&output).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn stable_collection_id(prototype: &StablePrototype, name: &str) -> Result<CollectionId, String> {
    prototype
        .collections()
        .find(|collection| collection.name == name)
        .map(|collection| collection.id)
        .ok_or_else(|| format!("stable collection missing: {name}"))
}

fn stable_field(field: &PrimitiveField) -> FieldComponentKey {
    FieldComponentKey {
        path: field.path.clone(),
        binding: field.binding.clone(),
        role: field.role,
    }
}

fn stable_text(field: &PrimitiveField) -> TextComponentKey {
    TextComponentKey {
        path: field.path.clone(),
        binding: field.binding.clone(),
        role: field.role,
    }
}

fn stable_relation(
    path: &[String],
    target_collection: &str,
) -> objekte_search_core::stable_index::RelationComponentKey {
    objekte_search_core::stable_index::RelationComponentKey {
        path: path.to_vec(),
        target_collection: target_collection.to_string(),
    }
}

fn primitives_mode(args: &Args) -> Result<(), String> {
    if let Some(queries) = &args.queries {
        return stable_primitives_with_plan(args, queries);
    }
    let input = args
        .snapshot_input
        .as_ref()
        .ok_or("primitives requires --snapshot-input")?;
    let mut prototype = StablePrototype::open_snapshot(input)?;
    let components = if args.components.is_empty() {
        prototype
            .component_descriptors()
            .map(|descriptor| descriptor.id)
            .collect::<Vec<_>>()
    } else {
        args.components.clone()
    };
    let mut first_load = Vec::new();
    let mut first_dependency_load = Vec::new();
    let mut resident = Vec::new();
    for component in &components {
        let started = Instant::now();
        prototype.load_component(*component)?;
        first_load.push(json!({
            "component": component.0,
            "load_ns": started.elapsed().as_nanos() as u64,
        }));
        let descriptor = prototype
            .component_descriptor(*component)
            .ok_or("component descriptor missing")?
            .clone();
        if descriptor.kind == objekte_search_core::stable_index::ComponentKind::Values {
            let collection = descriptor
                .collection
                .as_ref()
                .ok_or("value component has no collection")?
                .id;
            let dependency_started = Instant::now();
            let live = prototype.live_domain_owned(collection)?;
            first_dependency_load.push(json!({
                "component": component.0,
                "collection": collection.0,
                "load_ns": dependency_started.elapsed().as_nanos() as u64,
                "live_slots": live.len(),
            }));
            let keys = value_keys(&mut prototype, *component)?;
            let samples = select_samples(&keys, args.field.as_deref(), args.value.as_deref());
            let field = descriptor
                .field
                .as_ref()
                .ok_or("value component has no field")?;
            let key = FieldComponentKey {
                path: field.path.clone(),
                binding: field.binding.clone(),
                role: field.role,
            };
            for sample in samples {
                let (mut complete, mut members) = match prototype
                    .typed_posting(collection, &key, &sample)
                {
                    Ok(objekte_search_core::stable_index::Restriction::Complete(bitmap)) => {
                        (true, bitmap.len())
                    }
                    Ok(objekte_search_core::stable_index::Restriction::NoRestriction) => (false, 0),
                    Err(error) => return Err(error.to_string()),
                };
                let mut samples_ns = Vec::with_capacity(args.repetitions);
                for _ in 0..args.repetitions {
                    let started = Instant::now();
                    match prototype.typed_posting(collection, &key, &sample) {
                        Ok(objekte_search_core::stable_index::Restriction::Complete(bitmap)) => {
                            complete = true;
                            members = bitmap.len();
                        }
                        Ok(objekte_search_core::stable_index::Restriction::NoRestriction) => {
                            complete = false;
                        }
                        Err(error) => return Err(error.to_string()),
                    }
                    samples_ns.push(started.elapsed().as_nanos() as u64);
                }
                samples_ns.sort_unstable();
                let median = samples_ns[samples_ns.len() / 2];
                let p95 = samples_ns[(samples_ns.len() * 95 / 100).min(samples_ns.len() - 1)];
                resident.push(json!({
                    "component": component.0,
                    "key": format!("{sample:?}"),
                    "repetitions": args.repetitions,
                    "median_ns": median,
                    "p95_ns": p95,
                    "min_ns": samples_ns.first().copied(),
                    "max_ns": samples_ns.last().copied(),
                    "complete": complete,
                    "members": members,
                }));
            }
        }
    }
    let output = json!({
        "first_load": first_load,
        "first_dependency_load": first_dependency_load,
        "resident_primitives": resident,
        "repetitions": args.repetitions,
        "counters": prototype.counters(),
    });
    write_result(args.result_dir.as_deref(), "primitives.json", &output)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&output).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn mutations_mode(args: &Args) -> Result<(), String> {
    if let Some(index_input) = &args.index_input {
        let output = args
            .index_output
            .as_ref()
            .ok_or("mutations with --index-input requires --index-output")?;
        let mut standalone = StablePrototype::open_index(index_input)?;
        let started = Instant::now();
        let key = ComponentKey::Scalar(objekte_search_core::posting::ScalarKey::String(
            "detached-mutation".into(),
        ));
        let large = RoaringBitmap::from_iter(0..65_536);
        standalone.raw_update_posting(&key, &large, &RoaringBitmap::new())?;
        standalone.raw_update_posting(
            &key,
            &RoaringBitmap::from_iter([1, 3, 5]),
            &RoaringBitmap::new(),
        )?;
        let save_started = Instant::now();
        let info = standalone.save_as(output)?;
        let output_json = json!({
            "mutation_ns": started.elapsed().as_nanos() as u64,
            "save_ns": save_started.elapsed().as_nanos() as u64,
            "saved": info,
        });
        write_result(args.result_dir.as_deref(), "mutations.json", &output_json)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&output_json).map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    let input = args
        .snapshot_input
        .as_ref()
        .ok_or("mutations requires --snapshot-input or --index-input")?;
    let mut prototype = StablePrototype::open_snapshot(input)?;
    let component = args
        .components
        .first()
        .copied()
        .ok_or("mutations requires --component")?;
    let started = Instant::now();
    prototype.load_component(component)?;
    let descriptor = prototype
        .component_descriptor(component)
        .ok_or("component descriptor missing")?
        .clone();
    let field = descriptor.field.as_ref().ok_or("component has no field")?;
    let collection = descriptor
        .collection
        .as_ref()
        .ok_or("component has no collection")?
        .id;
    let _ = field;
    let existing = value_keys(&mut prototype, component)?.into_iter().next();
    let mut batches = Vec::new();
    if let Some(existing) = existing {
        for size in [1u32, 100, 1_000] {
            let add = RoaringBitmap::from_iter((0..size).map(|offset| offset + 1_000_000));
            let remove = RoaringBitmap::from_iter((0..size).map(|offset| offset + 2_000_000));
            let batch_started = Instant::now();
            prototype.update_typed_posting(
                collection,
                &field_component_key(&descriptor),
                &existing,
                &add,
                &remove,
            )?;
            batches.push(json!({
                "size": size,
                "elapsed_ns": batch_started.elapsed().as_nanos() as u64,
            }));
        }
    }
    let mut persistence = json!({});
    if let Some(output) = &args.snapshot_output {
        let save_started = Instant::now();
        prototype.save_snapshot(output)?;
        persistence = json!({
            "snapshot_save_ns": save_started.elapsed().as_nanos() as u64,
        });
    }
    let storage = prototype.storage_report(collection);
    let output_json = json!({
        "mutation_ns": started.elapsed().as_nanos() as u64,
        "batches": batches,
        "persistence": persistence,
        "storage": storage,
        "counters": prototype.counters(),
    });
    write_result(args.result_dir.as_deref(), "mutations.json", &output_json)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&output_json).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn lifecycle_mode(args: &Args) -> Result<(), String> {
    let root = create_lifecycle_fixture()?;
    let result = (|| -> Result<(), String> {
        let definitions = fixture_definitions();
        let started = Instant::now();
        let (mut prototype, _) = StablePrototype::from_sources_with_timings(
            &root,
            definitions,
            RepositoryOptions {
                bigram_alphabet: args.alphabet,
                build_workers: args.build_workers,
                ..RepositoryOptions::default()
            },
        )?;
        let build_ns = started.elapsed().as_nanos() as u64;
        let path = std::path::PathBuf::from("data/lifecycle.jsonl");
        let mut storage = Vec::new();
        storage.push(prototype.storage_report(CollectionId(0)));
        let mut replacement_ns = 0u64;
        for _ in 0..10 {
            let refs = prototype.records_for_path(&path)?;
            let mut records = Vec::with_capacity(refs.len());
            let mut file = None;
            for reference in refs {
                if let Some(record) =
                    prototype.record_by_slot(reference.collection, reference.slot)?
                {
                    file = Some(record.file);
                    records.push(record);
                }
            }
            let file_id = file.ok_or("lifecycle fixture has no file")?;
            let entry = prototype
                .file_by_id(file_id)?
                .ok_or("lifecycle file metadata missing")?;
            let replace_started = Instant::now();
            prototype.replace_prepared_file(
                &path,
                objekte_search_core::stable_index::PreparedFileChange {
                    file: entry,
                    collections: vec![
                        objekte_search_core::stable_index::PreparedCollectionChange {
                            collection: CollectionId(0),
                            records,
                            posting_adds: Vec::new(),
                            coverage: Vec::new(),
                        },
                    ],
                },
            )?;
            replacement_ns =
                replacement_ns.saturating_add(replace_started.elapsed().as_nanos() as u64);
            storage.push(prototype.storage_report(CollectionId(0)));
        }
        let generation = root.join("generation");
        let save_started = Instant::now();
        prototype.save_snapshot(&generation)?;
        let save_ns = save_started.elapsed().as_nanos() as u64;
        let disk_bytes = directory_bytes(&generation)?;
        drop(prototype);
        let mut reopened = StablePrototype::open_snapshot(&generation)?;
        let reopened_report = reopened.storage_report(CollectionId(0));
        let output = json!({
            "fixture_build_ns": build_ns,
            "replacements": 10,
            "replacement_ns": replacement_ns,
            "save_ns": save_ns,
            "snapshot_disk_bytes": disk_bytes,
            "storage": storage,
            "reopened": reopened_report,
            "path_refs_after_reopen": reopened.records_for_path(&path)?.len(),
        });
        write_result(args.result_dir.as_deref(), "mutations.json", &output)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&output).map_err(|error| error.to_string())?
        );
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&root);
    result
}

fn create_lifecycle_fixture() -> Result<PathBuf, String> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "objekte-icss-lifecycle-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("data")).map_err(|error| error.to_string())?;
    let path = root.join("data/lifecycle.jsonl");
    let mut bytes = String::new();
    for index in 0..200 {
        bytes.push_str(&format!(
            "{{\"name\":\"name-{index:03}\",\"age\":{},\"tags\":[\"t{}\",\"t{}\"]}}\n",
            index % 10,
            index % 17,
            index % 31
        ));
    }
    std::fs::write(path, bytes).map_err(|error| error.to_string())?;
    Ok(root)
}

fn directory_bytes(path: &std::path::Path) -> Result<u64, String> {
    let mut total = 0u64;
    for entry in std::fs::read_dir(path).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let metadata = entry.metadata().map_err(|error| error.to_string())?;
        if metadata.is_dir() {
            total = total.saturating_add(directory_bytes(&entry.path())?);
        } else {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

fn field_component_key(descriptor: &ComponentDescriptor) -> FieldComponentKey {
    let field = descriptor.field.as_ref().expect("field component");
    FieldComponentKey {
        path: field.path.clone(),
        binding: field.binding.clone(),
        role: field.role,
    }
}

fn value_keys(
    prototype: &mut StablePrototype,
    component: ComponentId,
) -> Result<Vec<objekte_search_core::posting::ScalarKey>, String> {
    let mut keys = Vec::new();
    prototype.for_each_raw_entry(component, |key, _value| {
        if let ComponentKey::Scalar(value) = key {
            keys.push(value.clone());
        }
        Ok(())
    })?;
    Ok(keys)
}

fn select_samples(
    keys: &[objekte_search_core::posting::ScalarKey],
    field: Option<&str>,
    value: Option<&str>,
) -> Vec<objekte_search_core::posting::ScalarKey> {
    if let Some(value) = value {
        if let Ok(number) = value.parse::<i128>() {
            return vec![objekte_search_core::posting::ScalarKey::Number(
                Number::integer(number),
            )];
        }
        if let Ok(number) = value.parse::<f64>() {
            if let Ok(number) = Number::float(number) {
                return vec![objekte_search_core::posting::ScalarKey::Number(number)];
            }
        }
        return vec![objekte_search_core::posting::ScalarKey::String(
            value.into(),
        )];
    }
    let _ = field;
    let mut samples = Vec::new();
    if let Some(first) = keys.first() {
        samples.push(first.clone());
    }
    if let Some(last) = keys.last() {
        if samples.first() != Some(last) {
            samples.push(last.clone());
        }
    }
    if keys.len() > 2 {
        samples.push(keys[keys.len() / 2].clone());
    }
    samples
}

fn create_fixture() -> Result<PathBuf, String> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "objekte-icss-bench-fixture-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("data/nested")).map_err(|error| error.to_string())?;
    std::fs::write(
        root.join("data/a.jsonl"),
        b"{\"name\":\"alpha\",\"age\":1,\"tags\":[\"x\",\"y\"]}\n{\"name\":\"beta\",\"age\":2,\"tags\":[\"x\"]}\n",
    )
    .map_err(|error| error.to_string())?;
    std::fs::write(
        root.join("data/nested/c.jsonl"),
        b"{\"name\":\"alpha\",\"age\":1,\"tags\":[]}\n{\"name\":\"gamma\",\"age\":10,\"tags\":[\"z\"]}\n",
    )
    .map_err(|error| error.to_string())?;
    Ok(root)
}

fn fixture_definitions() -> Vec<CollectionDefinition> {
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    for (path, kind, multi) in [
        (vec!["name".to_string()], LogicalType::String, false),
        (vec!["age".to_string()], LogicalType::Number, false),
        (
            vec!["tags".to_string()],
            LogicalType::Array(Box::new(LogicalType::String)),
            true,
        ),
    ] {
        definition
            .schema
            .register_field(FieldDefinition {
                path,
                kind,
                required: false,
                nullable: true,
                indexed: true,
                multi,
                element_nullable: Vec::new(),
            })
            .expect("fixture field");
    }
    definition.sort_fields =
        vec![objekte_search_core::model::FieldPath::parse("age").expect("sort")];
    vec![definition]
}

fn trace_metrics(
    records: &[objekte_search_core::stable_index::ComponentBuildRecord],
    workers: usize,
) -> serde_json::Value {
    if records.is_empty() {
        return json!({"available": false, "reason": "no component records"});
    }
    let start = records
        .iter()
        .map(|record| record.start_offset_ns)
        .min()
        .unwrap_or(0);
    let end = records
        .iter()
        .map(|record| record.end_offset_ns)
        .max()
        .unwrap_or(0);
    let makespan_ns = end.saturating_sub(start);
    let sum_job_ns = records.iter().map(|record| record.duration_ns).sum::<u64>();
    let largest = records
        .iter()
        .max_by_key(|record| record.duration_ns)
        .map(|record| {
            json!({
                "id": record.id.0,
                "kind": record.kind,
                "field": record.field,
                "duration_ns": record.duration_ns,
                "entry_count": record.entry_count,
                "estimated_work": record.estimated_work,
            })
        });
    let mut last_by_worker = std::collections::BTreeMap::<String, u64>::new();
    for record in records {
        last_by_worker
            .entry(record.worker.clone())
            .and_modify(|value| *value = (*value).max(record.end_offset_ns))
            .or_insert(record.end_offset_ns);
    }
    let mut events = Vec::with_capacity(records.len() * 2);
    for record in records {
        events.push((record.start_offset_ns, 1i64));
        events.push((record.end_offset_ns, -1i64));
    }
    events.sort_unstable();
    let mut active = 0i64;
    let mut started = 0usize;
    let mut previous = start;
    let mut idle_capacity_ns = 0u64;
    let mut active_sum_ns = 0u64;
    let mut index = 0usize;
    while index < events.len() {
        let instant = events[index].0;
        let elapsed = instant.saturating_sub(previous);
        if active < workers as i64 && started < records.len() {
            idle_capacity_ns = idle_capacity_ns
                .saturating_add(elapsed.saturating_mul((workers as i64 - active).max(0) as u64));
        }
        active_sum_ns = active_sum_ns.saturating_add(elapsed.saturating_mul(active.max(0) as u64));
        while index < events.len() && events[index].0 == instant {
            if events[index].1 > 0 {
                active += 1;
                started += 1;
            } else {
                active -= 1;
            }
            index += 1;
        }
        previous = instant;
    }
    json!({
        "available": true,
        "workers": workers,
        "makespan_ns": makespan_ns,
        "sum_job_duration_ns": sum_job_ns,
        "average_concurrent_jobs": if makespan_ns == 0 { 0.0 } else { active_sum_ns as f64 / makespan_ns as f64 },
        "idle_capacity_ns": idle_capacity_ns,
        "largest_job": largest,
        "last_completion_by_worker": last_by_worker,
        "components": records.len(),
    })
}

fn field_overlap_pairs(records: &[objekte_search_core::stable_index::ComponentBuildRecord]) -> u64 {
    let mut field_jobs = records
        .iter()
        .filter(|record| {
            matches!(
                record.kind,
                objekte_search_core::stable_index::ComponentKind::Values
                    | objekte_search_core::stable_index::ComponentKind::Lengths
                    | objekte_search_core::stable_index::ComponentKind::States
                    | objekte_search_core::stable_index::ComponentKind::NormalGrams
                    | objekte_search_core::stable_index::ComponentKind::SkipGrams
            )
        })
        .collect::<Vec<_>>();
    field_jobs.sort_by_key(|record| record.start_offset_ns);
    let mut pairs = 0u64;
    for (index, left) in field_jobs.iter().enumerate() {
        for right in &field_jobs[index + 1..] {
            if right.start_offset_ns >= left.end_offset_ns {
                break;
            }
            if right.worker != left.worker {
                pairs += 1;
            }
        }
    }
    pairs
}

fn lockfile_digest() -> Option<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock");
    digest_file(&path).ok()
}

fn executable_digest() -> Option<String> {
    std::env::current_exe()
        .ok()
        .and_then(|path| digest_file(&path).ok())
}

fn write_result(
    directory: Option<&Path>,
    name: &str,
    value: &impl Serialize,
) -> Result<(), String> {
    let Some(directory) = directory else {
        return Ok(());
    };
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    std::fs::write(directory.join(name), bytes).map_err(|error| error.to_string())
}

fn resident_bytes() -> Option<u64> {
    memory_status("VmRSS:")
}

fn high_water_bytes() -> Option<u64> {
    memory_status("VmHWM:")
}

fn memory_status(name: &str) -> Option<u64> {
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

fn digest_file(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[allow(dead_code)]
fn sorted_counts(values: impl IntoIterator<Item = String>) -> BTreeMap<String, usize> {
    let mut result = BTreeMap::new();
    for value in values {
        *result.entry(value).or_default() += 1;
    }
    result
}

#[allow(dead_code)]
fn slot(slot: u32) -> RecordSlot {
    RecordSlot(slot)
}

#[allow(dead_code)]
fn text_key() -> TextComponentKey {
    TextComponentKey {
        path: Vec::new(),
        binding: None,
        role: objekte_search_core::stable_index::BindingRole::RawText,
    }
}
