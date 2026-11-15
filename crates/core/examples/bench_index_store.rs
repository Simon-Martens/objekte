//! A2 feasibility benchmark.
//!
//! Prepare (native reference process):
//! `bench_index_store --prepare-input /tmp/a2.input --root examples/musenalm --alphabet ascii`
//!
//! Import (fresh store-only process):
//! `bench_index_store --input /tmp/a2.input --result-dir bench/results/...`
//!
//! Build with `--features store-feasibility` plus one variant feature set:
//! control: `store-posting-rows`
//! reverse: `store-posting-rows,store-reverse-lz4`
//! postings: (none)
//! combined: `store-reverse-lz4`

#[path = "support/musenalm.rs"]
mod musenalm;

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;

use redb::{ReadableTable, ReadableTableMetadata};
use std::time::Instant;

use objekte_search_core::index_store::construction::discover_file;
use objekte_search_core::index_store::fts;
use objekte_search_core::index_store::mutations::{admit_file, delete_file, PreparedFile};
use objekte_search_core::index_store::value_codec::{
    BindingRole, CollectionMetadata, GramKind, GramProjection, RecordMetadata, StateKind,
    StoredContributions,
};
use objekte_search_core::index_store::{
    feasibility_input, read, tables, write, IndexStore, StoreConfig,
};
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
    let mut prepare_input = None;
    let mut input = None;
    let mut validate = false;
    let mut mutations = false;
    let mut build_source = false;
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
                    Some(
                        NonZeroUsize::new(value.parse().map_err(|_| "bad worker count")?)
                            .ok_or("zero workers")?,
                    )
                };
            }
            "--result-dir" => {
                result_dir = Some(PathBuf::from(
                    arguments.next().ok_or("--result-dir needs a path")?,
                ));
            }
            "--extended" => extended = true,
            "--prepare-input" => {
                prepare_input = Some(PathBuf::from(
                    arguments.next().ok_or("--prepare-input needs a path")?,
                ));
            }
            "--input" => {
                input = Some(PathBuf::from(
                    arguments.next().ok_or("--input needs a path")?,
                ));
            }
            "--validate" => validate = true,
            "--mutations" => mutations = true,
            "--build-source" => build_source = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if build_source && (prepare_input.is_some() || input.is_some() || mutations) {
        return Err(
            "--build-source cannot be combined with --prepare-input, --input, or --mutations"
                .into(),
        );
    }
    if !build_source && mutations {
        return Err("--mutations currently requires --build-source".into());
    }
    if build_source {
        let build_started = Instant::now();
        let (definitions, _schemas): (Vec<CollectionDefinition>, Vec<musenalm::LoadedSchema>) =
            if extended {
                musenalm::load_extended_definitions_with_schemas(&root)?
            } else {
                musenalm::load_definitions_with_schemas(&root)?
            };
        let store = Arc::new(IndexStore::new(StoreConfig::default()).map_err(|error| error.to_string())?);
        let rss_before = resident_bytes();
        let peak_before = peak_resident_bytes();
        let report = objekte_search_core::index_store::store_build::build_from_source(
            &store,
            &root,
            &definitions,
            &objekte_search_core::index_store::store_build::SourceBuildOptions {
                build_workers,
                alphabet,
                inference: objekte_search_core::inference::InferenceConfig::default(),
            },
            &objekte_search_core::source::SourceRegistry::builtin(),
        )?;
        let rss_after = resident_bytes();
        let peak_after = peak_resident_bytes();
        let table_rows = table_statistics(&store)?;
        let validation = if validate {
            let view = store.view().map_err(|error| error.to_string())?;
            Some(
                objekte_search_core::index_store::validation::validate(&view)
                    .map_err(|error| error.to_string())?,
            )
        } else {
            None
        };
        let output = json!({
            "mode": "build-source",
            "report": {
                "collections": report.discovery.collections,
                "files": report.discovery.files,
                "memberships": report.discovery.memberships,
                "records": report.records,
                "batches": report.batches,
                "resolved_workers": report.resolved_workers,
                "discovery_ns": report.discovery_ns,
                "scheduling_job_ns": report.scheduling_job_ns,
                "receive_wait_ns": report.receive_wait_ns,
                "admission_ns": report.admission_ns,
                "finalization_ns": report.finalization_ns,
                "out_of_order_batches": report.out_of_order_batches,
                "peak_active_jobs": report.peak_active_jobs,
                "peak_ready_batches": report.peak_ready_batches,
                "peak_weight_bytes": report.peak_weight_bytes,
                "elapsed_ns": report.elapsed_ns,
                "caller_elapsed_ns": build_started.elapsed().as_nanos(),
            },
            "rss_before_bytes": rss_before,
            "rss_after_bytes": rss_after,
            "peak_before_bytes": peak_before,
            "peak_after_bytes": peak_after,
            "table_rows": table_rows.0,
            "table_bytes": table_rows.1,
            "validation": validation.as_ref().map(|validation| json!({
                "collections": validation.collections,
                "live_records": validation.live_records,
                "free_slots": validation.free_slots,
                "value_members": validation.value_members,
                "length_members": validation.length_members,
                "state_members": validation.state_members,
                "fts_blocks": validation.fts_blocks,
                "sort_entries": validation.sort_entries,
                "relation_occurrences": validation.relation_occurrences,
                "errors": validation.errors,
            })),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&output).map_err(|error| error.to_string())?
        );
        if let Some(directory) = result_dir {
            std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
            let path = directory.join(format!(
                "build-source-{alphabet:?}-{}.json",
                std::process::id()
            ));
            std::fs::write(
                &path,
                serde_json::to_vec_pretty(&output).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
        }
        return Ok(());
    }
    if let Some(path) = prepare_input {
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
        let prepare_started = Instant::now();
        let manifest = feasibility_input::prepare(&repository, &path)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "mode": "prepare-input",
                "build_ns": build_ns,
                "prepare_ns": prepare_started.elapsed().as_nanos(),
                "manifest": manifest,
                "resolved_build_workers": repository.startup().build_workers,
                "input_path": path,
                "input_bytes": std::fs::metadata(&path).map(|metadata| metadata.len()).unwrap_or(0),
            }))
            .map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    let Some(input_path) = input else {
        return Err("use --prepare-input PATH or --input PATH".into());
    };
    let rss_before = resident_bytes();
    let peak_before = peak_resident_bytes();
    let (store, report) = feasibility_input::import(&input_path, StoreConfig::default())?;
    let rss_after = resident_bytes();
    let peak_after = peak_resident_bytes();
    if validate {
        let view = store.view().map_err(|error| error.to_string())?;
        let validation = objekte_search_core::index_store::validation::validate(&view)
            .map_err(|error| error.to_string())?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "mode": "validate",
                "rss_after_bytes": rss_after,
                "peak_after_bytes": peak_after,
                "validation": {
                    "collections": validation.collections,
                    "live_records": validation.live_records,
                    "free_slots": validation.free_slots,
                    "value_members": validation.value_members,
                    "length_members": validation.length_members,
                    "state_members": validation.state_members,
                    "fts_blocks": validation.fts_blocks,
                    "sort_entries": validation.sort_entries,
                    "relation_occurrences": validation.relation_occurrences,
                    "errors": validation.errors,
                }
            }))
            .map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    let table_rows = table_statistics(&store)?;
    let read_measurements = measure_reads(&store)?;
    let churn = measure_churn(&store)?;
    let single_record = if mutations {
        Some(measure_single_record_mutations(&store)?)
    } else {
        None
    };
    let output = json!({
        "mode": "import",
        "input_path": input_path,
        "input_bytes": std::fs::metadata(&input_path).map(|metadata| metadata.len()).unwrap_or(0),
        "import_ns": report.import_ns,
        "backend_len_bytes": report.backend_len_bytes,
        "backend_peak_bytes": report.backend_peak_bytes,
        "manifest": report.manifest,
        "collections": report.collections,
        "files": report.files,
        "records": report.records,
        "value_members": report.value_members,
        "length_members": report.length_members,
        "state_facts": report.state_facts,
        "gram_memberships": report.gram_memberships,
        "sort_entries": report.sort_entries,
        "relation_occurrences": report.relation_occurrences,
        "rss_before_bytes": rss_before,
        "rss_after_bytes": rss_after,
        "peak_before_bytes": peak_before,
        "peak_after_bytes": peak_after,
        "table_rows": table_rows.0,
        "table_bytes": table_rows.1,
        "read_measurements": read_measurements,
        "churn": churn,
        "single_record_mutations": single_record,
    });
    println!("{}", serde_json::to_string_pretty(&output).map_err(|error| error.to_string())?);
    if let Some(directory) = result_dir {
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let path = directory.join(format!(
            "import-{alphabet:?}-{}.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&output).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn table_statistics(
    store: &Arc<IndexStore>,
) -> Result<(Vec<(String, u64)>, Vec<(String, u64, u64)>), String> {
    let view = store.view().map_err(|error| error.to_string())?;
    let mut rows = Vec::new();
    let mut bytes = Vec::new();
    for (name, definition) in tables::ALL_TABLES {
        let table = view
            .read()
            .open_table(*definition)
            .map_err(|error| error.to_string())?;
        let row_count = table.len().map_err(|error| error.to_string())?;
        let stats = table.stats().map_err(|error| error.to_string())?;
        let pages = stats.leaf_pages().saturating_add(stats.branch_pages());
        rows.push(((*name).to_string(), row_count));
        bytes.push((
            (*name).to_string(),
            stats.stored_bytes(),
            pages.saturating_mul(4096),
        ));
    }
    Ok((rows, bytes))
}

fn measure_reads(store: &Arc<IndexStore>) -> Result<serde_json::Value, String> {
    let view = store.view().map_err(|error| error.to_string())?;
    let collections = read::all_collections(&view).map_err(|error| error.to_string())?;
    let Some(collection) = collections.first() else {
        return Ok(json!({"empty": true}));
    };
    let bindings = read::all_bindings(&view, collection.id).map_err(|error| error.to_string())?;
    let Some(binding) = bindings
        .iter()
        .find(|binding| binding.descriptor.role == BindingRole::Structured)
    else {
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
    let (gram_binding, kind, key, gram_cardinality, _blocks) = gram.ok_or("no gram statistics")?;
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

    let state_slot = read::state_members(&view, collection.id, binding.id, StateKind::PathPresence)
        .map_err(|error| error.to_string())?
        .first()
        .copied();
    let started = Instant::now();
    let mut point_hits = 0usize;
    for _ in 0..1_000 {
        if let Some(slot) = state_slot {
            point_hits += usize::from(
                read::state_contains(
                    &view,
                    collection.id,
                    binding.id,
                    StateKind::PathPresence,
                    slot,
                )
                .map_err(|error| error.to_string())?,
            );
        }
    }
    let point_membership_ns = started.elapsed().as_nanos();

    let mut length = None;
    {
        let table = view
            .read()
            .open_table(tables::LENGTH_DIRECTORY)
            .map_err(|error| error.to_string())?;
        for row in table.iter().map_err(|error| error.to_string())? {
            let (key, _) = row.map_err(|error| error.to_string())?;
            let mut cursor = objekte_search_core::index_store::key_codec::Cursor::new(key.value());
            let _collection = cursor.read_u32().map_err(|error| error.to_string())?;
            let _binding = cursor.read_u32().map_err(|error| error.to_string())?;
            let scalar = cursor.read_scalar().map_err(|error| error.to_string())?;
            length = Some(scalar);
            break;
        }
    }
    let started = Instant::now();
    let mut length_rows = 0usize;
    if let Some(length) = &length {
        for _ in 0..100 {
            length_rows += read::length_members(&view, collection.id, binding.id, length)
                .map_err(|error| error.to_string())?
                .len();
        }
    }
    let length_lookup_ns = started.elapsed().as_nanos();

    let restriction = read::value_members(&view, collection.id, binding.id, &scalar)
        .map_err(|error| error.to_string())?;
    let restriction = restriction
        .iter()
        .map(|slot| slot.0)
        .collect::<roaring::RoaringBitmap>();
    let started = Instant::now();
    let mut restricted = 0usize;
    for _ in 0..20 {
        restricted += fts::candidates(
            &view,
            collection.id,
            gram_binding,
            kind,
            &[key],
            Some(&restriction),
        )
        .map_err(|error| error.to_string())?
        .map_or(0, |bitmap| bitmap.len() as usize);
    }
    let restricted_candidates_ns = started.elapsed().as_nanos();

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
        "point_membership_1000_ns": point_membership_ns,
        "point_membership_hits": point_hits,
        "length_lookup_100_ns": length_lookup_ns,
        "length_lookup_rows": length_rows,
        "restricted_candidates_20_ns": restricted_candidates_ns,
        "restricted_candidates": restricted,
    }))
}

