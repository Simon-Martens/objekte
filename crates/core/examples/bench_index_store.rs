//! Phase-5 feasibility export of the native index into the in-memory store.
//!
//! Build:
//! `cargo build --release --locked -p objekte-search-core --example bench_index_store --features store-feasibility`
//!
//! Run against the canonical corpus:
//! `target/release/examples/bench_index_store --root examples/musenalm --alphabet ascii`

#[path = "support/musenalm.rs"]
mod musenalm;

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::index_store::construction::discover_file;
use objekte_search_core::index_store::fts;
use objekte_search_core::index_store::mutations::{admit_file, delete_file, PreparedFile};
use objekte_search_core::index_store::value_codec::{
    BindingRole, CollectionMetadata, GramKind, GramProjection, RecordMetadata, StateKind,
    StoredContributions,
};
use objekte_search_core::index_store::{feasibility, read, write, IndexStore, StoreConfig};
use objekte_search_core::inventory::{
    ByteRange, DiagnosticRef, FileEntry, FileStatus, RecordClass, RecordEntry, SourceRevision,
};
use objekte_search_core::model::{CollectionId, FileId, RecordId, RecordSlot};
use objekte_search_core::posting::ScalarKey;
use objekte_search_core::repository::{CollectionDefinition, Repository, RepositoryOptions};
use objekte_search_core::workload::BigramAlphabet;
use serde_json::json;

