//! Single-pass initial construction into immutable lifecycle segments.
//! Discovery supplies one shared path inventory. Completed work units move
//! into segments; the final metadata repository shares that same inventory.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use roaring::RoaringBitmap;

use super::file_preparation::{StreamActivity, StreamUse};
use super::segment_jobs::{
    IndexedSegmentBuild, PrivateSegmentGroup, SegmentBuildScheduler, SegmentJobGate,
    schedule_private_segment, schedule_segment,
};
use crate::construction::batch::{BatchContribution, SourceFileInput};
use crate::construction::discovery::{
    prepare_discovered_file, prepare_discovered_window, register_discovery_window,
};
use crate::construction::scheduling::RepositoryFileJob;
use crate::index::segment::SegmentId;
use crate::index::storage::StorageFactory;
use crate::index::storage::file_catalog::{FileCatalogSource, FileRows};
use crate::index::storage::segment::build::{FileSegmentBuild, SegmentBuilder, SegmentSeal};
use crate::index::storage::transfer::StorageResult;
use crate::inventory::discovery::discover;
use crate::inventory::{CollectionBuilder, append_shifted_bitmap};
use crate::lifecycle::{Index, IndexError, IndexRevision, PublishedIndex};
use crate::model::{CollectionId, FileId};
use crate::repository::configuration::{CollectionDefinition, RepositoryOptions};
use crate::repository::statistics::AdmissionBreakdown;
use crate::repository::statistics::{BuildProfileStats, build_profile_rss_bytes};
use crate::repository::{Repository, next_repository_instance};
use crate::source::{registry::PreparedSource, registry::SourceRegistry};

enum ScheduledBuildJob {
    Files(crate::construction::scheduling::RepositoryFileJob),
    Large {
        collection_index: usize,
        collection: CollectionId,
        definition: Arc<CollectionDefinition>,
        source: PreparedSource,
        file: FileId,
    },
}

type InitialSegment = (
    crate::index::storage::segment::SegmentHandle,
    Option<crate::index::storage::inference::InferenceObservationHandle>,
);

struct StagedStreamBinding {
    collection: IncrementalCollection,
    segments: Vec<InitialSegment>,
    tail: Vec<BatchContribution>,
    tail_records: usize,
    private: StreamUse,
    unsealed: StreamUse,
    stats: super::scheduling::BuildScheduleStats,
    parses: u64,
    failed: u64,
}

#[derive(Clone)]
struct StreamBinding {
    definition: Arc<CollectionDefinition>,
    reader: PreparedSource,
    id: CollectionId,
}

struct StreamGroup {
    bindings: Arc<[StreamBinding]>,
    files: Vec<FileId>,
}

impl StagedStreamBinding {
    fn new(
        binding: &StreamBinding,
        options: RepositoryOptions,
        factory: &Arc<dyn StorageFactory>,
        budget: &super::file_preparation::StreamBudget,
    ) -> StorageResult<Self> {
        Ok(Self {
            collection: IncrementalCollection::new(
                (*binding.definition).clone(),
                binding.reader.clone(),
                binding.id,
                Vec::new(),
                options,
                Arc::clone(factory),
            )?,
            segments: Vec::new(),
            tail: Vec::new(),
            tail_records: 0,
            private: budget.activity(StreamActivity::PrivateSegments, 0, 0),
            unsealed: budget.activity(StreamActivity::Unsealed, 0, 0),
            stats: Default::default(),
            parses: 0,
            failed: 0,
        })
    }

    fn admit(
        &mut self,
        batch: BatchContribution,
        options: RepositoryOptions,
        collection_count: usize,
        pool: &rayon::ThreadPool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        if cancelled() {
            return Err("initial construction cancelled".into());
        }
        self.parses += batch.record_parses;
        self.failed += batch.failed_parses;
        self.collection.observe(&batch, None)?;
        self.tail_records += batch.record_count();
        self.tail.push(batch);
        self.unsealed.set(self.tail.len() as u64, 0);
        if self.tail_records == options.segment_policy.target_records.get() {
            for batch in self.tail.drain(..) {
                self.collection.admission.admit(batch, None)?;
            }
            self.unsealed.set(0, 0);
            let built = finish_stream_draft(
                self.collection
                    .take_segment()?
                    .expect("full private stream draft"),
                &self.collection.definition,
                self.collection.factory.as_ref(),
                options,
                collection_count,
                pool,
            )?;
            self.private.add_private_segment(&built.0);
            self.segments.push(built);
            self.tail_records = 0;
        }
        Ok(())
    }
}

fn stage_stream_file(
    pool: &rayon::ThreadPool,
    factory: &Arc<dyn crate::index::storage::StorageFactory>,
    root: Arc<PathBuf>,
    bindings: &[StreamBinding],
    file: FileId,
    source_files: Arc<Vec<SourceFileInput>>,
    options: RepositoryOptions,
    collection_count: usize,
    budget: &super::file_preparation::StreamBudget,
    file_window: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<StagedStreamBinding>, String> {
    if cancelled() {
        return Err("initial construction cancelled".into());
    }
    let mut staged = bindings
        .iter()
        .map(|binding| StagedStreamBinding::new(binding, options, factory, budget))
        .collect::<StorageResult<Vec<_>>>()?;
    let first = &bindings[0];
    if bindings.len() > 1
        && first.reader.reader().uses_pull_framing()
        && bindings
            .iter()
            .all(|binding| binding.reader.id() == first.reader.id())
    {
        let extra = bindings[1..]
            .iter()
            .map(|binding| (binding.id, Arc::clone(&binding.definition)))
            .collect();
        let stats = super::file_preparation::process_pull_framed_file(
            pool,
            factory,
            root,
            Arc::clone(&first.definition),
            first.id,
            first.reader.clone(),
            file,
            source_files,
            options.bigram_alphabet,
            options.inference,
            options.detailed_profiling_enabled(),
            options.segment_policy.target_records.get(),
            Some(budget),
            cancelled,
            file_window,
            extra,
            |binding, batch| {
                staged[binding].admit(batch, options, collection_count, pool, cancelled)
            },
        )?;
        for binding in &mut staged {
            binding.stats.file_fingerprint = stats.file_fingerprint;
        }
        staged[0].stats = stats;
    } else {
        for (binding, output) in bindings.iter().zip(&mut staged) {
            output.stats = super::file_preparation::process_framed_file_with_budget(
                pool,
                factory,
                Arc::clone(&root),
                Arc::clone(&binding.definition),
                binding.id,
                binding.reader.clone(),
                file,
                Arc::clone(&source_files),
                options.bigram_alphabet,
                options.inference,
                options.detailed_profiling_enabled(),
                options.segment_policy.target_records.get(),
                Some(budget),
                cancelled,
                file_window,
                |batch| output.admit(batch, options, collection_count, pool, cancelled),
            )?;
        }
    }
    Ok(staged)
}

fn finish_stream_draft(
    draft: Box<dyn SegmentBuilder>,
    definition: &CollectionDefinition,
    factory: &dyn StorageFactory,
    options: RepositoryOptions,
    collection_count: usize,
    pool: &rayon::ThreadPool,
) -> Result<InitialSegment, String> {
    let built = draft.seal(SegmentSeal {
        id: SegmentId::new(0),
        definition,
        options,
        collection_count,
        pool,
        profile: None,
        observe_inference: true,
        declared_orders: factory.declared_order_store(),
    })?;
    Ok((built.index, built.observation))
}

fn partition_file_jobs<'a>(
    batch: &'a [FileId],
    max_files: usize,
    record_target: usize,
    needs_chunks: impl Fn(FileId) -> bool,
    records_upper_bound: impl Fn(FileId) -> usize,
) -> Vec<&'a [FileId]> {
    let mut jobs = Vec::new();
    let mut start = 0;
    while start < batch.len() {
        let mut end = start + 1;
        if !needs_chunks(batch[start]) {
            let mut records = records_upper_bound(batch[start]);
            while end < batch.len() && end - start < max_files && !needs_chunks(batch[end]) {
                let next = records.saturating_add(records_upper_bound(batch[end]));
                if next > record_target {
                    break;
                }
                records = next;
                end += 1;
            }
        }
        jobs.push(&batch[start..end]);
        start = end;
    }
    jobs
}