fn measure_single_record_mutations(
    store: &Arc<IndexStore>,
) -> Result<serde_json::Value, String> {
    let metadata = store.metadata().map_err(|error| error.to_string())?;
    let collection_id = CollectionId(20_000);
    let tx = store.begin_write().map_err(|error| error.to_string())?;
    write::put_collection(
        &tx,
        &CollectionMetadata {
            id: collection_id,
            name: "single-record-mutations".into(),
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
        },
    )
    .map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())?;
    let epoch = store.view().map_err(|error| error.to_string())?.build_epoch();
    let mut files = Vec::new();
    for index in 0..1_000u32 {
        let file = single_file(index);
        let mut collection = read::collection(
            &store.view().map_err(|error| error.to_string())?,
            collection_id,
        )
        .map_err(|error| error.to_string())?;
        let tx = store.begin_write().map_err(|error| error.to_string())?;
        discover_file(&tx, &mut collection, &file, 1, epoch, metadata.revision)
            .map_err(|error| error.to_string())?;
        write::put_collection(&tx, &collection).map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        admit_file(
            store,
            collection_id,
            &PreparedFile {
                file: file.clone(),
                file_version: 1,
                fingerprint: None,
                records: vec![single_contribution(index, 0)],
            },
            epoch,
        )
        .map_err(|error| error.to_string())?;
        files.push(file);
    }
    let mut update = Vec::with_capacity(1_000);
    for (index, file) in files.iter().enumerate() {
        let mut replacement = file.clone();
        replacement.revision.modified_ns += 1;
        let started = Instant::now();
        objekte_search_core::index_store::mutations::update_file(
            store,
            collection_id,
            &PreparedFile {
                file: replacement,
                file_version: 2,
                fingerprint: None,
                records: vec![single_contribution(index as u32, 1)],
            },
            1,
            epoch,
        )
        .map_err(|error| error.to_string())?;
        update.push(started.elapsed().as_nanos());
    }
    let mut delete = Vec::with_capacity(1_000);
    for (index, file) in files.iter().enumerate() {
        let started = Instant::now();
        delete_file(store, file.id).map_err(|error| error.to_string())?;
        let mut collection = read::collection(
            &store.view().map_err(|error| error.to_string())?,
            collection_id,
        )
        .map_err(|error| error.to_string())?;
        let tx = store.begin_write().map_err(|error| error.to_string())?;
        discover_file(&tx, &mut collection, file, 2, epoch, metadata.revision)
            .map_err(|error| error.to_string())?;
        write::put_collection(&tx, &collection).map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        admit_file(
            store,
            collection_id,
            &PreparedFile {
                file: file.clone(),
                file_version: 2,
                fingerprint: None,
                records: vec![single_contribution(index as u32, 2)],
            },
            epoch,
        )
        .map_err(|error| error.to_string())?;
        delete.push(started.elapsed().as_nanos());
    }
    fn percentile(values: &[u128], percentile: f64) -> u128 {
        let mut sorted = values.to_vec();
        sorted.sort_unstable();
        let index = ((sorted.len() as f64 - 1.0) * percentile).round() as usize;
        sorted[index]
    }
    Ok(json!({
        "files": 1_000,
        "update_p50_ns": percentile(&update, 0.50),
        "update_p95_ns": percentile(&update, 0.95),
        "update_p99_ns": percentile(&update, 0.99),
        "delete_reinsert_p50_ns": percentile(&delete, 0.50),
        "delete_reinsert_p95_ns": percentile(&delete, 0.95),
        "delete_reinsert_p99_ns": percentile(&delete, 0.99),
    }))
}

