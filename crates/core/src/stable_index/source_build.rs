//! Direct source-to-stable construction from native batch contributions.
//!
//! This path deliberately does not construct a production `Repository`.  It
//! reuses discovery, bounded extraction and native contribution accumulation,
//! then finalizes independent stable components directly from the completed
//! native accumulators.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use rayon::prelude::*;
use roaring::RoaringBitmap;

use crate::bigram::BigramContribution;
use crate::discovery::discover;
use crate::inference::{InferenceBuilder, InferenceConfig, InferenceMode};
use crate::inventory::{CollectionBuilder, FileInventory, SourceRevision};
use crate::model::CollectionId;
use crate::posting::{CustomPostingIndex, PostingCollection, PostingIndex};
use crate::relation::RelationContribution;
use crate::repository::{
    CollectionDefinition, CollectionState, RepositoryOptions, canonical_custom_text_key,
};
use crate::sorting::SortBuilder;
use crate::source::{PreparedSource, SourceRegistry};

use super::StablePrototype;

#[derive(Clone, Debug, Default)]
pub struct SourceBuildTimings {
    pub discovery_ns: u64,
    pub setup_ns: u64,
    pub extraction_admission_ns: u64,
    pub finalization_ns: u64,
    pub stable_component_ns: u64,
    pub source_files: usize,
    pub source_records: usize,
    pub source_bytes: u64,
    pub source_parses: u64,
    pub failed_parses: u64,
    pub source_reads: u64,
    pub source_bytes_read: u64,
    pub peak_active_jobs: usize,
    pub peak_ready_batches: usize,
    pub job_execution_ns: u64,
    pub receive_wait_ns: u64,
    pub admission_ns: u64,
}

struct PreparedFile {
    relative: PathBuf,
    revision: SourceRevision,
    created_ns: Option<i128>,
}

struct PendingStable {
    definition: CollectionDefinition,
    definition_shared: Arc<CollectionDefinition>,
    source: PreparedSource,
    id: CollectionId,
    member_files: Vec<crate::model::FileId>,
    builder: CollectionBuilder,
    postings: PostingCollection,
    custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    raw: BigramContribution,
    fallback_text: BigramContribution,
    logical_records: RoaringBitmap,
    empty_array_records: RoaringBitmap,
    sort_builder: SortBuilder,
    custom_text: HashMap<(Vec<String>, String), BigramContribution>,
    relations: Vec<RelationContribution>,
}

enum ScheduledJob {
    Files(crate::build::RepositoryFileJob),
    Large {
        collection_index: usize,
        collection: CollectionId,
        definition: Arc<CollectionDefinition>,
        source: PreparedSource,
        file: crate::model::FileId,
    },
}

