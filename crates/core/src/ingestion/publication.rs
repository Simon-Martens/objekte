//! Typed batch preparation and one immutable lifecycle installation.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::construction::batch::build_retained_contribution;
use crate::construction::segment_admission::CollectionSegment;
use crate::index::segment::{
    FileRangeLocator, IndexedCollectionSegment, SegmentBacking, SegmentFile, SegmentId,
};
use crate::inventory::{FileEntry, FileStatus, SourceRevision};
use crate::lifecycle::publication::{RecordMutation, RecordMutationKind};
use crate::lifecycle::{Index, IndexError};
use crate::model::{FileId, RecordSlot};

use super::{Batch, IngestionError, Operation, PublishedRevision};

pub(super) fn install_batch(
    index: &Index,
    cancelled: &AtomicBool,
    batch: Batch,
    sequence: u64,
) -> PublishedRevision {
    let view = index.view().map_err(IngestionError::Index)?;
    let registration = view
        .repository()
        .record_registrations
        .get(batch.registration.ordinal as usize)
        .filter(|source| source.handle == batch.registration)
        .ok_or_else(|| IngestionError::Index(crate::lifecycle::IndexError::ForeignInstance))?;
    let collection = batch.registration.collection;
    let options = view.repository().options();
    if !batch.files.is_empty() && !registration.file_links {
        return Err(IngestionError::Index(IndexError::Build(
            "file links are disabled for this registration".into(),
        )));
    }
    let mut files = Vec::with_capacity(batch.files.len());
    let mut new_files = Vec::with_capacity(batch.files.len());
    if !batch.files.is_empty() {
        let root = view.repository().files().root().ok_or_else(|| {
            IngestionError::Index(IndexError::Build("file catalog is not registered".into()))
        })?;
        for staged in &batch.files {
            let path = root.join(&staged.path);
            let canonical = path
                .canonicalize()
                .map_err(|error| IngestionError::Index(IndexError::Build(error.to_string())))?;
            if !canonical.starts_with(root) {
                return Err(IngestionError::Index(IndexError::Build(
                    "file path escapes catalog root".into(),
                )));
            }
            let metadata = std::fs::metadata(&canonical)
                .map_err(|error| IngestionError::Index(IndexError::Build(error.to_string())))?;
            if !metadata.is_file() {
                return Err(IngestionError::Index(IndexError::Build(
                    "declared path is not a regular file".into(),
                )));
            }
            let observed = SourceRevision::from_metadata(&metadata)
                .map_err(|error| IngestionError::Index(IndexError::Build(error)))?;
            if observed != staged.revision {
                return Err(IngestionError::Index(IndexError::Build(
                    "declared file revision changed".into(),
                )));
            }
            let existing = view
                .0
                .catalog
                .id_for(&staged.path)
                .and_then(|id| view.0.catalog.entry(id));
            let entry = if let Some(existing) = existing {
                if existing.revision != staged.revision {
                    return Err(IngestionError::Index(IndexError::Build(
                        "catalog file revision differs from declaration".into(),
                    )));
                }
                let mut entry = existing.clone();
                if !entry.collections.contains(&collection) {
                    entry.collections.push(collection);
                }
                entry
            } else {
                let ordinal = view
                    .0
                    .catalog
                    .allocated_files()
                    .checked_add(new_files.len())
                    .ok_or_else(|| {
                        IngestionError::Index(IndexError::Build("file ID overflow".into()))
                    })?;
                let id = FileId(u32::try_from(ordinal).map_err(|_| {
                    IngestionError::Index(IndexError::Build("file ID overflow".into()))
                })?);
                new_files.push(id);
                FileEntry {
                    id,
                    relative_path: staged.path.clone().into(),
                    revision: staged.revision,
                    created_ns: metadata
                        .created()
                        .ok()
                        .and_then(|time| {
                            time.duration_since(std::time::SystemTime::UNIX_EPOCH).ok()
                        })
                        .map(|duration| duration.as_nanos() as i128),
                    collections: vec![collection],
                    status: FileStatus::Valid,
                    diagnostics: Vec::new(),
                    omitted_diagnostics: 0,
                }
            };
            files.push(entry);
        }
    }
    let mut changes = Vec::with_capacity(batch.operations.len());
    let mut inputs = Vec::new();
    let mut links = Vec::new();
    let mut provider_inputs = Vec::new();
    let mut provider_versions = Vec::new();
    let mut provider_links = Vec::new();
    for operation in batch.operations {
        match operation {
            Operation::Insert {
                id,
                value,
                text,
                file,
                version,
            } => {
                changes.push(RecordMutation {
                    id: id.clone(),
                    kind: RecordMutationKind::Insert,
                });
                if let Some(version) = version {
                    if registration.reader.is_none() {
                        return Err(IngestionError::Index(IndexError::Build(
                            "provider input needs a registered reader".into(),
                        )));
                    }
                    provider_inputs.push((id, value, text));
                    provider_versions.push(version);
                    provider_links.push(file);
                } else {
                    inputs.push((id, value, text));
                    links.push(file);
                }
            }
            Operation::Replace {
                id,
                value,
                text,
                file,
                version,
            } => {
                changes.push(RecordMutation {
                    id: id.clone(),
                    kind: RecordMutationKind::Replace,
                });
                if let Some(version) = version {
                    if registration.reader.is_none() {
                        return Err(IngestionError::Index(IndexError::Build(
                            "provider input needs a registered reader".into(),
                        )));
                    }
                    provider_inputs.push((id, value, text));
                    provider_versions.push(version);
                    provider_links.push(file);
                } else {
                    inputs.push((id, value, text));
                    links.push(file);
                }
            }
            Operation::Delete { id } => changes.push(RecordMutation {
                id,
                kind: RecordMutationKind::Delete,
            }),
        }
    }
    let target = options.segment_policy.target_records.get();
    let max_files = options.segment_policy.max_files.get();
    let mut segments = Vec::new();
    let mut retained = inputs.into_iter().zip(links).peekable();
    loop {
        let part = next_partition(&mut retained, target, max_files, |(_, link)| {
            link.map(|(token, _)| token)
        });
        if part.is_empty() {
            break;
        }
        let (inputs, links) = part.into_iter().unzip();
        segments.push(build_segment(
            &view,
            registration,
            inputs,
            links,
            None,
            &files,
        )?);
    }
    let mut provider = provider_inputs
        .into_iter()
        .zip(provider_links)
        .zip(provider_versions)
        .peekable();
    loop {
        let part = next_partition(&mut provider, target, max_files, |((_, link), _)| {
            link.map(|(token, _)| token)
        });
        if part.is_empty() {
            break;
        }
        let mut inputs = Vec::with_capacity(part.len());
        let mut links = Vec::with_capacity(part.len());
        let mut versions = Vec::with_capacity(part.len());
        for ((input, link), version) in part {
            inputs.push(input);
            links.push(link);
            versions.push(version);
        }
        segments.push(build_segment(
            &view,
            registration,
            inputs,
            links,
            Some(versions),
            &files,
        )?);
    }
    if cancelled.load(Ordering::Acquire) {
        return Err(IngestionError::Cancelled {
            originating_sequence: sequence,
        });
    }
    index
        .publish_record_batch_with_files(
            view.revision(),
            collection,
            batch.registration.ordinal,
            &changes,
            segments,
            &files,
        )
        .map_err(IngestionError::Index)
}

