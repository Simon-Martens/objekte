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
use crate::construction::discovery::{prepare_discovered_file, prepare_discovered_window};
use crate::construction::scheduling::RepositoryFileJob;
use crate::construction::segment_admission::{AdmissionBreakdown, SegmentAdmission};
use crate::index::segment::SegmentId;
use crate::inventory::discovery::discover;
use crate::inventory::{CollectionBuilder, FileInventory, append_shifted_bitmap};
use crate::lifecycle::{Index, IndexError, IndexRevision, PublishedIndex};
use crate::model::{CollectionId, FileId};
use crate::repository::configuration::{CollectionDefinition, RepositoryOptions};
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
    crate::index::segment::IndexedCollectionSegment,
    Option<Arc<crate::construction::inference::InferenceObservation>>,
);

struct StagedStreamBinding {
    collection: IncrementalCollection,
    segments: Vec<InitialSegment>,
    tail: Vec<BatchContribution>,
    private: StreamUse,
    _unsealed: StreamUse,
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

fn stage_stream_binding(
    pool: &rayon::ThreadPool,
    root: Arc<PathBuf>,
    binding: &StreamBinding,
    file: FileId,
    source_files: Arc<Vec<SourceFileInput>>,
    options: RepositoryOptions,
    collection_count: usize,
    budget: &super::file_preparation::StreamBudget,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<StagedStreamBinding, String> {
    if cancelled() {
        return Err("initial construction cancelled".into());
    }
    let StreamBinding {
        definition,
        reader,
        id,
    } = binding;
    let mut collection = IncrementalCollection::new(
        (**definition).clone(),
        reader.clone(),
        *id,
        Vec::new(),
        options,
        collection_count,
    );
    let mut segments = Vec::new();
    let mut private = budget.activity(StreamActivity::PrivateSegments, 0, 0);
    let mut unsealed = budget.activity(StreamActivity::Unsealed, 0, 0);
    let mut tail = Vec::new();
    let mut tail_records = 0;
    let mut parses = 0;
    let mut failed = 0;
    let stats = super::file_preparation::process_framed_file_with_budget(
        pool,
        root,
        Arc::clone(definition),
        *id,
        reader.clone(),
        file,
        source_files,
        options.bigram_alphabet,
        options.inference,
        options.detailed_profiling_enabled(),
        options.segment_policy.target_records.get(),
        Some(budget),
        cancelled,
        true,
        |batch| {
            if cancelled() {
                return Err("initial construction cancelled".into());
            }
            parses += batch.record_parses;
            failed += batch.failed_parses;
            collection.observe(&batch, None)?;
            tail_records += batch.record_count();
            tail.push(batch);
            unsealed.set(tail.len() as u64, 0);
            if tail_records == options.segment_policy.target_records.get() {
                for batch in tail.drain(..) {
                    collection.admission.admit_batch(batch)?;
                }
                unsealed.set(0, 0);
                let built = finish_stream_draft(
                    collection
                        .take_segment()
                        .expect("full private stream draft"),
                    definition,
                    options,
                    collection_count,
                    pool,
                )?;
                private.add_private_segment(&built.0);
                segments.push(built);
                tail_records = 0;
            }
            Ok(())
        },
    )?;
    Ok(StagedStreamBinding {
        collection,
        segments,
        tail,
        private,
        _unsealed: unsealed,
        stats,
        parses,
        failed,
    })
}

fn finish_stream_draft(
    draft: super::segment_admission::CollectionSegment,
    definition: &CollectionDefinition,
    options: RepositoryOptions,
    collection_count: usize,
    pool: &rayon::ThreadPool,
) -> Result<InitialSegment, String> {
    let (index, observation) = draft.into_indexed_observation_profiled(
        definition,
        options.bigram_alphabet,
        options.inference,
        collection_count,
        pool,
        None,
    )?;
    Ok((index, observation.map(Arc::new)))
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
    inventory: &FileInventory,
    options: RepositoryOptions,
    bindings: Arc<[StreamBinding]>,
    groups: &mut Vec<StreamGroup>,
) {
    let file_limit = options
        .segment_policy
        .max_files
        .get()
        .min(options.segment_policy.target_records.get());
    for batch in super::batch::file_batches(files, inventory) {
        for files in batch.chunks(file_limit) {
            groups.push(StreamGroup {
                bindings: Arc::clone(&bindings),
                files: files.to_vec(),
            });
        }
    }
}

fn run_stream_groups(
    groups: Vec<StreamGroup>,
    pool: &Arc<rayon::ThreadPool>,
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
    let window = pool.current_num_threads().min(4);
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
            bindings
                .iter()
                .map(|binding| {
                    stage_stream_binding(
                        pool,
                        Arc::clone(&root),
                        binding,
                        file,
                        Arc::clone(source_files),
                        options,
                        collection_count,
                        budget,
                        &should_stop,
                    )
                })
                .collect::<Result<Vec<_>, _>>()
        }))
        .unwrap_or_else(|_| Err("registered file build worker panicked".into()));
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
    admission: SegmentAdmission,
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
        _collection_count: usize,
    ) -> Self {
        let definition = Arc::new(definition);
        let admission = SegmentAdmission::with_limits_and_id(
            id,
            &source,
            options.segment_policy,
            SegmentId::new(0),
        );
        Self {
            definition,
            source,
            id,
            member_files,
            admission,
            records: 0,
            observed_batches: 0,
            logical_records: RoaringBitmap::new(),
            options,
        }
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
    ) -> Result<Option<crate::construction::segment_admission::CollectionSegment>, String> {
        self.observe(&batch, profile.as_deref_mut())?;
        let coordinator = crate::measurement::timer::start_if_enabled(profile.is_some());
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
    ) -> Result<Option<crate::construction::segment_admission::CollectionSegment>, String> {
        self.observe(&batch, profile.as_deref_mut())?;
        let coordinator = crate::measurement::timer::start_if_enabled(profile.is_some());
        let sealed = if self.admission.file_count() != 0 && !self.admission.can_admit_batch(&batch)
        {
            self.take_segment()
        } else {
            None
        };
        if let (Some(started), Some(profile)) = (coordinator, profile.as_mut()) {
            profile.coordinator_ns += started.elapsed().as_nanos() as u64;
        }
        self.admission.admit_batch_profiled(batch, profile)?;
        Ok(sealed.or_else(|| {
            self.admission
                .at_capacity()
                .then(|| self.take_segment())
                .flatten()
        }))
    }

    fn take_segment(
        &mut self,
    ) -> Option<crate::construction::segment_admission::CollectionSegment> {
        if self.admission.file_count() == 0 {
            return None;
        }
        let replacement = SegmentAdmission::with_limits_and_id(
            self.id,
            &self.source,
            self.options.segment_policy,
            SegmentId::new(0),
        );
        Some(std::mem::replace(&mut self.admission, replacement).finish())
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
    segment: crate::construction::segment_admission::CollectionSegment,
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
        on_catalogue: impl FnOnce(&Index) -> Result<(), IndexError>,
        on_publication: impl FnMut(&Index) -> Result<(), IndexError>,
        cancelled: impl Fn() -> bool + Sync,
    ) -> Result<Index, IndexError> {
        Self::build_incremental_with_sources_inner(
            root,
            definitions,
            options,
            sources,
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
        let setup_started = crate::measurement::timer::start_if_enabled(profile.is_some());
        let discovered = Arc::new(files);
        let source_files = Arc::new(
            discovered
                .files
                .iter()
                .map(SourceFileInput::from_entry)
                .collect::<Vec<_>>(),
        );
        let collection_count = selected.len();
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
                )
            })
            .collect::<Vec<_>>();
        let mut repository = crate::construction::repository_state::initialize_empty(
            root.clone(),
            definitions.clone(),
            options,
            &sources,
            Some(&pool),
        )
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
        let mut published = PublishedIndex::from_repository_owned(
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
        let mut scheduler =
            SegmentBuildScheduler::new(pool.current_num_threads(), pending.len(), gate);
        let phase = crate::measurement::timer::start_if_enabled(profile.is_some());
        let mut shared = std::collections::BTreeMap::<Vec<CollectionId>, Vec<FileId>>::new();
        for entry in &discovered.files {
            if entry.collections.len() > 1 {
                shared
                    .entry(entry.collections.clone())
                    .or_default()
                    .push(entry.id);
            }
        }
        let mut stream_groups = Vec::new();
        let registered_files = discovered
            .files
            .iter()
            .filter(|file| {
                file.collections
                    .iter()
                    .any(|id| pending[id.0 as usize].source.schedules_separate_file_jobs())
            })
            .take(2)
            .count();
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
                    .filter(|file| discovered.files[file.0 as usize].collections.len() == 1)
                    .collect::<Vec<_>>();
                &ordinary
            };
            if parallel_registered_files && state.source.schedules_separate_file_jobs() {
                group_stream_files(
                    members,
                    &discovered,
                    options,
                    Arc::from([StreamBinding {
                        definition: Arc::clone(&state.definition),
                        reader: state.source.clone(),
                        id: state.id,
                    }]),
                    &mut stream_groups,
                );
                continue;
            }
            let record_bound = |file: FileId| {
                state
                    .source
                    .reader()
                    .maximum_records(discovered.files[file.0 as usize].revision.size)
                    .max(1)
            };
            let record_target = options.segment_policy.target_records.get() as u64;
            let needs_chunks = |file: FileId| {
                let size = discovered.files[file.0 as usize].revision.size;
                state.source.schedules_separate_file_jobs()
                    || size > 8 * 1024 * 1024
                    || record_bound(file) > record_target
            };
            let batches = super::batch::file_batches(members, &discovered);
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
            shared.retain(|bindings, files| {
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
                    group_stream_files(files, &discovered, options, bindings, &mut stream_groups);
                }
                !streams
            });
        }
        // Publish bounded ordinary partitions before holding a large stream.
        // File positions and generated IDs come from discovery and framing.
        jobs.extend(large_jobs);
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.job_scheduling_ns = started.elapsed().as_nanos() as u64;
        }
        let extraction_started = crate::measurement::timer::DetailedInstant::now();
        let mut digest_bindings = vec![Vec::new(); discovered.files.len()];
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
                    if let Some(sealed) = pending[collection_index].take_segment() {
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
                            false,
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
                    if let Some(sealed) = pending[collection_index].take_segment() {
                        let submit_result = schedule_private_segment(
                            &mut scheduler,
                            &mut private_group,
                            collection_index,
                            sealed,
                            Arc::clone(&definition_for_private),
                            options,
                            collection_count,
                            &pool,
                            &cancelled,
                        );
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
            if let Some(sealed) = pending[number].take_segment() {
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
                let mut tails = BTreeMap::<CollectionId, SegmentAdmission>::new();
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
                            SegmentAdmission::with_limits_and_id(
                                id,
                                &state.source,
                                options.segment_policy,
                                SegmentId::new(0),
                            )
                        };
                        let tail = tails.entry(id).or_insert_with(new_tail);
                        for batch in binding.tail {
                            if !tail.can_admit_batch(&batch) {
                                let draft = std::mem::replace(tail, new_tail()).finish();
                                let built = finish_stream_draft(
                                    draft,
                                    &state.definition,
                                    options,
                                    collection_count,
                                    &pool,
                                )
                                .map_err(IndexError::Build)?;
                                sealed_tails.add_private_segment(&built.0);
                                segments.push(built);
                            }
                            tail.admit_batch(batch).map_err(IndexError::Build)?;
                        }
                    }
                }
                for (id, tail) in tails {
                    if tail.file_count() != 0 {
                        let built = finish_stream_draft(
                            tail.finish(),
                            &pending[id.0 as usize].definition,
                            options,
                            collection_count,
                            &pool,
                        )
                        .map_err(IndexError::Build)?;
                        sealed_tails.add_private_segment(&built.0);
                        segments.push(built);
                    }
                }
                let mut ordinal = index.view()?.0.next_segment_ordinal;
                for (segment, _) in &mut segments {
                    segment.id = SegmentId::new(ordinal);
                    ordinal = ordinal.checked_add(1).ok_or_else(|| {
                        IndexError::ResourceLimit("segment identity overflow".into())
                    })?;
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
                let size = discovered.files[file.0 as usize].revision.size;
                readers
                    .iter()
                    .map(|source| source.reader().maximum_records(size))
                    .max()
                    .unwrap_or(1)
                    .max(1)
            };
            let needs_chunks = |file: FileId| {
                let size = discovered.files[file.0 as usize].revision.size;
                registered
                    || size > 8 * 1024 * 1024
                    || record_bound(file) > options.segment_policy.target_records.get() as u64
            };
            let batches = super::batch::file_batches(&files, &discovered)
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
                    let mut seal = |mut draft: crate::construction::segment_admission::CollectionSegment| -> Result<(), String> {
                        draft.id = SegmentId::new(ordinal);
                        ordinal = ordinal.checked_add(1).ok_or("segment identity overflow")?;
                        let (index, observation) = draft.into_indexed_observation_profiled(
                            &*definition, options.bigram_alphabet, options.inference,
                            collection_count, &pool, None,
                        )?;
                        private_work.add_private_segment(&index);
                        builds.push((index, observation.map(Arc::new)));
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
                                false,
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
                    if let Some(draft) = state.take_segment() {
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
                    definition: Arc::clone(&state.definition),
                    records: state.records as u64,
                    observed_batches: state.observed_batches,
                },
            )
            .collect::<Vec<_>>();
        let mut final_repository =
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
        index.finish_initial_construction(expected, Arc::new(final_repository))?;
        Ok(index)
    }
}
