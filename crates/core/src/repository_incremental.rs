//! Single-pass initial construction into immutable lifecycle segments.
//! Discovery supplies one shared path inventory. Completed work units move
//! into segments; the final metadata repository shares that same inventory.

use super::*;

use crate::build::{
    AdmissionBreakdown, BatchContribution, IndexedCollectionSegment, RepositoryFileJob,
    SegmentAdmission, SegmentId, SourceFileInput,
};
use crate::inventory::append_shifted_bitmap;
use crate::lifecycle::{Index, IndexError, IndexRevision, PublishedIndex};
use crate::model::FileId;

pub(crate) struct SegmentJobGate {
    state: std::sync::Mutex<(usize, bool)>,
    changed: std::sync::Condvar,
}

#[allow(dead_code)]
impl SegmentJobGate {
    pub(crate) fn new() -> Self {
        Self {
            state: std::sync::Mutex::new((0, false)),
            changed: std::sync::Condvar::new(),
        }
    }

    fn wait(&self) {
        let mut state = self.state.lock().expect("segment gate lock poisoned");
        state.0 += 1;
        self.changed.notify_all();
        while !state.1 {
            state = self
                .changed
                .wait(state)
                .expect("segment gate lock poisoned");
        }
    }

    pub(crate) fn wait_for_workers(&self, count: usize) {
        let mut state = self.state.lock().expect("segment gate lock poisoned");
        while state.0 < count {
            state = self
                .changed
                .wait(state)
                .expect("segment gate lock poisoned");
        }
    }

