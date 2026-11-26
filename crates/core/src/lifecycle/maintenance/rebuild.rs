use std::time::{Duration, Instant};

use crate::index::storage::file_catalog::{
    FILE_TRANSFER_LIMIT, FileCatalogDelta, FileCatalogSource, edit_file_catalog, transfer_files,
};
use crate::index::storage::segment::{
    RecordBacking, RecordOrigin, SegmentReader, visit_checked_file_slots, visit_segment_files,
};
use crate::inventory::FileVersion;
use crate::lifecycle::initial::InitialBuildPhase;
use crate::lifecycle::maintenance::MaintenancePhase;
use crate::lifecycle::mutation::preparation::{index_prepared_file, prepare_file_at};
use crate::lifecycle::{Index, IndexError, IndexRevision};

/// Outcome of explicit full reconstruction from the configured source files.
#[derive(Clone, Debug)]
pub struct RebuildReport {
    pub revision: IndexRevision,
    pub attempts: usize,
    pub indexed_files: usize,
    pub live_records: u64,
    pub duration: Duration,
}
impl Index {
    /// Rebuild from the current explicit configuration. Queries keep using
    /// their pinned prior view while source extraction runs. A concurrent
    /// commit makes the candidate stale and causes a fresh bounded retry.
    pub fn rebuild(&self) -> Result<RebuildReport, IndexError> {
        self.rebuild_with_before_publish(|_| {})
    }

    pub(crate) fn rebuild_with_before_publish(
        &self,
        mut before_publish: impl FnMut(usize),
    ) -> Result<RebuildReport, IndexError> {
        self.rebuild_with_control(|| false, |_, _| {}, |attempt| before_publish(attempt))
    }

