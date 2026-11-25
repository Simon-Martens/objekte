//! Correctly configured empty repository state.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use roaring::RoaringBitmap;

use crate::construction::batch::canonical_custom_text_key;
use crate::index::bigram::BigramContribution;
use crate::index::memory::inference::InferenceBuilder;
use crate::index::posting::{CustomPostingIndex, PostingCollection, PostingIndex};
use crate::index::relation::RelationContribution;
use crate::index::sorting::SortBuilder;
use crate::index::storage::StorageFactory;
use crate::inference::InferenceBuildStats;
use crate::inventory::{
    ActiveRecords, CollectionBuilder, CollectionGeneration, FileInventory, RecordClass,
};
use crate::lifecycle::{IndexError, IndexRevision};
use crate::model::{CollectionId, RecordId, RecordSlot};
use crate::repository::configuration::{
    CollectionDeclaration, CollectionDefinition, FileSourceConfig, RepositoryOptions,
};
use crate::repository::state::{CollectionState, CollectionStats, GlobalInferenceSummary};
use crate::repository::statistics::{BuildProfileStats, StartupStats, build_profile_rss_bytes};
use crate::repository::{Repository, next_repository_instance};
use crate::source::registry::PreparedSource;
use crate::source::registry::SourceRegistry;

struct EmptyCollectionInput {
    declaration: CollectionDeclaration,
    file_source: Option<FileSourceConfig>,
    file_definition: Option<CollectionDefinition>,
    source: Option<PreparedSource>,
}

pub(crate) fn finish_fresh_fingerprints(
    files: &FileInventory,
    mut bindings: Vec<Vec<(CollectionId, [u8; 32])>>,
) -> Result<Vec<Option<[u8; 32]>>, String> {
    if bindings.len() != files.files.len() {
        return Err("fresh digest inventory length mismatch".into());
    }
    files
        .files
        .iter()
        .zip(&mut bindings)
        .map(|(file, reads)| {
            reads.sort_unstable_by_key(|(collection, _)| collection.0);
            let mut previous = None;
            let mut observed = 0usize;
            let mut consensus = None;
            for &(collection, digest) in reads.iter() {
                if !file.collections.contains(&collection) {
                    return Err("fresh digest belongs to an ineligible collection".into());
                }
                if consensus.is_some_and(|expected| expected != digest) {
                    return Err("source content changed between collection build reads".into());
                }
                consensus = Some(digest);
                if previous != Some(collection) {
                    previous = Some(collection);
                    observed += 1;
                }
            }
            if observed == file.collections.len() {
                Ok(consensus)
            } else {
                Ok(None)
            }
        })
        .collect()
}

