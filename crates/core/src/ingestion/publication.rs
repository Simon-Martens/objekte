//! Typed batch preparation and one immutable lifecycle installation.

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::index::segment::SegmentId;
use crate::index::storage::segment::build::{DirectSegmentBodies, DirectSegmentBuild, SegmentSeal};
use crate::index::storage::segment::{SegmentHandle, SegmentReader};
use crate::index::storage::worker::{DirectRecordBatch, DirectRecordInput, PreparedDirectBatch};
use crate::lifecycle::publication::{RecordMutation, RecordMutationKind};
use crate::lifecycle::{Index, IndexError, IndexRevision, IndexView, RecordRegistration};
use crate::records::RecordVersion;

use super::{Batch, IngestionError, Operation, PublishedRevision};

pub(super) fn install_batch(
    index: &Index,
    cancelled: &AtomicBool,
    publication_decision: &Mutex<()>,
    batch: Batch,
    sequence: u64,
    #[cfg(test)] prepared: impl FnOnce(),
) -> PublishedRevision {
    let view = index.view().map_err(IngestionError::Index)?;
    let registration = view
        .repository()
        .record_registrations
        .get(batch.registration.ordinal as usize)
        .filter(|source| source.handle == batch.registration)
        .ok_or_else(|| IngestionError::Index(crate::lifecycle::IndexError::ForeignInstance))?;
    let options = view.repository().options();
    let mut changes = Vec::with_capacity(batch.operations.len());
    let mut inputs = Vec::new();
    let mut provider_inputs = Vec::new();
    let mut provider_versions = Vec::new();
    for operation in batch.operations {
        match operation {
            Operation::Insert {
                id,
                value,
                text,
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
                } else {
                    inputs.push((id, value, text));
                }
            }
            Operation::Replace {
                id,
                value,
                text,
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
                } else {
                    inputs.push((id, value, text));
                }
            }
            Operation::Delete { id } => changes.push(RecordMutation {
                id,
                kind: RecordMutationKind::Delete,
            }),
        }
    }
    let target = options.segment_policy.target_records.get();
    let mut segments = Vec::new();
    let mut retained = inputs.into_iter();
    loop {
        let part = retained.by_ref().take(target).collect::<Vec<_>>();
        if part.is_empty() {
            break;
        }
        segments.push(build_segment(&view, registration, part, None)?);
    }
    let mut provider = provider_inputs.into_iter().zip(provider_versions);
    loop {
        let part = provider.by_ref().take(target).collect::<Vec<_>>();
        if part.is_empty() {
            break;
        }
        let (inputs, versions): (Vec<_>, Vec<_>) = part.into_iter().unzip();
        segments.push(build_segment(&view, registration, inputs, Some(versions))?);
    }
    #[cfg(test)]
    prepared();
    index
        .publish_record_batch(&view, batch.registration, &changes, segments, || {
            let guard = publication_decision
                .lock()
                .map_err(|_| IndexError::LockPoisoned)?;
            if cancelled.load(Ordering::Acquire) {
                return Err(IndexError::Cancelled);
            }
            Ok(guard)
        })
        .map_err(|error| match error {
            IndexError::Cancelled => IngestionError::Cancelled {
                originating_sequence: sequence,
            },
            error => IngestionError::Index(error),
        })
}

fn build_segment(
    view: &IndexView,
    registration: &crate::lifecycle::builder::RegisteredRecordSource,
    mut inputs: Vec<DirectRecordInput>,
    versions: Option<Vec<RecordVersion>>,
) -> Result<SegmentHandle, IngestionError> {
    build_segment_inputs(view, registration, &mut inputs, versions.as_deref(), &mut 0)
}