fn main() -> Result<(), String> {
    let mut root = PathBuf::from("examples/musenalm");
    let mut alphabet = BigramAlphabet::Ascii;
    let mut build_workers = None;
    let mut result_dir = None;
    let mut extended = false;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" => root = PathBuf::from(arguments.next().ok_or("--root needs a path")?),
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
                    Some(NonZeroUsize::new(value.parse().map_err(|_| "bad worker count")?).ok_or("zero workers")?)
                };
            }
            "--result-dir" => {
                result_dir = Some(PathBuf::from(
                    arguments.next().ok_or("--result-dir needs a path")?,
                ));
            }
            "--extended" => extended = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let (definitions, _schemas): (Vec<CollectionDefinition>, Vec<musenalm::LoadedSchema>) =
        if extended {
            musenalm::load_extended_definitions_with_schemas(&root)?
        } else {
            musenalm::load_definitions_with_schemas(&root)?
        };
    let build_started = Instant::now();
    let repository = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            bigram_alphabet: alphabet,
            build_workers,
            ..RepositoryOptions::default()
        },
    )?;
    let build_ns = build_started.elapsed().as_nanos();
    let export_started = Instant::now();
    let (store, mut report) = feasibility::export_native_index(
        &repository,
        StoreConfig::default(),
    )?;
    let export_ns = export_started.elapsed().as_nanos();
    let rss_with_native_bytes = report.rss_after_bytes;
    let resolved_build_workers = repository.startup().build_workers;
    drop(repository);
    trim_allocator();
    std::thread::sleep(std::time::Duration::from_millis(250));
    let rss_after_native_drop_bytes = resident_bytes();
    let read_measurements = measure_reads(&store)?;
    let churn = measure_churn(&store)?;
    let query = json!({
        "build_ns": build_ns,
        "export_ns": export_ns,
        "resolved_build_workers": resolved_build_workers,
        "collections": report.collections,
        "files": report.files,
        "records": report.records,
        "bindings": report.bindings,
        "value_members": report.value_members,
        "length_members": report.length_members,
        "state_members": report.state_members,
        "fts_bits": report.fts_bits,
        "fts_blocks": report.fts_blocks,
        "fts_grams": report.fts_grams,
        "sort_definitions": report.sort_definitions,
        "sort_entries": report.sort_entries,
        "relation_occurrences": report.relation_occurrences,
        "contribution_rows": report.contribution_rows,
        "reverse_contribution_bytes": report.reverse_contribution_bytes,
        "state_blocks": report.table_rows.iter().find(|(name, _)| name == "state_blocks").map_or(0, |(_, rows)| *rows),
        "estimated_record_gram_bytes": report.estimated_record_gram_bytes,
        "write_ns": report.write_ns,
        "rss_before_bytes": report.rss_before_bytes,
        "rss_with_native_bytes": rss_with_native_bytes,
        "rss_after_native_drop_bytes": rss_after_native_drop_bytes,
        "rss_peak_bytes": report.rss_peak_bytes,
        "table_rows": report.table_rows,
        "table_bytes": report.table_bytes,
        "read_measurements": read_measurements,
        "churn": churn,
    });
    println!("{}", serde_json::to_string_pretty(&query).map_err(|error| error.to_string())?);
    if let Some(directory) = result_dir {
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let path = directory.join(format!(
            "feasibility-{alphabet:?}-{}.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&query).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
    }
    let _ = report;
    Ok(())
}

/// Benchmark-only allocator trim: returns freed scratch pages so the
/// store-only RSS sample is not dominated by the temporary export buffers.
fn trim_allocator() {
    #[cfg(target_env = "gnu")]
    unsafe {
        libc::malloc_trim(0);
    }
}

fn resident_bytes() -> u64 {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    for line in status.lines() {
        if let Some(value) = line.strip_prefix("VmRSS:") {
            return value
                .trim()
                .trim_end_matches("kB")
                .trim()
                .parse::<u64>()
                .unwrap_or(0)
                * 1024;
        }
    }
    0
}

fn measure_reads(store: &std::sync::Arc<IndexStore>) -> Result<serde_json::Value, String> {
    let view = store.view().map_err(|error| error.to_string())?;
    let collections = read::all_collections(&view).map_err(|error| error.to_string())?;
    let Some(collection) = collections.first() else {
        return Ok(json!({"empty": true}));
    };
    let bindings = read::all_bindings(&view, collection.id).map_err(|error| error.to_string())?;
    let Some(binding) = bindings.iter().find(|binding| binding.descriptor.role == BindingRole::Structured) else {
        return Ok(json!({"empty": true}));
    };
    let mut value = None;
    read::for_each_value_directory(&view, collection.id, binding.id, |scalar, entry| {
        value = Some((scalar, entry.cardinality));
        Ok(false)
    })
    .map_err(|error| error.to_string())?;
    let (scalar, cardinality) = value.ok_or("no value directory entries")?;
    let started = Instant::now();
    let mut rows = 0usize;
    for _ in 0..200 {
        rows += read::value_members(&view, collection.id, binding.id, &scalar)
            .map_err(|error| error.to_string())?
            .len();
    }
    let value_lookup_ns = started.elapsed().as_nanos();

    let started = Instant::now();
    let mut state_rows = 0usize;
    for _ in 0..200 {
        state_rows += read::state_members(&view, collection.id, binding.id, StateKind::PathPresence)
            .map_err(|error| error.to_string())?
            .len();
    }
    let state_lookup_ns = started.elapsed().as_nanos();

    let started = Instant::now();
    let mut pages = 0usize;
    for _ in 0..50 {
        pages += read::sort_entries(&view, collection.id, 0, 100)
            .map_err(|error| error.to_string())?
            .len();
    }
    let sort_page_ns = started.elapsed().as_nanos();

    let mut gram = None;
    read::for_each_gram_statistics(&view, collection.id, |binding, kind, key, cardinality, blocks| {
        gram = Some((binding, kind, key, cardinality, blocks));
        Ok(false)
    })
    .map_err(|error| error.to_string())?;
    let (gram_binding, kind, key, gram_cardinality, _blocks) =
        gram.ok_or("no gram statistics")?;
    let started = Instant::now();
    let mut candidates = 0usize;
    for _ in 0..20 {
        candidates += fts::candidates(
            &view,
            collection.id,
            gram_binding,
            kind,
            &[key],
            None,
        )
        .map_err(|error| error.to_string())?
        .map_or(0, |bitmap| bitmap.len() as usize);
    }
    let fts_candidates_ns = started.elapsed().as_nanos();

    let started = Instant::now();
    let mut record_rows = 0usize;
    for slot in 0..100u32 {
        record_rows += usize::from(
            read::record(&view, collection.id, RecordSlot(slot))
                .map_err(|error| error.to_string())?
                .is_some(),
        );
    }
    let record_lookup_ns = started.elapsed().as_nanos();
    Ok(json!({
        "collection": collection.name,
        "binding": binding.id,
        "value_cardinality": cardinality,
        "value_lookup_200_ns": value_lookup_ns,
        "value_lookup_rows": rows,
        "state_lookup_200_ns": state_lookup_ns,
        "state_lookup_rows": state_rows,
        "sort_page_100x50_ns": sort_page_ns,
        "sort_page_rows": pages,
        "fts_gram_key": key,
        "fts_gram_cardinality": gram_cardinality,
        "fts_candidates_20_ns": fts_candidates_ns,
        "fts_candidates": candidates,
        "record_lookup_100_ns": record_lookup_ns,
        "record_rows": record_rows,
    }))
}

/// Fixed-population churn on a synthetic collection, using the ordinary
/// admission/update/delete APIs.
fn measure_churn(store: &std::sync::Arc<IndexStore>) -> Result<serde_json::Value, String> {
    let tx = store.begin_write().map_err(|error| error.to_string())?;
    let metadata = store.metadata().map_err(|error| error.to_string())?;
    let churn_id = CollectionId(10_000);
    let mut collection = CollectionMetadata {
        id: churn_id,
        name: "churn".into(),
        generation: 1,
        source_fingerprint: [0; 32],
        slot_high_water: 0,
        live_records: 0,
        discovered_files: 0,
        pending_files: 0,
        indexed_files: 0,
        unavailable_files: 0,
        processed_files: 0,
        next_binding_id: 1,
    };
    write::put_collection(&tx, &collection).map_err(|error| error.to_string())?;
    let file = FileEntry {
        id: FileId(10_000),
        relative_path: std::path::PathBuf::from("churn.jsonl").into(),
        revision: SourceRevision {
            modified_ns: 1,
            size: 1_000_000,
        },
        created_ns: None,
        collections: vec![churn_id],
        status: FileStatus::Valid,
        diagnostics: Vec::new(),
        omitted_diagnostics: 0,
    };
    discover_file(&tx, &mut collection, &file, 1, metadata.build_epoch, metadata.revision)
        .map_err(|error| error.to_string())?;
    write::put_collection(&tx, &collection).map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())?;
    let epoch = store.view().map_err(|error| error.to_string())?.build_epoch();
    let mut prepared = PreparedFile {
        file: file.clone(),
        file_version: 1,
        fingerprint: None,
        records: (0..1_000)
            .map(|index| churn_contribution(index, 0))
            .collect(),
    };
    admit_file(store, churn_id, &prepared, epoch).map_err(|error| error.to_string())?;
    let mut update_ns = 0u128;
    let mut delete_ns = 0u128;
    for round in 0..2 {
        let mut file = file.clone();
        file.revision.modified_ns += 1;
        prepared.file = file;
        prepared.file_version += 1;
        for (index, contribution) in prepared.records.iter_mut().enumerate() {
            contribution.values = vec![(0, ScalarKey::String(format!("value-{round}-{index}")))];
            contribution.grams = vec![GramProjection {
                binding: 0,
                kind: GramKind::Normal,
                grams: vec![u64::from(index as u32) + u64::from(round as u32)],
            }];
        }
        let started = Instant::now();
        objekte_search_core::index_store::mutations::update_file(
            store,
            churn_id,
            &prepared,
            prepared.file_version - 1,
            epoch,
        )
        .map_err(|error| error.to_string())?;
        update_ns += started.elapsed().as_nanos();
    }
    for round in 0..2 {
        let started = Instant::now();
        delete_file(store, FileId(10_000)).map_err(|error| error.to_string())?;
        delete_ns += started.elapsed().as_nanos();
        prepared.file_version += 1;
        prepared.file.revision.modified_ns += 1;
        let mut collection = read::collection(
            &store.view().map_err(|error| error.to_string())?,
            churn_id,
        )
        .map_err(|error| error.to_string())?;
        let tx = store.begin_write().map_err(|error| error.to_string())?;
        discover_file(&tx, &mut collection, &prepared.file, prepared.file_version, epoch, 0)
            .map_err(|error| error.to_string())?;
        write::put_collection(&tx, &collection).map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        admit_file(store, churn_id, &prepared, epoch).map_err(|error| error.to_string())?;
        let _ = round;
    }
    Ok(json!({
        "records": 1000,
        "update_rounds": 2,
        "update_ns": update_ns,
        "delete_reinsert_rounds": 2,
        "delete_ns": delete_ns,
    }))
}

fn churn_contribution(index: usize, round: usize) -> StoredContributions {
    let entry = RecordEntry {
        slot: RecordSlot(index as u32),
        id: RecordId(format!("churn-{index}").into()),
        file: FileId(10_000),
        range: ByteRange {
            start: index as u64 * 10,
            end: index as u64 * 10 + 8,
        },
        revision: SourceRevision {
            modified_ns: 1,
            size: 1_000_000,
        },
        class: RecordClass::Logical,
        diagnostic_ref: DiagnosticRef(0),
    };
    StoredContributions {
        record: RecordMetadata::from_entry(&entry, 1),
        values: vec![(0, ScalarKey::String(format!("value-{round}-{index}")))],
        lengths: vec![(0, objekte_search_core::model::Number::Integer(8))],
        state_masks: vec![(0, (1 << StateKind::PathPresence.tag()) | (1 << StateKind::NonEmpty.tag()))],
        coverage: Vec::new(),
        domains: Vec::new(),
        sorts: Vec::new(),
        relations: Vec::new(),
        grams: vec![GramProjection {
            binding: 0,
            kind: GramKind::Normal,
            grams: vec![u64::from(index as u32) + u64::from(round as u32)],
        }],
    }
}
