#[path = "support/musenalm.rs"]
mod musenalm;

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::Instant;

use objekte_search_core::configuration::configuration_hash;
use objekte_search_core::inference::InferenceConfig;
use objekte_search_core::repository::{Repository, RepositoryOptions};
use objekte_search_core::rocksdb_store::build::{
    DirectBuildOptions, DirectBuildReport, build_from_source,
};
use objekte_search_core::rocksdb_store::reference::{
    ReferenceDigests, compare_reference, native_reference, store_reference,
};
use objekte_search_core::rocksdb_store::validation::{
    ValidationReport, validate as validate_store,
};
use objekte_search_core::rocksdb_store::{StoreConfig, StoreLocation};
use objekte_search_core::source::SourceRegistry;
use objekte_search_core::workload::BigramAlphabet;
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    NativeBuild,
    SourceBuild,
    ReferenceExport,
    Validate,
    Primitives,
    Mutations,
    Reopen,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Storage {
    Memory,
    File,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SchemaProfile {
    Basic,
    Extended,
}

#[derive(Debug)]
struct Args {
    mode: Mode,
    root: PathBuf,
    alphabet: BigramAlphabet,
    schema_profile: SchemaProfile,
    storage: Storage,
    db_path: Option<PathBuf>,
    prepare_workers: Option<usize>,
    write_workers: Option<usize>,
    reference: Option<PathBuf>,
    result_dir: PathBuf,
    profile: bool,
    pipelined_writes: bool,
    raw_arguments: Vec<String>,
}

#[derive(Serialize)]
struct Environment {
    available_parallelism: usize,
    executable_sha256: String,
    rust_version: &'static str,
    os: &'static str,
    args: Vec<String>,
}

#[derive(Serialize)]
struct BuildOutput<'a> {
    mode: &'static str,
    schema_profile: &'static str,
    alphabet: &'static str,
    storage: &'static str,
    root: String,
    configuration_digest: String,
    build_identity: String,
    schema_digest: Option<String>,
    elapsed_ns: u64,
    resident_bytes: Option<u64>,
    peak_resident_bytes: Option<u64>,
    user_cpu_ticks: Option<u64>,
    system_cpu_ticks: Option<u64>,
    memory_host_files: Option<Vec<String>>,
    report: Option<&'a DirectBuildReport>,
    validation: Option<&'a ValidationReport>,
    native: Option<NativeOutput>,
    environment: Environment,
}

