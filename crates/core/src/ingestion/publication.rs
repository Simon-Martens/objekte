//! Typed batch preparation and one immutable lifecycle installation.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::construction::batch::build_retained_contribution;
use crate::construction::segment_admission::CollectionSegment;
use crate::index::segment::SegmentId;
use crate::lifecycle::Index;
use crate::lifecycle::publication::{RecordMutation, RecordMutationKind};

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
    let declaration = &view.repository().collections[collection.0 as usize].definition;
    let options = view.repository().options();
    let mut changes = Vec::with_capacity(batch.operations.len());
    let mut inputs = Vec::new();
    for operation in batch.operations {
        match operation {
            Operation::Insert { id, value, text } => {
                changes.push(RecordMutation {
                    id: id.clone(),
                    kind: RecordMutationKind::Insert,
                });
                inputs.push((id, value, text));
            }
            Operation::Replace { id, value, text } => {
                changes.push(RecordMutation {
                    id: id.clone(),
                    kind: RecordMutationKind::Replace,
                });
                inputs.push((id, value, text));
            }
            Operation::Delete { id } => changes.push(RecordMutation {
                id,
                kind: RecordMutationKind::Delete,
            }),
        }
    }
    let segment = if inputs.is_empty() {
        None
    } else {
        let (headers, bodies, contribution) =
            build_retained_contribution(inputs, declaration, &options).map_err(|error| {
                IngestionError::Index(crate::lifecycle::IndexError::Build(error))
            })?;
        let draft = CollectionSegment::from_retained(
            SegmentId::new(2),
            collection,
            registration.adapter_id.clone(),
            registration.version,
            batch.registration.ordinal,
            headers,
            bodies,
            contribution,
        )
        .map_err(|error| IngestionError::Index(crate::lifecycle::IndexError::Build(error)))?;
        Some(
            draft
                .into_indexed(
                    declaration,
                    options.bigram_alphabet,
                    options.inference,
                    view.repository().collections.len(),
                    &view.repository().workers,
                )
                .map_err(|error| {
                    IngestionError::Index(crate::lifecycle::IndexError::Build(error))
                })?,
        )
    };
    if cancelled.load(Ordering::Acquire) {
        return Err(IngestionError::Cancelled {
            originating_sequence: sequence,
        });
    }
    index
        .publish_record_batch(
            view.revision(),
            collection,
            batch.registration.ordinal,
            &changes,
            segment,
        )
        .map_err(IngestionError::Index)
}