    pub(crate) fn rebuild_with_control(
        &self,
        cancelled: impl Fn() -> bool,
        mut progress: impl FnMut(MaintenancePhase, usize),
        mut before_publish: impl FnMut(usize),
    ) -> Result<RebuildReport, IndexError> {
        let started = Instant::now();
        let mut conflict = None;
        for attempt in 1..=3 {
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            progress(MaintenancePhase::Preparing, attempt);
            let view = self.view()?;
            let repository = view.repository();
            if repository.programmatic {
                // These registrations own their records independently of the
                // file catalog. Their immutable indexed segments already hold
                // the retained input needed by a new publication. Validate
                // versioned external sources before publishing them again.
                for group in view.0.segment_groups()? {
                    for segment in group.iter() {
                        let descriptor = segment.descriptor();
                        match segment.backing_kind() {
                            RecordBacking::Retained => {}
                            RecordBacking::Provider => {
                                let registration =
                                    segment.source().registration.ok_or_else(|| {
                                        IndexError::Build("provider registration missing".into())
                                    })?;
                                let source = repository
                                    .record_registrations
                                    .get(registration as usize)
                                    .ok_or_else(|| {
                                        IndexError::Build("provider registration missing".into())
                                    })?;
                                let reader = source.reader.as_ref().ok_or_else(|| {
                                    IndexError::SourceUnavailable("provider reader missing".into())
                                })?;
                                for slot in segment.active.bitmap()?.iter() {
                                    if cancelled() {
                                        return Err(IndexError::Cancelled);
                                    }
                                    let slot = crate::model::RecordSlot(slot);
                                    let record = segment
                                        .header(slot)?
                                        .filter(|record| record.slot == slot)
                                        .ok_or_else(|| {
                                            IndexError::Build("provider record missing".into())
                                        })?;
                                    let Some(RecordOrigin::Provider {
                                        registration: owner,
                                        version: expected,
                                        ..
                                    }) = segment.origin(slot)?
                                    else {
                                        return Err(IndexError::Build(
                                            "provider version missing".into(),
                                        ));
                                    };
                                    if owner != registration {
                                        return Err(IndexError::InvalidPublication(
                                            "provider registration changed".into(),
                                        ));
                                    }
                                    let key = crate::records::RecordKey::new(
                                        descriptor.collection,
                                        record.id.clone(),
                                    );
                                    let read = reader
                                        .read_projected(
                                            &key,
                                            expected,
                                            &crate::records::reader::RecordProjection::full(),
                                        )
                                        .map_err(|error| {
                                            IndexError::SourceUnavailable(error.to_string())
                                        })?;
                                    if read.version != expected {
                                        return Err(IndexError::SourceUnavailable(
                                            "provider returned a different record version".into(),
                                        ));
                                    }
                                }
                            }
                            RecordBacking::FileRange => {
                                visit_segment_files(segment.as_ref(), |file| {
                                    if cancelled() {
                                        return Err(IndexError::Cancelled);
                                    }
                                    if let Some(entry) = view.file_entry(file.id)?
                                        && entry.revision == file.revision
                                        && entry.created_ns == file.created_ns
                                        && view
                                            .0
                                            .imported_collections(&entry)?
                                            .contains(&descriptor.collection)
                                    {
                                        return Ok(());
                                    }
                                    let mut live = false;
                                    visit_checked_file_slots(
                                        segment.as_ref(),
                                        file.id,
                                        &mut |slots| {
                                            live |= segment.active.any(slots)?;
                                            Ok(())
                                        },
                                    )?;
                                    if live {
                                        return Err(IndexError::InvalidPublication(
                                            "obsolete file incarnation still has live records"
                                                .into(),
                                        ));
                                    }
                                    Ok(())
                                })?;
                            }
                        }
                    }
                }
                let mut next = (*view.0).clone();
                let mut observed = Vec::new();
                let catalog = &view.0.catalog;
                transfer_files(
                    catalog,
                    catalog.high_water_mark(),
                    FILE_TRANSFER_LIMIT,
                    &cancelled,
                    |row| -> Result<(), IndexError> {
                        if cancelled() {
                            return Err(IndexError::Cancelled);
                        }
                        let old = row.entry.as_ref();
                        if view.0.imported_collections(old)?.is_empty() {
                            return Ok(());
                        }
                        let relative = old.relative_path.as_path();
                        let prepared = prepare_file_at(&view, relative, old.id)?;
                        if prepared.source.revision != old.revision
                            || prepared.source.created_ns != old.created_ns
                        {
                            return Err(IndexError::SourceUnavailable(format!(
                                "file changed during rebuild: {}",
                                relative.display()
                            )));
                        }
                        let backend = prepared.reader(repository);
                        let (mut entry, indexed) = index_prepared_file(
                            repository,
                            &prepared.source,
                            prepared.collections,
                            next.next_segment_ordinal,
                            repository.options().segment_policy,
                        )?;
                        next.retire_file_records(old)?;
                        for collection in next.refresh_record_links(old.id, Some(&entry))? {
                            if !entry.collections.contains(&collection) {
                                entry.collections.push(collection);
                            }
                        }
                        next.catalog = next.catalog.replaced_incarnation(
                            entry,
                            prepared.fingerprint,
                            Some(view.configuration_identity()),
                        )?;

                        observed.push((
                            prepared.source,
                            prepared.identity,
                            prepared.fingerprint,
                            backend,
                        ));
                        for segment in indexed {
                            next.stage_owned_segment(segment)?;
                        }
                        Ok(())
                    },
                )?;
                next.revision = view.revision().next()?;
                if cancelled() {
                    return Err(IndexError::Cancelled);
                }
                progress(MaintenancePhase::Publishing, attempt);
                before_publish(attempt);
                if cancelled() {
                    return Err(IndexError::Cancelled);
                }
                for (source, identity, fingerprint, backend) in observed {
                    crate::construction::file_preparation::verify_current_source(
                        repository.required_file_root(),
                        &source,
                        identity,
                        fingerprint,
                        backend.map(|source| source.reader()),
                    )
                    .map_err(IndexError::SourceUnavailable)?;
                }
                match self.publish_prepared(view.revision(), next) {
                    Ok(revision) => {
                        let status = self.status()?;
                        return Ok(RebuildReport {
                            revision,
                            attempts: attempt,
                            indexed_files: status.indexed_files,
                            live_records: status.live_records,
                            duration: started.elapsed(),
                        });
                    }
                    Err(error @ IndexError::RevisionConflict { .. }) => {
                        conflict = Some(error);
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            }
            let definitions = repository
                .collections()
                .iter()
                .map(|collection| collection.required_file_definition().clone())
                .collect::<Vec<_>>();
            let task = Self::start_build_with_factory(
                repository.required_file_root().to_path_buf(),
                definitions,
                repository.options(),
                repository.sources.clone(),
                std::sync::Arc::clone(&repository.storage_factory),
            )?;
            let fresh = loop {
                if cancelled() {
                    task.cancel();
                    let _ = task.wait_complete();
                    return Err(IndexError::Cancelled);
                }
                match task.status()?.phase {
                    InitialBuildPhase::Complete => break task.wait_complete()?,
                    InitialBuildPhase::Cancelled => return Err(IndexError::Cancelled),
                    InitialBuildPhase::Failed => {
                        return match task.wait_complete() {
                            Err(error) => Err(error),
                            Ok(_) => unreachable!("failed build task cannot complete successfully"),
                        };
                    }
                    InitialBuildPhase::Discovering | InitialBuildPhase::Building => {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
            };
            let next_revision = view.revision().next()?;
            let mut next = (*fresh.view()?.0).clone();
            next.instance_id = self.instance_id();
            next.revision = next_revision;
            next.catalog = edit_file_catalog(
                next.catalog.as_ref(),
                [FileCatalogDelta::ResetVersions(FileVersion(
                    next_revision.0,
                ))],
            )?;
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            progress(MaintenancePhase::Publishing, attempt);
            before_publish(attempt);
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            match self.publish_prepared(view.revision(), next) {
                Ok(revision) => {
                    let status = self.status()?;
                    return Ok(RebuildReport {
                        revision,
                        attempts: attempt,
                        indexed_files: status.indexed_files,
                        live_records: status.live_records,
                        duration: started.elapsed(),
                    });
                }
                Err(error @ IndexError::RevisionConflict { .. }) => conflict = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(conflict.expect("rebuild retry always captures a revision conflict"))
    }
}