fn group_stream_files(
    files: &[FileId],
    size_of: impl Fn(FileId) -> Result<u64, String>,
    options: RepositoryOptions,
    bindings: Arc<[StreamBinding]>,
    groups: &mut Vec<StreamGroup>,
) -> Result<(), IndexError> {
    let file_limit = options
        .segment_policy
        .max_files
        .get()
        .min(options.segment_policy.target_records.get());
    for batch in super::batch::file_batches(files, size_of).map_err(IndexError::Build)? {
        for files in batch.chunks(file_limit) {
            groups.push(StreamGroup {
                bindings: Arc::clone(&bindings),
                files: files.to_vec(),
            });
        }
    }
    Ok(())
}

fn run_stream_groups(
    groups: Vec<StreamGroup>,
    pool: &Arc<rayon::ThreadPool>,
    factory: &Arc<dyn crate::index::storage::StorageFactory>,
    root: Arc<PathBuf>,
    source_files: &Arc<Vec<SourceFileInput>>,
    options: RepositoryOptions,
    collection_count: usize,
    budget: &Arc<super::file_preparation::StreamBudget>,
    cancelled: &(impl Fn() -> bool + Sync),
    mut publish: impl FnMut(Vec<Vec<StagedStreamBinding>>) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    use std::sync::{Mutex, mpsc};

    if groups.is_empty() {
        return Ok(());
    }
    let window = pool
        .current_num_threads()
        .min(4)
        .min(groups.iter().map(|group| group.files.len()).sum());
    let stop = AtomicBool::new(false);
    let should_stop = || stop.load(Ordering::Acquire) || cancelled();
    type Job = (usize, FileId, Arc<[StreamBinding]>);
    let use_external_framers = pool.current_num_threads() > 1
        && groups
            .iter()
            .flat_map(|group| &group.files)
            .any(|file| source_files[file.0 as usize].revision.size > 8 * 1024 * 1024);
    let (sender, receiver) =
        mpsc::channel::<(usize, FileId, Result<Vec<StagedStreamBinding>, String>)>();
    let run_file = |(group, file, bindings): Job| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            stage_stream_file(
                pool,
                factory,
                Arc::clone(&root),
                &bindings,
                file,
                Arc::clone(source_files),
                options,
                collection_count,
                budget,
                window,
                &should_stop,
            )
        }))
        .unwrap_or_else(|_| Err("streamed file build worker panicked".into()));
        (group, file, result)
    };
    let drive = |dispatch_file: &mut dyn FnMut(Job) -> Result<(), IndexError>| {
        let mut work = VecDeque::from(groups);
        let mut next_group = 0;
        let mut dispatch = VecDeque::new();
        let mut outstanding = 0;
        let mut waiting =
            BTreeMap::<usize, (usize, BTreeMap<FileId, Vec<StagedStreamBinding>>)>::new();
        let result = (|| {
            while outstanding != 0 || !work.is_empty() || !dispatch.is_empty() {
                while outstanding < window && !cancelled() {
                    if dispatch.is_empty() {
                        let Some(group) = work.pop_front() else { break };
                        waiting.insert(next_group, (group.files.len(), BTreeMap::new()));
                        dispatch.extend(
                            group
                                .files
                                .into_iter()
                                .map(|file| (next_group, file, Arc::clone(&group.bindings))),
                        );
                        next_group += 1;
                    }
                    dispatch_file(dispatch.pop_front().expect("stream job"))?;
                    outstanding += 1;
                }
                if cancelled() {
                    return Err(IndexError::Cancelled);
                }
                let (group, file, result) = receiver
                    .recv()
                    .map_err(|error| IndexError::Build(error.to_string()))?;
                outstanding -= 1;
                if cancelled() {
                    return Err(IndexError::Cancelled);
                }
                let (count, staged) = waiting.get_mut(&group).expect("active stream group");
                staged.insert(file, result.map_err(IndexError::Build)?);
                if staged.len() == *count {
                    let (_, staged) = waiting.remove(&group).expect("complete stream group");
                    publish(staged.into_values().collect())?;
                }
            }
            Ok(())
        })();
        stop.store(true, Ordering::Release);
        drop(waiting);
        // Keep receiving until every dispatched file returns. The scope also
        // joins the coordinators before the terminal outcome becomes visible.
        for _ in 0..outstanding {
            if receiver.recv().is_err() {
                break;
            }
        }
        result
    };
    if use_external_framers {
        // Large registered inputs need spare workers for their decoder jobs.
        // Only these builds take a bounded set of external framing threads.
        let (jobs, pending) = mpsc::channel::<Job>();
        let pending = Mutex::new(pending);
        std::thread::scope(|scope| {
            for _ in 0..window {
                let pending = &pending;
                let sender = &sender;
                let run_file = &run_file;
                scope.spawn(move || {
                    loop {
                        let job = pending
                            .lock()
                            .expect("registered job queue poisoned")
                            .recv();
                        let Ok(job) = job else { break };
                        if sender.send(run_file(job)).is_err() {
                            break;
                        }
                    }
                });
            }
            let outcome = drive(&mut |job| {
                jobs.send(job)
                    .map_err(|_| IndexError::Build("registered job queue disconnected".into()))
            });
            drop(jobs);
            outcome
        })
    } else {
        pool.in_place_scope(|scope| {
            drive(&mut |job| {
                let sender = &sender;
                let run_file = &run_file;
                scope.spawn(move |_| {
                    let _ = sender.send(run_file(job));
                });
                Ok(())
            })
        })
    }
}

