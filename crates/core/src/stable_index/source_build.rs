//! Direct source-to-stable construction from native batch contributions.
//!
//! This path deliberately does not construct a production `Repository`.  It
//! reuses discovery, bounded extraction and native contribution accumulation,
//! then moves independent owner state directly into owned stable components.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use rayon::prelude::*;
use roaring::RoaringBitmap;

use crate::bigram::BigramContribution;
use crate::discovery::discover;
use crate::inference::{InferenceConfig, InferenceMode};
use crate::inventory::{
    CollectionBuilder, DiagnosticRef, FileInventory, RecordClass, RecordEntry, SourceRevision,
};
use crate::model::{CollectionId, FieldPath, RecordId, RecordSlot};
use crate::posting::{CustomPostingIndex, PostingCollection, PostingIndex};
use crate::relation::RelationContribution;
use crate::repository::{CollectionDefinition, RepositoryOptions, canonical_custom_text_key};
use crate::sorting::{SortBuilder, SortKey};
use crate::source::{PreparedSource, SourceRegistry};

use super::StablePrototype;
use super::build::{self, OwnedCollection};

/// Estimated transient preparation target for one extraction wave.
const PREPARATION_TARGET_BYTES: u64 = 64 * 1024 * 1024;
const MIN_PREPARATION_WEIGHT: u64 = 16 * 1024;

