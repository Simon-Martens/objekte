//! Deterministic synthetic fixture benchmark shared by native and stable runs.
//!
//! The same source file is compiled in the pinned baseline worktree with the
//! stable feature unavailable; `--implementation native` uses only base core
//! APIs there.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::model::{FieldPath, LogicalType};
use objekte_search_core::reference::FieldDefinition;
use objekte_search_core::repository::{CollectionDefinition, Repository, RepositoryOptions};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::BigramAlphabet;
use serde::Serialize;
use serde_json::json;

fn main() {
    if let Err(error) = run() {
        eprintln!("stable-index fixture benchmark error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut root = None;
    let mut workers = None;
    let mut alphabet = BigramAlphabet::Ascii;
    let mut implementation = "native".to_string();
    let mut result_dir = None;
    #[allow(unused_variables, unused_assignments)]
    let mut validate = false;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" => {
                root = Some(PathBuf::from(
                    arguments.next().ok_or("--root needs a path")?,
                ))
            }
            "--build-workers" => {
                let value = arguments.next().ok_or("--build-workers needs a value")?;
                workers = if value == "auto" {
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
            "--alphabet" => {
                alphabet = match arguments.next().ok_or("--alphabet needs a value")?.as_str() {
                    "ascii" => BigramAlphabet::Ascii,
                    "unicode" => BigramAlphabet::Unicode,
                    other => return Err(format!("unknown alphabet {other}")),
                }
            }
            "--implementation" => {
                implementation = arguments
                    .next()
                    .ok_or("--implementation needs native or stable")?;
                if implementation != "native" && implementation != "stable" {
                    return Err(format!("unknown implementation {implementation}"));
                }
            }
            "--result-dir" => {
                result_dir = Some(PathBuf::from(
                    arguments.next().ok_or("--result-dir needs a path")?,
                ));
            }
            "--validate" => validate = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let root = root.ok_or("--root is required")?;
    let _ = validate;
    let definitions = fixture_definitions();
    let options = RepositoryOptions {
        bigram_alphabet: alphabet,
        build_workers: workers,
        ..RepositoryOptions::default()
    };
    if implementation == "native" {
        let started = Instant::now();
        let repository = Repository::open_with_options(&root, definitions, options)?;
        let total_ns = started.elapsed().as_nanos() as u64;
        let output = json!({
            "implementation": "native",
            "construction_mode": "repository_build",
            "build_workers": workers.map(NonZeroUsize::get),
            "available_parallelism": std::thread::available_parallelism().map_or(1, NonZeroUsize::get),
            "total_source_build_ns": total_ns,
            "source_files": repository.files().files.len(),
            "source_records": repository.collections().iter().map(|collection| collection.generation.records.len()).sum::<usize>(),
            "source_bytes": repository.files().files.iter().map(|file| file.revision.size).sum::<u64>(),
            "resident_bytes": resident_bytes(),
            "peak_resident_bytes": high_water_bytes(),
            "posting_fields": repository.collections().iter().map(|collection| collection.postings.fields.len()).sum::<usize>(),
            "custom_postings": repository.collections().iter().map(|collection| collection.custom_postings.len()).sum::<usize>(),
            "bigram_columns": repository.collections().iter().map(|collection| 1 + 1 + collection.custom_text.len()).sum::<usize>(),
            "sort_columns": repository.collections().iter().map(|collection| collection.sorting.columns.len()).sum::<usize>(),
        });
        write_result(result_dir.as_deref(), "build.json", &output)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&output).map_err(|error| error.to_string())?
        );
        return Ok(());
    }

    #[cfg(feature = "ic-stable-prototype")]
    {
        use objekte_search_core::stable_index::StablePrototype;
        let started = Instant::now();
        let (mut prototype, timings) =
            StablePrototype::from_sources_with_timings(&root, definitions.clone(), options)?;
        let total_ns = started.elapsed().as_nanos() as u64;
        let readiness_rss = resident_bytes();
        let readiness_hwm = high_water_bytes();
        let build_records = prototype.build_records().to_vec();
        let overlap = field_overlap_pairs(&build_records);
        let payload_bytes = build_records
            .iter()
            .map(|record| record.payload_len)
            .sum::<u64>();
        let mut validation = None;
        if validate {
            let repository = Repository::open_with_options(&root, definitions, options)?;
            let report = prototype.validate_against_repository(&repository)?;
            validation = Some(report);
        }
        let output = json!({
            "implementation": "stable",
            "construction_mode": "direct_batch_contributions",
            "build_workers": workers.map(NonZeroUsize::get),
            "available_parallelism": std::thread::available_parallelism().map_or(1, NonZeroUsize::get),
            "total_source_build_ns": total_ns,
            "source_files": timings.source_files,
            "source_records": timings.source_records,
            "source_bytes": timings.source_bytes,
            "source_reads": timings.source_reads,
            "source_bytes_read": timings.source_bytes_read,
            "source_parses": timings.source_parses,
            "failed_parses": timings.failed_parses,
            "peak_active_jobs": timings.peak_active_jobs,
            "stage_ns": {
                "discovery": timings.discovery_ns,
                "setup": timings.setup_ns,
                "extraction_admission": timings.extraction_admission_ns,
                "finalization": timings.finalization_ns,
                "stable_component": timings.stable_component_ns,
            },
            "resident_bytes": readiness_rss,
            "peak_resident_bytes": readiness_hwm,
            "owned_component_payload_bytes": payload_bytes,
            "components": prototype.component_descriptors().count(),
            "component_build_records": build_records,
            "field_job_overlap_pairs": overlap,
            "field_job_overlap_demonstrated": overlap > 0,
            "counters": prototype.counters(),
            "validation": validation,
        });
        write_result(result_dir.as_deref(), "build.json", &output)?;
        if let Some(report) = &validation {
            write_result(result_dir.as_deref(), "validation.json", report)?;
            if !report.is_valid() {
                return Err(format!("validation failed: {:?}", report.errors));
            }
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&output).map_err(|error| error.to_string())?
        );
        return Ok(());
    }

    #[cfg(not(feature = "ic-stable-prototype"))]
    Err("stable implementation requires the ic-stable-prototype feature".into())
}

