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
use crate::index::storage::file_catalog::{
    DiscoveredFileRows, EmptyFileCatalogBase, FileCatalogHandle, FileCatalogSource, FileRows,
};
use crate::index::storage::segment::{
    RecordOrigin, SegmentReader, SegmentSlotFact, read_slot_members, visit_record_headers,
    visit_segment_files,
};
use crate::inference::InferenceBuildStats;
#[cfg(test)]
use crate::inventory::CollectionGeneration;
#[cfg(test)]
use crate::inventory::FileInventory;
use crate::inventory::{ActiveRecords, CollectionBuilder, FileEntry, RecordClass};
use crate::lifecycle::{IndexError, IndexRevision};
use crate::model::CollectionId;
#[cfg(test)]
use crate::model::{RecordId, RecordSlot};
use crate::repository::configuration::{
    CollectionDeclaration, CollectionDefinition, FileSourceConfig, RepositoryOptions,
};
use crate::repository::state::{
    CollectionState, CollectionStats, GlobalInferenceSummary, InitialCollectionCounts,
};
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
    files: &[FileEntry],
    mut bindings: Vec<Vec<(CollectionId, [u8; 32])>>,
) -> Result<Vec<Option<[u8; 32]>>, String> {
    if bindings.len() != files.len() {
        return Err("fresh digest inventory length mismatch".into());
    }
    files
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
    let source_root = root.as_deref().map(Arc::from);
    #[cfg(test)]
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
        let base_generation = IndexRevision::INITIAL.0;
        let generation = CollectionBuilder::new(id, definition.name.clone(), base_generation, 0)
            .finish_empty()?;
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
            crate::index::storage::text::TextBindingCatalog::new(&definition.schema);
        let index_family_bytes = crate::index::memory::segment::statistics::index_family_total(
            &raw,
            &fallback_text,
            &custom_text,
            &postings,
            &sorting,
        );
        pending_relations.push(RelationContribution::for_schema(&definition.schema));
        collections.push(CollectionState {
            repository_instance: 0,
            id,
            base_generation,
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
            initial_counts: None,
            stats: CollectionStats {
                file_count: 0,
                record_count: 0,
                total_file_bytes: 0,
            },
            relations: Vec::new(),
        });
    }
    for (source_index, contributions) in pending_relations.into_iter().enumerate() {
        let source = collections[source_index].collection_id();
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
        &EmptyFileCatalogBase {
            root: source_root.clone(),
            configuration: configuration_identity,
            collections: collections.len(),
        },
        crate::inventory::FileVersion::INITIAL,
    )?;
    let active = (0..collections.len())
        .map(|_| ActiveRecords::empty())
        .collect::<Vec<_>>();
    let segment_catalogs = (0..collections.len())
        .map(|_| storage_factory.segment_catalog()?.seal())
        .collect::<crate::index::storage::transfer::StorageResult<Vec<_>>>()?;
    #[cfg(test)]
    let segments = vec![Arc::new(Vec::new()); collections.len()];
    let instance_id = next_repository_instance();
    for collection in &mut collections {
        collection.repository_instance = instance_id;
    }
    let catalog_registered = !programmatic || source_root.is_some();
    Ok(Repository {
        storage_factory,
        source_root,
        #[cfg(test)]
        files,
        #[cfg(test)]
        base_fingerprints: Vec::new(),
        collections,
        active,
        segment_catalogs,
        #[cfg(test)]
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
        inferred_orders: Mutex::new(
            crate::index::storage::order::inferred_cache::InferredOrderCache::new(
                crate::index::storage::order::inferred_cache::INFERRED_ORDER_CACHE_BYTES,
            ),
        ),
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
/// Capture the collection counters and normalize diagnostic slot references
/// while the sealed segments keep authoritative record and relation rows.
fn summarize_published_collection(
    segments: &[Arc<crate::index::storage::publication::PublishedSegment>],
    files: &mut [FileEntry],
) -> Result<(CollectionStats, InitialCollectionCounts), String> {
    let mut file_ids = HashSet::new();
    let mut records = 0u32;
    let mut fallback = 0usize;
    let mut arrays = RoaringBitmap::new();
    for segment in segments {
        let base = records;
        let slots =
            u32::try_from(segment.descriptor().slots).map_err(|_| "too many segment slots")?;
        records = records
            .checked_add(slots)
            .ok_or("too many collection records")?;
        visit_segment_files(segment.as_ref(), |file| {
            file_ids.insert(file.id);
            Ok::<_, String>(())
        })?;
        arrays
            .append(
                read_slot_members(segment, SegmentSlotFact::EmptyArray)?
                    .view()
                    .iter()
                    .map(|slot| base + slot),
            )
            .map_err(|_| "out of order structural facts")?;
        let fallback_count = segment.active_fallback_count(&segment.active);
        fallback += fallback_count as usize;
        if fallback_count == 0 {
            continue;
        }
        visit_record_headers(segment.as_ref(), |headers| -> Result<(), String> {
            for record in headers
                .iter()
                .filter(|record| record.class == RecordClass::RawFallback)
            {
                let Some(local) = record.diagnostic_ref.stored_index() else {
                    continue;
                };
                let Some(RecordOrigin::FileRange(locator)) = segment.origin(record.slot)? else {
                    return Err("file-backed segment has no locator for its fallback record".into());
                };
                let file = files
                    .get_mut(locator.file.0 as usize)
                    .ok_or("segment metadata file missing")?;
                let metadata = segment
                    .file_metadata(locator.file)?
                    .ok_or("fallback segment file missing")?;
                let diagnostic = metadata
                    .diagnostics
                    .get(local)
                    .ok_or("fallback diagnostic missing")?;
                let prior = file.diagnostics.iter_mut().find(|candidate| {
                    let mut candidate = (*candidate).clone();
                    candidate.record = None;
                    let mut diagnostic = diagnostic.clone();
                    diagnostic.record = None;
                    candidate == diagnostic
                });
                let slot = crate::model::RecordSlot(base + record.slot.0);
                if let Some(prior) = prior {
                    prior.record = Some(slot);
                } else if file.omitted_diagnostics == 0 {
                    let mut diagnostic = diagnostic.clone();
                    diagnostic.record = Some(slot);
                    file.diagnostics.push(diagnostic);
                }
            }
            Ok(())
        })?;
    }
    let stats = CollectionStats {
        file_count: file_ids.len(),
        record_count: records as usize,
        total_file_bytes: file_ids
            .iter()
            .map(|file| {
                files
                    .get(file.0 as usize)
                    .map_or(0, |entry| entry.revision.size)
            })
            .sum(),
    };
    Ok((
        stats,
        InitialCollectionCounts {
            fallback_records: fallback,
            structural_membership_bytes: arrays.serialized_size(),
        },
    ))
}

/// Legacy base-only tests intentionally exercise coverage from a complete
/// collection generation. Normal publication keeps records in its segments.
#[cfg(test)]
struct SegmentMetadata {
    generation: CollectionGeneration,
    id_lookup: HashMap<RecordId, RecordSlot>,
    identity_order: Vec<RecordSlot>,
    logical_records: RoaringBitmap,
    empty_array_records: RoaringBitmap,
}

#[cfg(test)]
fn segment_metadata_generation(
    collection: CollectionId,
    name: &str,
    segments: &[Arc<crate::index::storage::publication::PublishedSegment>],
    files: &mut [FileEntry],
    relations: &mut [RelationContribution],
) -> Result<SegmentMetadata, String> {
    let mut builder = CollectionBuilder::new(collection, name.to_owned(), 1, files.len());
    let mut file_set = HashSet::new();
    let mut record_count = 0usize;
    let mut logical_records = RoaringBitmap::new();
    let mut empty_array_records = RoaringBitmap::new();
    for segment in segments {
        let base = u32::try_from(record_count).map_err(|_| "too many segment records")?;
        let segment_count =
            u32::try_from(segment.descriptor().slots).map_err(|_| "too many segment records")?;
        if relations.len() != segment.relation_count() {
            return Err("segment relation coverage mismatch".into());
        }
        for (ordinal, global) in relations.iter_mut().enumerate() {
            let local = segment
                .relation(ordinal)?
                .ok_or("missing segment relation reader")?;
            global.merge_reader(local, base, segment_count)?;
        }
        for slot in read_slot_members(segment, SegmentSlotFact::Logical)?
            .view()
            .iter()
        {
            logical_records.insert(
                base.checked_add(slot)
                    .ok_or("logical record slot overflow")?,
            );
        }
        for slot in read_slot_members(segment, SegmentSlotFact::EmptyArray)?
            .view()
            .iter()
        {
            empty_array_records.insert(
                base.checked_add(slot)
                    .ok_or("empty-array record slot overflow")?,
            );
        }
        visit_segment_files(segment.as_ref(), |file| {
            if file_set.insert(file.id) {
                builder.include_file(file.id)?;
            }
            Ok::<_, String>(())
        })?;
        visit_record_headers(segment.as_ref(), |headers| {
            for record in headers {
                let Some(RecordOrigin::FileRange(locator)) = segment.origin(record.slot)? else {
                    return Err("file-backed segment metadata has no file locator".to_owned());
                };
                let range = locator
                    .range
                    .ok_or("file-backed record has no byte range")?;
                let file = files
                    .get_mut(locator.file.0 as usize)
                    .ok_or("segment metadata file missing")?;
                if record.class == RecordClass::Logical {
                    builder.add_record(file, record.id.clone(), range.start, range.end)?;
                } else {
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
                    let segment_file = segment
                        .file_metadata(locator.file)?
                        .ok_or("fallback segment file missing")?;
                    let diagnostic = segment_file
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
                    let slot =
                        builder.add_raw_fallback(file, record.id.clone(), range, reference)?;
                    file.diagnostics[global].record = Some(slot);
                }
                record_count += 1;
            }
            Ok::<_, String>(())
        })?;
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
    Ok(SegmentMetadata {
        generation,
        id_lookup,
        identity_order,
        logical_records,
        empty_array_records,
    })
}

pub(crate) struct FinalCollectionObservation {
    pub(crate) id: CollectionId,
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
    discovered: FileCatalogHandle,
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
) -> Result<(Repository, DiscoveredFileRows), IndexError> {
    let finalization_started = crate::measurement::timer::DetailedInstant::now();
    let phase = crate::measurement::timer::start_if_enabled(profile.is_some());
    let mut final_files = Vec::with_capacity(discovered.high_water_mark() as usize);
    for row in FileRows::new(&discovered) {
        final_files.push(row?.entry.into_owned());
    }
    let base_fingerprints =
        finish_fresh_fingerprints(&final_files, digest_bindings).map_err(IndexError::Build)?;
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
    final_repository.build_parses = build_parses;
    final_repository.build_failed_parses = failed_parses;
    let mut inference_build = InferenceBuildStats::default();
    let segment_view = index.view()?;
    #[cfg(test)]
    let mut pending_relations = Vec::with_capacity(final_repository.collections.len());
    for file in &mut final_files {
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
        let segments = segment_view.segments(state.id)?;
        let (stats, counts) = summarize_published_collection(&segments, &mut final_files)
            .map_err(IndexError::Build)?;
        collection.stats = stats;
        collection.initial_counts = Some(counts);
        #[cfg(test)]
        {
            let mut relations = RelationContribution::for_schema(&collection.definition.schema);
            let metadata = crate::construction::repository_state::segment_metadata_generation(
                state.id,
                &collection.definition.name,
                &segments,
                &mut final_files,
                &mut relations,
            )
            .map_err(IndexError::Build)?;
            collection.generation = metadata.generation;
            collection.id_lookup = metadata.id_lookup;
            collection.identity_order = metadata.identity_order;
            collection.logical_records = metadata.logical_records;
            collection.empty_array_records = metadata.empty_array_records;
            // The empty sort columns already carry the declaration and collation
            // metadata. Keep the finished keys and orders on their source segments.
            pending_relations.push(relations);
        }
        let (_, stats) = state.finish_summary(options).map_err(IndexError::Build)?;
        collection.global_inference = crate::inference::admission::aggregate_segment_inference(
            options.inference,
            &collection.definition.schema,
            &segments,
        )
        .map_err(IndexError::Build)?;
        inference_build.absorb(stats);
    }
    #[cfg(test)]
    {
        for (source_index, contributions) in pending_relations.into_iter().enumerate() {
            let source = final_repository.collections[source_index].collection_id();
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
    }
    let root = final_repository
        .source_root
        .clone()
        .ok_or_else(|| IndexError::Build("missing discovered source root".into()))?;
    #[cfg(test)]
    {
        final_repository.files = Arc::new(
            FileInventory::from_restored_entries(root.to_path_buf(), final_files.clone())
                .map_err(IndexError::Build)?,
        );
        final_repository.base_fingerprints = base_fingerprints.clone();
    }
    let file_base = DiscoveredFileRows {
        root,
        configuration: final_repository.configuration_identity,
        collections: final_repository.collections.len(),
        files: final_files,
        fingerprints: base_fingerprints,
    };
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
    Ok((final_repository, file_base))
}

#[cfg(test)]
#[path = "repository_state/tests.rs"]
mod tests;