fn single_file(index: u32) -> FileEntry {
    FileEntry {
        id: FileId(20_000 + index),
        relative_path: std::path::PathBuf::from(format!("m{index}.jsonl")).into(),
        revision: SourceRevision {
            modified_ns: 1,
            size: 100,
        },
        created_ns: None,
        collections: vec![CollectionId(20_000)],
        status: FileStatus::Valid,
        diagnostics: Vec::new(),
        omitted_diagnostics: 0,
    }
}

fn single_contribution(index: u32, round: usize) -> StoredContributions {
    let entry = RecordEntry {
        slot: RecordSlot(0),
        id: RecordId(format!("m{index}").into()),
        file: FileId(20_000 + index),
        range: ByteRange { start: 0, end: 8 },
        revision: SourceRevision {
            modified_ns: 1,
            size: 100,
        },
        class: RecordClass::Logical,
        diagnostic_ref: DiagnosticRef(0),
    };
    StoredContributions {
        record: RecordMetadata::from_entry(&entry, 1),
        values: vec![(0, ScalarKey::String(format!("value-{round}-{index}")))],
        lengths: vec![(0, objekte_search_core::model::Number::Integer(8))],
        state_masks: vec![(
            0,
            (1 << StateKind::PathPresence.tag()) | (1 << StateKind::NonEmpty.tag()),
        )],
        coverage: Vec::new(),
        domains: Vec::new(),
        sorts: Vec::new(),
        relations: Vec::new(),
        grams: vec![GramProjection {
            binding: 0,
            kind: GramKind::Normal,
            grams: vec![u64::from(index) + u64::from(round as u32)],
        }],
    }
}