struct IncrementalCollection {
    definition: Arc<CollectionDefinition>,
    source: PreparedSource,
    id: CollectionId,
    member_files: Vec<FileId>,
    admission: Box<dyn SegmentBuilder>,
    factory: Arc<dyn StorageFactory>,
    records: u32,
    observed_batches: u64,
    logical_records: RoaringBitmap,
    options: RepositoryOptions,
}

impl IncrementalCollection {
    fn new(
        definition: CollectionDefinition,
        source: PreparedSource,
        id: CollectionId,
        member_files: Vec<FileId>,
        options: RepositoryOptions,
        factory: Arc<dyn StorageFactory>,
    ) -> StorageResult<Self> {
        let definition = Arc::new(definition);
        let admission = factory.start_file_segment(FileSegmentBuild {
            collection: id,
            source: &source,
            policy: options.segment_policy,
        })?;
        Ok(Self {
            definition,
            source,
            id,
            member_files,
            admission,
            factory,
            records: 0,
            observed_batches: 0,
            logical_records: RoaringBitmap::new(),
            options,
        })
    }

    fn observe(
        &mut self,
        batch: &BatchContribution,
        mut profile: Option<&mut AdmissionBreakdown>,
    ) -> Result<(), String> {
        let base = self.records;
        let count = u32::try_from(batch.record_count()).map_err(|_| "too many batch records")?;
        self.records = base
            .checked_add(count)
            .ok_or("collection record slot overflow")?;
        self.observed_batches = self
            .observed_batches
            .checked_add(1)
            .ok_or("inference batch count overflow")?;
        let coordinator = crate::measurement::timer::start_if_enabled(profile.is_some());
        let logical = batch.logical_slots();
        if logical.last().is_some_and(|slot| slot >= count) {
            return Err("worker logical slot exceeds its batch".into());
        }
        self.logical_records
            .append(logical.iter().map(|slot| base + slot))
            .map_err(|_| "observed logical slots overlap or are out of order")?;
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
    ) -> Result<Option<Box<dyn SegmentBuilder>>, String> {
        self.observe(&batch, profile.as_deref_mut())?;
        let coordinator = crate::measurement::timer::start_if_enabled(profile.is_some());
        let sealed = if !self.admission.can_admit(&batch)? {
            self.take_segment()?
        } else {
            None
        };
        if let (Some(started), Some(profile)) = (coordinator, profile.as_mut()) {
            profile.coordinator_ns += started.elapsed().as_nanos() as u64;
        }
        self.admission.admit(batch, profile)?;
        Ok(sealed)
    }

    /// A large framed file is never split between publications. Its chunks
    /// remain one logical file contribution until framing/validation finish.
    fn admit_large_chunk_profiled(
        &mut self,
        batch: BatchContribution,
        mut profile: Option<&mut AdmissionBreakdown>,
    ) -> Result<Option<Box<dyn SegmentBuilder>>, String> {
        self.observe(&batch, profile.as_deref_mut())?;
        let coordinator = crate::measurement::timer::start_if_enabled(profile.is_some());
        let sealed = if self.admission.file_count() != 0 && !self.admission.can_admit(&batch)? {
            self.take_segment()?
        } else {
            None
        };
        if let (Some(started), Some(profile)) = (coordinator, profile.as_mut()) {
            profile.coordinator_ns += started.elapsed().as_nanos() as u64;
        }
        self.admission.admit(batch, profile)?;
        if sealed.is_none() && self.admission.at_capacity() {
            return self.take_segment().map_err(String::from);
        }
        Ok(sealed)
    }

    fn take_segment(&mut self) -> StorageResult<Option<Box<dyn SegmentBuilder>>> {
        if self.admission.file_count() == 0 {
            return Ok(None);
        }
        let replacement = self.factory.start_file_segment(FileSegmentBuild {
            collection: self.id,
            source: &self.source,
            policy: self.options.segment_policy,
        })?;
        Ok(Some(std::mem::replace(&mut self.admission, replacement)))
    }
}

impl SegmentBuildScheduler {
    fn publish_ready(
        &mut self,
        index: &Index,
        expected: &mut IndexRevision,
        pending: &[IncrementalCollection],
        on_publication: &mut impl FnMut(&Index) -> Result<(), IndexError>,
        publication_ns: &mut u64,
        profile: &mut Option<BuildProfileStats>,
    ) -> Result<(), IndexError> {
        for collection_index in 0..self.next_publish.len() {
            loop {
                let position = self.next_publish[collection_index];
                let Some(&id) = self.planned_ids[collection_index].get(position) else {
                    break;
                };
                let Some(result) = self.buffered.remove(&(collection_index, id)) else {
                    break;
                };
                let built = result.map_err(IndexError::Build)?;
                publish_indexed_segment(
                    index,
                    expected,
                    built,
                    pending
                        .get(collection_index)
                        .ok_or_else(|| IndexError::Build("segment collection missing".into()))?,
                    on_publication,
                    publication_ns,
                    profile,
                )?;
                self.outstanding = self.outstanding.saturating_sub(1);
                self.next_publish[collection_index] = position.checked_add(1).ok_or_else(|| {
                    IndexError::ResourceLimit("segment publication cursor overflow".into())
                })?;
            }
        }
        Ok(())
    }

    fn drain(
        &mut self,
        index: &Index,
        expected: &mut IndexRevision,
        pending: &[IncrementalCollection],
        on_publication: &mut impl FnMut(&Index) -> Result<(), IndexError>,
        publication_ns: &mut u64,
        profile: &mut Option<BuildProfileStats>,
    ) -> Result<(), IndexError> {
        while self.outstanding != 0 {
            self.receive(true).map_err(IndexError::Build)?;
            self.publish_ready(
                index,
                expected,
                pending,
                on_publication,
                publication_ns,
                profile,
            )?;
            if self.buffered.len() > self.limit {
                return Err(IndexError::ResourceLimit(
                    "segment completion window exceeded".into(),
                ));
            }
        }
        Ok(())
    }
}