#[derive(Serialize)]
struct NativeOutput {
    build_ns: u64,
    discovery_ns: u64,
    extraction_index_ns: u64,
    finalization_ns: u64,
    records: usize,
    requested_build_workers: Option<usize>,
    resolved_build_workers: usize,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("rocksdb prototype benchmark error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    std::fs::create_dir_all(&args.result_dir).map_err(|error| error.to_string())?;
    if std::fs::read_dir(&args.result_dir)
        .map_err(|error| error.to_string())?
        .next()
        .is_some()
    {
        return Err(format!(
            "result directory is not empty: {}",
            args.result_dir.display()
        ));
    }
    let root = args
        .root
        .canonicalize()
        .map_err(|error| format!("{}: {error}", args.root.display()))?;
    let (definitions, schemas) = match args.schema_profile {
        SchemaProfile::Basic => musenalm::load_definitions_with_schemas(&root)?,
        SchemaProfile::Extended => musenalm::load_extended_definitions_with_schemas(&root)?,
    };
    let schema_manifest = musenalm::schema_manifest(&schemas)?;
    let schema_digest = schema_manifest["sha256"].as_str().map(str::to_owned);
    let sources = SourceRegistry::builtin();
    let native_options = RepositoryOptions {
        bigram_alphabet: args.alphabet,
        build_workers: args
            .prepare_workers
            .map(|workers| {
                NonZeroUsize::new(workers)
                    .ok_or_else(|| "--prepare-workers must be positive".to_string())
            })
            .transpose()?,
        inference: InferenceConfig::default(),
        profile_build: args.profile,
        ..RepositoryOptions::default()
    };
    let configuration_digest = configuration_hash(&definitions, native_options, &sources)?;
    let mut build_identity = Sha256::new();
    build_identity.update(b"OBJEKTE_ROCKSDB_PROTOTYPE_BUILD");
    build_identity.update(configuration_digest);
    build_identity.update(objekte_search_core::path_repr::native_bytes(&root));
    let build_identity: [u8; 32] = build_identity.finalize().into();
    let cpu_start = process_cpu_ticks();

    match args.mode {
        Mode::NativeBuild => {
            let build_started = Instant::now();
            let repository = Repository::open_with_options(&root, definitions, native_options)?;
            let elapsed = elapsed_ns(build_started);
            let records = repository
                .collections()
                .iter()
                .map(|collection| collection.generation.records.len())
                .sum();
            let output = BuildOutput {
                mode: mode_name(args.mode),
                schema_profile: schema_name(args.schema_profile),
                alphabet: alphabet_name(args.alphabet),
                storage: storage_name(args.storage),
                root: root.display().to_string(),
                configuration_digest: hex(&configuration_digest),
                build_identity: hex(&build_identity),
                schema_digest,
                elapsed_ns: elapsed,
                resident_bytes: memory_bytes("VmRSS:"),
                peak_resident_bytes: memory_bytes("VmHWM:"),
                user_cpu_ticks: process_cpu_ticks()
                    .zip(cpu_start)
                    .map(|(end, start)| end.0.saturating_sub(start.0)),
                system_cpu_ticks: process_cpu_ticks()
                    .zip(cpu_start)
                    .map(|(end, start)| end.1.saturating_sub(start.1)),
                memory_host_files: None,
                report: None,
                validation: None,
                native: Some(NativeOutput {
                    build_ns: repository.startup().build_ns,
                    discovery_ns: repository.startup().build_discovery_ns,
                    extraction_index_ns: repository.startup().build_extraction_index_ns,
                    finalization_ns: repository.startup().build_finalization_ns,
                    records,
                    requested_build_workers: args.prepare_workers,
                    resolved_build_workers: repository.startup().build_workers,
                }),
                environment: environment(&args),
            };
            write_json(&args.result_dir.join("build.json"), &output)?;
            println!(
                "{}",
                serde_json::to_string(&output).map_err(|error| error.to_string())?
            );
            return Ok(());
        }
        Mode::SourceBuild => {
            let location = match args.storage {
                Storage::Memory => StoreLocation::Memory,
                Storage::File => StoreLocation::File(
                    args.db_path
                        .clone()
                        .ok_or("--db-path is required for file storage")?,
                ),
            };
            let config = StoreConfig {
                location,
                configuration_digest,
                alphabet: alphabet_name(args.alphabet).into(),
                source_root: objekte_search_core::path_repr::native_bytes(&root),
                build_identity,
            };
            let built = build_from_source(
                config,
                &root,
                &definitions,
                &sources,
                args.alphabet,
                DirectBuildOptions {
                    prepare_workers: args.prepare_workers,
                    write_workers: args.write_workers,
                    profile: args.profile,
                    pipelined_writes: args.pipelined_writes,
                    ..DirectBuildOptions::default()
                },
            )
            .map_err(|error| error.to_string())?;
            let mut host_files = None;
            if args.storage == Storage::Memory {
                let directory = built
                    .store
                    .memory_host_directory()
                    .ok_or("memory store has no host directory")?;
                let mut names = std::fs::read_dir(directory)
                    .map_err(|error| error.to_string())?
                    .map(|entry| {
                        entry
                            .map(|entry| entry.file_name().to_string_lossy().into_owned())
                            .map_err(|error| error.to_string())
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                names.sort();
                host_files = Some(names);
            } else {
                built
                    .store
                    .flush_wal(true)
                    .map_err(|error| error.to_string())?;
                built
                    .store
                    .flush_memtable()
                    .map_err(|error| error.to_string())?;
            }
            let output = BuildOutput {
                mode: mode_name(args.mode),
                schema_profile: schema_name(args.schema_profile),
                alphabet: alphabet_name(args.alphabet),
                storage: storage_name(args.storage),
                root: root.display().to_string(),
                configuration_digest: hex(&configuration_digest),
                build_identity: hex(&build_identity),
                schema_digest,
                elapsed_ns: built.report.timing.ready_ns,
                resident_bytes: memory_bytes("VmRSS:"),
                peak_resident_bytes: memory_bytes("VmHWM:"),
                user_cpu_ticks: process_cpu_ticks()
                    .zip(cpu_start)
                    .map(|(end, start)| end.0.saturating_sub(start.0)),
                system_cpu_ticks: process_cpu_ticks()
                    .zip(cpu_start)
                    .map(|(end, start)| end.1.saturating_sub(start.1)),
                memory_host_files: host_files,
                report: Some(&built.report),
                validation: None,
                native: None,
                environment: environment(&args),
            };
            write_json(&args.result_dir.join("build.json"), &output)?;
            println!(
                "{}",
                serde_json::to_string(&output).map_err(|error| error.to_string())?
            );
            return Ok(());
        }
        Mode::Validate => {
            let location = match args.storage {
                Storage::Memory => StoreLocation::Memory,
                Storage::File => StoreLocation::File(
                    args.db_path
                        .clone()
                        .ok_or("--db-path is required for file storage")?,
                ),
            };
            let config = StoreConfig {
                location,
                configuration_digest,
                alphabet: alphabet_name(args.alphabet).into(),
                source_root: objekte_search_core::path_repr::native_bytes(&root),
                build_identity,
            };
            let built = build_from_source(
                config,
                &root,
                &definitions,
                &sources,
                args.alphabet,
                DirectBuildOptions {
                    prepare_workers: args.prepare_workers,
                    write_workers: args.write_workers,
                    profile: args.profile,
                    pipelined_writes: args.pipelined_writes,
                    ..DirectBuildOptions::default()
                },
            )
            .map_err(|error| error.to_string())?;
            let validation = validate_store(&built.store).map_err(|error| error.to_string())?;
            write_json(&args.result_dir.join("validation.json"), &validation)?;
            if let Some(reference_path) = &args.reference {
                let native: ReferenceDigests = serde_json::from_slice(
                    &std::fs::read(reference_path).map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
                let candidate = store_reference(&built.store, &definitions)
                    .map_err(|error| error.to_string())?;
                let comparison = compare_reference(&native, &candidate);
                write_json(
                    &args.result_dir.join("reference-comparison.json"),
                    &comparison,
                )?;
                if !comparison.equal {
                    return Err(format!(
                        "native reference mismatch: {:?}",
                        comparison.mismatched_families
                    ));
                }
            }
            let output = BuildOutput {
                mode: mode_name(args.mode),
                schema_profile: schema_name(args.schema_profile),
                alphabet: alphabet_name(args.alphabet),
                storage: storage_name(args.storage),
                root: root.display().to_string(),
                configuration_digest: hex(&configuration_digest),
                build_identity: hex(&build_identity),
                schema_digest,
                elapsed_ns: built.report.timing.ready_ns,
                resident_bytes: memory_bytes("VmRSS:"),
                peak_resident_bytes: memory_bytes("VmHWM:"),
                user_cpu_ticks: process_cpu_ticks()
                    .zip(cpu_start)
                    .map(|(end, start)| end.0.saturating_sub(start.0)),
                system_cpu_ticks: process_cpu_ticks()
                    .zip(cpu_start)
                    .map(|(end, start)| end.1.saturating_sub(start.1)),
                memory_host_files: None,
                report: Some(&built.report),
                validation: Some(&validation),
                native: None,
                environment: environment(&args),
            };
            write_json(&args.result_dir.join("build.json"), &output)?;
            println!(
                "{}",
                serde_json::to_string(&output).map_err(|error| error.to_string())?
            );
            if !validation.is_valid() {
                return Err(format!(
                    "prototype validation found {} errors",
                    validation.errors.len()
                ));
            }
            return Ok(());
        }
        Mode::ReferenceExport => {
            let repository = Repository::open_with_options(&root, definitions, native_options)?;
            let reference = native_reference(&repository).map_err(|error| error.to_string())?;
            write_json(&args.result_dir.join("reference.json"), &reference)?;
            let output = serde_json::json!({
                "mode": "reference-export",
                "schema_profile": schema_name(args.schema_profile),
                "alphabet": alphabet_name(args.alphabet),
                "root": root.display().to_string(),
                "configuration_digest": hex(&configuration_digest),
                "build_identity": hex(&build_identity),
                "schema_digest": schema_digest,
                "reference": reference,
                "environment": environment(&args),
            });
            write_json(&args.result_dir.join("manifest.json"), &output)?;
            println!(
                "{}",
                serde_json::to_string(&output).map_err(|error| error.to_string())?
            );
            return Ok(());
        }
        Mode::Primitives | Mode::Mutations | Mode::Reopen => {
            return Err(format!(
                "mode {} is implemented in a later prototype phase",
                mode_name(args.mode)
            ));
        }
    }
}

fn parse_args() -> Result<Args, String> {
    let raw_arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let mut arguments = raw_arguments.iter();
    let mut mode = None;
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let mut alphabet = BigramAlphabet::Ascii;
    let mut schema_profile = SchemaProfile::Basic;
    let mut storage = Storage::Memory;
    let mut db_path = None;
    let mut prepare_workers = None;
    let mut write_workers = None;
    let mut backend = None;
    let mut reference = None;
    let mut result_dir = None;
    let mut profile = false;
    let mut pipelined_writes = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--mode" => {
                mode = Some(match next(&mut arguments, "--mode")? {
                    "native-build" => Mode::NativeBuild,
                    "source-build" => Mode::SourceBuild,
                    "reference-export" => Mode::ReferenceExport,
                    "validate" => Mode::Validate,
                    "primitives" => Mode::Primitives,
                    "mutations" => Mode::Mutations,
                    "reopen" => Mode::Reopen,
                    other => return Err(format!("unknown mode {other}")),
                });
            }
            "--root" => root = PathBuf::from(next(&mut arguments, "--root")?),
            "--alphabet" => {
                alphabet = match next(&mut arguments, "--alphabet")? {
                    "ascii" => BigramAlphabet::Ascii,
                    "unicode" => BigramAlphabet::Unicode,
                    other => return Err(format!("unknown alphabet {other}")),
                }
            }
            "--schema-profile" => {
                schema_profile = match next(&mut arguments, "--schema-profile")? {
                    "basic" => SchemaProfile::Basic,
                    "extended" => SchemaProfile::Extended,
                    other => return Err(format!("unknown schema profile {other}")),
                }
            }
            "--storage" => {
                storage = match next(&mut arguments, "--storage")? {
                    "memory" => Storage::Memory,
                    "file" => Storage::File,
                    other => return Err(format!("unknown storage mode {other}")),
                }
            }
            "--db-path" => db_path = Some(PathBuf::from(next(&mut arguments, "--db-path")?)),
            "--prepare-workers" => {
                prepare_workers = parse_workers(next(&mut arguments, "--prepare-workers")?)?
            }
            "--write-workers" => {
                write_workers = parse_workers(next(&mut arguments, "--write-workers")?)?
            }
            "--backend" => backend = Some(next(&mut arguments, "--backend")?.to_owned()),
            "--reference" => {
                reference = Some(PathBuf::from(next(&mut arguments, "--reference")?));
            }
            "--result-dir" => {
                result_dir = Some(PathBuf::from(next(&mut arguments, "--result-dir")?))
            }
            "--profile" => profile = true,
            "--pipelined-writes" => pipelined_writes = true,
            "--inference" => return Err("inference is unsupported by the RocksDB prototype".into()),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let mode = mode.ok_or("--mode is required")?;
    let result_dir = result_dir.ok_or("--result-dir is required")?;
    if profile && !cfg!(feature = "detailed-profiling") {
        return Err("--profile requires the detailed-profiling feature".into());
    }
    if let Some(backend) = backend {
        if !matches!(backend.as_str(), "native" | "rocksdb") {
            return Err(format!("unknown backend {backend}"));
        }
        if mode != Mode::Primitives {
            return Err("--backend is valid only with --mode primitives".into());
        }
    }
    if reference.is_some() && !matches!(mode, Mode::Validate | Mode::Reopen) {
        return Err("--reference is valid only with validate or reopen".into());
    }
    if storage == Storage::File && db_path.is_none() {
        return Err("--db-path is required for file storage".into());
    }
    if storage == Storage::Memory && db_path.is_some() {
        return Err("--db-path is incompatible with memory storage".into());
    }
    Ok(Args {
        mode,
        root,
        alphabet,
        schema_profile,
        storage,
        db_path,
        prepare_workers,
        write_workers,
        reference,
        result_dir,
        profile,
        pipelined_writes,
        raw_arguments,
    })
}

fn next<'a>(
    arguments: &mut impl Iterator<Item = &'a String>,
    name: &str,
) -> Result<&'a str, String> {
    arguments
        .next()
        .map(String::as_str)
        .ok_or_else(|| format!("{name} needs a value"))
}

fn parse_workers(value: &str) -> Result<Option<usize>, String> {
    if value == "auto" {
        return Ok(None);
    }
    let workers = value
        .parse::<usize>()
        .map_err(|error| format!("invalid worker count: {error}"))?;
    if workers == 0 {
        return Err("worker count must be positive".into());
    }
    Ok(Some(workers))
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    std::fs::write(path, bytes).map_err(|error| error.to_string())
}

fn environment(args: &Args) -> Environment {
    let executable_sha256 = std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::read(path).ok())
        .map(|bytes| hex(&Sha256::digest(bytes)))
        .unwrap_or_else(|| "unavailable".into());
    Environment {
        available_parallelism: std::thread::available_parallelism().map_or(1, NonZeroUsize::get),
        executable_sha256,
        rust_version: env!("CARGO_PKG_RUST_VERSION"),
        os: std::env::consts::OS,
        args: args.raw_arguments.clone(),
    }
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

fn process_cpu_ticks() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let after_command = stat.rsplit_once(')')?.1;
    let fields = after_command.split_whitespace().collect::<Vec<_>>();
    Some((fields.get(11)?.parse().ok()?, fields.get(12)?.parse().ok()?))
}

fn elapsed_ns(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn alphabet_name(alphabet: BigramAlphabet) -> &'static str {
    match alphabet {
        BigramAlphabet::Ascii => "ascii",
        BigramAlphabet::Unicode => "unicode",
    }
}

fn schema_name(profile: SchemaProfile) -> &'static str {
    match profile {
        SchemaProfile::Basic => "basic",
        SchemaProfile::Extended => "extended",
    }
}

fn storage_name(storage: Storage) -> &'static str {
    match storage {
        Storage::Memory => "memory",
        Storage::File => "file",
    }
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::NativeBuild => "native-build",
        Mode::SourceBuild => "source-build",
        Mode::ReferenceExport => "reference-export",
        Mode::Validate => "validate",
        Mode::Primitives => "primitives",
        Mode::Mutations => "mutations",
        Mode::Reopen => "reopen",
    }
}
