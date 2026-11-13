//! Direct fresh construction of bounded lifecycle shards for one-record JSON
//! sources. The static repository API continues to use its own full builder.

use super::*;

use crate::build::{
    BatchContribution, BatchMetadata, IndexedCollectionSegment, RepositoryFileJob,
    SegmentAdmission, SourceFileInput,
};
use crate::inventory::FileEntry;
use crate::inventory::append_shifted_bitmap;
use crate::model::FileId;
use crate::source::SourceFormat;


pub(crate) struct DirectBuild {
    pub(crate) repository: Repository,
    pub(crate) catalog_slots: Vec<Option<(FileEntry, Option<[u8; 32]>, Option<[u8; 32]>)>>,
    pub(crate) segments: Vec<Vec<IndexedCollectionSegment>>,
}


struct DirectCollection {
    definition: Arc<CollectionDefinition>,
    source: PreparedSource,
    id: CollectionId,
    member_files: Vec<FileId>,
    builder: CollectionBuilder,
    inference: Option<InferenceBuilder>,
    logical_records: RoaringBitmap,
    admission: SegmentAdmission,
    sealed: Vec<crate::build::CollectionSegment>,
}


impl DirectCollection {
    fn new(
        definition: CollectionDefinition,
        source: PreparedSource,
        id: CollectionId,
        member_files: Vec<FileId>,
        inventory_files: usize,
        options: RepositoryOptions,
        collection_count: usize,
    ) -> Self {
        let admission = SegmentAdmission::with_limits(
            id,
            &source,
            crate::build::SegmentLimits {
                files: crate::lifecycle_partition::BASE_PARTITION_RECORDS,
                records: crate::lifecycle_partition::BASE_PARTITION_RECORDS,
                source_bytes: 64 * 1024 * 1024,
            },
        );
        let builder = CollectionBuilder::new(id, definition.name.clone(), 1, inventory_files);
        Self {
            definition: Arc::new(definition),
            source,
            id,
            member_files,
            builder,
            inference: InferenceBuilder::for_collection(options.inference, collection_count),
            logical_records: RoaringBitmap::new(),
            admission,
            sealed: Vec::new(),
        }
    }