fn publish_indexed_segment(
    index: &Index,
    expected: &mut IndexRevision,
    built: IndexedSegmentBuild,
    collection: &IncrementalCollection,
    on_publication: &mut impl FnMut(&Index) -> Result<(), IndexError>,
    publication_ns: &mut u64,
    profile: &mut Option<BuildProfileStats>,
) -> Result<(), IndexError> {
    let started = crate::measurement::timer::DetailedInstant::now();
    let IndexedSegmentBuild {
        index: indexed,
        observation,
        profile: mut segment_profile,
        build_ns,
    } = built;
    if let (Some(profile), Some(segment)) = (profile.as_mut(), segment_profile.as_mut()) {
        profile.publication_index_ns = profile.publication_index_ns.saturating_add(build_ns);
        segment.index_ns = build_ns;
    }
    let _ = collection;
    let (revision, phases) =
        index.publish_initial_segment(*expected, indexed, observation, profile.is_some())?;
    *expected = revision;
    if let Some(profile) = profile.as_mut() {
        profile.publication_prepare_ns = profile
            .publication_prepare_ns
            .saturating_add(phases.preparation_ns);
        profile.publication_install_ns = profile
            .publication_install_ns
            .saturating_add(phases.install_ns);
        let segment = segment_profile.as_mut().unwrap();
        segment.prepare_ns = phases.preparation_ns;
        segment.install_ns = phases.install_ns;
    }
    let phase = crate::measurement::timer::start_if_enabled(profile.is_some());
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

#[allow(clippy::too_many_arguments)]
fn publish_private_segment_group(
    scheduler: &mut SegmentBuildScheduler,
    index: &Index,
    expected: &mut IndexRevision,
    mut private_group: PrivateSegmentGroup,
    collection_index: usize,
    pending: &[IncrementalCollection],
    on_publication: &mut impl FnMut(&Index) -> Result<(), IndexError>,
    publication_ns: &mut u64,
    profile: &mut Option<BuildProfileStats>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), IndexError> {
    #[cfg(test)]
    if let Some(gate) = scheduler.gate.as_ref() {
        gate.record_private_final_drain();
    }
    private_group.drain(scheduler).map_err(IndexError::Build)?;
    let mut builds = private_group.into_ordered();
    let total = builds.len();
    if total == 0 {
        scheduler.mark_plan_published(collection_index, 0)?;
        return Ok(());
    }
    let _ = pending
        .get(collection_index)
        .ok_or_else(|| IndexError::Build("segment collection missing".into()))?;
    if cancelled() {
        return Err(IndexError::Cancelled);
    }
    let mut index_build_ns = 0u64;
    for build in &mut builds {
        index_build_ns = index_build_ns.saturating_add(build.build_ns);
        if let (Some(profile), Some(segment)) = (profile.as_mut(), build.profile.as_mut()) {
            segment.index_ns = build.build_ns;
            profile.segments.push(segment.clone());
        }
    }
    if let Some(profile) = profile.as_mut() {
        profile.publication_index_ns = profile.publication_index_ns.saturating_add(index_build_ns);
    }
    let phase = crate::measurement::timer::start_if_enabled(profile.is_some());
    let (revision, phases) = index.publish_initial_segments(
        *expected,
        builds
            .into_iter()
            .map(|build| (build.index, build.observation))
            .collect::<Vec<_>>(),
        profile.is_some(),
        cancelled,
    )?;
    *expected = revision;
    if let (Some(profile), phases) = (profile.as_mut(), phases) {
        profile.publication_prepare_ns = profile
            .publication_prepare_ns
            .saturating_add(phases.preparation_ns);
        profile.publication_install_ns = profile
            .publication_install_ns
            .saturating_add(phases.install_ns);
    }
    scheduler.mark_plan_published(collection_index, total)?;
    on_publication(index)?;
    *publication_ns = publication_ns
        .saturating_add(phase.map_or(0, |started| started.elapsed().as_nanos() as u64));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
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
fn schedule_public_segment(
    scheduler: &mut SegmentBuildScheduler,
    index: &Index,
    expected: &mut IndexRevision,
    pending: &[IncrementalCollection],
    collection_index: usize,
    segment: Box<dyn SegmentBuilder>,
    options: RepositoryOptions,
    pool: &Arc<rayon::ThreadPool>,
    on_publication: &mut impl FnMut(&Index) -> Result<(), IndexError>,
    publication_ns: &mut u64,
    profile: &mut Option<BuildProfileStats>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), IndexError> {
    let definition = Arc::clone(
        &pending
            .get(collection_index)
            .ok_or_else(|| IndexError::Build("segment collection missing".into()))?
            .definition,
    );
    let mut publish_ready = |scheduler: &mut SegmentBuildScheduler| {
        scheduler.publish_ready(
            index,
            expected,
            pending,
            on_publication,
            publication_ns,
            profile,
        )
    };
    schedule_segment(
        scheduler,
        collection_index,
        segment,
        definition,
        options,
        pending.len(),
        pool,
        cancelled,
        &mut publish_ready,
    )
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
    scheduler: &mut SegmentBuildScheduler,
    options: RepositoryOptions,
    pool: &Arc<rayon::ThreadPool>,
    on_publication: &mut impl FnMut(&Index) -> Result<(), IndexError>,
    publication_ns: &mut u64,
    profile: &mut Option<BuildProfileStats>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), String> {
    if cancelled() {
        return Err("initial construction cancelled".into());
    }
    let admission_started = crate::measurement::timer::start_if_enabled(profile.is_some());
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
        schedule_public_segment(
            scheduler,
            index,
            expected,
            pending,
            collection_index,
            sealed,
            options,
            pool,
            on_publication,
            publication_ns,
            profile,
            cancelled,
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
        storage_factory: Arc<dyn crate::index::storage::StorageFactory>,
        on_catalogue: impl FnOnce(&Index) -> Result<(), IndexError>,
        on_publication: impl FnMut(&Index) -> Result<(), IndexError>,
        cancelled: impl Fn() -> bool + Sync,
    ) -> Result<Index, IndexError> {
        Self::build_incremental_with_sources_inner(
            root,
            definitions,
            options,
            sources,
            storage_factory,
            on_catalogue,
            on_publication,
            cancelled,
            None,
        )
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_incremental_with_sources_gated(
        root: PathBuf,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        sources: SourceRegistry,
        on_catalogue: impl FnOnce(&Index) -> Result<(), IndexError>,
        on_publication: impl FnMut(&Index) -> Result<(), IndexError>,
        cancelled: impl Fn() -> bool + Sync,
        gate: Arc<SegmentJobGate>,
    ) -> Result<Index, IndexError> {
        Self::build_incremental_with_sources_inner(
            root,
            definitions,
            options,
            sources,
            crate::index::storage::default_factory(),
            on_catalogue,
            on_publication,
            cancelled,
            Some(gate),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build_incremental_with_sources_inner(
        root: PathBuf,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        sources: SourceRegistry,
        storage_factory: Arc<dyn crate::index::storage::StorageFactory>,
        on_catalogue: impl FnOnce(&Index) -> Result<(), IndexError>,
        mut on_publication: impl FnMut(&Index) -> Result<(), IndexError>,
        cancelled: impl Fn() -> bool + Sync,
        gate: Option<Arc<SegmentJobGate>>,
    ) -> Result<Index, IndexError> {
        let started = Instant::now();
        let mut profile = options
            .detailed_profiling_enabled()
            .then(BuildProfileStats::default);
        let root = root
            .canonicalize()
            .map_err(|error| IndexError::Io(error.to_string()))?;
        let phase = crate::measurement::timer::start_if_enabled(profile.is_some());
        let build_workers = crate::construction::scheduling::resolve_build_workers(
            options.build_workers,
            std::thread::available_parallelism().ok(),
        )
        .map_err(IndexError::Build)?;
        let pool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(build_workers)
                .thread_name(|number| format!("objekte-build-{number}"))
                .build()
                .map_err(|error| IndexError::Build(error.to_string()))?,
        );
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.pool_init_ns = started.elapsed().as_nanos() as u64;
        }
        let discovery_started = Instant::now();
        let mut discovery_writer = storage_factory.start_file_discovery(root.clone())?;
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
            let phase = crate::measurement::timer::start_if_enabled(profile.is_some());
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
                let phase = crate::measurement::timer::start_if_enabled(profile.is_some());
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
                let phase = crate::measurement::timer::start_if_enabled(profile.is_some());
                register_discovery_window(discovery_writer.as_mut(), id, prepared, &mut members)?;
                if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
                    profile.discovery_ordered_insert_ns = profile
                        .discovery_ordered_insert_ns
                        .saturating_add(started.elapsed().as_nanos() as u64);
                }
            }
            selected.push((definition, id, source, members));
        }
        let discovery_ns = discovery_started.elapsed().as_nanos() as u64;
        if let Some(profile) = profile.as_mut() {
            profile.discovery_catalogue_ns = discovery_ns;
            profile.discovery_metadata_peak_active = metadata_peak_active.load(Ordering::Relaxed);
            profile.discovery_rss_bytes = build_profile_rss_bytes();
        }
        let setup_started = crate::measurement::timer::start_if_enabled(profile.is_some());
        let mut pending = selected
            .into_iter()
            .map(|(definition, id, source, members)| {
                IncrementalCollection::new(
                    definition,
                    source,
                    id,
                    members,
                    options,
                    Arc::clone(&storage_factory),
                )
            })
            .collect::<StorageResult<Vec<_>>>()?;
        let mut repository = crate::construction::repository_state::initialize_empty(
            root.clone(),
            definitions.clone(),
            options,
            &sources,
            Some(&pool),
            Arc::clone(&storage_factory),
        )
        .map_err(IndexError::Build)?;
        let discovered = discovery_writer.seal(repository.configuration_identity)?;
        let file_count = discovered.high_water_mark() as usize;
        let mut source_files = Vec::with_capacity(file_count);
        let mut membership_counts = Vec::with_capacity(file_count);
        let mut shared = BTreeMap::<Vec<CollectionId>, Vec<FileId>>::new();
        let mut registered_files = 0usize;
        for row in FileRows::new(&discovered) {
            let entry = row?.entry;
            if entry.id.0 as usize != source_files.len()
                || entry
                    .collections
                    .iter()
                    .any(|id| id.0 as usize >= pending.len())
            {
                return Err(IndexError::Build(
                    "invalid discovered file IDs or collection membership".into(),
                ));
            }
            if entry.collections.len() > 1 {
                shared
                    .entry(entry.collections.clone())
                    .or_default()
                    .push(entry.id);
            }
            if registered_files < 2
                && entry
                    .collections
                    .iter()
                    .any(|id| pending[id.0 as usize].source.schedules_separate_file_jobs())
            {
                registered_files += 1;
            }
            membership_counts.push(entry.collections.len());
            source_files.push(SourceFileInput::from_entry(&entry));
        }
        if source_files.len() != file_count {
            return Err(IndexError::Build(
                "discovered file IDs contain holes".into(),
            ));
        }
        let source_files = Arc::new(source_files);
        let source_size = |id: FileId| {
            source_files
                .get(id.0 as usize)
                .map(|file| file.revision.size)
                .ok_or_else(|| "missing scheduled source metadata".to_owned())
        };
        repository.catalog = Arc::clone(&discovered);
        for (collection, state) in repository.collections.iter_mut().zip(&pending) {
            collection.generation =
                CollectionBuilder::new(state.id, state.definition.name.clone(), 1, file_count)
                    .finish_empty()
                    .map_err(IndexError::Build)?;
            collection.stats.file_count = state.member_files.len();
            collection.stats.total_file_bytes = state
                .member_files
                .iter()
                .map(|file| source_files[file.0 as usize].revision.size)
                .sum();
        }
        repository.startup.build_discovery_ns = discovery_ns;
        repository.startup.build_workers = build_workers;
        let mut published = PublishedIndex::from_repository_owned(
            next_repository_instance(),
            IndexRevision::INITIAL,
            repository,
        )?;
        published.begin_initial_construction()?;
        let index = Index::from_published(published);
        on_catalogue(&index)?;
        if let (Some(started), Some(profile)) = (setup_started, profile.as_mut()) {
            profile.builder_setup_ns = started.elapsed().as_nanos() as u64;
        }
        if cancelled() {
            return Err(IndexError::Cancelled);
        }
        let mut expected = IndexRevision::INITIAL;
        let mut scheduler = SegmentBuildScheduler::new(
            pool.current_num_threads(),
            pending.len(),
            storage_factory.declared_order_store(),
            gate,
        );
        let phase = crate::measurement::timer::start_if_enabled(profile.is_some());
        let mut stream_groups = Vec::new();
        let parallel_registered_files = registered_files > 1;
        let mut jobs = Vec::new();
        let mut large_jobs = Vec::new();
        for (collection_index, state) in pending.iter().enumerate() {
            let ordinary;
            let members = if shared.is_empty() {
                state.member_files.as_slice()
            } else {
                ordinary = state
                    .member_files
                    .iter()
                    .copied()
                    .filter(|file| membership_counts[file.0 as usize] == 1)
                    .collect::<Vec<_>>();
                &ordinary
            };
            if parallel_registered_files && state.source.schedules_separate_file_jobs() {
                group_stream_files(
                    members,
                    source_size,
                    options,
                    Arc::from([StreamBinding {
                        definition: Arc::clone(&state.definition),
                        reader: state.source.clone(),
                        id: state.id,
                    }]),
                    &mut stream_groups,
                )?;
                continue;
            }
            let record_bound = |file: FileId| {
                state
                    .source
                    .reader()
                    .maximum_records(source_files[file.0 as usize].revision.size)
                    .max(1)
            };
            let record_target = options.segment_policy.target_records.get() as u64;
            let needs_chunks = |file: FileId| {
                let size = source_files[file.0 as usize].revision.size;
                state.source.schedules_separate_file_jobs()
                    || size > 8 * 1024 * 1024
                    || record_bound(file) > record_target
            };
            let batches =
                super::batch::file_batches(members, source_size).map_err(IndexError::Build)?;
            for batch in batches.into_iter().flat_map(|batch| {
                partition_file_jobs(
                    batch,
                    options.segment_policy.max_files.get(),
                    options.segment_policy.target_records.get(),
                    needs_chunks,
                    |file| record_bound(file) as usize,
                )
            }) {
                if batch.len() == 1 && needs_chunks(batch[0]) {
                    large_jobs.push(ScheduledBuildJob::Large {
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
        if parallel_registered_files {
            let mut group_result = Ok(());
            shared.retain(|bindings, files| {
                if group_result.is_err() {
                    return true;
                }
                let streams = bindings
                    .iter()
                    .any(|id| pending[id.0 as usize].source.schedules_separate_file_jobs());
                if streams {
                    let bindings = bindings
                        .iter()
                        .map(|id| {
                            let state = &pending[id.0 as usize];
                            StreamBinding {
                                definition: Arc::clone(&state.definition),
                                reader: state.source.clone(),
                                id: *id,
                            }
                        })
                        .collect::<Arc<[_]>>();
                    group_result = group_stream_files(
                        files,
                        source_size,
                        options,
                        bindings,
                        &mut stream_groups,
                    );
                }
                !streams
            });
            group_result?;
        }
        let shared_pull_file = |bindings: &[CollectionId], file: FileId| {
            let size = source_files[file.0 as usize].revision.size;
            bindings.iter().any(|id| {
                let reader = pending[id.0 as usize].source.reader();
                reader.uses_pull_framing()
                    && (size > 8 * 1024 * 1024
                        || reader.maximum_records(size)
                            > options.segment_policy.target_records.get() as u64)
            })
        };
        let grouped_files = stream_groups
            .iter()
            .map(|group| group.files.len())
            .sum::<usize>();
        let independently_framed_files = large_jobs.iter().filter(|job| {
            matches!(job, ScheduledBuildJob::Large { source, .. } if source.reader().uses_pull_framing())
        }).take(2).count() + shared.iter().flat_map(|(bindings, files)| {
            files.iter().filter(|&&file| shared_pull_file(bindings, file))
        }).take(2).count();
        let parallel_pull =
            pool.current_num_threads() > 1 && grouped_files + independently_framed_files > 1;
        if parallel_pull {
            large_jobs.retain(|job| {
                if let ScheduledBuildJob::Large {
                    collection,
                    definition,
                    source,
                    file,
                    ..
                } = job
                    && source.reader().uses_pull_framing()
                {
                    stream_groups.push(StreamGroup {
                        bindings: Arc::from([StreamBinding {
                            definition: Arc::clone(definition),
                            reader: source.clone(),
                            id: *collection,
                        }]),
                        files: vec![*file],
                    });
                    return false;
                }
                true
            });
        }
        shared.retain(|ids, files| {
            let first = &pending[ids[0].0 as usize].source;
            let shared_reader = first.reader().uses_pull_framing()
                && ids
                    .iter()
                    .all(|id| pending[id.0 as usize].source.id() == first.id());
            if !parallel_pull && !shared_reader {
                return true;
            }
            let mut bindings: Option<Arc<[StreamBinding]>> = None;
            files.retain(|&file| {
                if !shared_pull_file(ids, file) {
                    return true;
                }
                let bindings = bindings.get_or_insert_with(|| {
                    ids.iter()
                        .map(|id| {
                            let state = &pending[id.0 as usize];
                            StreamBinding {
                                definition: Arc::clone(&state.definition),
                                reader: state.source.clone(),
                                id: *id,
                            }
                        })
                        .collect()
                });
                stream_groups.push(StreamGroup {
                    bindings: Arc::clone(bindings),
                    files: vec![file],
                });
                false
            });
            !files.is_empty()
        });
        // Publish bounded ordinary partitions before holding a large stream.
        // File positions and generated IDs come from discovery and framing.
        jobs.extend(large_jobs);
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.job_scheduling_ns = started.elapsed().as_nanos() as u64;
        }
        let extraction_started = crate::measurement::timer::DetailedInstant::now();
        let mut digest_bindings = vec![Vec::new(); file_count];
        let mut build_parses = 0u64;
        let mut failed_parses = 0u64;
        let mut source_reads = 0u64;
        let mut source_bytes_read = 0u64;
        let mut schedule_stats = crate::construction::scheduling::BuildScheduleStats::default();
        let mut publication_ns = 0u64;
        let streaming_budget = Arc::new(index.view()?.repository().stream_budget.clone());
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
                        let result = crate::construction::scheduling::process_repository_file_batches_profiled(
                            &pool,
                            &storage_factory,
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
                                    &mut scheduler,
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
                    if let Some(sealed) = pending[collection_index].take_segment()? {
                        schedule_public_segment(
                            &mut scheduler,
                            &index,
                            &mut expected,
                            &pending,
                            collection_index,
                            sealed,
                            options,
                            &pool,
                            &mut on_publication,
                            &mut publication_ns,
                            &mut profile,
                            &cancelled,
                        )?;
                    }
                    scheduler.drain(
                        &index,
                        &mut expected,
                        &pending,
                        &mut on_publication,
                        &mut publication_ns,
                        &mut profile,
                    )?;
                    let mut private_group =
                        PrivateSegmentGroup::new(collection_index, &streaming_budget);
                    let definition_for_private = Arc::clone(&definition);
                    let collection_count = pending.len();
                    let process_result =
                        crate::construction::file_preparation::process_framed_file_with_budget(
                            &pool,
                            &storage_factory,
                            Arc::new(root.clone()),
                            definition,
                            collection,
                            source,
                            file,
                            Arc::clone(&source_files),
                            options.bigram_alphabet,
                            options.inference,
                            options.detailed_profiling_enabled(),
                            options.segment_policy.target_records.get(),
                            Some(&streaming_budget),
                            &cancelled,
                            1,
                            |batch| {
                                if cancelled() {
                                    return Err("initial construction cancelled".into());
                                }
                                let admission_started =
                                    crate::measurement::timer::start_if_enabled(profile.is_some());
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
                                if let Some(segment) = pending[collection_index]
                                    .admit_large_chunk_profiled(batch, detail.as_mut())?
                                {
                                    schedule_private_segment(
                                        &mut scheduler,
                                        &mut private_group,
                                        collection_index,
                                        segment,
                                        Arc::clone(&definition_for_private),
                                        options,
                                        collection_count,
                                        &pool,
                                        &cancelled,
                                    )
                                    .map_err(|error| error.to_string())?;
                                }
                                finish_admission_profile(
                                    &mut profile,
                                    &pending[collection_index].definition.name,
                                    admission_started,
                                    detail,
                                )
                            },
                        );
                    let result = match process_result {
                        Ok(result) => result,
                        Err(error) => {
                            let drain_error = private_group.discard(&mut scheduler).err();
                            let error = match drain_error {
                                Some(drain) => format!("{error}; private segment drain: {drain}"),
                                None => error,
                            };
                            return Err(if cancelled() {
                                IndexError::Cancelled
                            } else {
                                IndexError::Build(error)
                            });
                        }
                    };
                    #[cfg(test)]
                    if definition_for_private.name == "__test_cancel_after_validation__"
                        && let Some(gate) = &scheduler.gate
                    {
                        gate.wait_after_validation();
                    }
                    #[cfg(test)]
                    if definition_for_private.name
                        == "__test_invalidate_after_private_finalization__"
                    {
                        let consume_error = private_group.consume_ready(&mut scheduler, true).err();
                        let finalized = private_group.indexed.len();
                        let drain_error = private_group.discard(&mut scheduler).err();
                        if consume_error.is_none() && finalized == 0 {
                            return Err(IndexError::Build(
                                "late invalidation test observed no finalized segment".into(),
                            ));
                        }
                        return Err(IndexError::Build(match drain_error {
                            Some(error) => format!(
                                "injected late invalidation; private segment drain: {error}"
                            ),
                            None => consume_error.unwrap_or_else(|| {
                                format!(
                                    "injected late framed-file invalidation after {finalized} finalized segment(s)"
                                )
                            }),
                        }));
                    }
                    if cancelled() {
                        let drain_error = private_group.discard(&mut scheduler).err();
                        return Err(match drain_error {
                            Some(error) => IndexError::Build(error),
                            None => IndexError::Cancelled,
                        });
                    }
                    if let Some((file, digest)) = result.file_fingerprint {
                        digest_bindings[file.0 as usize].push((collection, digest));
                    }
                    schedule_stats.absorb(result);
                    let submit_result = pending[collection_index]
                        .take_segment()
                        .map_err(IndexError::from)
                        .and_then(|sealed| {
                            if let Some(sealed) = sealed {
                                schedule_private_segment(
                                    &mut scheduler,
                                    &mut private_group,
                                    collection_index,
                                    sealed,
                                    Arc::clone(&definition_for_private),
                                    options,
                                    collection_count,
                                    &pool,
                                    &cancelled,
                                )?;
                            }
                            Ok(())
                        });
                    if let Err(error) = submit_result {
                        let drain_error = private_group.discard(&mut scheduler).err();
                        return Err(match (cancelled(), drain_error) {
                            (true, _) => IndexError::Cancelled,
                            (false, Some(drain)) => IndexError::Build(format!(
                                "{error}; private segment drain: {drain}"
                            )),
                            (false, None) => error,
                        });
                    }
                    let publish_result = publish_private_segment_group(
                        &mut scheduler,
                        &index,
                        &mut expected,
                        private_group,
                        collection_index,
                        &pending,
                        &mut on_publication,
                        &mut publication_ns,
                        &mut profile,
                        &cancelled,
                    );
                    if let Err(error) = publish_result {
                        return Err(if cancelled() {
                            IndexError::Cancelled
                        } else {
                            error
                        });
                    }
                }
            }
        }
        if !normal.is_empty() {
            let result = crate::construction::scheduling::process_repository_file_batches_profiled(
                &pool,
                &storage_factory,
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
                        &mut scheduler,
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
        let flush_started = crate::measurement::timer::start_if_enabled(profile.is_some());
        for number in 0..pending.len() {
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            if let Some(sealed) = pending[number].take_segment()? {
                schedule_public_segment(
                    &mut scheduler,
                    &index,
                    &mut expected,
                    &pending,
                    number,
                    sealed,
                    options,
                    &pool,
                    &mut on_publication,
                    &mut publication_ns,
                    &mut profile,
                    &cancelled,
                )?;
            }
        }
        scheduler.drain(
            &index,
            &mut expected,
            &pending,
            &mut on_publication,
            &mut publication_ns,
            &mut profile,
        )?;
        let collection_count = pending.len();
        run_stream_groups(
            stream_groups,
            &pool,
            &storage_factory,
            Arc::new(root.clone()),
            &source_files,
            options,
            collection_count,
            &streaming_budget,
            &cancelled,
            |files| {
                let mut segments = Vec::new();
                let mut private_work = Vec::new();
                let mut sealed_tails =
                    streaming_budget.activity(StreamActivity::PrivateSegments, 0, 0);
                let mut tails = BTreeMap::<CollectionId, Box<dyn SegmentBuilder>>::new();
                for bindings in files {
                    for binding in bindings {
                        let id = binding.collection.id;
                        let state = &mut pending[id.0 as usize];
                        append_shifted_bitmap(
                            &mut state.logical_records,
                            &binding.collection.logical_records,
                            state.records,
                        )
                        .map_err(IndexError::Build)?;
                        state.records = state
                            .records
                            .checked_add(binding.collection.records)
                            .ok_or_else(|| {
                                IndexError::ResourceLimit("collection record slot overflow".into())
                            })?;
                        state.observed_batches += binding.collection.observed_batches;
                        build_parses += binding.parses;
                        failed_parses += binding.failed;
                        if let Some((file, digest)) = binding.stats.file_fingerprint {
                            digest_bindings[file.0 as usize].push((id, digest));
                        }
                        schedule_stats.absorb(binding.stats);
                        segments.extend(binding.segments);
                        private_work.push(binding.private);
                        let new_tail = || {
                            storage_factory.start_file_segment(FileSegmentBuild {
                                collection: id,
                                source: &state.source,
                                policy: options.segment_policy,
                            })
                        };
                        let tail = match tails.entry(id) {
                            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                            std::collections::btree_map::Entry::Vacant(entry) => {
                                entry.insert(new_tail()?)
                            }
                        };
                        for batch in binding.tail {
                            if !tail.can_admit(&batch)? {
                                let draft = std::mem::replace(tail, new_tail()?);
                                let built = finish_stream_draft(
                                    draft,
                                    &state.definition,
                                    storage_factory.as_ref(),
                                    options,
                                    collection_count,
                                    &pool,
                                )
                                .map_err(IndexError::Build)?;
                                sealed_tails.add_private_segment(&built.0);
                                segments.push(built);
                            }
                            tail.admit(batch, None)?;
                        }
                    }
                }
                for (id, tail) in tails {
                    if tail.file_count() != 0 {
                        let built = finish_stream_draft(
                            tail,
                            &pending[id.0 as usize].definition,
                            storage_factory.as_ref(),
                            options,
                            collection_count,
                            &pool,
                        )
                        .map_err(IndexError::Build)?;
                        sealed_tails.add_private_segment(&built.0);
                        segments.push(built);
                    }
                }
                let phase = crate::measurement::timer::start_if_enabled(profile.is_some());
                let (revision, phases) = index.publish_initial_segments(
                    expected,
                    segments,
                    profile.is_some(),
                    &cancelled,
                )?;
                expected = revision;
                drop(private_work);
                drop(sealed_tails);
                if let Some(profile) = profile.as_mut() {
                    profile.publication_prepare_ns += phases.preparation_ns;
                    profile.publication_install_ns += phases.install_ns;
                }
                on_publication(&index)?;
                publication_ns += phase.map_or(0, |started| started.elapsed().as_nanos() as u64);
                Ok(())
            },
        )?;
        // Ordinary partitions can publish independently. A shared group
        // stays private until the last binding has finished each of its files.
        for (bindings, files) in shared {
            let registered = bindings
                .iter()
                .any(|id| pending[id.0 as usize].source.schedules_separate_file_jobs());
            let readers: Vec<_> = bindings
                .iter()
                .map(|id| pending[id.0 as usize].source.clone())
                .collect();
            let record_bound = |file: FileId| {
                let size = source_files[file.0 as usize].revision.size;
                readers
                    .iter()
                    .map(|source| source.reader().maximum_records(size))
                    .max()
                    .unwrap_or(1)
                    .max(1)
            };
            let needs_chunks = |file: FileId| {
                let size = source_files[file.0 as usize].revision.size;
                registered
                    || size > 8 * 1024 * 1024
                    || record_bound(file) > options.segment_policy.target_records.get() as u64
            };
            let batches = super::batch::file_batches(&files, source_size)
                .map_err(IndexError::Build)?
                .into_iter()
                .flat_map(|batch| {
                    partition_file_jobs(
                        batch,
                        options.segment_policy.max_files.get(),
                        options.segment_policy.target_records.get(),
                        needs_chunks,
                        |file| record_bound(file) as usize,
                    )
                })
                .collect::<Vec<_>>();
            for files in batches {
                let mut builds = Vec::new();
                let mut private_work =
                    streaming_budget.activity(StreamActivity::PrivateSegments, 0, 0);
                let collection_count = pending.len();
                let mut ordinal = index.view()?.0.next_segment_ordinal;
                for id in &bindings {
                    let state = &mut pending[id.0 as usize];
                    let source = state.source.clone();
                    let definition = Arc::clone(&state.definition);
                    let mut seal = |draft: Box<dyn SegmentBuilder>| -> Result<(), String> {
                        let id = SegmentId::new(ordinal);
                        ordinal = ordinal.checked_add(1).ok_or("segment identity overflow")?;
                        let built = draft.seal(SegmentSeal {
                            id,
                            definition: &*definition,
                            options,
                            collection_count,
                            pool: &pool,
                            profile: None,
                            observe_inference: true,
                            declared_orders: storage_factory.declared_order_store(),
                        })?;
                        private_work.add_private_segment(&built.index);
                        builds.push((built.index, built.observation));
                        Ok(())
                    };
                    let mut admit = |batch: BatchContribution| -> Result<(), String> {
                        if cancelled() {
                            return Err("initial construction cancelled".into());
                        }
                        build_parses += batch.record_parses;
                        failed_parses += batch.failed_parses;
                        source_reads += batch.source_reads;
                        source_bytes_read += batch.source_bytes_read;
                        for &(file, digest) in batch.file_fingerprints() {
                            digest_bindings[file.0 as usize].push((*id, digest));
                        }
                        if let Some(profile) = profile.as_mut() {
                            profile.work.absorb(&batch.profile);
                            profile
                                .work_by_collection
                                .entry(definition.name.clone())
                                .or_default()
                                .absorb(&batch.profile);
                        }
                        if let Some(draft) = state.admit_large_chunk_profiled(batch, None)? {
                            seal(draft)?;
                        }
                        Ok(())
                    };
                    if files.len() == 1 && needs_chunks(files[0]) {
                        let stats =
                            crate::construction::file_preparation::process_framed_file_with_budget(
                                &pool,
                                &storage_factory,
                                Arc::new(root.clone()),
                                Arc::clone(&definition),
                                *id,
                                source,
                                files[0],
                                Arc::clone(&source_files),
                                options.bigram_alphabet,
                                options.inference,
                                options.detailed_profiling_enabled(),
                                options.segment_policy.target_records.get(),
                                Some(&streaming_budget),
                                &cancelled,
                                1,
                                &mut admit,
                            )
                            .map_err(|error| {
                                if cancelled() {
                                    IndexError::Cancelled
                                } else {
                                    IndexError::Build(error)
                                }
                            })?;
                        if let Some((file, digest)) = stats.file_fingerprint {
                            digest_bindings[file.0 as usize].push((*id, digest));
                        }
                        schedule_stats.absorb(stats);
                    } else {
                        let batch = pool
                            .install(|| {
                                crate::construction::batch::process_batch_profiled(
                                    storage_factory.as_ref(),
                                    &root,
                                    &definition,
                                    *id,
                                    &source,
                                    files,
                                    &source_files,
                                    options.bigram_alphabet,
                                    options.inference,
                                    options.inference.max_build_jobs(pool.current_num_threads()),
                                    options.detailed_profiling_enabled(),
                                )
                            })
                            .map_err(IndexError::Build)?;
                        admit(batch).map_err(|error| {
                            if cancelled() {
                                IndexError::Cancelled
                            } else {
                                IndexError::Build(error)
                            }
                        })?;
                    }
                    if let Some(draft) = state.take_segment()? {
                        seal(draft).map_err(IndexError::Build)?;
                    }
                }
                if cancelled() {
                    return Err(IndexError::Cancelled);
                }
                let phase = crate::measurement::timer::start_if_enabled(profile.is_some());
                let (revision, phases) = index.publish_initial_segments(
                    expected,
                    builds,
                    profile.is_some(),
                    &cancelled,
                )?;
                expected = revision;
                if let Some(profile) = profile.as_mut() {
                    profile.publication_prepare_ns += phases.preparation_ns;
                    profile.publication_install_ns += phases.install_ns;
                }
                on_publication(&index)?;
                publication_ns += phase.map_or(0, |started| started.elapsed().as_nanos() as u64);
            }
        }
        schedule_stats.stream_peak = streaming_budget.peak();
        let extraction_index_ns = extraction_started.elapsed().as_nanos() as u64;
        if let Some(profile) = profile.as_mut() {
            profile.source_work_ns = extraction_index_ns;
            profile.admission_schedule = schedule_stats.admission_schedule;
            profile.extraction_rss_bytes = build_profile_rss_bytes();
        }
        let peak_segment_jobs = scheduler.peak_running.load(Ordering::Relaxed);
        let peak_segment_window = scheduler.peak_outstanding;
        if let Some(profile) = profile.as_mut() {
            profile.peak_segment_jobs = peak_segment_jobs;
            profile.peak_segment_window = peak_segment_window;
        }
        if let (Some(started), Some(profile)) = (flush_started, profile.as_mut()) {
            profile.final_segment_flush_ns = started.elapsed().as_nanos() as u64;
        }
        let final_collections = pending
            .iter()
            .map(
                |state| crate::construction::repository_state::FinalCollectionObservation {
                    id: state.id,
                    records: state.records as u64,
                    observed_batches: state.observed_batches,
                },
            )
            .collect::<Vec<_>>();
        let (mut final_repository, file_base) =
            crate::construction::repository_state::assemble_final_repository(
                root,
                definitions,
                options,
                sources,
                &pool,
                &index,
                discovered,
                digest_bindings,
                &final_collections,
                build_parses,
                failed_parses,
                source_reads,
                source_bytes_read,
                schedule_stats,
                build_workers,
                peak_segment_jobs,
                peak_segment_window,
                publication_ns,
                discovery_ns,
                extraction_index_ns,
                started,
                profile,
            )?;
        if cancelled() {
            return Err(IndexError::Cancelled);
        }
        let instance_id = index.instance_id();
        final_repository.instance_id = instance_id;
        for collection in &mut final_repository.collections {
            collection.repository_instance = instance_id;
        }
        index.finish_initial_construction(expected, final_repository, Box::new(file_base))?;
        Ok(index)
    }
}
