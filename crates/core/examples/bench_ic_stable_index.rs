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
use objekte_search_core::reference::FieldDefinition;
use objekte_search_core::repository::{CollectionDefinition, Repository, RepositoryOptions};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::stable_index::{
    ComponentDescriptor, ComponentId, ComponentKey, FieldComponentKey, StablePrototype,
    TextComponentKey,
};
use objekte_search_core::workload::BigramAlphabet;
use roaring::RoaringBitmap;
use serde::Serialize;
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
    field: Option<String>,
    value: Option<String>,
    construction: String,
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
            field: None,
            value: None,
            construction: "direct".into(),
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
            "--field" => args.field = Some(arguments.next().ok_or("--field needs a value")?),
            "--value" => args.value = Some(arguments.next().ok_or("--value needs a value")?),
            "--construction" => {
                let value = arguments.next().ok_or("--construction needs a value")?;
                if value != "direct" && value != "transfer" {
                    return Err(format!("unknown construction mode {value}"));
                }
                args.construction = value;
            }
            "--inference" => {
                return Err("inference is explicitly unsupported by this prototype".into());
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(args)
}

fn source_build(args: &Args, force_snapshot: bool) -> Result<(), String> {
    if args.construction == "direct" {
        return direct_source_build(args, force_snapshot);
    }
    let root = args.root.clone().ok_or("--root is required")?;
    let (definitions, schemas) = musenalm::load_definitions_with_schemas(&root)?;
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
    let (definitions, schemas) = musenalm::load_definitions_with_schemas(&root)?;
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
        "job_execution_ns": timings.job_execution_ns,
        "receive_wait_ns": timings.receive_wait_ns,
        "admission_ns": timings.admission_ns,
        "source_build_timings": {
            "discovery_ns": timings.discovery_ns,
            "setup_ns": timings.setup_ns,
            "extraction_admission_ns": timings.extraction_admission_ns,
            "finalization_ns": timings.finalization_ns,
            "stable_component_ns": timings.stable_component_ns,
        },
        "schema_sha256": schema_digest,
        "counters": prototype.counters(),
        "components": prototype.component_descriptors().count(),
    });
    let components_json = json!({
        "components": descriptors,
        "records": build_records,
        "overlap_pairs": overlap,
        "field_job_overlap_demonstrated": overlap > 0,
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
    let (definitions, schemas) = musenalm::load_definitions_with_schemas(&root)?;
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

fn primitives_mode(args: &Args) -> Result<(), String> {
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
            let keys = value_keys(&mut prototype, *component)?;
            let samples = select_samples(&keys, args.field.as_deref(), args.value.as_deref());
            let field = descriptor
                .field
                .as_ref()
                .ok_or("value component has no field")?;
            let collection = descriptor
                .collection
                .as_ref()
                .ok_or("value component has no collection")?
                .id;
            let key = FieldComponentKey {
                path: field.path.clone(),
                binding: field.binding.clone(),
                role: field.role,
            };
            for sample in samples {
                let started = Instant::now();
                let posting = prototype.typed_posting(collection, &key, &sample);
                let elapsed = started.elapsed().as_nanos() as u64;
                resident.push(json!({
                    "component": component.0,
                    "key": format!("{sample:?}"),
                    "elapsed_ns": elapsed,
                    "members": posting.map(|bitmap| bitmap.len()).unwrap_or(0),
                }));
            }
        }
    }
    let output = json!({
        "first_load": first_load,
        "resident_primitives": resident,
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
            prototype.replace_file(&path, entry, CollectionId(0), records)?;
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