    pub(crate) fn release(&self) {
        let mut state = self.state.lock().expect("segment gate lock poisoned");
        state.1 = true;
        self.changed.notify_all();
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
    ) -> Result<Option<crate::build::CollectionSegment>, String> {
        self.observe(&batch, profile.as_deref_mut())?;
        let coordinator = crate::detailed_timer::start_if_enabled(profile.is_some());
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

    fn take_segment(&mut self) -> Option<crate::build::CollectionSegment> {
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

    fn finish_summary(
        &mut self,
    ) -> Result<(Option<GlobalInferenceSummary>, InferenceBuildStats), String> {
        Ok((
            None,
            if self.options.inference.mode == crate::inference::InferenceMode::Disabled {
                InferenceBuildStats::default()
            } else {
                InferenceBuildStats {
                    observed_records: self.records as u64,
                    merged_batches: self.observed_batches,
                    ..Default::default()
                }
            },
        ))
    }
}

struct IndexedSegmentBuild {
    index: IndexedCollectionSegment,
    profile: Option<IncrementalSegmentProfile>,
    build_ns: u64,
}

struct SegmentBuildMessage {
    collection_index: usize,
    segment_id: u64,
    result: Result<IndexedSegmentBuild, String>,
}

struct SegmentBuildScheduler {
    sender: std::sync::mpsc::Sender<SegmentBuildMessage>,
    receiver: std::sync::mpsc::Receiver<SegmentBuildMessage>,
    buffered: std::collections::BTreeMap<(usize, u64), Result<IndexedSegmentBuild, String>>,
    planned_ids: Vec<Vec<u64>>,
    next_publish: Vec<usize>,
    next_plan_id: u64,
    outstanding: usize,
    peak_outstanding: usize,
    limit: usize,
    running: Arc<AtomicUsize>,
    peak_running: Arc<AtomicUsize>,
    gate: Option<Arc<SegmentJobGate>>,
}

impl SegmentBuildScheduler {
    fn new(workers: usize, collections: usize, gate: Option<Arc<SegmentJobGate>>) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel();
        Self {
            sender,
            receiver,
            buffered: std::collections::BTreeMap::new(),
            planned_ids: vec![Vec::new(); collections],
            next_publish: vec![0; collections],
            next_plan_id: 1,
            outstanding: 0,
            peak_outstanding: 0,
            limit: workers.saturating_mul(2).max(1),
            running: Arc::new(AtomicUsize::new(0)),
            peak_running: Arc::new(AtomicUsize::new(0)),
            gate,
        }
    }

    fn submit(
        &mut self,
        collection_index: usize,
        mut segment: crate::build::CollectionSegment,
        definition: Arc<CollectionDefinition>,
        options: RepositoryOptions,
        collection_count: usize,
        pool: &Arc<rayon::ThreadPool>,
    ) -> Result<(), String> {
        let planned = self
            .planned_ids
            .get_mut(collection_index)
            .ok_or("segment collection out of bounds")?;
        let segment_id = self.next_plan_id;
        self.next_plan_id = self
            .next_plan_id
            .checked_add(1)
            .ok_or("segment plan overflow")?;
        segment.id = SegmentId::new(segment_id);
        planned.push(segment_id);
        self.outstanding = self
            .outstanding
            .checked_add(1)
            .ok_or("segment scheduler overflow")?;
        self.peak_outstanding = self.peak_outstanding.max(self.outstanding);
        let collection = segment.collection;
        let sender = self.sender.clone();
        let profile_build = options.detailed_profiling_enabled();
        let running = Arc::clone(&self.running);
        let peak_running = Arc::clone(&self.peak_running);
        let gate = self.gate.clone();
        let worker_pool = Arc::clone(pool);
        pool.spawn(move || {
            let active = running.fetch_add(1, Ordering::Relaxed) + 1;
            peak_running.fetch_max(active, Ordering::Relaxed);
            if let Some(gate) = gate {
                gate.wait();
            }
            let phase = crate::detailed_timer::DetailedInstant::now();
            let mut profile = profile_build.then(|| IncrementalSegmentProfile {
                collection: definition.name.clone(),
                files: segment.files.len(),
                records: segment.records.len(),
                source_bytes: segment
                    .files
                    .iter()
                    .map(|file| file.entry.revision.size)
                    .sum(),
                ..Default::default()
            });
            let result = segment
                .into_indexed_profiled(
                    &definition,
                    options.bigram_alphabet,
                    options.inference,
                    collection_count,
                    &worker_pool,
                    profile.as_mut(),
                )
                .map(|index| IndexedSegmentBuild {
                    index,
                    profile,
                    build_ns: phase.elapsed().as_nanos() as u64,
                });
            debug_assert!(match result.as_ref() {
                Ok(built) => built.index.collection == collection,
                Err(_) => true,
            });
            running.fetch_sub(1, Ordering::Relaxed);
            let _ = sender.send(SegmentBuildMessage {
                collection_index,
                segment_id,
                result,
            });
        });
        Ok(())
    }

    fn allocate_plan_ids(
        &mut self,
        collection_index: usize,
        count: usize,
    ) -> Result<Vec<u64>, String> {
        let planned = self
            .planned_ids
            .get_mut(collection_index)
            .ok_or("segment collection out of bounds")?;
        let mut ids = Vec::with_capacity(count);
        for _ in 0..count {
            ids.push(self.next_plan_id);
            self.next_plan_id = self
                .next_plan_id
                .checked_add(1)
                .ok_or("segment plan overflow")?;
            planned.push(ids[ids.len() - 1]);
        }
        Ok(ids)
    }

    fn mark_plan_published(
        &mut self,
        collection_index: usize,
        count: usize,
    ) -> Result<(), IndexError> {
        let cursor = self
            .next_publish
            .get_mut(collection_index)
            .ok_or_else(|| IndexError::InvalidPublication("segment collection missing".into()))?;
        *cursor = cursor.checked_add(count).ok_or_else(|| {
            IndexError::ResourceLimit("segment publication cursor overflow".into())
        })?;
        Ok(())
    }

    fn receive(&mut self, block: bool) -> Result<bool, String> {
        let message = if block {
            self.receiver.recv().map_err(|error| error.to_string())?
        } else {
            match self.receiver.try_recv() {
                Ok(message) => message,
                Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(false),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Err("segment build channel disconnected".into());
                }
            }
        };
        self.buffered.insert(
            (message.collection_index, message.segment_id),
            message.result,
        );
        Ok(true)
    }

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
    let started = crate::detailed_timer::DetailedInstant::now();
    let IndexedSegmentBuild {
        index: indexed,
        profile: mut segment_profile,
        build_ns,
    } = built;
    if let (Some(profile), Some(segment)) = (profile.as_mut(), segment_profile.as_mut()) {
        profile.publication_index_ns = profile.publication_index_ns.saturating_add(build_ns);
        segment.index_ns = build_ns;
    }
    let _ = collection;
    let (revision, phases) =
        index.publish_initial_segment(*expected, indexed, profile.is_some())?;
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

#[allow(clippy::too_many_arguments)]
fn publish_private_segment_group(
    scheduler: &mut SegmentBuildScheduler,
    index: &Index,
    expected: &mut IndexRevision,
    private_segments: Vec<crate::build::CollectionSegment>,
    collection_index: usize,
    pending: &[IncrementalCollection],
    options: RepositoryOptions,
    pool: &Arc<rayon::ThreadPool>,
    on_publication: &mut impl FnMut(&Index) -> Result<(), IndexError>,
    publication_ns: &mut u64,
    profile: &mut Option<BuildProfileStats>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), IndexError> {
    if private_segments.is_empty() {
        return Ok(());
    }
    let total = private_segments.len();
    scheduler.drain(
        index,
        expected,
        pending,
        on_publication,
        publication_ns,
        profile,
    )?;
    let collection = pending
        .get(collection_index)
        .ok_or_else(|| IndexError::Build("segment collection missing".into()))?;
    let ids = scheduler
        .allocate_plan_ids(collection_index, total)
        .map_err(IndexError::Build)?;
    let mut jobs = private_segments
        .into_iter()
        .zip(ids)
        .collect::<std::collections::VecDeque<_>>();
    let mut indexed = Vec::with_capacity(jobs.len());
    let collection_count = scheduler.next_publish.len();
    while !jobs.is_empty() {
        if cancelled() {
            return Err(IndexError::Cancelled);
        }
        let count = scheduler.limit.min(jobs.len());
        let chunk = jobs.drain(..count).collect::<Vec<_>>();
        let built = pool.install(|| {
            use rayon::prelude::*;
            chunk
                .into_par_iter()
                .map(|(mut segment, id)| {
                    segment.id = SegmentId::new(id);
                    segment.into_indexed(
                        &collection.definition,
                        options.bigram_alphabet,
                        options.inference,
                        collection_count,
                        pool,
                    )
                })
                .collect::<Vec<_>>()
        });
        for result in built {
            indexed.push(result.map_err(IndexError::Build)?);
        }
    }
    indexed.sort_by_key(|segment| segment.id);
    let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
    let (revision, phases) =
        index.publish_initial_segments(*expected, indexed, profile.is_some())?;
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
fn schedule_segment(
    scheduler: &mut SegmentBuildScheduler,
    index: &Index,
    expected: &mut IndexRevision,
    pending: &[IncrementalCollection],
    collection_index: usize,
    segment: crate::build::CollectionSegment,
    options: RepositoryOptions,
    pool: &Arc<rayon::ThreadPool>,
    on_publication: &mut impl FnMut(&Index) -> Result<(), IndexError>,
    publication_ns: &mut u64,
    profile: &mut Option<BuildProfileStats>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), IndexError> {
    while scheduler.outstanding >= scheduler.limit {
        if cancelled() {
            return Err(IndexError::Cancelled);
        }
        scheduler.receive(true).map_err(IndexError::Build)?;
        scheduler.publish_ready(
            index,
            expected,
            pending,
            on_publication,
            publication_ns,
            profile,
        )?;
    }
    let collection = pending
        .get(collection_index)
        .ok_or_else(|| IndexError::Build("segment collection missing".into()))?;
    scheduler
        .submit(
            collection_index,
            segment,
            Arc::clone(&collection.definition),
            options,
            pending.len(),
            pool,
        )
        .map_err(IndexError::Build)?;
    while scheduler.receive(false).map_err(IndexError::Build)? {}
    scheduler.publish_ready(
        index,
        expected,
        pending,
        on_publication,
        publication_ns,
        profile,
    )
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
    segments: &[Arc<crate::native_segment::PublishedSegment>],
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
            let file = files
                .files
                .get_mut(record.file.0 as usize)
                .ok_or("segment metadata file missing")?;
            if record.class == RecordClass::Logical {
                builder.add_record(
                    file,
                    record.id.clone(),
                    record.range.start,
                    record.range.end,
                )?;
            } else {
                let segment_file = segment
                    .index
                    .file(record.file)
                    .ok_or("fallback segment file missing")?;
                let Some(local) = record.diagnostic_ref.stored_index() else {
                    builder.add_raw_fallback(
                        file,
                        record.id.clone(),
                        record.range,
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
                        record.range,
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
                    builder.add_raw_fallback(file, record.id.clone(), record.range, reference)?;
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
        schedule_segment(
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
        cancelled: impl Fn() -> bool,
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
        cancelled: impl Fn() -> bool,
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
        cancelled: impl Fn() -> bool,
        gate: Option<Arc<SegmentJobGate>>,
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
                        schedule_segment(
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
                    let mut private_segments = Vec::new();
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
                            if let Some(segment) = pending[collection_index]
                                .admit_large_chunk_profiled(batch, detail.as_mut())?
                            {
                                private_segments.push(segment);
                            }
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
                        private_segments.push(sealed);
                    }
                    publish_private_segment_group(
                        &mut scheduler,
                        &index,
                        &mut expected,
                        private_segments,
                        collection_index,
                        &pending,
                        options,
                        &pool,
                        &mut on_publication,
                        &mut publication_ns,
                        &mut profile,
                        &cancelled,
                    )?;
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
                schedule_segment(
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
        let peak_segment_jobs = scheduler.peak_running.load(Ordering::Relaxed);
        let peak_segment_window = scheduler.peak_outstanding;
        if let Some(profile) = profile.as_mut() {
            profile.peak_segment_jobs = peak_segment_jobs;
            profile.peak_segment_window = peak_segment_window;
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
        final_repository.base_fingerprints = base_fingerprints;
        final_repository.build_parses = build_parses;
        final_repository.build_failed_parses = failed_parses;
        let mut inference_build = InferenceBuildStats::default();
        let segment_view = index.view()?;
        let mut final_files = (*discovered).clone();
        let mut pending_relations = Vec::with_capacity(final_repository.collections.len());
        for file in &mut final_files.files {
            if let Some(current) = segment_view.file_entry(file.id) {
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
        for (collection, state) in final_repository.collections.iter_mut().zip(&mut pending) {
            let segments = segment_view
                .segments(state.id)
                .ok_or_else(|| IndexError::Build("segment collection missing".into()))?;
            let mut relations = RelationContribution::for_schema(&state.definition.schema);
            let metadata = segment_metadata_generation(
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
                collection.sorting = first.index.sorting.clone();
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
            let (_, stats) = state.finish_summary().map_err(IndexError::Build)?;
            collection.global_inference = crate::inference::build::aggregate_segment_inference(
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
        crate::relation::validate_indexes(&final_repository.collections)
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
            build_source_bytes_read: source_bytes_read
                .saturating_add(schedule_stats.source_bytes_read),
            peak_build_admitted_bytes: schedule_stats.peak_weight_bytes,
            build_weight_limit_bytes: schedule_stats.weight_limit_bytes,
            build_peak_active_jobs: schedule_stats.peak_active_jobs,
            build_peak_ready_batches: schedule_stats.peak_ready_batches,
            build_peak_segment_jobs: peak_segment_jobs,
            build_peak_segment_window: peak_segment_window,
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