fn measure_churn(store: &Arc<IndexStore>) -> Result<serde_json::Value, String> {
    let metadata = store.metadata().map_err(|error| error.to_string())?;
    let churn_id = CollectionId(10_000);
    let tx = store.begin_write().map_err(|error| error.to_string())?;
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
    for _ in 0..2 {
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
        discover_file(
            &tx,
            &mut collection,
            &prepared.file,
            prepared.file_version,
            epoch,
            0,
        )
        .map_err(|error| error.to_string())?;
        write::put_collection(&tx, &collection).map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        admit_file(store, churn_id, &prepared, epoch).map_err(|error| error.to_string())?;
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
        state_masks: vec![(
            0,
            (1 << StateKind::PathPresence.tag()) | (1 << StateKind::NonEmpty.tag()),
        )],
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

fn resident_bytes() -> u64 {
    proc_status_kib("VmRSS:").saturating_mul(1024)
}

fn peak_resident_bytes() -> u64 {
    proc_status_kib("VmHWM:").saturating_mul(1024)
}

fn proc_status_kib(field: &str) -> u64 {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    for line in status.lines() {
        if let Some(value) = line.strip_prefix(field) {
            return value
                .trim()
                .trim_end_matches("kB")
                .trim()
                .parse::<u64>()
                .unwrap_or(0);
        }
    }
    0
}