fn next_partition<I, T>(
    pending: &mut std::iter::Peekable<I>,
    target_records: usize,
    max_files: usize,
    file_of: impl Fn(&T) -> Option<super::FileToken>,
) -> Vec<T>
where
    I: Iterator<Item = T>,
{
    let mut part = Vec::new();
    let mut files = std::collections::HashSet::new();
    while part.len() < target_records {
        let Some(next) = pending.peek() else { break };
        if let Some(file) = file_of(next)
            && !files.contains(&file.0)
            && files.len() >= max_files
        {
            break;
        }
        let next = pending.next().expect("peeked input");
        if let Some(file) = file_of(&next) {
            files.insert(file.0);
        }
        part.push(next);
    }
    part
}

fn build_segment(
    view: &crate::lifecycle::IndexView,
    registration: &crate::lifecycle::builder::RegisteredRecordSource,
    inputs: Vec<(
        crate::model::RecordId,
        crate::model::Value,
        Vec<crate::source::TextSegment>,
    )>,
    links: Vec<Option<(super::FileToken, Option<crate::inventory::ByteRange>)>>,
    versions: Option<Vec<crate::records::RecordVersion>>,
    files: &[FileEntry],
) -> Result<IndexedCollectionSegment, IngestionError> {
    let collection = registration.handle.collection();
    let declaration = &view.repository().collections[collection.0 as usize].definition;
    let options = view.repository().options();
    let raw_covers_logical_fields = inputs.iter().all(|(_, _, text)| text.is_empty());
    let (headers, bodies, contribution) =
        build_retained_contribution(inputs, declaration, &options)
            .map_err(|error| IngestionError::Index(IndexError::Build(error)))?;
    let draft = CollectionSegment::from_retained(
        SegmentId::new(2),
        collection,
        registration.adapter_id.clone(),
        registration.version,
        registration.handle.ordinal,
        raw_covers_logical_fields,
        headers,
        bodies,
        contribution,
    )
    .map_err(|error| IngestionError::Index(IndexError::Build(error)))?;
    let mut segment = draft
        .into_indexed(
            declaration,
            options.bigram_alphabet,
            options.inference,
            view.repository().collections.len(),
            &view.repository().workers,
        )
        .map_err(|error| IngestionError::Index(IndexError::Build(error)))?;
    if let Some(versions) = versions {
        segment.backing = SegmentBacking::Provider {
            versions,
            links: vec![None; segment.records.len()],
            registration: registration.handle.ordinal,
        };
    }
    attach_links(
        &mut segment,
        links,
        files,
        view.repository().files().root(),
        &declaration.name,
    )?;
    segment.recompute_accounting();
    Ok(segment)
}

