//! Single-pass initial construction into immutable lifecycle segments.
//! Discovery supplies one shared path inventory. Completed work units move
//! into segments; the final metadata repository shares that same inventory.

use super::*;

use crate::build::{
    AdmissionBreakdown, BatchContribution, IndexedCollectionSegment, RepositoryFileJob,
    SegmentAdmission, SegmentLimits, SourceFileInput,
};
use crate::inventory::append_shifted_bitmap;
use crate::lifecycle::{Index, IndexError, IndexRevision, PublishedIndex};
use crate::model::FileId;

const FIRST_SEGMENT_RECORDS: usize = crate::lifecycle_partition::BASE_PARTITION_RECORDS;
const LATER_SEGMENT_RECORDS: usize = 132_000;
const FIRST_SEGMENT_SOURCE_BYTES: u64 = 128 * 1024 * 1024;
const LATER_SEGMENT_SOURCE_BYTES: u64 = 160 * 1024 * 1024;


struct IncrementalCollection {
    definition: Arc<CollectionDefinition>,
    source: PreparedSource,
    id: CollectionId,
    member_files: Vec<FileId>,
    admission: SegmentAdmission,
    records: u32,
    logical_records: RoaringBitmap,
    global_inference: Option<InferenceBuilder>,
    options: RepositoryOptions,
    collection_count: usize,
}


impl IncrementalCollection {
    fn new(
        definition: CollectionDefinition,
        source: PreparedSource,
        id: CollectionId,
        member_files: Vec<FileId>,
        options: RepositoryOptions,
        collection_count: usize,
        early: bool,
    ) -> Self {
        let definition = Arc::new(definition);
        let admission = SegmentAdmission::with_limits_eager(
            id,
            &source,
            SegmentLimits {
                files: if early {
                    FIRST_SEGMENT_RECORDS
                } else {
                    LATER_SEGMENT_RECORDS
                },
                records: if early {
                    FIRST_SEGMENT_RECORDS
                } else {
                    LATER_SEGMENT_RECORDS
                },
                source_bytes: if early {
                    FIRST_SEGMENT_SOURCE_BYTES
                } else {
                    LATER_SEGMENT_SOURCE_BYTES
                },
            },
            Arc::clone(&definition),
            options.bigram_alphabet,
            options.inference,
            collection_count,
        );
        Self {
            definition,
            source,
            id,
            member_files,
            admission,
            records: 0,
            logical_records: RoaringBitmap::new(),
            global_inference: InferenceBuilder::for_collection(options.inference, collection_count),
            options,
            collection_count,
        }
    }

    fn observe(
        &mut self,
        batch: &BatchContribution,
        mut profile: Option<&mut AdmissionBreakdown>,
    ) -> Result<(), String> {
        let coordinator = crate::detailed_timer::start_if_enabled(profile.is_some());
        let base = self.records;
        let count = u32::try_from(batch.record_count()).map_err(|_| "too many batch records")?;
        self.records = base
            .checked_add(count)
            .ok_or("collection record slot overflow")?;
        if let (Some(started), Some(profile)) = (coordinator, profile.as_mut()) {
            profile.coordinator_ns += started.elapsed().as_nanos() as u64;
        }
        let inference_started = crate::detailed_timer::start_if_enabled(profile.is_some());
        let inference = batch.inference_copy();
        match (self.global_inference.as_mut(), inference) {
            (Some(global), Some(local)) => global.merge_shifted(local, base)?,
            (None, None) => {}
            _ => return Err("collection inference mode changed during build".into()),
        }
        if let (Some(started), Some(profile)) = (inference_started, profile.as_mut()) {
            profile.inference_observation_ns += started.elapsed().as_nanos() as u64;
        }
        let coordinator = crate::detailed_timer::start_if_enabled(profile.is_some());
        append_shifted_bitmap(&mut self.logical_records, batch.logical_records(), base)?;
        if let (Some(started), Some(profile)) = (coordinator, profile.as_mut()) {
            profile.coordinator_ns += started.elapsed().as_nanos() as u64;
        }
        Ok(())
    }

