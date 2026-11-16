//! Preparation of a complete stable-index file change from one source file.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use roaring::RoaringBitmap;

use crate::inventory::{FileEntry, RecordEntry};
use crate::model::{CollectionId, RecordId, RecordSlot};
use crate::posting::{PostingIndex, ScalarKey, StructuralKey};
use crate::reference::FieldDefinition;
use crate::repository::CollectionDefinition;
use crate::source::SourceRegistry;

use super::codec::{self, ComponentKey, PathKey};
use super::memory::StoredBytes;
use super::{
    ComponentId, CoverageDescriptor, FieldComponentKey, PreparedCollectionChange,
    PreparedFileChange, PreparedPostingAdd, StablePrototype, TextComponentKey,
};

/// Prepare all facts for one physical file in one collection using the normal
/// source adapter and batch extraction path. The file is parsed once; no
/// repository rebuild is performed.
pub fn prepare_file_change(
    prototype: &mut StablePrototype,
    root: &Path,
    definition: &CollectionDefinition,
    collection: CollectionId,
    file: FileEntry,
    alphabet: crate::workload::BigramAlphabet,
) -> Result<PreparedFileChange, String> {
    let source = SourceRegistry::builtin().resolve(&definition.format)?;
    let input = crate::build::SourceFileInput::from_entry(&file);
    let mut source_files = vec![input.clone(); file.id.0 as usize + 1];
    source_files[file.id.0 as usize] = input;
    let batch = crate::build::process_batch(
        root,
        definition,
        collection,
        &source,
        &[file.id],
        &source_files,
        alphabet,
        crate::inference::InferenceConfig::default(),
        1,
    )?;
    let parts = batch.into_parts();
    if parts.inference.is_some() {
        return Err("inference is unsupported".into());
    }
    let records = parts.records;
    let count = u32::try_from(records.len()).map_err(|_| "too many prepared records")?;
    let old_count = prototype
        .records_for_path(&file.relative_path)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|reference| reference.collection == collection)
        .count() as u32;
    let mut posting_adds = Vec::new();
    let mut coverage = Vec::new();
    let mut add = |component, key, slots: RoaringBitmap, value: Option<StoredBytes>| {
        if !slots.is_empty() || value.is_some() {
            posting_adds.push(PreparedPostingAdd {
                component,
                key,
                slots,
                value,
            });
        }
    };
    for (path, index) in &parts.postings.fields {
        let key = FieldComponentKey {
            path: path.clone(),
            binding: None,
            role: super::BindingRole::DeclaredField,
        };
        let ids = prototype
            .field_components(collection, &key)
            .ok_or("prepared field component missing")?
            .clone();
        if let Some(component) = ids.values {
            for (value, bitmap) in &index.values {
                add(
                    component,
                    ComponentKey::Scalar(value.clone()),
                    bitmap.clone(),
                    None,
                );
            }
            if let Some(descriptor) = prototype.components.get(&component) {
                coverage.push((
                    component,
                    replace_coverage(descriptor.descriptor.coverage.clone(), old_count, count),
                ));
            }
        }
        if let Some(component) = ids.lengths {
            for (value, bitmap) in &index.lengths {
                add(
                    component,
                    ComponentKey::Length(value.clone()),
                    bitmap.clone(),
                    None,
                );
            }
            if let Some(descriptor) = prototype.components.get(&component) {
                coverage.push((
                    component,
                    replace_coverage(descriptor.descriptor.coverage.clone(), old_count, count),
                ));
            }
        }
        if let Some(component) = ids.states {
            add(
                component,
                ComponentKey::State(codec::STATE_PRESENCE),
                index.presence.clone(),
                None,
            );
            add(
                component,
                ComponentKey::State(codec::STATE_EMPTY_SELECTION),
                index.empty_selections.clone(),
                None,
            );
            add(
                component,
                ComponentKey::State(codec::STATE_LEGACY_NULL),
                index.nulls.clone(),
                None,
            );
            if let Some(facts) = &index.structural {
                for (state, bitmap) in &facts.postings {
                    let code = match state {
                        StructuralKey::AnyNull => codec::STATE_ANY_NULL,
                        StructuralKey::NonEmpty => codec::STATE_NON_EMPTY,
                        StructuralKey::String => codec::STATE_STRING,
                        StructuralKey::Number => codec::STATE_NUMBER,
                        StructuralKey::Array => codec::STATE_ARRAY,
                        StructuralKey::Object => codec::STATE_OBJECT,
                        StructuralKey::EmptyObject => codec::STATE_EMPTY_OBJECT,
                    };
                    add(component, ComponentKey::State(code), bitmap.clone(), None);
                }
            }
            if let Some(descriptor) = prototype.components.get(&component) {
                coverage.push((
                    component,
                    replace_coverage(descriptor.descriptor.coverage.clone(), old_count, count),
                ));
            }
        }
    }
    for ((path, name), custom) in &parts.custom_postings {
        let key = FieldComponentKey {
            path: path.clone(),
            binding: Some(name.clone()),
            role: super::BindingRole::NamedBinding,
        };
        let Some(ids) = prototype.field_components(collection, &key).cloned() else {
            continue;
        };
        if let Some(component) = ids.values {
            for (value, bitmap) in &custom.index.values {
                add(
                    component,
                    ComponentKey::Scalar(value.clone()),
                    bitmap.clone(),
                    None,
                );
            }
        }
        if let Some(component) = ids.lengths {
            for (value, bitmap) in &custom.index.lengths {
                add(
                    component,
                    ComponentKey::Length(value.clone()),
                    bitmap.clone(),
                    None,
                );
            }
        }
        if let Some(component) = ids.states {
            add(
                component,
                ComponentKey::State(codec::STATE_PRESENCE),
                custom.index.presence.clone(),
                None,
            );
            add(
                component,
                ComponentKey::State(codec::STATE_LEGACY_NULL),
                custom.index.nulls.clone(),
                None,
            );
        }
    }
    for (key, contribution) in [
        (
            TextComponentKey {
                path: Vec::new(),
                binding: None,
                role: super::BindingRole::RawText,
            },
            parts.raw,
        ),
        (
            TextComponentKey {
                path: Vec::new(),
                binding: None,
                role: super::BindingRole::FallbackText,
            },
            parts.fallback_text,
        ),
    ] {
        if let Some(ids) = prototype.text_components(collection, &key).cloned() {
            let index = contribution.into_index()?;
            if let Some(component) = ids.normal {
                for (gram, bitmap) in index.normal {
                    add(component, ComponentKey::Gram(gram), bitmap, None);
                }
            }
            if let Some(component) = ids.skip {
                for (gram, bitmap) in index.skip {
                    add(component, ComponentKey::Gram(gram), bitmap, None);
                }
            }
        }
    }
    for ((path, name), contribution) in parts.custom_text {
        let key = TextComponentKey {
            path,
            binding: Some(name),
            role: super::BindingRole::CustomText,
        };
        if let Some(ids) = prototype.text_components(collection, &key).cloned() {
            let index = contribution.into_index()?;
            if let Some(component) = ids.normal {
                for (gram, bitmap) in index.normal {
                    add(component, ComponentKey::Gram(gram), bitmap, None);
                }
            }
            if let Some(component) = ids.skip {
                for (gram, bitmap) in index.skip {
                    add(component, ComponentKey::Gram(gram), bitmap, None);
                }
            }
        }
    }
    let sort_keys = parts.sort_keys;
    for descriptor in prototype.components.values() {
        let Some(identity) = descriptor.descriptor.collection.as_ref() else {
            continue;
        };
        if identity.id != collection {
            continue;
        }
        match descriptor.descriptor.kind {
            super::ComponentKind::SortAscending | super::ComponentKind::SortDescending => {
                let Some(field) = descriptor.descriptor.field.as_ref() else {
                    continue;
                };
                let Some(ordering) = descriptor.descriptor.ordering.as_ref() else {
                    continue;
                };
                let path = crate::model::FieldPath {
                    root: crate::model::PathRoot::Data,
                    segments: field.path.clone(),
                };
                let Some(keys) = sort_keys.get(&path) else {
                    continue;
                };
                let descending = descriptor.descriptor.kind == super::ComponentKind::SortDescending;
                for (slot, key) in keys.iter().enumerate() {
                    let slot = RecordSlot(u32::try_from(slot).map_err(|_| "slot overflow")?);
                    add(
                        descriptor.descriptor.id,
                        ComponentKey::Sort(super::SortEntryKey {
                            options: ordering.options,
                            descending,
                            key: key.clone(),
                            tie_break_id: records[slot.0 as usize].id.clone(),
                            slot,
                        }),
                        RoaringBitmap::new(),
                        Some(StoredBytes::empty()),
                    );
                }
            }
            super::ComponentKind::CompositeSort => {
                let parts_desc = &descriptor.descriptor.composite;
                if parts_desc.is_empty() {
                    continue;
                }
                let columns = parts_desc
                    .iter()
                    .map(|part| {
                        let path = crate::model::FieldPath {
                            root: crate::model::PathRoot::Data,
                            segments: part.path.clone(),
                        };
                        sort_keys
                            .get(&path)
                            .ok_or_else(|| format!("composite constituent missing: {path:?}"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                for slot in 0..records.len() {
                    let slot = RecordSlot(slot as u32);
                    let parts = parts_desc
                        .iter()
                        .zip(&columns)
                        .map(|(part, keys)| codec::CompositePartKey {
                            options: part.options,
                            descending: part.descending,
                            key: keys[slot.0 as usize].clone(),
                        })
                        .collect();
                    add(
                        descriptor.descriptor.id,
                        ComponentKey::CompositeSort(codec::CompositeSortEntryKey {
                            parts,
                            tie_break_id: records[slot.0 as usize].id.clone(),
                            slot,
                        }),
                        RoaringBitmap::new(),
                        Some(StoredBytes::empty()),
                    );
                }
            }
            _ => {}
        }
    }
    for contribution in parts.relations {
        let declaration = contribution.declaration().clone();
        let key = super::RelationComponentKey {
            path: declaration.path.clone(),
            target_collection: declaration.target_collection.clone(),
        };
        let Some(ids) = prototype.relation_components(collection, &key).cloned() else {
            continue;
        };
        let target_collection = ids.target_collection;
        let mut targets = BTreeMap::<String, RoaringBitmap>::new();
        for (source, target_id, positions) in contribution.references() {
            let resolved = {
                let id = RecordId(target_id.into());
                !prototype
                    .records_for_identity(target_collection, &id)
                    .map_err(|error| error.to_string())?
                    .is_empty()
            };
            let value = codec::encode_meta(&super::RelationOccurrenceValue {
                target_collection: Some(target_collection),
                target_id: target_id.to_owned(),
                positions: positions.to_vec(),
                target_resolved: resolved,
            })?;
            add(
                ids.occurrences,
                ComponentKey::Occurrence(codec::RelationOccurrenceKey { source, ordinal: 0 }),
                RoaringBitmap::new(),
                Some(value),
            );
            targets
                .entry(target_id.to_owned())
                .or_default()
                .insert(source.0);
        }
        for (target_id, slots) in targets {
            add(
                ids.targets,
                ComponentKey::RelationTarget(codec::RelationTargetKey {
                    target_collection,
                    target_id,
                }),
                slots,
                None,
            );
        }
        add(
            ids.states,
            ComponentKey::RelationState(codec::RELATION_PRESENT),
            contribution.present().clone(),
            None,
        );
        add(
            ids.states,
            ComponentKey::RelationState(codec::RELATION_NULLS),
            contribution.nulls().clone(),
            None,
        );
        add(
            ids.states,
            ComponentKey::RelationState(codec::RELATION_EMPTY_ARRAY),
            contribution.empty_arrays().clone(),
            None,
        );
        add(
            ids.states,
            ComponentKey::RelationState(codec::RELATION_UNAVAILABLE),
            contribution.unavailable().clone(),
            None,
        );
    }
    Ok(PreparedFileChange {
        file,
        collections: vec![PreparedCollectionChange {
            collection,
            records,
            posting_adds,
            coverage,
        }],
    })
}

fn replace_coverage(
    mut coverage: CoverageDescriptor,
    old_count: u32,
    new_count: u32,
) -> CoverageDescriptor {
    coverage.covered_records = coverage
        .covered_records
        .saturating_sub(old_count)
        .saturating_add(new_count);
    coverage
}

#[allow(dead_code)]
fn _posting(_: &PostingIndex) {}
#[allow(dead_code)]
fn _field(_: &FieldDefinition) {}
#[allow(dead_code)]
fn _scalar(_: &ScalarKey) {}
#[allow(dead_code)]
fn _path(_: &PathKey) {}
#[allow(dead_code)]
fn _component(_: ComponentId) {}
#[allow(dead_code)]
fn _record(_: &RecordEntry) {}
#[allow(dead_code)]
fn _hashmap(_: HashMap<(), ()>) {}
