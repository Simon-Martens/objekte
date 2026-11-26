use std::path::PathBuf;
use std::sync::Arc;

use crate::construction::batch::{SourceFileInput, file_batches};
use crate::construction::scheduling::{
    RepositoryFileJob, process_repository_file_batches_profiled,
};
use crate::index::storage::segment::build::{FileSegmentBuild, SegmentSeal};
use crate::repository::configuration::{CollectionDefinition, RepositoryOptions};
use crate::source::registry::SourceRegistry;

/// Measure direct batch-to-shard construction using caller-supplied readers.
pub fn measure_direct_shard_build(
    root: PathBuf,
    definitions: Vec<CollectionDefinition>,
    sources: SourceRegistry,
    collection_name: &str,
) {
    let options = RepositoryOptions {
        ..Default::default()
    };
    let baseline_started = std::time::Instant::now();
    let index = crate::lifecycle::Index::start_build(&root, definitions, options, sources)
        .and_then(|task| task.wait_complete())
        .unwrap();
    let view = index.view().unwrap();
    let repository = view.repository();
    eprintln!(
        "direct shard prototype static_build={:?}",
        baseline_started.elapsed()
    );
    let collection = repository
        .collections()
        .iter()
        .find(|collection| collection.definition.name == collection_name)
        .unwrap();
    let id = collection.generation.collection;
    let definition = Arc::new(collection.required_file_definition().clone());
    let source = collection.required_file_reader().clone();
    let source_files = Arc::new(
        crate::index::storage::file_catalog::FileRows::new(&repository.catalog)
            .map(|row| row.map(|row| SourceFileInput::from_entry(&row.entry)))
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
    );
    let jobs = file_batches(&collection.generation.files, |id| {
        source_files
            .get(id.0 as usize)
            .map(|file| file.revision.size)
            .ok_or_else(|| "missing scheduled source metadata".to_owned())
    })
    .unwrap()
    .into_iter()
    .map(|files| RepositoryFileJob {
        collection_index: id.0 as usize,
        collection: id,
        definition: Arc::clone(&definition),
        source: source.clone(),
        files: files.to_vec(),
    })
    .collect::<Vec<_>>();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(repository.startup().build_workers)
        .build()
        .unwrap();
    let new_admission = || {
        repository
            .storage_factory
            .start_file_segment(FileSegmentBuild {
                collection: id,
                source: &source,
                policy: crate::index::segment::SegmentPolicy {
                    target_records: std::num::NonZeroUsize::new(2_048).unwrap(),
                    max_files: std::num::NonZeroUsize::new(2_048).unwrap(),
                },
            })
    };
    let mut admission = new_admission().unwrap();
    let mut chunks = Vec::new();
    let mut admitted_records = 0;
    let started = std::time::Instant::now();
    let stats = process_repository_file_batches_profiled(
        &pool,
        &repository.storage_factory,
        Arc::new(root.clone()),
        &jobs,
        source_files,
        options.bigram_alphabet,
        options.inference,
        false,
        |_, batch| {
            if admitted_records > 0
                && (admitted_records + batch.records.len() > 2_048
                    || admission.file_count() + batch.files.len() > 2_048)
            {
                chunks.push(std::mem::replace(&mut admission, new_admission()?));
                admitted_records = 0;
            }
            admitted_records += batch.records.len();
            admission.admit(batch, None).map_err(String::from)
        },
    )
    .unwrap();
    if admission.file_count() > 0 {
        chunks.push(admission);
    }
    let extracted = started.elapsed();
    let started = std::time::Instant::now();
    let shards = chunks
        .into_iter()
        .map(|chunk| {
            chunk
                .seal(SegmentSeal {
                    id: crate::index::segment::SegmentId::new(1),
                    definition: definition.as_ref(),
                    options,
                    collection_count: repository.collections().len(),
                    pool: &pool,
                    profile: None,
                    observe_inference: false,
                    declared_orders: repository.storage_factory.declared_order_store(),
                })
                .map(|built| built.index)
        })
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let finalized = started.elapsed();
    let records = shards
        .iter()
        .map(|shard| shard.descriptor().slots)
        .sum::<usize>();
    assert_eq!(records, collection.generation.records.len());
    assert!(shards.iter().all(|shard| shard.descriptor().slots <= 2_048));
    eprintln!(
        "direct shard prototype files={} records={} shards={} extract_admit={extracted:?} finalize={finalized:?} jobs={} peak_active={}",
        collection.generation.files.len(),
        records,
        shards.len(),
        jobs.len(),
        stats.peak_active_jobs,
    );
}