    /// Admit one complete ordinary batch, sealing the preceding group if the
    /// next batch would exceed its bounded segment target.
    fn admit_profiled(
        &mut self,
        batch: BatchContribution,
        mut profile: Option<&mut AdmissionBreakdown>,
    ) -> Result<Option<crate::build::CollectionSegment>, String> {
        self.observe(&batch, profile.as_deref_mut())?;
        let coordinator = crate::detailed_timer::start_if_enabled(profile.is_some());
        let sealed = if !self.admission.can_admit_batch(&batch) {
            self.take_segment()
        } else {
            None
        };
        if let (Some(started), Some(profile)) = (coordinator, profile.as_mut()) {
            profile.coordinator_ns += started.elapsed().as_nanos() as u64;
        }
        self.admission.admit_batch_profiled(batch, profile)?;
        Ok(sealed)
    }

    /// A large framed file is never split between publications. Its chunks
    /// remain one logical file contribution until framing/validation finish.
    fn admit_large_chunk_profiled(
        &mut self,
        batch: BatchContribution,
        mut profile: Option<&mut AdmissionBreakdown>,
    ) -> Result<(), String> {
        self.observe(&batch, profile.as_deref_mut())?;
        let coordinator = crate::detailed_timer::start_if_enabled(profile.is_some());
        if let (Some(started), Some(profile)) = (coordinator, profile.as_mut()) {
            profile.coordinator_ns += started.elapsed().as_nanos() as u64;
        }
        self.admission.admit_batch_profiled(batch, profile)
    }

    fn take_segment(&mut self) -> Option<crate::build::CollectionSegment> {
        if self.admission.file_count() == 0 {
            return None;
        }
        let replacement = SegmentAdmission::with_limits_eager(
            self.id,
            &self.source,
            SegmentLimits {
                files: LATER_SEGMENT_RECORDS,
                records: LATER_SEGMENT_RECORDS,
                source_bytes: LATER_SEGMENT_SOURCE_BYTES,
            },
            Arc::clone(&self.definition),
            self.options.bigram_alphabet,
            self.options.inference,
            self.collection_count,
        );
        Some(std::mem::replace(&mut self.admission, replacement).seal())
    }

    fn finish_summary(
        &mut self,
    ) -> Result<(Option<GlobalInferenceSummary>, InferenceBuildStats), String> {
        let mut stats = InferenceBuildStats::default();
        let state = self
            .global_inference
            .take()
            .map(|builder| {
                stats = builder.build_stats();
                let started = crate::detailed_timer::DetailedInstant::now();
                let state = builder.finish(
                    &self.logical_records,
                    self.records as usize,
                    &self.definition.schema,
                )?;
                stats.finish_ns = started.elapsed().as_nanos() as u64;
                let diagnostics = state.path_diagnostics();
                Ok::<_, String>(GlobalInferenceSummary {
                    report: state.report,
                    diagnostics,
                })
            })
            .transpose()?;
        Ok((state, stats))
    }
}


