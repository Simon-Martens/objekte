//! Publication of optional inferred indexes after source admission.

use std::collections::BTreeMap;

use crate::index_store::value_codec::{
    BindingIdentity, CoverageKind, FieldCoverage, FieldDescriptor, StateKind,
};
use crate::index_store::{posting_blocks, states};
use crate::model::{CollectionId, Number, PathRoot, RecordSlot};
use crate::posting::ScalarKey;

use super::{BuildCollection, IndexStore, read, write};

#[derive(Default)]
struct ContributionAdditions {
    values: Vec<(u32, ScalarKey)>,
    lengths: Vec<(u32, Number)>,
    states: BTreeMap<u32, u16>,
    coverage: Vec<(u32, CoverageKind)>,
}

pub(super) fn finalize(
    store: &std::sync::Arc<IndexStore>,
    collections: &mut [BuildCollection],
) -> Result<(), String> {
    for collection in collections {
        let Some(inference) = collection.inference.take() else {
            continue;
        };
        let state = inference
            .finish(
                &collection.logical_records,
                collection.record_count as usize,
                &collection.definition.schema,
            )
            .map_err(|error| error.to_string())?;
        let tx = store.begin_write().map_err(|error| error.to_string())?;
        let mut metadata = read::metadata_from_write(&tx).map_err(|error| error.to_string())?;
        for (path, index) in &state.indexes {
            let identity = BindingIdentity::Declared { path: path.clone() };
            let binding = if let Some(binding) = collection.bindings.get(&identity) {
                *binding
            } else {
                let binding = collection.next_binding;
                collection.next_binding = collection
                    .next_binding
                    .checked_add(1)
                    .ok_or_else(|| "binding overflow".to_string())?;
                let encoded = bincode::serialize(&identity).map_err(|error| error.to_string())?;
                write::put_binding(
                    &tx,
                    collection.id,
                    &encoded,
                    &crate::index_store::value_codec::StoredBinding {
                        id: binding,
                        descriptor: crate::index_store::value_codec::BindingDescriptor {
                            identity: identity.clone(),
                            role: crate::index_store::value_codec::BindingRole::Structured,
                            kind: index.kind.clone(),
                            multi: index.multi,
                            text_alphabet: None,
                            text_segment: None,
                            fingerprint: [0; 32],
                        },
                    },
                )
                .map_err(|error| error.to_string())?;
                collection.bindings.insert(identity, binding);
                binding
            };
            write::put_field(
                &tx,
                collection.id,
                path,
                &FieldDescriptor {
                    path: crate::model::FieldPath {
                        root: PathRoot::Data,
                        segments: path.clone(),
                    },
                    kind: index.kind.clone(),
                    multi: index.multi,
                    indexed: index.values_indexed,
                    values_indexed: index.values_indexed,
                    value_complete: index.value_complete,
                    length_complete: index.length_complete,
                    empty_string_complete: index.empty_string_complete,
                    covered_records: index.covered_records,
                },
            )
            .map_err(|error| error.to_string())?;
            write::put_field_coverage(
                &tx,
                collection.id,
                binding,
                &FieldCoverage {
                    kind: index.kind.clone(),
                    multi: index.multi,
                    values_indexed: index.values_indexed,
                    value_complete: index.value_complete,
                    length_complete: index.length_complete,
                    empty_string_complete: index.empty_string_complete,
                    covered_records: index.covered_records,
                    inference_status: crate::index_store::value_codec::INFERENCE_STATUS_BUILT,
                    value_cardinality: index.distinct_values() as u64,
                    length_cardinality: index.distinct_lengths() as u64,
                },
            )
            .map_err(|error| error.to_string())?;

            let mut posting_batch = posting_blocks::PostingBatch::new();
            let mut state_batch = states::StateBatch::new();
            let mut affected = BTreeMap::<RecordSlot, ContributionAdditions>::new();
            for (value, slots) in &index.values {
                for slot in slots.iter() {
                    let slot = RecordSlot(slot);
                    posting_batch.set(
                        posting_blocks::PostingFamily::Value,
                        binding,
                        value,
                        slot,
                    );
                    affected
                        .entry(slot)
                        .or_default()
                        .values
                        .push((binding, value.clone()));
                }
            }
            for (length, slots) in &index.lengths {
                let scalar = ScalarKey::Number(length.clone());
                for slot in slots.iter() {
                    let slot = RecordSlot(slot);
                    posting_batch.set(
                        posting_blocks::PostingFamily::Length,
                        binding,
                        &scalar,
                        slot,
                    );
                    affected
                        .entry(slot)
                        .or_default()
                        .lengths
                        .push((binding, length.clone()));
                }
            }
            for (state_kind, slots) in [
                (StateKind::PathPresence, &index.presence),
                (
                    StateKind::EmptySelection,
                    state
                        .empty_selections
                        .get(path)
                        .unwrap_or(&index.empty_selections),
                ),
                (StateKind::LegacyNullSet, &index.nulls),
            ] {
                add_states(
                    binding,
                    state_kind,
                    slots,
                    &mut state_batch,
                    &mut affected,
                );
            }
            if let Some(structural) = &index.structural {
                for (key, slots) in &structural.postings {
                    let state_kind = match key {
                        crate::posting::StructuralKey::AnyNull => StateKind::AnyNull,
                        crate::posting::StructuralKey::NonEmpty => StateKind::NonEmpty,
                        crate::posting::StructuralKey::String => StateKind::String,
                        crate::posting::StructuralKey::Number => StateKind::Number,
                        crate::posting::StructuralKey::Array => StateKind::Array,
                        crate::posting::StructuralKey::Object => StateKind::Object,
                        crate::posting::StructuralKey::EmptyObject => StateKind::EmptyObject,
                    };
                    add_states(
                        binding,
                        state_kind,
                        slots,
                        &mut state_batch,
                        &mut affected,
                    );
                }
            }
            if let Some(short) = state.short_string_coverage.get(path) {
                for slot in short.iter() {
                    let slot = RecordSlot(slot);
                    write::insert_coverage_member(
                        &tx,
                        collection.id,
                        binding,
                        CoverageKind::ShortString,
                        slot,
                    )
                    .map_err(|error| error.to_string())?;
                    affected
                        .entry(slot)
                        .or_default()
                        .coverage
                        .push((binding, CoverageKind::ShortString));
                }
            }
            posting_blocks::apply_batch(&tx, collection.id, &posting_batch)
                .map_err(|error| error.to_string())?;
            states::apply_batch(&tx, collection.id, &state_batch)
                .map_err(|error| error.to_string())?;
            update_reverse_contributions(&tx, collection.id, affected)?;
        }
        metadata.revision = metadata
            .revision
            .checked_add(1)
            .ok_or_else(|| "publication revision overflow".to_string())?;
        write::put_metadata(&tx, &metadata).map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn add_states(
    binding: u32,
    state: StateKind,
    slots: &roaring::RoaringBitmap,
    batch: &mut states::StateBatch,
    affected: &mut BTreeMap<RecordSlot, ContributionAdditions>,
) {
    for slot in slots.iter() {
        batch.set(binding, state, slot);
        *affected
            .entry(RecordSlot(slot))
            .or_default()
            .states
            .entry(binding)
            .or_insert(0) |= 1 << state.tag();
    }
}

fn update_reverse_contributions(
    tx: &redb::WriteTransaction,
    collection: CollectionId,
    affected: BTreeMap<RecordSlot, ContributionAdditions>,
) -> Result<(), String> {
    for (slot, additions) in affected {
        let mut contribution = write::get_contributions(tx, collection, slot)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| {
                format!(
                    "inferred facts reference missing contribution {}/{}",
                    collection.0, slot.0
                )
            })?;
        contribution.values.extend(additions.values);
        contribution.lengths.extend(additions.lengths);
        contribution.coverage.extend(additions.coverage);
        for (binding, mask) in additions.states {
            if let Some((_, existing)) = contribution
                .state_masks
                .iter_mut()
                .find(|(candidate, _)| *candidate == binding)
            {
                *existing |= mask;
            } else {
                contribution.state_masks.push((binding, mask));
            }
        }
        contribution.normalize();
        write::put_contributions(tx, collection, slot, &contribution)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