pub(crate) fn initialize_empty(
    root: PathBuf,
    definitions: Vec<CollectionDefinition>,
    options: RepositoryOptions,
    sources: &SourceRegistry,
    pool: Option<&rayon::ThreadPool>,
    storage_factory: Arc<dyn StorageFactory>,
) -> Result<Repository, String> {
    let configuration_identity = crate::repository::configuration::identity::configuration_hash(
        &definitions,
        options,
        sources,
    )?;
    let inputs = definitions
        .into_iter()
        .map(|definition| {
            let source = sources.resolve(&definition.format)?;
            Ok(EmptyCollectionInput {
                declaration: definition.declaration(),
                file_source: Some(definition.file_source()),
                file_definition: Some(definition),
                source: Some(source),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    initialize_empty_configured(
        Some(root),
        inputs,
        options,
        sources,
        configuration_identity,
        false,
        pool,
        storage_factory,
    )
}

pub(crate) fn initialize_empty_programmatic(
    declarations: Vec<CollectionDeclaration>,
    options: RepositoryOptions,
    registrations: &[(CollectionId, String, u32, u32)],
    catalog_root: Option<PathBuf>,
    file_sources: Vec<Option<FileSourceConfig>>,
    sources: &SourceRegistry,
    storage_factory: Arc<dyn StorageFactory>,
) -> Result<Repository, String> {
    if declarations.len() != file_sources.len() {
        return Err("programmatic file-source count mismatch".into());
    }
    let configuration_identity =
        crate::repository::configuration::identity::programmatic_configuration_hash(
            &declarations,
            options,
            registrations,
            catalog_root.is_some(),
            &file_sources,
            sources,
        )?;
    let inputs = declarations
        .into_iter()
        .zip(file_sources)
        .map(|(declaration, file_source)| {
            let source = file_source
                .as_ref()
                .map(|file| sources.resolve(&file.format))
                .transpose()?;
            let file_definition = file_source
                .as_ref()
                .map(|file| CollectionDefinition::from_parts(declaration.clone(), file.clone()));
            Ok(EmptyCollectionInput {
                declaration,
                file_source,
                file_definition,
                source,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    initialize_empty_configured(
        catalog_root,
        inputs,
        options,
        sources,
        configuration_identity,
        true,
        None,
        storage_factory,
    )
}

fn initialize_empty_configured(
    root: Option<PathBuf>,
    inputs: Vec<EmptyCollectionInput>,
    options: RepositoryOptions,
    sources: &SourceRegistry,
    configuration_identity: [u8; 32],
    programmatic: bool,
    pool: Option<&rayon::ThreadPool>,
    storage_factory: Arc<dyn StorageFactory>,
) -> Result<Repository, String> {
    let build_workers = crate::construction::scheduling::resolve_build_workers(
        options.build_workers,
        std::thread::available_parallelism().ok(),
    )?;
    let files = Arc::new(match root {
        Some(root) => FileInventory::new(root),
        None => FileInventory::without_root(),
    });
    let fallback_pool = pool
        .is_none()
        .then(|| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .thread_name(|index| format!("objekte-empty-{index}"))
                .build()
                .map_err(|error| error.to_string())
        })
        .transpose()?;
    let empty_pool = pool
        .or(fallback_pool.as_ref())
        .expect("empty construction pool is always available");
    let collection_count = inputs.len();
    let mut collections = Vec::with_capacity(collection_count);
    let mut pending_relations = Vec::with_capacity(collection_count);
    for (number, input) in inputs.into_iter().enumerate() {
        let definition = input.declaration;
        if collections
            .iter()
            .any(|state: &CollectionState| state.definition.name == definition.name)
        {
            return Err(format!("duplicate collection: {}", definition.name));
        }
        let id = CollectionId(u32::try_from(number).map_err(|_| "too many collections")?);
        let generation =
            CollectionBuilder::new(id, definition.name.clone(), 1, 0).finish(&files)?;
        let postings = PostingCollection::new(&definition.schema);
        let inference = InferenceBuilder::for_collection(options.inference, collection_count)
            .map(|builder| builder.finish(&RoaringBitmap::new(), 0, &definition.schema))
            .transpose()?;
        let custom_postings: HashMap<_, _> = definition
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
        let raw =
            BigramContribution::new(options.bigram_alphabet).into_index_in_pool(empty_pool)?;
        let fallback_text =
            BigramContribution::new(options.bigram_alphabet).into_index_in_pool(empty_pool)?;
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
                BigramContribution::new(options.bigram_alphabet)
                    .into_index_in_pool(empty_pool)
                    .map(|index| {
                        (
                            canonical_custom_text_key(&definition.schema, &key.0, &key.1),
                            index,
                        )
                    })
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        let logical_records = RoaringBitmap::new();
        let empty_array_records = RoaringBitmap::new();
        let sort_builder = SortBuilder::new(
            &definition.sort_fields,
            &definition.schema,
            &definition.sort_locale,
            definition.sort_options,
            &definition.sort_overrides,
        )?;
        let sorting = sort_builder.finish(&generation)?;
        let text_bindings =
            crate::query_access::catalog::TextBindingCatalog::new(&definition.schema);
        let index_family_bytes = crate::lifecycle::index_family_total(
            &raw,
            &fallback_text,
            &custom_text,
            &postings,
            &sorting,
        );
        pending_relations.push(RelationContribution::for_schema(&definition.schema));
        collections.push(CollectionState {
            repository_instance: 0,
            definition,
            file_source: input.file_source,
            file_definition: input.file_definition,
            source: input.source,
            generation,
            postings,
            inference,
            global_inference: None,
            custom_postings,
            raw,
            fallback_text,
            logical_records,
            empty_array_records,
            custom_text,
            text_bindings,
            sorting,
            index_family_bytes,
            id_lookup: HashMap::new(),
            identity_order: Vec::new(),
            stats: CollectionStats {
                file_count: 0,
                record_count: 0,
                total_file_bytes: 0,
            },
            relations: Vec::new(),
        });
    }
    for (source_index, contributions) in pending_relations.into_iter().enumerate() {
        let source = collections[source_index].generation.collection;
        let indexes = contributions
            .into_iter()
            .enumerate()
            .map(|(ordinal, contribution)| {
                contribution.resolve(
                    source,
                    u32::try_from(ordinal).map_err(|_| "too many relations")?,
                    &collections,
                )
            })
            .collect::<Result<Vec<_>, String>>()?;
        collections[source_index].relations = indexes;
    }
    crate::index::relation::validate_indexes(&collections)?;
    let workers = Repository::create_workers(options.query_workers)?;
    let catalog = storage_factory.file_catalog(
        &files.catalog_base(Vec::new(), configuration_identity, collections.len()),
        crate::inventory::FileVersion::INITIAL,
    )?;
    let active = (0..collections.len())
        .map(|_| ActiveRecords::all(0))
        .collect::<Result<Vec<_>, _>>()?;
    let segments = vec![Arc::new(Vec::new()); collections.len()];
    let instance_id = next_repository_instance();
    for collection in &mut collections {
        collection.repository_instance = instance_id;
    }
    let catalog_registered = !programmatic || files.root().is_some();
    Ok(Repository {
        storage_factory,
        files,
        base_fingerprints: Vec::new(),
        collections,
        active,
        segments,
        catalog,
        configuration_identity,
        programmatic,
        catalog_registered,
        record_registrations: Arc::new(Vec::new()),
        revision: IndexRevision::INITIAL,
        sources: sources.clone(),
        build_parses: 0,
        build_failed_parses: 0,
        instance_id,
        options,
        startup: StartupStats {
            build_workers,
            ..StartupStats::default()
        },
        workers,
        verified: Mutex::new(crate::query_results::verified::VerifiedCache::new(
            options.verified_cache_bytes,
        )),
        stream_budget: Default::default(),
        inferred_orders: Mutex::new(crate::index::sorting::InferredOrderCache::new(
            crate::index::sorting::INFERRED_ORDER_CACHE_BYTES,
        )),
    })
}

#[cfg(test)]
mod programmatic_tests {
    use super::*;
    use crate::lifecycle::Index;
    use crate::schema::Schema;

    #[test]
    fn fileless_empty_index_publishes_without_a_root_or_reader() {
        let declaration = CollectionDeclaration::new("entries", Schema::default());
        let repository = initialize_empty_programmatic(
            vec![declaration],
            RepositoryOptions::default(),
            &[(CollectionId(0), "application.entries".into(), 1, 1)],
            None,
            vec![None],
            &crate::source::test_support::fixture_registry(),
            crate::index::storage::default_factory(),
        )
        .unwrap();
        assert!(repository.files.root().is_none());
        assert!(repository.collections[0].source.is_none());
        let expected_identity = repository.configuration_identity;
        let index = Index::from_repository(repository).unwrap();
        let view = index.view().unwrap();
        assert_eq!(view.configuration_identity(), expected_identity);
        assert_eq!(view.collection_declarations().count(), 1);
        assert_eq!(view.index_observation().unwrap().records, 0);
    }
}
struct SegmentMetadata {
    generation: CollectionGeneration,
    id_lookup: HashMap<RecordId, RecordSlot>,
    identity_order: Vec<RecordSlot>,
    stats: CollectionStats,
    logical_records: RoaringBitmap,
    empty_array_records: RoaringBitmap,
}

fn segment_metadata_generation(
    collection: CollectionId,
    name: &str,
    segments: &[Arc<crate::index::segment::PublishedSegment>],
    files: &mut FileInventory,
    relations: &mut [RelationContribution],
) -> Result<SegmentMetadata, String> {
    let mut builder = CollectionBuilder::new(collection, name.to_owned(), 1, files.files.len());
    let mut file_set = HashSet::new();
    let mut record_count = 0usize;
    let mut logical_records = RoaringBitmap::new();
    let mut empty_array_records = RoaringBitmap::new();
    for segment in segments {
        let base = u32::try_from(record_count).map_err(|_| "too many segment records")?;
        let segment_count =
            u32::try_from(segment.index.records.len()).map_err(|_| "too many segment records")?;
        if relations.len() != segment.index.relations.len() {
            return Err("segment relation coverage mismatch".into());
        }
        for (global, local) in relations
            .iter_mut()
            .zip(segment.index.relations.iter().cloned())
        {
            global.merge_shifted(local, base, segment_count)?;
        }
        for slot in &segment.index.logical_records {
            logical_records.insert(
                base.checked_add(slot)
                    .ok_or("logical record slot overflow")?,
            );
        }
        for slot in &segment.index.empty_array_records {
            empty_array_records.insert(
                base.checked_add(slot)
                    .ok_or("empty-array record slot overflow")?,
            );
        }
        for segment_file in &segment.index.files {
            if file_set.insert(segment_file.entry.id) {
                builder.include_file(segment_file.entry.id)?;
            }
        }
        for record in &segment.index.records {
            let locator = segment
                .index
                .backing
                .file_locator(record.slot)
                .ok_or("file-backed segment metadata has no file locator")?;
            let range = locator
                .range
                .ok_or("file-backed record has no byte range")?;
            let file = files
                .files
                .get_mut(locator.file.0 as usize)
                .ok_or("segment metadata file missing")?;
            if record.class == RecordClass::Logical {
                builder.add_record(file, record.id.clone(), range.start, range.end)?;
            } else {
                let segment_file = segment
                    .index
                    .file(locator.file)
                    .ok_or("fallback segment file missing")?;
                let Some(local) = record.diagnostic_ref.stored_index() else {
                    builder.add_raw_fallback(
                        file,
                        record.id.clone(),
                        range,
                        record.diagnostic_ref,
                    )?;
                    record_count += 1;
                    continue;
                };
                let diagnostic = segment_file
                    .entry
                    .diagnostics
                    .get(local)
                    .ok_or("fallback diagnostic missing")?
                    .clone();
                let existing = file.diagnostics.iter().position(|candidate| {
                    let mut candidate = candidate.clone();
                    candidate.record = None;
                    let mut diagnostic = diagnostic.clone();
                    diagnostic.record = None;
                    candidate == diagnostic
                });
                if existing.is_none() && file.omitted_diagnostics != 0 {
                    builder.add_raw_fallback(
                        file,
                        record.id.clone(),
                        range,
                        crate::inventory::DiagnosticRef(u32::MAX),
                    )?;
                    record_count += 1;
                    continue;
                }
                let global = existing.unwrap_or_else(|| {
                    let index = file.diagnostics.len();
                    file.diagnostics.push(diagnostic);
                    index
                });
                let reference = crate::inventory::DiagnosticRef(
                    u32::try_from(global + 1).map_err(|_| "diagnostic index overflow")?,
                );
                let slot = builder.add_raw_fallback(file, record.id.clone(), range, reference)?;
                file.diagnostics[global].record = Some(slot);
            }
            record_count += 1;
        }
    }
    let generation = builder.finish(files)?;
    let id_lookup = generation
        .records
        .iter()
        .map(|record| (record.id.clone(), record.slot))
        .collect::<HashMap<_, _>>();
    let mut identity_order = generation
        .records
        .iter()
        .map(|record| record.slot)
        .collect::<Vec<_>>();
    identity_order.sort_by(|left, right| {
        generation.records[left.0 as usize]
            .id
            .cmp(&generation.records[right.0 as usize].id)
    });
    let stats = CollectionStats {
        file_count: file_set.len(),
        record_count,
        total_file_bytes: file_set
            .iter()
            .map(|file| {
                files
                    .files
                    .get(file.0 as usize)
                    .map_or(0, |entry| entry.revision.size)
            })
            .sum(),
    };
    Ok(SegmentMetadata {
        generation,
        id_lookup,
        identity_order,
        stats,
        logical_records,
        empty_array_records,
    })
}

pub(crate) struct FinalCollectionObservation {
    pub(crate) id: CollectionId,
    pub(crate) definition: Arc<CollectionDefinition>,
    pub(crate) records: u64,
    pub(crate) observed_batches: u64,
}

impl FinalCollectionObservation {
    fn finish_summary(
        &self,
        options: RepositoryOptions,
    ) -> Result<(Option<GlobalInferenceSummary>, InferenceBuildStats), String> {
        Ok((
            None,
            if options.inference.mode == crate::inference::InferenceMode::Disabled {
                InferenceBuildStats::default()
            } else {
                InferenceBuildStats {
                    observed_records: self.records,
                    merged_batches: self.observed_batches,
                    ..Default::default()
                }
            },
        ))
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn assemble_final_repository(
    root: PathBuf,
    definitions: Vec<CollectionDefinition>,
    options: RepositoryOptions,
    sources: SourceRegistry,
    pool: &rayon::ThreadPool,
    index: &crate::lifecycle::Index,
    discovered: Arc<FileInventory>,
    digest_bindings: Vec<Vec<(CollectionId, [u8; 32])>>,
    pending: &[FinalCollectionObservation],
    build_parses: u64,
    failed_parses: u64,
    source_reads: u64,
    source_bytes_read: u64,
    schedule_stats: crate::construction::scheduling::BuildScheduleStats,
    build_workers: usize,
    peak_segment_jobs: usize,
    peak_segment_window: usize,
    publication_ns: u64,
    discovery_ns: u64,
    extraction_index_ns: u64,
    started: Instant,
    mut profile: Option<BuildProfileStats>,
) -> Result<Repository, crate::lifecycle::IndexError> {
    let finalization_started = crate::measurement::timer::DetailedInstant::now();
    let phase = crate::measurement::timer::start_if_enabled(profile.is_some());
    let base_fingerprints =
        finish_fresh_fingerprints(&discovered, digest_bindings).map_err(IndexError::Build)?;
    if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
        profile.fingerprint_finish_ns = started.elapsed().as_nanos() as u64;
    }
    let phase = crate::measurement::timer::start_if_enabled(profile.is_some());
    let storage_factory = Arc::clone(&index.view()?.repository().storage_factory);
    let mut final_repository = crate::construction::repository_state::initialize_empty(
        root,
        definitions,
        options,
        &sources,
        Some(pool),
        storage_factory,
    )
    .map_err(IndexError::Build)?;
    final_repository.base_fingerprints = base_fingerprints;
    final_repository.build_parses = build_parses;
    final_repository.build_failed_parses = failed_parses;
    let mut inference_build = InferenceBuildStats::default();
    let segment_view = index.view()?;
    let mut final_files = (*discovered).clone();
    let mut pending_relations = Vec::with_capacity(final_repository.collections.len());
    for file in &mut final_files.files {
        if let Some(current) = segment_view.file_entry(file.id)? {
            file.status = current.status;
            file.omitted_diagnostics = current.omitted_diagnostics;
            for diagnostic in &current.diagnostics {
                if !file.diagnostics.iter().any(|candidate| {
                    let mut candidate = candidate.clone();
                    candidate.record = None;
                    let mut diagnostic = diagnostic.clone();
                    diagnostic.record = None;
                    candidate == diagnostic
                }) {
                    file.diagnostics.push(diagnostic.clone());
                }
            }
        }
    }
    for (collection, state) in final_repository.collections.iter_mut().zip(pending) {
        let segments = segment_view
            .segments(state.id)
            .ok_or_else(|| IndexError::Build("segment collection missing".into()))?;
        let mut relations = RelationContribution::for_schema(&state.definition.schema);
        let metadata = crate::construction::repository_state::segment_metadata_generation(
            state.id,
            &state.definition.name,
            segments,
            &mut final_files,
            &mut relations,
        )
        .map_err(IndexError::Build)?;
        collection.generation = metadata.generation;
        collection.id_lookup = metadata.id_lookup;
        collection.identity_order = metadata.identity_order;
        collection.stats = metadata.stats;
        collection.logical_records = metadata.logical_records;
        collection.empty_array_records = metadata.empty_array_records;
        if let Some(first) = segments.first() {
            // Sort bindings need locale/options/direction metadata before
            // segment-local columns can be selected. Keys remain owned by
            // their source segments and are never addressed through this
            // compatibility header.
            collection.sorting = (*first.index.sorting).clone();
            for column in collection.sorting.columns.values_mut() {
                column.keys.clear();
                column.keys.shrink_to_fit();
                column.ascending.clear();
                column.ascending.shrink_to_fit();
                column.descending.clear();
                column.descending.shrink_to_fit();
            }
            for composite in &mut collection.sorting.composites {
                composite.order.clear();
                composite.order.shrink_to_fit();
            }
        }
        pending_relations.push(relations);
        let (_, stats) = state.finish_summary(options).map_err(IndexError::Build)?;
        collection.global_inference = crate::index::memory::inference::aggregate_segment_inference(
            options.inference,
            &collection.definition.schema,
            segments,
            &collection.definition.name,
        )
        .map_err(IndexError::Build)?;
        inference_build.absorb(stats);
    }
    for (source_index, contributions) in pending_relations.into_iter().enumerate() {
        let source = final_repository.collections[source_index]
            .generation
            .collection;
        for contribution in &contributions {
            let logical_count = final_repository.collections[source_index]
                .logical_records
                .len() as u32;
            if let Some(posting) = final_repository.collections[source_index]
                .postings
                .fields
                .get_mut(&contribution.declaration().path)
            {
                posting.presence = contribution.presence().clone();
                posting.values = contribution.reverse_postings();
                posting.value_complete = true;
                posting.covered_records = logical_count;
            }
        }
        let indexes = contributions
            .into_iter()
            .enumerate()
            .map(|(ordinal, contribution)| {
                contribution.resolve(
                    source,
                    u32::try_from(ordinal).map_err(|_| "too many relations")?,
                    &final_repository.collections,
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(IndexError::Build)?;
        final_repository.collections[source_index].relations = indexes;
    }
    crate::index::relation::validate_indexes(&final_repository.collections)
        .map_err(IndexError::Build)?;
    final_repository.files = Arc::new(final_files);
    if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
        profile.finalize_generation_ns = started.elapsed().as_nanos() as u64;
        profile.finalization_rss_bytes = build_profile_rss_bytes();
    }
    final_repository.startup = StartupStats {
        build_ns: started.elapsed().as_nanos() as u64,
        build_discovery_ns: discovery_ns,
        build_extraction_index_ns: extraction_index_ns,
        build_publication_ns: publication_ns,
        build_finalization_ns: finalization_started.elapsed().as_nanos() as u64,
        build_workers,
        build_source_reads: source_reads.saturating_add(schedule_stats.source_reads),
        build_source_bytes_read: source_bytes_read.saturating_add(schedule_stats.source_bytes_read),
        peak_build_admitted_bytes: schedule_stats.peak_weight_bytes,
        build_weight_limit_bytes: schedule_stats.weight_limit_bytes,
        build_peak_active_jobs: schedule_stats.peak_active_jobs,
        build_peak_ready_batches: schedule_stats.peak_ready_batches,
        build_peak_segment_jobs: peak_segment_jobs,
        build_peak_segment_window: peak_segment_window,
        build_job_execution_ns: schedule_stats.job_execution_ns,
        build_receive_wait_ns: schedule_stats.receive_wait_ns,
        build_admission_ns: schedule_stats.admission_ns,
        build_framing_ns: schedule_stats.framing_ns,
        build_framing_chunks: schedule_stats.framing_chunks,
        build_framing_bytes: schedule_stats.framing_bytes,
        build_stream_peak: schedule_stats.stream_peak,
        inference_build,
        build_profile: profile,
        ..StartupStats::default()
    };
    Ok(final_repository)
}

#[cfg(test)]
#[path = "repository_state/tests.rs"]
mod tests;
