use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Instant;
use objekte_search_core::repository::{CollectionDefinition, Repository, RepositoryOptions, SnapshotMode};
use objekte_search_core::source::SourceFormat;

struct Fingerprint(u64);
impl Fingerprint {
    fn new() -> Self { Self(0xcbf29ce484222325) }
    fn bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    fn u64(&mut self, number: u64) { self.bytes(&number.to_le_bytes()); }
}

fn memory_kib(name: &str) -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    status.lines().find_map(|line| {
        let rest = line.strip_prefix(name)?;
        rest.split_whitespace().next()?.parse().ok()
    }).unwrap()
}

fn main() -> Result<(), String> {
    let root = PathBuf::from(std::env::args().nth(1).ok_or("missing root")?);
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::Disabled,
        build_workers: NonZeroUsize::new(4),
        ..RepositoryOptions::default()
    };
    let started = Instant::now();
    let repo = Repository::open_with_options(&root, vec![definition], options)?;
    let wall_ns = started.elapsed().as_nanos() as u64;
    let peak_kib = memory_kib("VmHWM:");
    let resident_kib = memory_kib("VmRSS:");
    let collection = &repo.collections()[0];
    let mut hash = Fingerprint::new();
    for file in &repo.files().files {
        hash.bytes(file.relative_path.to_string_lossy().as_bytes());
        hash.u64(file.revision.size);
        hash.bytes(&file.revision.modified_ns.to_le_bytes());
        hash.bytes(&[file.status.code()]);
        hash.u64(file.diagnostics.len() as u64);
    }
    for record in &collection.generation.records {
        hash.u64(record.slot.0 as u64);
        hash.bytes(record.id.0.as_bytes());
        hash.u64(record.file.0 as u64);
        hash.u64(record.range.start);
        hash.u64(record.range.end);
        hash.bytes(&[record.class.code()]);
    }
    for grams in [&collection.raw.normal, &collection.raw.skip, &collection.fallback_text.normal, &collection.fallback_text.skip] {
        let mut keys: Vec<_> = grams.keys().copied().collect();
        keys.sort_unstable();
        hash.u64(keys.len() as u64);
        for key in keys {
            hash.u64(key);
            for slot in grams[&key].iter() { hash.u64(slot as u64); }
            hash.u64(u64::MAX);
        }
    }
    let stats = repo.startup();
    println!("{{\"wall_ns\":{wall_ns},\"build_ns\":{},\"discovery_ns\":{},\"extraction_index_ns\":{},\"finalization_ns\":{},\"peak_kib\":{peak_kib},\"resident_kib\":{resident_kib},\"peak_build_admitted_bytes\":{},\"build_weight_limit_bytes\":{},\"build_peak_active_jobs\":{},\"build_peak_ready_batches\":{},\"build_receive_wait_ns\":{},\"build_admission_ns\":{},\"build_workers\":{},\"files\":{},\"records\":{},\"parses\":{},\"failed_parses\":{},\"raw_normal_grams\":{},\"raw_skip_grams\":{},\"signature\":\"{:016x}\"}}",
        stats.build_ns,
        stats.build_discovery_ns,
        stats.build_extraction_index_ns,
        stats.build_finalization_ns,
        stats.peak_build_admitted_bytes,
        stats.build_weight_limit_bytes,
        stats.build_peak_active_jobs,
        stats.build_peak_ready_batches,
        stats.build_receive_wait_ns,
        stats.build_admission_ns,
        stats.build_workers,
        repo.files().files.len(),
        collection.generation.records.len(),
        repo.build_parses(),
        repo.build_failed_parses(),
        collection.raw.normal.len(),
        collection.raw.skip.len(),
        hash.0,
    );
    Ok(())
}