    fn admit_metadata(
        &mut self,
        inventory: &mut FileInventory,
        metadata: BatchMetadata,
    ) -> Result<(), String> {
        let base = self.builder.next_slot()?.0;
        let count = u32::try_from(metadata.records.len()).map_err(|_| "too many batch records")?;
        base.checked_add(count).ok_or("record slot overflow")?;
        let mut records = metadata.records.into_iter();
        for batch_file in metadata.files {
            let file_id = batch_file.entry.id;
            if batch_file.entry.collections != [self.id] {
                return Err("direct build collection binding changed".into());
            }
            let path = inventory.root.join(&batch_file.entry.relative_path);
            if batch_file.first_range {
                self.builder.include_file(file_id)?;
            } else if !self.builder.contains_file(file_id) {
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
            let file = inventory
                .files
                .get_mut(file_id.0 as usize)
                .ok_or("direct build file missing")?;
            file.status = file.status.max(batch_file.entry.status);
            file.omitted_diagnostics = file
                .omitted_diagnostics
                .saturating_add(batch_file.entry.omitted_diagnostics);
            for expected_slot in batch_file.records {
                let local = records.next().ok_or("batch record range out of bounds")?;
                if local.slot.0 as usize != expected_slot || local.file != file_id {
                    return Err("batch record ownership or order mismatch".into());
                }
                let actual = if local.class == RecordClass::Logical {
                    self.builder
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
                    self.builder
                        .add_raw_fallback(file, local.id, local.range, reference)?
                };
                if actual.0 != base + local.slot.0 {
                    return Err("batch record slot mapping mismatch".into());
                }
            }
        }
        if records.next().is_some() || self.builder.next_slot()?.0 != base + count {
            return Err("incomplete batch record admission".into());
        }
        match (self.inference.as_mut(), metadata.inference) {
            (Some(global), Some(local)) => global.merge_shifted(local, base)?,
            (None, None) => {}
            _ => return Err("inference batch mode mismatch".into()),
        }
        append_shifted_bitmap(&mut self.logical_records, &metadata.logical_records, base)?;
        Ok(())
    }

    fn admit_segment(&mut self, batch: BatchContribution) -> Result<(), String> {
        if !self.admission.can_admit_batch(&batch) {
            let replacement = SegmentAdmission::with_limits(
                self.id,
                &self.source,
                crate::build::SegmentLimits {
                    files: crate::lifecycle_partition::BASE_PARTITION_RECORDS,
                    records: crate::lifecycle_partition::BASE_PARTITION_RECORDS,
                    source_bytes: 64 * 1024 * 1024,
                },
            );
            self.sealed
                .push(std::mem::replace(&mut self.admission, replacement).seal());
        }
        self.admission.admit_batch(batch)
    }

    fn finish(
        mut self,
        inventory: &FileInventory,
        pool: &rayon::ThreadPool,
        options: RepositoryOptions,
        collection_count: usize,
    ) -> Result<
        (
            Option<GlobalInferenceSummary>,
            Vec<IndexedCollectionSegment>,
            InferenceBuildStats,
        ),
        String,
    > {
        let generation = self.builder.finish(inventory)?;
        let count = generation.records.len();
        let mut inference_build = InferenceBuildStats::default();
        let summary = self
            .inference
            .map(|builder| {
                inference_build = builder.build_stats();
                let started = crate::detailed_timer::DetailedInstant::now();
                let state =
                    builder.finish(&self.logical_records, count, &self.definition.schema)?;
                inference_build.finish_ns = started.elapsed().as_nanos() as u64;
                Ok::<_, String>(GlobalInferenceSummary {
                    diagnostics: state.path_diagnostics(),
                    report: state.report,
                })
            })
            .transpose()?;
        if self.admission.file_count() > 0 {
            self.sealed.push(self.admission.seal());
        }
        let segments = self
            .sealed
            .into_iter()
            .map(|segment| {
                segment.into_indexed(
                    &self.definition,
                    options.bigram_alphabet,
                    options.inference,
                    collection_count,
                    pool,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let owned = segments
            .iter()
            .map(|segment| segment.records.len())
            .sum::<usize>();
        if owned != count {
            return Err("direct shard record coverage mismatch".into());
        }
        Ok((summary, segments, inference_build))
    }
}


impl Repository {
    pub(crate) fn build_direct_json(
        root: PathBuf,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        sources: SourceRegistry,
    ) -> Result<Option<DirectBuild>, String> {
        if definitions.iter().any(|definition| {
            !matches!(
                definition.format,
                SourceFormat::Json | SourceFormat::JsonDecodedScalars
            )
        }) {
            return Ok(None);
        }
        options.inference.validate()?;
        crate::relation::validate_definitions(&definitions)?;
        let started = Instant::now();
        let root = root.canonicalize().map_err(|error| error.to_string())?;
        let mut files = FileInventory::new(root.clone());
        let mut selected = Vec::new();
        for (number, definition) in definitions.iter().cloned().enumerate() {
            let id = CollectionId(u32::try_from(number).map_err(|_| "too many collections")?);
            let source = sources.resolve(&definition.format)?;
            let paths = discover(
                &root,
                &definition.source_path,
                &definition.discovery,
                &source,
            )?;
            let mut members = Vec::with_capacity(paths.len());
            for path in paths {
                let relative = path
                    .strip_prefix(&root)
                    .map_err(|error| error.to_string())?;
                let metadata = std::fs::metadata(&path).map_err(|error| error.to_string())?;
                let revision = SourceRevision::from_metadata(&metadata)?;
                if revision.size > 8 * 1024 * 1024 {
                    return Ok(None);
                }
                let created_ns = metadata
                    .created()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_nanos() as i128);
                members.push(files.add(relative.to_owned(), revision, created_ns, id)?);
            }
            selected.push((definition, source, id, members));
        }
        let discovery_ns = started.elapsed().as_nanos() as u64;
        let build_workers = crate::build::resolve_build_workers(
            options.build_workers,
            std::thread::available_parallelism().ok(),
        )?;
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(build_workers)
            .thread_name(|index| format!("objekte-build-{index}"))
            .build()
            .map_err(|error| error.to_string())?;
        let source_files = Arc::new(
            files
                .files
                .iter()
                .map(SourceFileInput::from_entry)
                .collect::<Vec<_>>(),
        );
        let collection_count = selected.len();
        let mut pending = selected
            .into_iter()
            .map(|(definition, source, id, members)| {
                DirectCollection::new(
                    definition,
                    source,
                    id,
                    members,
                    files.files.len(),
                    options,
                    collection_count,
                )
            })
            .collect::<Vec<_>>();
        let jobs = pending
            .iter()
            .enumerate()
            .flat_map(|(collection_index, state)| {
                crate::build::file_batches(&state.member_files, &files)
                    .into_iter()
                    .map(move |batch| RepositoryFileJob {
                        collection_index,
                        collection: state.id,
                        definition: Arc::clone(&state.definition),
                        source: state.source.clone(),
                        files: batch.to_vec(),
                    })
            })
            .collect::<Vec<_>>();
        let mut digest_bindings = vec![Vec::new(); files.files.len()];
        let mut build_parses = 0u64;
        let mut build_failed_parses = 0u64;
        let extraction_started = crate::detailed_timer::DetailedInstant::now();
        let schedule_stats = crate::build::process_repository_file_batches(
            &pool,
            Arc::new(root.clone()),
            &jobs,
            Arc::clone(&source_files),
            options.bigram_alphabet,
            options.inference,
            |collection_index, batch| {
                for &(file, digest) in batch.file_fingerprints() {
                    digest_bindings[file.0 as usize]
                        .push((CollectionId(collection_index as u32), digest));
                }
                build_parses = build_parses.saturating_add(batch.record_parses);
                build_failed_parses = build_failed_parses.saturating_add(batch.failed_parses);
                let metadata = batch.metadata_copy();
                pending[collection_index].admit_metadata(&mut files, metadata)?;
                pending[collection_index].admit_segment(batch)
            },
        )?;
        let extraction_index_ns = extraction_started.elapsed().as_nanos() as u64;
        let base_fingerprints = finish_fresh_fingerprints(&files, digest_bindings)?;
        files.validate_diagnostics()?;
        let configuration = crate::storage::configuration_hash(&definitions, options, &sources)?;
        let catalog_slots = files
            .files
            .iter()
            .zip(&base_fingerprints)
            .map(|(entry, fingerprint)| {
                Some((
                    entry.clone(),
                    *fingerprint,
                    fingerprint.map(|_| configuration),
                ))
            })
            .collect();
        let finalization_started = crate::detailed_timer::DetailedInstant::now();
        let mut summaries = Vec::with_capacity(collection_count);
        let mut segments = Vec::with_capacity(collection_count);
        let mut inference_build = InferenceBuildStats::default();
        for state in pending {
            let (summary, group, stats) = state.finish(&files, &pool, options, collection_count)?;
            summaries.push(summary);
            segments.push(group);
            inference_build.absorb(stats);
        }
        let finalization_ns = finalization_started.elapsed().as_nanos() as u64;
        let mut repository = Self::build_fresh(root, definitions, options, &sources, false)?;
        for (state, summary) in repository.collections.iter_mut().zip(summaries) {
            state.global_inference = summary;
        }
        repository.build_parses = build_parses;
        repository.build_failed_parses = build_failed_parses;
        repository.startup = StartupStats {
            build_ns: started.elapsed().as_nanos() as u64,
            build_discovery_ns: discovery_ns,
            build_extraction_index_ns: extraction_index_ns,
            build_finalization_ns: finalization_ns,
            build_workers,
            peak_build_admitted_bytes: schedule_stats.peak_weight_bytes,
            build_weight_limit_bytes: schedule_stats.weight_limit_bytes,
            build_peak_active_jobs: schedule_stats.peak_active_jobs,
            build_peak_ready_batches: schedule_stats.peak_ready_batches,
            build_job_execution_ns: schedule_stats.job_execution_ns,
            build_receive_wait_ns: schedule_stats.receive_wait_ns,
            build_admission_ns: schedule_stats.admission_ns,
            inference_build,
            ..StartupStats::default()
        };
        Ok(Some(DirectBuild {
            repository,
            catalog_slots,
            segments,
        }))
    }
}