#[derive(Clone, Debug, Default)]
pub struct SourceBuildTimings {
    pub discovery_ns: u64,
    pub setup_ns: u64,
    pub extraction_admission_ns: u64,
    pub owner_merge_ns: u64,
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
    pub preparation_waves: u64,
    pub oversized_preparation_jobs: u64,
    pub job_execution_ns: Option<u64>,
    pub receive_wait_ns: Option<u64>,
    pub admission_ns: Option<u64>,
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

struct FinalizedCollection {
    definition: CollectionDefinition,
    id: CollectionId,
    generation: u64,
    records: Arc<Vec<RecordEntry>>,
    logical_records: RoaringBitmap,
    empty_array_records: RoaringBitmap,
    postings: PostingCollection,
    custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    raw: crate::bigram::BigramIndex,
    fallback_text: crate::bigram::BigramIndex,
    custom_text: HashMap<(Vec<String>, String), crate::bigram::BigramIndex>,
    sorting: crate::sorting::SortCollection,
    relation_contributions: Vec<RelationContribution>,
    relations: Vec<crate::relation::RelationIndex>,
    total_file_bytes: u64,
}

#[derive(Default)]
struct OwnerWave {
    postings: HashMap<Vec<String>, Vec<(u32, PostingIndex)>>,
    custom_postings: HashMap<(Vec<String>, String), Vec<(u32, CustomPostingIndex)>>,
    raw: Vec<(u32, BigramContribution)>,
    fallback_text: Vec<(u32, BigramContribution)>,
    custom_text: HashMap<(Vec<String>, String), Vec<(u32, BigramContribution)>>,
    sort_keys: HashMap<FieldPath, Vec<(u32, u32, Vec<SortKey>)>>,
    relations: Vec<Vec<(u32, u32, RelationContribution)>>,
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
    let mut owner_waves = (0..pending.len())
        .map(|_| OwnerWave::default())
        .collect::<Vec<_>>();
    let extraction_started = Instant::now();
    let mut schedule_stats = crate::build::BuildScheduleStats::default();
    let mut source_parses = 0u64;
    let mut failed_parses = 0u64;
    let profile_build = options.detailed_profiling_enabled();
    let max_wave_jobs = workers.saturating_mul(2).max(1);
    while !scheduled.is_empty() {
        let (wave_len, oversized) = wave_end(
            &scheduled,
            max_wave_jobs,
            &source_files,
            PREPARATION_TARGET_BYTES,
        );
        timings.preparation_waves = timings.preparation_waves.saturating_add(1);
        if oversized {
            timings.oversized_preparation_jobs =
                timings.oversized_preparation_jobs.saturating_add(1);
        }
        let wave = scheduled.drain(0..wave_len).collect::<Vec<_>>();
        let stats = {
            let mut admit = |collection_index: usize,
                             batch: crate::build::BatchContribution|
             -> Result<(), String> {
                let parts = batch.into_parts();
                source_parses = source_parses.saturating_add(parts.record_parses);
                failed_parses = failed_parses.saturating_add(parts.failed_parses);
                admit_parts(
                    &mut pending[collection_index],
                    &mut files,
                    &mut owner_waves[collection_index],
                    parts,
                )
            };
            if matches!(wave.first(), Some(ScheduledJob::Large { .. })) {
                let ScheduledJob::Large {
                    collection_index,
                    collection,
                    definition,
                    source,
                    file,
                } = wave.into_iter().next().expect("non-empty wave")
                else {
                    unreachable!("Large wave");
                };
                crate::build::process_framed_file_profiled(
                    &pool,
                    Arc::clone(&root_shared),
                    definition,
                    collection,
                    source,
                    file,
                    Arc::clone(&source_files),
                    options.bigram_alphabet,
                    InferenceConfig::default(),
                    profile_build,
                    |batch| admit(collection_index, batch),
                )?
            } else {
                let jobs = wave
                    .into_iter()
                    .map(|job| match job {
                        ScheduledJob::Files(job) => job,
                        ScheduledJob::Large { .. } => unreachable!("mixed wave"),
                    })
                    .collect::<Vec<_>>();
                crate::build::process_repository_file_batches_profiled(
                    &pool,
                    Arc::clone(&root_shared),
                    &jobs,
                    Arc::clone(&source_files),
                    options.bigram_alphabet,
                    InferenceConfig::default(),
                    profile_build,
                    |index, batch| admit(index, batch),
                )?
            }
        };
        schedule_stats.absorb(stats);
        let merge_started = Instant::now();
        for (state, wave) in pending.iter_mut().zip(owner_waves.iter_mut()) {
            merge_owner_wave(state, wave, &pool)?;
        }
        timings.owner_merge_ns = timings
            .owner_merge_ns
            .saturating_add(merge_started.elapsed().as_nanos() as u64);
    }
    timings.extraction_admission_ns = extraction_started.elapsed().as_nanos() as u64;
    timings.source_reads = schedule_stats.source_reads;
    timings.source_bytes_read = schedule_stats.source_bytes_read;
    timings.source_parses = source_parses;
    timings.failed_parses = failed_parses;
    timings.peak_active_jobs = schedule_stats.peak_active_jobs;
    timings.peak_ready_batches = schedule_stats.peak_ready_batches;
    timings.job_execution_ns = profile_build.then_some(schedule_stats.job_execution_ns);
    timings.receive_wait_ns = profile_build.then_some(schedule_stats.receive_wait_ns);
    timings.admission_ns = profile_build.then_some(schedule_stats.admission_ns);