fn publish_segment(
    index: &Index,
    expected: &mut IndexRevision,
    segment: crate::build::CollectionSegment,
    collection: &IncrementalCollection,
    options: RepositoryOptions,
    collection_count: usize,
    pool: &rayon::ThreadPool,
    on_publication: &mut impl FnMut(&Index) -> Result<(), IndexError>,
    publication_ns: &mut u64,
    profile: &mut Option<BuildProfileStats>,
) -> Result<(), IndexError> {
    let started = crate::detailed_timer::DetailedInstant::now();
    let mut segment_profile = profile.as_ref().map(|_| IncrementalSegmentProfile {
        collection: collection.definition.name.clone(),
        files: segment.files.len(),
        records: segment.records.len(),
        source_bytes: segment
            .files
            .iter()
            .map(|file| file.entry.revision.size)
            .sum(),
        ..Default::default()
    });
    let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
    let indexed: IndexedCollectionSegment = segment
        .into_indexed_profiled(
            &collection.definition,
            options.bigram_alphabet,
            options.inference,
            collection_count,
            pool,
            segment_profile.as_mut(),
        )
        .map_err(IndexError::Build)?;
    if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
        let ns = started.elapsed().as_nanos() as u64;
        profile.publication_index_ns = profile.publication_index_ns.saturating_add(ns);
        segment_profile.as_mut().unwrap().index_ns = ns;
    }
    let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
    *expected = index.publish_initial_segment(*expected, indexed)?;
    if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
        let ns = started.elapsed().as_nanos() as u64;
        profile.publication_install_ns = profile.publication_install_ns.saturating_add(ns);
        segment_profile.as_mut().unwrap().install_ns = ns;
    }
    let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
    on_publication(index)?;
    if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
        let ns = started.elapsed().as_nanos() as u64;
        profile.publication_callback_ns = profile.publication_callback_ns.saturating_add(ns);
        segment_profile.as_mut().unwrap().callback_ns = ns;
    }
    *publication_ns = publication_ns.saturating_add(started.elapsed().as_nanos() as u64);
    if let (Some(profile), Some(segment)) = (profile.as_mut(), segment_profile) {
        profile.segments.push(segment);
    }
    Ok(())
}