fn build_segment_inputs(
    view: &IndexView,
    registration: &crate::lifecycle::builder::RegisteredRecordSource,
    inputs: &mut [DirectRecordInput],
    versions: Option<&[RecordVersion]>,
    completed: &mut usize,
) -> Result<SegmentHandle, IngestionError> {
    let collection = registration.handle.collection();
    let declaration = &view.repository().collections[collection.0 as usize].definition;
    let options = view.repository().options();
    let raw_covers_logical_fields = inputs.iter().all(|(_, _, text)| text.is_empty());
    let PreparedDirectBatch {
        headers,
        bodies,
        contribution,
    } = view
        .repository()
        .storage_factory
        .prepare_direct_records(DirectRecordBatch {
            inputs,
            declaration,
            options,
            retain_bodies: versions.is_none(),
            completed_records: completed,
        })
        .map_err(|error| IngestionError::Index(IndexError::Build(error.to_string())))?;
    let factory = &view.repository().storage_factory;
    let bodies = if let Some(versions) = versions {
        DirectSegmentBodies::Versioned(versions)
    } else {
        DirectSegmentBodies::Retained(
            factory
                .retain_record_bodies(bodies)
                .map_err(IndexError::from)
                .map_err(IngestionError::Index)?,
        )
    };
    let segment = factory
        .seal_direct_segment(
            DirectSegmentBuild {
                collection,
                source_id: &registration.adapter_id,
                source_version: registration.version,
                registration: registration.handle.ordinal,
                raw_covers_logical_fields,
                bodies,
                headers,
                contribution,
            },
            SegmentSeal {
                id: SegmentId::new(2),
                definition: declaration,
                options,
                collection_count: view.repository().collections.len(),
                pool: &view.repository().workers,
                profile: None,
                observe_inference: false,
                declared_orders: factory.declared_order_store(),
            },
        )
        .map_err(IndexError::from)
        .map_err(IngestionError::Index)?
        .index;
    Ok(segment)
}

#[derive(Debug, Default)]
pub(super) struct GroupPublication {
    pub(super) committed: Option<(u64, IndexRevision)>,
    pub(super) failure: Option<(u64, IngestionError)>,
}

struct Member {
    sequence: u64,
    end: usize,
}