fn fixture_definitions() -> Vec<CollectionDefinition> {
    let mut definition = CollectionDefinition::folder("synthetic", "data", SourceFormat::JsonLines);
    for (path, kind, multi, indexed) in [
        (vec!["id".to_string()], LogicalType::Id, false, true),
        (
            vec!["high_card".to_string()],
            LogicalType::String,
            false,
            true,
        ),
        (vec!["bucket".to_string()], LogicalType::Number, false, true),
        (vec!["flag".to_string()], LogicalType::Bool, false, true),
        (vec!["text".to_string()], LogicalType::String, false, true),
        (
            vec!["tags".to_string()],
            LogicalType::Array(Box::new(LogicalType::String)),
            true,
            true,
        ),
        (
            vec!["nested".to_string(), "group".to_string()],
            LogicalType::String,
            false,
            true,
        ),
        (
            vec!["nested".to_string(), "value".to_string()],
            LogicalType::Number,
            false,
            true,
        ),
        (
            vec!["unindexed_note".to_string()],
            LogicalType::String,
            false,
            false,
        ),
    ] {
        definition
            .schema
            .register_field(FieldDefinition {
                path,
                kind,
                required: false,
                nullable: true,
                indexed,
                multi,
                element_nullable: Vec::new(),
            })
            .expect("fixture field");
    }
    definition.sort_fields = vec![
        FieldPath::parse("high_card").expect("sort high_card"),
        FieldPath::parse("bucket").expect("sort bucket"),
    ];
    vec![definition]
}

#[cfg(feature = "ic-stable-prototype")]
fn field_overlap_pairs(records: &[objekte_search_core::stable_index::ComponentBuildRecord]) -> u64 {
    let mut jobs = records
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
    jobs.sort_by_key(|record| record.start_offset_ns);
    let mut pairs = 0u64;
    for (index, left) in jobs.iter().enumerate() {
        for right in &jobs[index + 1..] {
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

fn write_result(
    directory: Option<&std::path::Path>,
    name: &str,
    value: &impl Serialize,
) -> Result<(), String> {
    let Some(directory) = directory else {
        return Ok(());
    };
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
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
