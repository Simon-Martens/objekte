//! Fresh-process, build-only construction measurement. The measured interval
//! excludes query execution, snapshot writing, and benchmark result output.

#[path = "support/musenalm.rs"]
mod musenalm;

use std::collections::BTreeMap;
use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use objekte_search_core::inference::InferenceMode;
use objekte_search_core::repository::{
    CollectionDefinition, Repository, RepositoryOptions, SnapshotMode,
};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::BigramAlphabet;
use sha2::{Digest, Sha256};

fn main() {
    if let Err(error) = run() {
        eprintln!("construction benchmark: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let arguments = std::env::args().collect::<Vec<_>>();
    let mut args = arguments.iter().skip(1);
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let mut result_dir = None;
    let mut alphabet = BigramAlphabet::Ascii;
    let mut workers = None;
    let mut plain = false;
    let mut jsonl = false;
    let mut inference = false;
    let mut snapshot = false;
    let mut profile = false;
    let mut check_queries = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = PathBuf::from(args.next().ok_or("--root needs a path")?),
            "--result-dir" => {
                result_dir = Some(PathBuf::from(
                    args.next().ok_or("--result-dir needs a path")?,
                ))
            }
            "--alphabet" => {
                alphabet = match args.next().map(String::as_str) {
                    Some("ascii") => BigramAlphabet::Ascii,
                    Some("unicode") => BigramAlphabet::Unicode,
                    _ => return Err("--alphabet needs ascii or unicode".into()),
                }
            }
            "--workers" => {
                workers = Some(
                    NonZeroUsize::new(
                        args.next()
                            .ok_or("--workers needs a value")?
                            .parse::<usize>()
                            .map_err(|e| e.to_string())?,
                    )
                    .ok_or("workers must be positive")?,
                )
            }
            "--plain" => plain = true,
            "--jsonl" => jsonl = true,
            "--inference" => inference = true,
            "--snapshot" => snapshot = true,
            "--profile" => profile = true,
            "--check-queries" => check_queries = true,
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    if plain && jsonl {
        return Err("--plain and --jsonl select different fixtures".into());
    }
    if check_queries && (plain || jsonl) {
        return Err("--check-queries requires Musenalm schema mode".into());
    }
    let result_root = result_dir.ok_or("--result-dir is required")?;
    fs::create_dir_all(&result_root).map_err(|e| format!("{}: {e}", result_root.display()))?;
    let run_id = format!(
        "trial-{}-{}",
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos(),
        std::process::id(),
    );
    let result_dir = result_root.join(run_id);
    fs::create_dir(&result_dir).map_err(|e| format!("{}: {e}", result_dir.display()))?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let definitions = if plain || jsonl {
        vec![CollectionDefinition::folder(
            if jsonl { "jsonl" } else { "plain" },
            "data",
            if jsonl {
                SourceFormat::JsonLines
            } else {
                SourceFormat::Json
            },
        )]
    } else {
        musenalm::load_extended_definitions_with_schemas(&root)?.0
    };
    let mut options = RepositoryOptions {
        bigram_alphabet: alphabet,
        snapshot_mode: SnapshotMode::Disabled,
        build_workers: workers,
        profile_build: profile,
        ..RepositoryOptions::default()
    };
    if inference {
        options.inference.mode = InferenceMode::AutoIndex;
    }
    let before_rss = resident_bytes();
    let started = Instant::now();
    let repo = Repository::open_with_options(&root, definitions, options)?;
    let open_ns = started.elapsed().as_nanos() as u64;
    let after_build_rss = resident_bytes();
    let after_build_hwm = peak_resident_bytes();
    let startup = repo.startup().clone();
    let profile_reconciliation = startup.build_profile.as_ref().map(|profile| -> Result<_, String> {
        let top_level = startup.build_discovery_ns
            + startup.build_extraction_index_ns
            + startup.build_finalization_ns
            + profile.pool_init_ns
            + profile.relations_ns
            + profile.final_checks_ns
            + profile.pool_teardown_ns
            + profile.query_pool_init_ns;
        let extraction = profile.builder_setup_ns + profile.source_work_ns + profile.fingerprint_finish_ns;
        let finalization = profile.finalize_generation_ns + profile.finalize_bigrams_ns
            + profile.finalize_validation_ns + profile.finalize_sort_ns
            + profile.finalize_inference_ns + profile.finalize_identity_ns;
        let remainder = serde_json::json!({
            "top_level_unattributed_ns": startup.build_ns.checked_sub(top_level).ok_or("top-level profile intervals overlap")?,
            "extraction_unattributed_ns": startup.build_extraction_index_ns.checked_sub(extraction).ok_or("extraction profile intervals overlap")?,
            "finalization_unattributed_ns": startup.build_finalization_ns.checked_sub(finalization).ok_or("finalization profile intervals overlap")?,
        });
        if profile.work.source_extraction_ns < profile.work.sampled_callback_ns {
            return Err("sampled callback exceeds sampled source extraction".into());
        }
        Ok(remainder)
    }).transpose()?;
    let corpus_fingerprint = fingerprint_inventory(&repo);
    let files = repo.files().files.len();
    let records = repo
        .collections()
        .iter()
        .map(|c| c.generation.records.len())
        .sum::<usize>();
    let build_parses = repo.build_parses();
    let failed_parses = repo.build_failed_parses();
    let postings = repo
        .collections()
        .iter()
        .map(|c| c.postings.fields.len())
        .sum::<usize>();
    let sort_fields = repo
        .collections()
        .iter()
        .map(|c| c.sorting.columns.len())
        .sum::<usize>();
    let relation_edges = repo
        .collections()
        .iter()
        .flat_map(|c| &c.relations)
        .map(|r| r.edge_count())
        .sum::<usize>();
    let relation_heap_bytes = repo
        .collections()
        .iter()
        .flat_map(|c| &c.relations)
        .map(|r| r.estimated_heap_bytes())
        .sum::<usize>();
    let estimated_index_bytes = repo
        .collections()
        .iter()
        .map(|c| {
            c.raw.estimated_heap_bytes()
                + c.fallback_text.estimated_heap_bytes()
                + c.custom_text
                    .values()
                    .map(|i| i.estimated_heap_bytes())
                    .sum::<usize>()
                + c.postings
                    .fields
                    .values()
                    .map(|p| {
                        p.values
                            .values()
                            .map(|bitmap| bitmap.serialized_size() as usize)
                            .sum::<usize>()
                            + p.lengths
                                .values()
                                .map(|bitmap| bitmap.serialized_size() as usize)
                                .sum::<usize>()
                    })
                    .sum::<usize>()
                + c.sorting
                    .columns
                    .values()
                    .map(|column| {
                        column
                            .keys
                            .iter()
                            .map(|key| match key {
                                objekte_search_core::sorting::SortKey::String(value) => value.len(),
                                _ => 24,
                            })
                            .sum::<usize>()
                    })
                    .sum::<usize>()
        })
        .sum::<usize>();
    let (bigram_stores, distinct_gram_keys) =
        repo.collections()
            .iter()
            .fold((0usize, 0usize), |total, collection| {
                std::iter::once(&collection.raw)
                    .chain(std::iter::once(&collection.fallback_text))
                    .chain(collection.custom_text.values())
                    .fold(total, |(stores, keys), index| {
                        (stores + 1, keys + index.normal.len() + index.skip.len())
                    })
            });
    let (inventory_heap_bytes, diagnostic_heap_bytes) = repo.files().estimated_heap_bytes();
    let source_bytes = repo
        .files()
        .files
        .iter()
        .map(|f| f.revision.size)
        .sum::<u64>();
    let snapshot_result = if snapshot {
        let path = result_dir.join("index.snapshot");
        let stats = repo.save_snapshot_with_stats(&path)?;
        let bytes = fs::metadata(&path).map_err(|e| e.to_string())?.len();
        let sections = stats.sections.iter().map(|section| serde_json::json!({
            "id":section.id,"serialize_ns":section.serialize_ns,"checksum_ns":section.checksum_ns,
            "write_ns":section.write_ns,"payload_bytes":section.payload_bytes,
        })).collect::<Vec<_>>();
        Some(
            serde_json::json!({"total_ns":stats.total_ns,"directory_ns":stats.directory_ns,"header_ns":stats.header_ns,"inventory_ns":stats.inventory_ns,"write_ns":stats.write_ns,"sync_ns":stats.sync_ns,"rename_ns":stats.rename_ns,"directory_sync_ns":stats.directory_sync_ns,"bytes":bytes,"sections":sections,"rss_after_write":resident_bytes(),"hwm_after_write":peak_resident_bytes()}),
        )
    } else {
        None
    };
    let query_digests = if check_queries {
        let core = Path::new(env!("CARGO_MANIFEST_DIR"));
        let cases = objekte_search_core::workload::load_extended_manifest(
            &core.join("benches/musenalm_queries.json"),
            &core.join("benches/musenalm_queries_extended.json"),
        )?;
        let mut digests = BTreeMap::new();
        for case in cases
            .iter()
            .filter(|case| ["Q001", "Q108", "Q116", "Q120", "Q194"].contains(&case.id.as_str()))
        {
            let result = repo.search(&case.query()?)?;
            let digest = Sha256::digest(format!("{:?}", (result.count, result.hits)).as_bytes());
            digests.insert(case.id.clone(), format!("{digest:x}"));
        }
        Some(digests)
    } else {
        None
    };
    drop(repo);
    let after_release_rss = resident_bytes();
    let rev = command_output("git", &["rev-parse", "HEAD"])?;
    let status = command_output("git", &["status", "--porcelain"])?;
    let schema_fingerprint = if plain || jsonl {
        None
    } else {
        Some(hash_schema_files(&root)?)
    };
    let manifest = serde_json::json!({
        "code_revision":rev,"dirty_status":status,"command":arguments,"root":root,
        "compiler":command_output("rustc", &["--version"])?,
        "kernel":command_output("uname", &["-r"])?,
        "cpu_model":proc_field("/proc/cpuinfo", "model name"),
        "cpu_governor":fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor").ok().map(|value| value.trim().to_string()),
        "cpu_affinity":proc_field("/proc/self/status", "Cpus_allowed_list"),
        "alphabet":alphabet,"requested_workers":workers.map(NonZeroUsize::get),
        "resolved_workers":startup.build_workers,"inference":inference,
        "configuration":if jsonl { "jsonl_fixture" } else if plain { "plain_unconfigured" } else { "musenalm_extended" },
        "snapshot":snapshot,"corpus_fingerprint":corpus_fingerprint,"schema_fingerprint":schema_fingerprint,
        "profile":profile,
        "check_queries":check_queries,
        "query_workers":options.query_workers,"prepared_cache_bytes":options.prepared_cache_bytes,
        "verified_cache_bytes":options.verified_cache_bytes,
        "source_bytes":source_bytes,"records":records,"files":files,
    });
    let sample = serde_json::json!({
        "open_ns":open_ns,"build_ns":startup.build_ns,
        "discovery_ns":startup.build_discovery_ns,
        "extraction_index_ns":startup.build_extraction_index_ns,
        "finalization_ns":startup.build_finalization_ns,
        "build_job_execution_ns":startup.build_job_execution_ns,
        "build_receive_wait_ns":startup.build_receive_wait_ns,
        "build_admission_ns":startup.build_admission_ns,
        "out_of_order_completed":startup.build_out_of_order_completed,
        "ready_wait_sum_ns":startup.build_ready_wait_sum_ns,
        "ready_wait_max_ns":startup.build_ready_wait_max_ns,
        "framing_ns":startup.build_framing_ns,
        "framing_chunks":startup.build_framing_chunks,
        "framing_bytes":startup.build_framing_bytes,
        "peak_active_jobs":startup.build_peak_active_jobs,
        "peak_ready_batches":startup.build_peak_ready_batches,
        "peak_admitted_bytes":startup.peak_build_admitted_bytes,
        "weight_limit_bytes":startup.build_weight_limit_bytes,
        "inference_build":format!("{:?}",startup.inference_build),
        "profile":startup.build_profile,
        "profile_reconciliation":profile_reconciliation,
        "before_rss":before_rss,"after_build_rss":after_build_rss,"after_build_hwm":after_build_hwm,
        "after_release_rss":after_release_rss,"build_parses":build_parses,"failed_parses":failed_parses,"postings":postings,
        "sort_fields":sort_fields,"relation_edges":relation_edges,"relation_heap_bytes":relation_heap_bytes,
        "estimated_index_bytes":estimated_index_bytes,"bigram_stores":bigram_stores,
        "distinct_gram_keys":distinct_gram_keys,
        "inventory_heap_bytes":inventory_heap_bytes,"diagnostic_heap_bytes":diagnostic_heap_bytes,
        "snapshot_result":snapshot_result,
        "query_digests":query_digests,
    });
    write_json(&result_dir.join("run-manifest.json"), &manifest)?;
    write_json(&result_dir.join("sample.json"), &sample)?;
    println!(
        "{}",
        serde_json::json!({"build_ns":startup.build_ns,"result_dir":result_dir})
    );
    Ok(())
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    fs::write(
        path,
        serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

fn command_output(command: &str, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new(command)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!("{command} failed"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}

fn fingerprint_inventory(repo: &Repository) -> String {
    let mut hash = Sha256::new();
    for file in &repo.files().files {
        hash.update(file.relative_path.to_string_lossy().as_bytes());
        hash.update(file.revision.modified_ns.to_le_bytes());
        hash.update(file.revision.size.to_le_bytes());
    }
    format!("{:x}", hash.finalize())
}

fn hash_schema_files(root: &Path) -> Result<String, String> {
    let mut files = fs::read_dir(root.join(".objekte/collections"))
        .map_err(|e| e.to_string())?
        .map(|entry| entry.map(|entry| entry.path()).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    files.sort();
    let mut hash = Sha256::new();
    for file in files {
        hash.update(
            file.file_name()
                .ok_or("schema name missing")?
                .to_string_lossy()
                .as_bytes(),
        );
        hash.update(fs::read(file).map_err(|e| e.to_string())?);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn proc_field(path: &str, field: &str) -> Option<String> {
    fs::read_to_string(path).ok().and_then(|text| {
        text.lines().find_map(|line| {
            line.split_once(':')
                .and_then(|(name, value)| (name.trim() == field).then(|| value.trim().to_string()))
        })
    })
}

fn resident_bytes() -> u64 {
    status_bytes("VmRSS:")
}

fn peak_resident_bytes() -> u64 {
    status_bytes("VmHWM:")
}

fn status_bytes(field: &str) -> u64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with(field))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|n| n.parse::<u64>().ok())
        })
        .unwrap_or(0)
        * 1024
}