pub(super) fn install_group(
    index: &Index,
    cancelled: &AtomicBool,
    publication_decision: &Mutex<()>,
    handle: RecordRegistration,
    batches: Vec<(u64, Batch)>,
    record_capacity: usize,
    mut failure: Option<(u64, IngestionError)>,
    #[cfg(test)] prepared: impl FnOnce(),
) -> GroupPublication {
    let Some(first) = batches.first().map(|(sequence, _)| *sequence) else {
        return GroupPublication {
            committed: None,
            failure,
        };
    };
    let reject = |error| GroupPublication {
        committed: None,
        failure: Some((first, error)),
    };
    let view = match index.view() {
        Ok(view) => view,
        Err(error) => return reject(IngestionError::Index(error)),
    };
    let Some(registration) = view
        .repository()
        .record_registrations
        .get(handle.ordinal as usize)
        .filter(|source| source.handle == handle)
    else {
        return reject(IngestionError::Index(IndexError::ForeignInstance));
    };
    if cancelled.load(Ordering::Acquire) {
        return reject(IngestionError::Cancelled {
            originating_sequence: first,
        });
    }
    let mut inputs = Vec::with_capacity(record_capacity);
    let mut versions = Vec::new();
    let mut changes = Vec::with_capacity(record_capacity);
    let mut members = Vec::with_capacity(batches.len());
    let mut seen = HashSet::with_capacity(record_capacity);
    'batches: for (sequence, batch) in batches {
        for operation in batch.operations {
            let Operation::Insert {
                id,
                value,
                text,
                version,
            } = operation
            else {
                unreachable!("only compatible insert batches can coalesce");
            };
            if !seen.insert(id.clone()) {
                failure = Some((
                    sequence,
                    IngestionError::Index(IndexError::DuplicateRecordId {
                        collection: view.repository().collections[handle.collection.0 as usize]
                            .definition
                            .name
                            .clone(),
                        record_id: id.0.to_string(),
                    }),
                ));
                // The entire member is discarded, including its earlier inputs.
                break 'batches;
            }
            changes.push(RecordMutation {
                id: id.clone(),
                kind: RecordMutationKind::Insert,
            });
            inputs.push((id, value, text));
            if let Some(version) = version {
                versions.push(version);
            }
        }
        members.push(Member {
            sequence,
            end: inputs.len(),
        });
    }
    drop(seen);
    let mut segment = loop {
        let Some(member) = members.last() else {
            return GroupPublication {
                committed: None,
                failure,
            };
        };
        inputs.truncate(member.end);
        changes.truncate(member.end);
        versions.truncate(member.end);
        let mut completed = 0;
        match build_segment_inputs(
            &view,
            registration,
            &mut inputs,
            (!versions.is_empty()).then_some(versions.as_slice()),
            &mut completed,
        ) {
            Ok(segment) => break segment,
            Err(error) if completed < member.end => {
                let invalid = members.partition_point(|member| member.end <= completed);
                failure = Some((members[invalid].sequence, error));
                members.truncate(invalid);
            }
            Err(error) => return reject(error),
        }
    };
    drop(inputs);
    drop(versions);
    #[cfg(test)]
    prepared();

    loop {
        let prefix = members.last().expect("nonempty prepared prefix");
        let result = index.publish_prepared_record_batch(
            &view,
            handle,
            &changes[..prefix.end],
            std::slice::from_ref(&segment),
            || {
                let guard = publication_decision
                    .lock()
                    .map_err(|_| IndexError::LockPoisoned)?;
                if cancelled.load(Ordering::Acquire) {
                    return Err(IndexError::Cancelled);
                }
                Ok(guard)
            },
        );
        match result {
            Ok(revision) => {
                return GroupPublication {
                    committed: Some((prefix.sequence, revision)),
                    failure,
                };
            }
            Err(IndexError::Cancelled) => {
                return reject(IngestionError::Cancelled {
                    originating_sequence: first,
                });
            }
            Err(error @ IndexError::DuplicateRecordId { .. }) => {
                let IndexError::DuplicateRecordId { ref record_id, .. } = error else {
                    unreachable!()
                };
                let Some(slot) = changes[..prefix.end]
                    .iter()
                    .position(|change| change.id.0.as_ref() == record_id)
                else {
                    return reject(IngestionError::Index(error));
                };
                let member = members.partition_point(|member| member.end <= slot);
                failure = Some((members[member].sequence, IngestionError::Index(error)));
                members.truncate(member);
                let Some(prefix) = members.last() else {
                    return GroupPublication {
                        committed: None,
                        failure,
                    };
                };
                match compact_prefix(&segment, prefix.end) {
                    Ok(compacted) => segment = compacted,
                    Err(error) => return reject(IngestionError::Index(error)),
                }
            }
            Err(error) => return reject(IngestionError::Index(error)),
        }
    }
}

fn compact_prefix(segment: &SegmentHandle, length: usize) -> Result<SegmentHandle, IndexError> {
    let mut live = roaring::RoaringBitmap::new();
    live.insert_range(
        0..u32::try_from(length)
            .map_err(|_| IndexError::ResourceLimit("group record count overflow".into()))?,
    );
    let slots = segment.descriptor().slots;
    let active = crate::inventory::ActiveRecords::from_bitmap(slots, live)
        .map_err(IndexError::InvalidPublication)?;
    use crate::index::storage::segment::maintenance::{
        project_active_checked, require_maintenance,
    };
    let reader = segment.as_ref();
    let editor = require_maintenance(reader)?;
    Ok(project_active_checked(reader, editor, &active, slots, &mut |_| false)?.payload)
}

#[cfg(test)]
mod tests {
    use crate::index::segment::SegmentPolicy;
    use crate::index::storage::segment::SegmentReader;
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
            view.0
                .segment_group(0)
                .unwrap()
                .iter()
                .map(|segment| segment.descriptor().slots)
                .collect::<Vec<_>>(),
            vec![3, 3, 2]
        );
        session.finish().unwrap();
    }
}
