//! Typed batch preparation and one immutable lifecycle installation.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::construction::batch::{build_provider_contribution, build_retained_contribution};
use crate::construction::segment_admission::CollectionSegment;
use crate::index::segment::{IndexedCollectionSegment, SegmentId};
use crate::lifecycle::publication::{RecordMutation, RecordMutationKind};
use crate::lifecycle::{Index, IndexError};

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
    view: &crate::lifecycle::IndexView,
    registration: &crate::lifecycle::builder::RegisteredRecordSource,
    inputs: Vec<(
        crate::model::RecordId,
        crate::model::Value,
        Vec<crate::source::TextSegment>,
    )>,
    versions: Option<Vec<crate::records::RecordVersion>>,
) -> Result<IndexedCollectionSegment, IngestionError> {
    let collection = registration.handle.collection();
    let declaration = &view.repository().collections[collection.0 as usize].definition;
    let options = view.repository().options();
    let raw_covers_logical_fields = inputs.iter().all(|(_, _, text)| text.is_empty());
    let draft = if let Some(versions) = versions {
        let (headers, contribution) = build_provider_contribution(inputs, declaration, &options)
            .map_err(|error| IngestionError::Index(IndexError::Build(error)))?;
        CollectionSegment::from_provider(
            SegmentId::new(2),
            collection,
            registration.adapter_id.clone(),
            registration.version,
            registration.handle.ordinal,
            raw_covers_logical_fields,
            headers,
            versions,
            contribution,
        )
    } else {
        let (headers, bodies, contribution) =
            build_retained_contribution(inputs, declaration, &options)
                .map_err(|error| IngestionError::Index(IndexError::Build(error)))?;
        CollectionSegment::from_retained(
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
    }
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
    segment.recompute_accounting();
    Ok(segment)
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