fn finish_admission_profile(
    profile: &mut Option<BuildProfileStats>,
    collection: &str,
    started: Option<Instant>,
    detail: Option<AdmissionBreakdown>,
) -> Result<(), String> {
    // The outer interval ends before profile aggregation and before any
    // segment publication. Consumed contribution temporaries are dropped by
    // the inner admission calls before this boundary.
    if let (Some(started), Some(mut detail), Some(profile)) = (started, detail, profile.as_mut()) {
        detail.total_ns = started.elapsed().as_nanos() as u64;
        detail.remainder_ns = detail
            .total_ns
            .checked_sub(detail.incremental_component_ns())
            .ok_or("incremental admission components overlap")?;
        profile.admission.absorb(detail);
        profile
            .admission_by_collection
            .entry(collection.to_owned())
            .or_default()
            .absorb(detail);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn admit_normal(
    collection_index: usize,
    batch: BatchContribution,
    pending: &mut [IncrementalCollection],
    digest_bindings: &mut [Vec<(CollectionId, [u8; 32])>],
    build_parses: &mut u64,
    failed_parses: &mut u64,
    source_reads: &mut u64,
    source_bytes_read: &mut u64,
    index: &Index,
    expected: &mut IndexRevision,
    options: RepositoryOptions,
    pool: &rayon::ThreadPool,
    on_publication: &mut impl FnMut(&Index) -> Result<(), IndexError>,
    publication_ns: &mut u64,
    profile: &mut Option<BuildProfileStats>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), String> {
    if cancelled() {
        return Err("initial construction cancelled".into());
    }
    let admission_started = crate::detailed_timer::start_if_enabled(profile.is_some());
    let mut detail = profile.as_ref().map(|_| AdmissionBreakdown {
        batches: 1,
        files: batch.new_file_count() as u64,
        records: batch.record_count() as u64,
        ..Default::default()
    });
    let collection_id = pending[collection_index].id;
    for &(file, digest) in batch.file_fingerprints() {
        digest_bindings
            .get_mut(file.0 as usize)
            .ok_or("digest file ID out of bounds")?
            .push((collection_id, digest));
    }
    *build_parses = build_parses.saturating_add(batch.record_parses);
    *failed_parses = failed_parses.saturating_add(batch.failed_parses);
    *source_reads = source_reads.saturating_add(batch.source_reads);
    *source_bytes_read = source_bytes_read.saturating_add(batch.source_bytes_read);
    if let Some(profile) = profile.as_mut() {
        profile.work.absorb(&batch.profile);
        profile
            .work_by_collection
            .entry(pending[collection_index].definition.name.clone())
            .or_default()
            .absorb(&batch.profile);
    }
    if let (Some(started), Some(detail)) = (admission_started, detail.as_mut()) {
        detail.coordinator_ns += started.elapsed().as_nanos() as u64;
    }
    let sealed = pending[collection_index].admit_profiled(batch, detail.as_mut())?;
    finish_admission_profile(
        profile,
        &pending[collection_index].definition.name,
        admission_started,
        detail,
    )?;
    if let Some(sealed) = sealed {
        publish_segment(
            index,
            expected,
            sealed,
            &pending[collection_index],
            options,
            pending.len(),
            pool,
            on_publication,
            publication_ns,
            profile,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}


impl Repository {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_incremental_with_sources(
        root: PathBuf,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        sources: SourceRegistry,
        on_catalogue: impl FnOnce(&Index) -> Result<(), IndexError>,
        mut on_publication: impl FnMut(&Index) -> Result<(), IndexError>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Index, IndexError> {
        let started = Instant::now();
        let mut profile = options
            .detailed_profiling_enabled()
            .then(BuildProfileStats::default);
        let root = root
            .canonicalize()
            .map_err(|error| IndexError::Io(error.to_string()))?;
        let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
        let build_workers = crate::build::resolve_build_workers(
            options.build_workers,
            std::thread::available_parallelism().ok(),
        )
        .map_err(IndexError::Build)?;
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(build_workers)
            .thread_name(|number| format!("objekte-build-{number}"))
            .build()
            .map_err(|error| IndexError::Build(error.to_string()))?;
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.pool_init_ns = started.elapsed().as_nanos() as u64;
        }
        let discovery_started = Instant::now();
        let mut files = FileInventory::new(root.clone());
        let mut selected = Vec::new();
        let metadata_window = build_workers.saturating_mul(64).max(1);
        let metadata_peak_active = AtomicUsize::new(0);
        for (number, definition) in definitions.iter().cloned().enumerate() {
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            if selected.iter().any(
                |(existing, _, _, _): &(
                    CollectionDefinition,
                    CollectionId,
                    PreparedSource,
                    Vec<FileId>,
                )| existing.name == definition.name,
            ) {
                return Err(IndexError::Build(format!(
                    "duplicate collection: {}",
                    definition.name
                )));
            }
            let id = CollectionId(
                u32::try_from(number)
                    .map_err(|_| IndexError::ResourceLimit("too many collections".into()))?,
            );
            let source = sources
                .resolve(&definition.format)
                .map_err(IndexError::Build)?;
            let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
            let paths = discover(
                &root,
                &definition.source_path,
                &definition.discovery,
                &source,
            )
            .map_err(IndexError::Build)?;
            if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
                profile.discovery_enumeration_ns = profile
                    .discovery_enumeration_ns
                    .saturating_add(started.elapsed().as_nanos() as u64);
            }
            let mut members = Vec::with_capacity(paths.len());
            let mut paths = paths.into_iter();
            loop {
                if cancelled() {
                    return Err(IndexError::Cancelled);
                }
                let window = paths.by_ref().take(metadata_window).collect::<Vec<_>>();
                if window.is_empty() {
                    break;
                }
                let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
                let prepared = if build_workers == 1 {
                    window
                        .into_iter()
                        .map(|path| prepare_discovered_file(&root, path))
                        .collect::<Vec<_>>()
                } else {
                    prepare_discovered_window(&pool, &root, window, false, &metadata_peak_active)
                };
                if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
                    profile.discovery_metadata_prepare_ns = profile
                        .discovery_metadata_prepare_ns
                        .saturating_add(started.elapsed().as_nanos() as u64);
                }
                let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
                for entry in prepared {
                    let entry = entry.map_err(IndexError::Build)?;
                    members.push(
                        files
                            .add(entry.relative, entry.revision, entry.created_ns, id)
                            .map_err(IndexError::Build)?,
                    );
                }
                if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
                    profile.discovery_ordered_insert_ns = profile
                        .discovery_ordered_insert_ns
                        .saturating_add(started.elapsed().as_nanos() as u64);
                }
            }
            selected.push((definition, id, source, members));
        }
        files.validate_diagnostics().map_err(IndexError::Build)?;
        let discovery_ns = discovery_started.elapsed().as_nanos() as u64;
        if let Some(profile) = profile.as_mut() {
            profile.discovery_catalogue_ns = discovery_ns;
            profile.discovery_metadata_peak_active = metadata_peak_active.load(Ordering::Relaxed);
            profile.discovery_rss_bytes = build_profile_rss_bytes();
        }
        let setup_started = crate::detailed_timer::start_if_enabled(profile.is_some());
        let discovered = Arc::new(files);
        let source_files = Arc::new(
            discovered
                .files
                .iter()
                .map(SourceFileInput::from_entry)
                .collect::<Vec<_>>(),
        );
        let collection_count = selected.len();
        let early_collection = selected
            .iter()
            .max_by_key(|(_, _, _, members)| members.len())
            .map(|(_, id, _, _)| *id);
        let mut pending = selected
            .into_iter()
            .map(|(definition, id, source, members)| {
                IncrementalCollection::new(
                    definition,
                    source,
                    id,
                    members,
                    options,
                    collection_count,
                    Some(id) == early_collection,
                )
            })
            .collect::<Vec<_>>();
        let mut repository =
            Self::build_fresh(root.clone(), definitions.clone(), options, &sources, false)
                .map_err(IndexError::Build)?;
        repository.files = Arc::clone(&discovered);
        repository.base_fingerprints = vec![None; discovered.files.len()];
        for (collection, state) in repository.collections.iter_mut().zip(&pending) {
            collection.generation = CollectionBuilder::new(
                state.id,
                state.definition.name.clone(),
                1,
                discovered.files.len(),
            )
            .finish(&discovered)
            .map_err(IndexError::Build)?;
            collection.stats.file_count = state.member_files.len();
            collection.stats.total_file_bytes = state
                .member_files
                .iter()
                .map(|file| discovered.files[file.0 as usize].revision.size)
                .sum();
        }
        repository.startup.build_discovery_ns = discovery_ns;
        repository.startup.build_workers = build_workers;
        let repository = Arc::new(repository);
        let mut published = PublishedIndex::from_repository(
            next_repository_instance(),
            IndexRevision::INITIAL,
            repository,
        )?;
        published.begin_initial_construction();
        let index = Index::from_published(published);
        on_catalogue(&index)?;
        if let (Some(started), Some(profile)) = (setup_started, profile.as_mut()) {
            profile.builder_setup_ns = started.elapsed().as_nanos() as u64;
        }
        if cancelled() {
            return Err(IndexError::Cancelled);
        }
        let mut expected = IndexRevision::INITIAL;
        let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
        let mut jobs = Vec::new();
        for (collection_index, state) in pending.iter().enumerate() {
            for batch in crate::build::file_batches(&state.member_files, &discovered) {
                if batch.len() == 1
                    && discovered.files[batch[0].0 as usize].revision.size > 8 * 1024 * 1024
                {
                    jobs.push(ScheduledBuildJob::Large {
                        collection_index,
                        collection: state.id,
                        definition: Arc::clone(&state.definition),
                        source: state.source.clone(),
                        file: batch[0],
                    });
                } else {
                    jobs.push(ScheduledBuildJob::Files(RepositoryFileJob {
                        collection_index,
                        collection: state.id,
                        definition: Arc::clone(&state.definition),
                        source: state.source.clone(),
                        files: batch.to_vec(),
                    }));
                }
            }
        }
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.job_scheduling_ns = started.elapsed().as_nanos() as u64;
        }
        let extraction_started = crate::detailed_timer::DetailedInstant::now();
        let mut digest_bindings = vec![Vec::new(); discovered.files.len()];
        let mut build_parses = 0u64;
        let mut failed_parses = 0u64;
        let mut source_reads = 0u64;
        let mut source_bytes_read = 0u64;
        let mut schedule_stats = crate::build::BuildScheduleStats::default();
        let mut publication_ns = 0u64;
        let mut normal = Vec::new();
        for job in jobs {
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            match job {
                ScheduledBuildJob::Files(job) => normal.push(job),
                ScheduledBuildJob::Large {
                    collection_index,
                    collection,
                    definition,
                    source,
                    file,
                } => {
                    if !normal.is_empty() {
                        let result = crate::build::process_repository_file_batches_profiled(
                            &pool,
                            Arc::new(root.clone()),
                            &normal,
                            Arc::clone(&source_files),
                            options.bigram_alphabet,
                            options.inference,
                            options.detailed_profiling_enabled(),
                            |number, batch| {
                                admit_normal(
                                    number,
                                    batch,
                                    &mut pending,
                                    &mut digest_bindings,
                                    &mut build_parses,
                                    &mut failed_parses,
                                    &mut source_reads,
                                    &mut source_bytes_read,
                                    &index,
                                    &mut expected,
                                    options,
                                    &pool,
                                    &mut on_publication,
                                    &mut publication_ns,
                                    &mut profile,
                                    &cancelled,
                                )
                            },
                        );
                        schedule_stats.absorb(result.map_err(|error| {
                            if cancelled() {
                                IndexError::Cancelled
                            } else {
                                IndexError::Build(error)
                            }
                        })?);
                        normal.clear();
                    }
                    if let Some(sealed) = pending[collection_index].take_segment() {
                        publish_segment(
                            &index,
                            &mut expected,
                            sealed,
                            &pending[collection_index],
                            options,
                            pending.len(),
                            &pool,
                            &mut on_publication,
                            &mut publication_ns,
                            &mut profile,
                        )?;
                    }
                    let result = crate::build::process_framed_file_profiled(
                        &pool,
                        Arc::new(root.clone()),
                        definition,
                        collection,
                        source,
                        file,
                        Arc::clone(&source_files),
                        options.bigram_alphabet,
                        options.inference,
                        options.detailed_profiling_enabled(),
                        |batch| {
                            if cancelled() {
                                return Err("initial construction cancelled".into());
                            }
                            let admission_started =
                                crate::detailed_timer::start_if_enabled(profile.is_some());
                            let mut detail = profile.as_ref().map(|_| AdmissionBreakdown {
                                batches: 1,
                                files: batch.new_file_count() as u64,
                                records: batch.record_count() as u64,
                                ..Default::default()
                            });
                            build_parses = build_parses.saturating_add(batch.record_parses);
                            failed_parses = failed_parses.saturating_add(batch.failed_parses);
                            for &(file, digest) in batch.file_fingerprints() {
                                digest_bindings[file.0 as usize].push((collection, digest));
                            }
                            if let Some(profile) = profile.as_mut() {
                                profile.work.absorb(&batch.profile);
                                profile
                                    .work_by_collection
                                    .entry(pending[collection_index].definition.name.clone())
                                    .or_default()
                                    .absorb(&batch.profile);
                            }
                            if let (Some(started), Some(detail)) =
                                (admission_started, detail.as_mut())
                            {
                                detail.coordinator_ns += started.elapsed().as_nanos() as u64;
                            }
                            pending[collection_index]
                                .admit_large_chunk_profiled(batch, detail.as_mut())?;
                            finish_admission_profile(
                                &mut profile,
                                &pending[collection_index].definition.name,
                                admission_started,
                                detail,
                            )
                        },
                    )
                    .map_err(|error| {
                        if cancelled() {
                            IndexError::Cancelled
                        } else {
                            IndexError::Build(error)
                        }
                    })?;
                    if let Some((file, digest)) = result.file_fingerprint {
                        digest_bindings[file.0 as usize].push((collection, digest));
                    }
                    schedule_stats.absorb(result);
                    if let Some(sealed) = pending[collection_index].take_segment() {
                        publish_segment(
                            &index,
                            &mut expected,
                            sealed,
                            &pending[collection_index],
                            options,
                            pending.len(),
                            &pool,
                            &mut on_publication,
                            &mut publication_ns,
                            &mut profile,
                        )?;
                    }
                }
            }
        }
        if !normal.is_empty() {
            let result = crate::build::process_repository_file_batches_profiled(
                &pool,
                Arc::new(root.clone()),
                &normal,
                Arc::clone(&source_files),
                options.bigram_alphabet,
                options.inference,
                options.detailed_profiling_enabled(),
                |number, batch| {
                    admit_normal(
                        number,
                        batch,
                        &mut pending,
                        &mut digest_bindings,
                        &mut build_parses,
                        &mut failed_parses,
                        &mut source_reads,
                        &mut source_bytes_read,
                        &index,
                        &mut expected,
                        options,
                        &pool,
                        &mut on_publication,
                        &mut publication_ns,
                        &mut profile,
                        &cancelled,
                    )
                },
            );
            schedule_stats.absorb(result.map_err(|error| {
                if cancelled() {
                    IndexError::Cancelled
                } else {
                    IndexError::Build(error)
                }
            })?);
        }
        let extraction_index_ns = extraction_started.elapsed().as_nanos() as u64;
        if let Some(profile) = profile.as_mut() {
            profile.source_work_ns = extraction_index_ns;
            profile.admission_schedule = schedule_stats.admission_schedule;
            profile.extraction_rss_bytes = build_profile_rss_bytes();
        }
        let flush_started = crate::detailed_timer::start_if_enabled(profile.is_some());
        for number in 0..pending.len() {
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            if let Some(sealed) = pending[number].take_segment() {
                publish_segment(
                    &index,
                    &mut expected,
                    sealed,
                    &pending[number],
                    options,
                    pending.len(),
                    &pool,
                    &mut on_publication,
                    &mut publication_ns,
                    &mut profile,
                )?;
            }
        }
        if let (Some(started), Some(profile)) = (flush_started, profile.as_mut()) {
            profile.final_segment_flush_ns = started.elapsed().as_nanos() as u64;
        }
        let finalization_started = crate::detailed_timer::DetailedInstant::now();
        let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
        let base_fingerprints =
            finish_fresh_fingerprints(&discovered, digest_bindings).map_err(IndexError::Build)?;
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.fingerprint_finish_ns = started.elapsed().as_nanos() as u64;
        }
        let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
        let mut final_repository = Self::build_fresh(root, definitions, options, &sources, false)
            .map_err(IndexError::Build)?;
        final_repository.files = Arc::clone(&discovered);
        final_repository.base_fingerprints = base_fingerprints;
        final_repository.build_parses = build_parses;
        final_repository.build_failed_parses = failed_parses;
        let mut inference_build = InferenceBuildStats::default();
        for (collection, state) in final_repository.collections.iter_mut().zip(&mut pending) {
            collection.generation = CollectionBuilder::new(
                state.id,
                state.definition.name.clone(),
                1,
                discovered.files.len(),
            )
            .finish(&discovered)
            .map_err(IndexError::Build)?;
            let (summary, stats) = state.finish_summary().map_err(IndexError::Build)?;
            collection.global_inference = summary;
            // These statistics describe the immutable base generation. The
            // completed initial build lives in lifecycle segments, which are
            // accounted for separately by the published view. Snapshot
            // validation recomputes this base-only summary.
            collection.stats = CollectionStats {
                file_count: 0,
                record_count: 0,
                total_file_bytes: 0,
            };
            inference_build.absorb(stats);
        }
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
            build_source_bytes_read: source_bytes_read
                .saturating_add(schedule_stats.source_bytes_read),
            peak_build_admitted_bytes: schedule_stats.peak_weight_bytes,
            build_weight_limit_bytes: schedule_stats.weight_limit_bytes,
            build_peak_active_jobs: schedule_stats.peak_active_jobs,
            build_peak_ready_batches: schedule_stats.peak_ready_batches,
            build_job_execution_ns: schedule_stats.job_execution_ns,
            build_receive_wait_ns: schedule_stats.receive_wait_ns,
            build_admission_ns: schedule_stats.admission_ns,
            inference_build,
            build_profile: profile,
            ..StartupStats::default()
        };
        if cancelled() {
            return Err(IndexError::Cancelled);
        }
        index.finish_initial_construction(expected, Arc::new(final_repository))?;
        Ok(index)
    }
}