pub(crate) fn from_sources(
    root: PathBuf,
    definitions: Vec<CollectionDefinition>,
    options: RepositoryOptions,
) -> Result<(StablePrototype, SourceBuildTimings), String> {
    if options.inference.mode != InferenceMode::Disabled {
        return Err("inference is explicitly unsupported by the stable-index prototype".into());
    }
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let workers = crate::build::resolve_build_workers(
        options.build_workers,
        std::thread::available_parallelism().ok(),
    )?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .thread_name(|index| format!("objekte-icss-source-{index}"))
        .build()
        .map_err(|error| error.to_string())?;
    let mut timings = SourceBuildTimings::default();

    let discovery_started = Instant::now();
    let mut files = FileInventory::new(root.clone());
    let mut selected = Vec::new();
    let registry = SourceRegistry::builtin();
    let metadata_window = workers.saturating_mul(64);
    for (collection_index, definition) in definitions.into_iter().enumerate() {
        let source = registry.resolve(&definition.format)?;
        let id = CollectionId(u32::try_from(collection_index).map_err(|_| "too many collections")?);
        if selected.iter().any(
            |(previous, _, _, _): &(CollectionDefinition, CollectionId, PreparedSource, Vec<_>)| {
                previous.name == definition.name
            },
        ) {
            return Err(format!("duplicate collection: {}", definition.name));
        }
        let paths = discover(
            &root,
            &definition.source_path,
            &definition.discovery,
            &source,
        )?;
        let mut member_files = Vec::new();
        let mut paths = paths.into_iter();
        loop {
            let window = paths.by_ref().take(metadata_window).collect::<Vec<_>>();
            if window.is_empty() {
                break;
            }
            let prepared = if workers == 1 {
                window
                    .into_iter()
                    .map(|path| prepare_discovered_file(&root, path))
                    .collect::<Vec<_>>()
            } else {
                pool.install(|| {
                    window
                        .into_par_iter()
                        .map(|path| prepare_discovered_file(&root, path))
                        .collect::<Vec<_>>()
                })
            };
            for entry in prepared {
                let entry = entry?;
                let file = files.add(entry.relative, entry.revision, entry.created_ns, id)?;
                member_files.push(file);
            }
        }
        selected.push((definition, id, source, member_files));
    }
    timings.discovery_ns = discovery_started.elapsed().as_nanos() as u64;
    timings.source_files = files.files.len();
    timings.source_bytes = files.files.iter().map(|file| file.revision.size).sum();

    let setup_started = Instant::now();
    let mut pending = Vec::new();
    for (definition, id, source, member_files) in selected {
        let builder = CollectionBuilder::new(id, definition.name.clone(), 1, files.files.len());
        let postings = PostingCollection::new(&definition.schema);
        let custom_postings = definition
            .schema
            .operations
            .iter()
            .filter(|(_, binding)| binding.operation.posting())
            .map(|(key, binding)| {
                (
                    key.clone(),
                    CustomPostingIndex {
                        index: PostingIndex::new(&definition.schema.fields[&key.0]),
                        complete: binding.operation.complete_coverage(),
                    },
                )
            })
            .collect();
        let sort_builder = SortBuilder::new(
            &definition.sort_fields,
            &definition.schema,
            &definition.sort_locale,
            definition.sort_options,
            &definition.sort_overrides,
        )?;
        let custom_text = definition
            .schema
            .custom
            .keys()
            .chain(
                definition
                    .schema
                    .operations
                    .iter()
                    .filter(|(_, binding)| {
                        binding.operation.bigram() && binding.operation.complete_coverage()
                    })
                    .map(|(key, _)| key),
            )
            .map(|key| {
                (
                    canonical_custom_text_key(&definition.schema, &key.0, &key.1),
                    BigramContribution::new(options.bigram_alphabet),
                )
            })
            .collect();
        let relations = RelationContribution::for_schema(&definition.schema);
        pending.push(PendingStable {
            definition_shared: Arc::new(definition.clone()),
            definition,
            source,
            id,
            member_files,
            builder,
            postings,
            custom_postings,
            raw: BigramContribution::new(options.bigram_alphabet),
            fallback_text: BigramContribution::new(options.bigram_alphabet),
            logical_records: RoaringBitmap::new(),
            empty_array_records: RoaringBitmap::new(),
            sort_builder,
            custom_text,
            relations,
        });
    }
    let mut scheduled = Vec::new();
    for (collection_index, state) in pending.iter().enumerate() {
        for batch in crate::build::file_batches(&state.member_files, &files) {
            if batch.len() == 1 && files.files[batch[0].0 as usize].revision.size > 8 * 1024 * 1024
            {
                scheduled.push(ScheduledJob::Large {
                    collection_index,
                    collection: state.id,
                    definition: Arc::clone(&state.definition_shared),
                    source: state.source.clone(),
                    file: batch[0],
                });
            } else {
                scheduled.push(ScheduledJob::Files(crate::build::RepositoryFileJob {
                    collection_index,
                    collection: state.id,
                    definition: Arc::clone(&state.definition_shared),
                    source: state.source.clone(),
                    files: batch.to_vec(),
                }));
            }
        }
    }
    timings.setup_ns = setup_started.elapsed().as_nanos() as u64;
    let source_files = Arc::new(
        files
            .files
            .iter()
            .map(crate::build::SourceFileInput::from_entry)
            .collect::<Vec<_>>(),
    );
    let root_shared = Arc::new(root.clone());
    let extraction_started = Instant::now();
    let mut normal = Vec::new();
    let mut schedule_stats = crate::build::BuildScheduleStats::default();
    let mut source_parses = 0u64;
    let mut failed_parses = 0u64;
    let mut admit =
        |collection_index: usize, batch: crate::build::BatchContribution| -> Result<(), String> {
            source_parses = source_parses.saturating_add(batch.record_parses);
            failed_parses = failed_parses.saturating_add(batch.failed_parses);
            let state = &mut pending[collection_index];
            let mut inference: Option<InferenceBuilder> = None;
            crate::build::CollectionAdmission {
                inventory: &mut files,
                builder: &mut state.builder,
                postings: &mut state.postings,
                inference: &mut inference,
                custom_postings: &mut state.custom_postings,
                raw: &mut state.raw,
                fallback_text: &mut state.fallback_text,
                custom_text: &mut state.custom_text,
                logical_records: &mut state.logical_records,
                empty_array_records: &mut state.empty_array_records,
                sorting: &mut state.sort_builder,
                relations: &mut state.relations,
            }
            .admit_profiled(batch, None)
        };
    for job in scheduled {
        match job {
            ScheduledJob::Files(job) => normal.push(job),
            ScheduledJob::Large {
                collection_index,
                collection,
                definition,
                source,
                file,
            } => {
                if !normal.is_empty() {
                    let stats = crate::build::process_repository_file_batches(
                        &pool,
                        Arc::clone(&root_shared),
                        &normal,
                        Arc::clone(&source_files),
                        options.bigram_alphabet,
                        InferenceConfig::default(),
                        |index, batch| admit(index, batch),
                    )?;
                    schedule_stats.absorb(stats);
                    normal.clear();
                }
                let stats = crate::build::process_framed_file_profiled(
                    &pool,
                    Arc::clone(&root_shared),
                    definition,
                    collection,
                    source,
                    file,
                    Arc::clone(&source_files),
                    options.bigram_alphabet,
                    InferenceConfig::default(),
                    false,
                    |batch| admit(collection_index, batch),
                )?;
                schedule_stats.absorb(stats);
            }
        }
    }
    if !normal.is_empty() {
        let stats = crate::build::process_repository_file_batches(
            &pool,
            Arc::clone(&root_shared),
            &normal,
            Arc::clone(&source_files),
            options.bigram_alphabet,
            InferenceConfig::default(),
            |index, batch| admit(index, batch),
        )?;
        schedule_stats.absorb(stats);
    }
    drop(admit);
    timings.extraction_admission_ns = extraction_started.elapsed().as_nanos() as u64;
    timings.source_reads = schedule_stats.source_reads;
    timings.source_bytes_read = schedule_stats.source_bytes_read;
    timings.source_parses = source_parses;
    timings.failed_parses = failed_parses;
    timings.peak_active_jobs = schedule_stats.peak_active_jobs;
    timings.peak_ready_batches = schedule_stats.peak_ready_batches;
    timings.job_execution_ns = schedule_stats.job_execution_ns;
    timings.receive_wait_ns = schedule_stats.receive_wait_ns;
    timings.admission_ns = schedule_stats.admission_ns;

    let finalization_started = Instant::now();
    let mut relation_parts = Vec::with_capacity(pending.len());
    let mut collections = Vec::with_capacity(pending.len());
    for state in pending {
        let PendingStable {
            definition,
            definition_shared: _,
            source,
            id,
            member_files: _,
            builder,
            mut postings,
            custom_postings,
            raw,
            fallback_text,
            logical_records,
            empty_array_records,
            sort_builder,
            custom_text,
            relations,
        } = state;
        let generation = builder.finish(&files)?;
        let all_logical = logical_records.len() == generation.records.len() as u64;
        postings.finish_stats();
        postings.validate_eligible(generation.records.len(), logical_records.len() as usize)?;
        if !all_logical {
            postings.validate_domain(&logical_records)?;
        }
        if !(&empty_array_records - &logical_records).is_empty() {
            return Err("empty-array structural bitmap exceeds logical records".into());
        }
        let mut custom_postings = custom_postings;
        for custom in custom_postings.values_mut() {
            custom.index.finish_stats();
            custom
                .index
                .validate_eligible(generation.records.len(), logical_records.len() as usize)?;
            if !all_logical {
                custom.index.validate_domain(&logical_records)?;
            }
        }
        let raw = raw.into_index_in_pool(&pool)?;
        raw.validate(generation.records.len())?;
        if !all_logical {
            raw.validate_domain(&logical_records)?;
        }
        let fallback_text = fallback_text.into_index_in_pool(&pool)?;
        fallback_text.validate(generation.records.len())?;
        let custom_text = custom_text
            .into_iter()
            .map(|(key, index)| index.into_index_in_pool(&pool).map(|index| (key, index)))
            .collect::<Result<HashMap<_, _>, _>>()?;
        for index in custom_text.values() {
            index.validate(generation.records.len())?;
            if !all_logical {
                index.validate_domain(&logical_records)?;
            }
        }
        let sorting = sort_builder.finish(&generation)?;
        let total_file_bytes = generation
            .files
            .iter()
            .map(|file| files.files[file.0 as usize].revision.size)
            .sum();
        let collection = CollectionState::from_fresh_parts(
            definition,
            source,
            generation,
            postings,
            custom_postings,
            raw,
            fallback_text,
            logical_records,
            empty_array_records,
            custom_text,
            sorting,
            total_file_bytes,
        );
        relation_parts.push(relations);
        collections.push(collection);
        let _ = id;
    }
    for (index, contributions) in relation_parts.into_iter().enumerate() {
        let source = collections[index].generation.collection;
        let mut resolved = Vec::with_capacity(contributions.len());
        for (ordinal, contribution) in contributions.into_iter().enumerate() {
            resolved.push(contribution.resolve(
                source,
                u32::try_from(ordinal).map_err(|_| "relation ordinal overflow")?,
                &collections,
            )?);
        }
        collections[index].relations = resolved;
    }
    timings.finalization_ns = finalization_started.elapsed().as_nanos() as u64;
    timings.source_records = collections
        .iter()
        .map(|collection| collection.generation.records.len())
        .sum();

    let stable_started = Instant::now();
    let mut prototype = super::build::from_collections(
        &files,
        &collections,
        &root,
        options.build_workers.map(NonZeroUsize::get),
    )?;
    timings.stable_component_ns = stable_started.elapsed().as_nanos() as u64;
    prototype.build_stage_ns = std::collections::BTreeMap::from([
        ("discovery".into(), timings.discovery_ns),
        ("setup".into(), timings.setup_ns),
        (
            "extraction_admission".into(),
            timings.extraction_admission_ns,
        ),
        ("finalization".into(), timings.finalization_ns),
        ("stable_component".into(), timings.stable_component_ns),
    ]);
    Ok((prototype, timings))
}

fn prepare_discovered_file(root: &Path, path: PathBuf) -> Result<PreparedFile, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|error| error.to_string())?
        .to_owned();
    let metadata = std::fs::metadata(&path).map_err(|error| error.to_string())?;
    let revision = SourceRevision::from_metadata(&metadata)?;
    let created_ns = metadata
        .created()
        .ok()
        .and_then(|time| time.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as i128);
    Ok(PreparedFile {
        relative,
        revision,
        created_ns,
    })
}