fn attach_links(
    segment: &mut IndexedCollectionSegment,
    links: Vec<Option<(super::FileToken, Option<crate::inventory::ByteRange>)>>,
    files: &[FileEntry],
    root: Option<&std::path::Path>,
    collection_name: &str,
) -> Result<(), IngestionError> {
    let mut changed_slots = Vec::new();
    for (slot, link) in links.into_iter().enumerate() {
        let Some((token, range)) = link else { continue };
        let entry = files.get(token.0 as usize).ok_or_else(|| {
            IngestionError::Index(IndexError::Build("unknown batch file token".into()))
        })?;
        let target = match &mut segment.backing {
            SegmentBacking::Retained { links, .. } | SegmentBacking::Provider { links, .. } => {
                links
            }
            SegmentBacking::FileRange { .. } => {
                return Err(IngestionError::Index(IndexError::Build(
                    "record link on file segment".into(),
                )));
            }
        };
        target[slot] = Some(FileRangeLocator {
            file: entry.id,
            range,
            revision: entry.revision,
        });
        let file_index = if let Some(index) = segment.file_index.get(&entry.id) {
            *index
        } else {
            let index = segment.files.len();
            segment.file_index.insert(entry.id, index);
            segment.files.push(SegmentFile {
                entry: entry.clone(),
                records: Vec::new(),
            });
            index
        };
        segment.files[file_index]
            .records
            .push(RecordSlot(slot as u32));
        changed_slots.push(RecordSlot(slot as u32));
    }
    segment
        .refresh_metadata_sort_slots(changed_slots, root, collection_name)
        .map_err(|error| IngestionError::Index(IndexError::Build(error)))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::index::segment::SegmentPolicy;
    use crate::lifecycle::Index;
    use crate::model::Value;
    use crate::repository::configuration::RepositoryOptions;
    use crate::schema::Schema;
    use std::num::NonZeroUsize;

    #[test]
    fn one_batch_seals_at_record_policy_target() {
        let mut options = RepositoryOptions::default();
        options.segment_policy =
            SegmentPolicy::with_target_records(NonZeroUsize::new(3).unwrap()).unwrap();
        let mut builder = Index::builder().with_options(options);
        let collection = builder.collection("records", Schema::default()).unwrap();
        let registration = builder
            .register_records(collection, "test.partition", 1)
            .unwrap();
        let index = builder.build().unwrap();
        let session = index.ingestion();
        let writer = session.writer(registration).unwrap();
        let mut batch = writer.batch();
        for number in 0..8 {
            batch
                .insert(
                    format!("record-{number}"),
                    Value::String(format!("value-{number}")),
                )
                .unwrap();
        }
        writer.submit(batch).unwrap().wait_published().unwrap();
        let view = index.view().unwrap();
        assert_eq!(
            view.0.segments[0]
                .iter()
                .map(|segment| segment.index.record_count())
                .collect::<Vec<_>>(),
            vec![3, 3, 2]
        );
        session.finish().unwrap();
    }
}