    let finalization_started = Instant::now();
    let mut target_names = BTreeSet::new();
    for state in &pending {
        for relation in state.definition.schema.relations().values() {
            target_names.insert(relation.target_collection.clone());
        }
    }
    let mut finalized = Vec::with_capacity(pending.len());
    for state in pending {
        let PendingStable {
            definition,
            definition_shared: _,
            source: _source,
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
        let sorting = sort_builder.finish_keys_only(generation.records.len())?;
        let total_file_bytes = generation
            .files
            .iter()
            .map(|file| files.files[file.0 as usize].revision.size)
            .sum();
        finalized.push(FinalizedCollection {
            definition,
            id,
            generation: generation.generation,
            records: Arc::new(generation.records),
            logical_records,
            empty_array_records,
            postings,
            custom_postings,
            raw,
            fallback_text,
            custom_text,
            sorting,
            relation_contributions: relations,
            relations: Vec::new(),
            total_file_bytes,
        });
    }

    let mut target_lookups =
        HashMap::<String, (CollectionId, HashMap<RecordId, RecordSlot>)>::new();
    for collection in &finalized {
        if target_names.contains(&collection.definition.name) {
            let lookup = collection
                .records
                .iter()
                .map(|record| (record.id.clone(), record.slot))
                .collect::<HashMap<_, _>>();
            target_lookups.insert(collection.definition.name.clone(), (collection.id, lookup));
        }
    }
    for collection in &mut finalized {
        let mut resolved = Vec::with_capacity(collection.relation_contributions.len());
        for (ordinal, contribution) in collection.relation_contributions.drain(..).enumerate() {
            let target_name = contribution.target_collection().to_string();
            let (target_collection, lookup) = target_lookups
                .get(&target_name)
                .ok_or_else(|| format!("relation target collection missing: {target_name}"))?;
            resolved.push(contribution.resolve_with_lookup(
                collection.id,
                u32::try_from(ordinal).map_err(|_| "relation ordinal overflow")?,
                *target_collection,
                lookup,
            )?);
        }
        collection.relations = resolved;
    }
    timings.finalization_ns = finalization_started.elapsed().as_nanos() as u64;
    timings.source_records = finalized
        .iter()
        .map(|collection| collection.records.len())
        .sum();

    let collections = finalized
        .into_iter()
        .map(|collection| OwnedCollection {
            definition: collection.definition,
            id: collection.id,
            generation: collection.generation,
            records: collection.records,
            logical_records: collection.logical_records,
            empty_array_records: collection.empty_array_records,
            postings: collection.postings,
            custom_postings: collection.custom_postings,
            raw: collection.raw,
            fallback_text: collection.fallback_text,
            custom_text: collection.custom_text,
            sorting: collection.sorting,
            relations: collection.relations,
            total_file_bytes: collection.total_file_bytes,
        })
        .collect::<Vec<_>>();

    let stable_started = Instant::now();
    let mut prototype = build::from_owned(files, collections, &root, &pool)?;
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

fn wave_end(
    scheduled: &[ScheduledJob],
    max_jobs: usize,
    source_files: &[crate::build::SourceFileInput],
    target_bytes: u64,
) -> (usize, bool) {
    let Some(first) = scheduled.first() else {
        return (0, false);
    };
    if matches!(first, ScheduledJob::Large { .. }) {
        return (1, scheduled_weight(first, source_files) > target_bytes);
    }
    let mut end = 0usize;
    let mut bytes = 0u64;
    let mut oversized = false;
    while end < scheduled.len() && end < max_jobs {
        if matches!(scheduled[end], ScheduledJob::Large { .. }) {
            break;
        }
        let weight = scheduled_weight(&scheduled[end], source_files);
        if end > 0 && bytes.saturating_add(weight) > target_bytes {
            break;
        }
        if end == 0 && weight > target_bytes {
            oversized = true;
        }
        bytes = bytes.saturating_add(weight);
        end += 1;
        if bytes >= target_bytes {
            break;
        }
    }
    (end.max(1), oversized)
}

fn scheduled_weight(job: &ScheduledJob, source_files: &[crate::build::SourceFileInput]) -> u64 {
    match job {
        ScheduledJob::Files(job) => job.files.iter().fold(0u64, |total, id| {
            let file = &source_files[id.0 as usize];
            total.saturating_add(file.revision.size.max(MIN_PREPARATION_WEIGHT))
        }),
        ScheduledJob::Large { file, .. } => source_files[file.0 as usize].revision.size,
    }
}

fn merge_owner_wave(
    state: &mut PendingStable,
    wave: &mut OwnerWave,
    pool: &rayon::ThreadPool,
) -> Result<(), String> {
    if !wave.postings.is_empty() {
        let mut owners = std::mem::take(&mut state.postings.fields)
            .into_iter()
            .collect::<Vec<_>>();
        let mut locals = owners
            .iter()
            .map(|(path, _)| wave.postings.remove(path))
            .collect::<Vec<Option<Vec<(u32, PostingIndex)>>>>();
        if !wave.postings.is_empty() {
            return Err("posting wave field coverage mismatch".into());
        }
        pool.install(|| {
            owners
                .par_iter_mut()
                .zip(locals.par_iter_mut())
                .try_for_each(|((_, owner), local)| {
                    let Some(fragments) = local.take() else {
                        return Err::<(), String>("posting wave field missing".into());
                    };
                    for (base, fragment) in fragments {
                        owner.merge_shifted(fragment, base)?;
                    }
                    Ok(())
                })
        })?;
        state.postings.fields = owners.into_iter().collect();
    }

    if !wave.custom_postings.is_empty() {
        let mut owners = std::mem::take(&mut state.custom_postings)
            .into_iter()
            .collect::<Vec<_>>();
        let mut locals = owners
            .iter()
            .map(|(key, _)| wave.custom_postings.remove(key))
            .collect::<Vec<Option<Vec<(u32, CustomPostingIndex)>>>>();
        if !wave.custom_postings.is_empty() {
            return Err("custom posting wave coverage mismatch".into());
        }
        pool.install(|| {
            owners
                .par_iter_mut()
                .zip(locals.par_iter_mut())
                .try_for_each(|((_, owner), local)| {
                    let Some(fragments) = local.take() else {
                        return Err::<(), String>("custom posting wave key missing".into());
                    };
                    for (base, fragment) in fragments {
                        owner.index.merge_shifted(fragment.index, base)?;
                        owner.complete &= fragment.complete;
                    }
                    Ok(())
                })
        })?;
        state.custom_postings = owners.into_iter().collect();
    }

    let raw = std::mem::take(&mut wave.raw);
    let fallback = std::mem::take(&mut wave.fallback_text);
    if !raw.is_empty() || !fallback.is_empty() {
        let (raw_result, fallback_result) = pool.join(
            || merge_bigram_fragments(&mut state.raw, raw),
            || merge_bigram_fragments(&mut state.fallback_text, fallback),
        );
        raw_result?;
        fallback_result?;
    }

    if !wave.custom_text.is_empty() {
        let mut owners = std::mem::take(&mut state.custom_text)
            .into_iter()
            .collect::<Vec<_>>();
        let mut locals = owners
            .iter()
            .map(|(key, _)| wave.custom_text.remove(key))
            .collect::<Vec<Option<Vec<(u32, BigramContribution)>>>>();
        if !wave.custom_text.is_empty() {
            return Err("custom bigram wave coverage mismatch".into());
        }
        pool.install(|| {
            owners
                .par_iter_mut()
                .zip(locals.par_iter_mut())
                .try_for_each(|((_, owner), local)| {
                    let Some(fragments) = local.take() else {
                        return Err::<(), String>("custom bigram wave key missing".into());
                    };
                    for (base, fragment) in fragments {
                        owner.merge_shifted(fragment, base)?;
                    }
                    Ok(())
                })
        })?;
        state.custom_text = owners.into_iter().collect();
    }

    if !wave.sort_keys.is_empty() {
        state
            .sort_builder
            .append_wave_in_pool(std::mem::take(&mut wave.sort_keys), pool)?;
    }

    if !wave.relations.is_empty() {
        let mut locals = std::mem::take(&mut wave.relations)
            .into_iter()
            .map(Some)
            .collect::<Vec<_>>();
        if locals.len() != state.relations.len() {
            return Err("relation wave coverage mismatch".into());
        }
        pool.install(|| {
            state
                .relations
                .par_iter_mut()
                .zip(locals.par_iter_mut())
                .try_for_each(|(owner, local)| {
                    let Some(fragments) = local.take() else {
                        return Err::<(), String>("relation wave owner missing".into());
                    };
                    for (base, count, fragment) in fragments {
                        owner.merge_shifted(fragment, base, count)?;
                    }
                    Ok(())
                })
        })?;
    }
    Ok(())
}

fn merge_bigram_fragments(
    owner: &mut BigramContribution,
    fragments: Vec<(u32, BigramContribution)>,
) -> Result<(), String> {
    for (base, fragment) in fragments {
        owner.merge_shifted(fragment, base)?;
    }
    Ok(())
}

fn admit_parts(
    state: &mut PendingStable,
    inventory: &mut FileInventory,
    wave: &mut OwnerWave,
    parts: crate::build::ContributionParts,
) -> Result<(), String> {
    if parts.inference.is_some() {
        return Err("inference is unsupported by the stable prototype".into());
    }
    let crate::build::ContributionParts {
        files,
        file_fingerprints: _,
        records,
        postings,
        inference: _,
        custom_postings,
        raw,
        fallback_text,
        custom_text,
        logical_records,
        empty_array_records,
        sort_keys,
        relations,
        record_parses: _,
        failed_parses: _,
        source_reads: _,
        source_bytes_read: _,
        profile: _,
    } = parts;
    let base = state.builder.next_slot()?.0;
    let count = u32::try_from(records.len()).map_err(|_| "too many batch records")?;
    base.checked_add(count).ok_or("record slot overflow")?;
    let mut records = records.into_iter();
    for batch_file in files {
        let file_id = batch_file.entry.id;
        let path = inventory.root.join(&batch_file.entry.relative_path);
        if batch_file.first_range {
            state.builder.include_file(file_id)?;
        } else if !state.builder.contains_file(file_id) {
            return Err("later record range precedes first file range".into());
        }
        let mut diagnostic_refs = Vec::with_capacity(batch_file.entry.diagnostics.len());
        for mut diagnostic in batch_file.entry.diagnostics {
            if let Some(local_slot) = diagnostic.record {
                diagnostic.record = Some(RecordSlot(
                    base.checked_add(local_slot.0)
                        .ok_or("diagnostic record slot overflow")?,
                ));
            }
            diagnostic_refs.push(inventory.record_diagnostic(file_id, diagnostic)?);
        }
        let file = &mut inventory.files[file_id.0 as usize];
        file.status = file.status.max(batch_file.entry.status);
        file.omitted_diagnostics = file
            .omitted_diagnostics
            .saturating_add(batch_file.entry.omitted_diagnostics);
        for expected_slot in batch_file.records {
            let local = records.next().ok_or("batch record range out of bounds")?;
            if local.slot.0 as usize != expected_slot {
                return Err("batch record order mismatch".into());
            }
            if local.file != file_id {
                return Err("batch record file ownership mismatch".into());
            }
            let actual = if local.class == RecordClass::Logical {
                state
                    .builder
                    .add_record(file, local.id, local.range.start, local.range.end)
                    .map_err(|error| {
                        format!(
                            "{}:{}..{}: {error}",
                            path.display(),
                            local.range.start,
                            local.range.end,
                        )
                    })?
            } else {
                let reference = match local.diagnostic_ref.stored_index() {
                    Some(index) => *diagnostic_refs
                        .get(index)
                        .ok_or("batch diagnostic reference out of bounds")?,
                    None => DiagnosticRef(u32::MAX),
                };
                state
                    .builder
                    .add_raw_fallback(file, local.id, local.range, reference)?
            };
            if actual.0 != base + local.slot.0 {
                return Err("batch record slot mapping mismatch".into());
            }
        }
    }
    if records.next().is_some() {
        return Err("unadmitted batch records".into());
    }
    if state.builder.next_slot()?.0 != base + count {
        return Err("incomplete batch record admission".into());
    }

    for (path, index) in postings.fields {
        wave.postings.entry(path).or_default().push((base, index));
    }
    for (key, index) in custom_postings {
        wave.custom_postings
            .entry(key)
            .or_default()
            .push((base, index));
    }
    wave.raw.push((base, raw));
    wave.fallback_text.push((base, fallback_text));
    for (key, index) in custom_text {
        wave.custom_text.entry(key).or_default().push((base, index));
    }
    for (path, keys) in sort_keys {
        wave.sort_keys
            .entry(path)
            .or_default()
            .push((base, count, keys));
    }
    if relations.len() != state.relations.len() {
        return Err("relation batch coverage mismatch".into());
    }
    if wave.relations.is_empty() && !relations.is_empty() {
        wave.relations.resize_with(relations.len(), Vec::new);
    }
    for (ordinal, contribution) in relations.into_iter().enumerate() {
        wave.relations[ordinal].push((base, count, contribution));
    }
    crate::inventory::append_shifted_bitmap(&mut state.logical_records, &logical_records, base)?;
    crate::inventory::append_shifted_bitmap(
        &mut state.empty_array_records,
        &empty_array_records,
        base,
    )?;
    Ok(())
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
