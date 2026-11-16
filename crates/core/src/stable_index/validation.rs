//! Independent semantic and storage invariants for the prototype.

use std::collections::BTreeMap;

use roaring::RoaringBitmap;
use serde::Serialize;

use crate::bigram::BigramIndex;
use crate::inventory::RecordClass;
use crate::model::{CollectionId, RecordSlot};
use crate::posting::{PostingIndex, ScalarKey, StructuralKey};
use crate::relation::{RelationIndex, RelationTarget};
use crate::repository::Repository;

use super::codec::{self, ComponentKey, RelationTargetKey};
use super::{
    FieldComponentKey, RelationComponentKey, StableIndexError, StablePrototype, TextComponentKey,
};

#[derive(Clone, Debug, Default, Serialize)]
pub struct ValidationReport {
    pub components: u64,
    pub files: u64,
    pub records: u64,
    pub identities: u64,
    pub domains: u64,
    pub posting_keys: u64,
    pub posting_members: u64,
    pub gram_keys: u64,
    pub gram_members: u64,
    pub sort_entries: u64,
    pub relation_occurrences: u64,
    pub errors: Vec<String>,
}

impl ValidationReport {
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    fn error(&mut self, error: impl Into<String>) {
        if self.errors.len() < 64 {
            self.errors.push(error.into());
        }
    }
}

impl StablePrototype {
    pub fn validate_against_repository(
        &mut self,
        repository: &Repository,
    ) -> Result<ValidationReport, StableIndexError> {
        let mut report = ValidationReport::default();
        report.components = self.components.len() as u64;

        for file in &repository.files().files {
            let Some(stable) = self.file_catalog_record(file.id)? else {
                report.error(format!("file {} is missing", file.id.0));
                continue;
            };
            report.files += 1;
            if stable.entry != *file || !stable.live {
                report.error(format!("file {} metadata differs", file.id.0));
            }
            let path_key =
                super::PathKey::from_path(&file.relative_path).map_err(StableIndexError::Format)?;
            let refs = self.records_for_path_key(&path_key)?;
            let mut expected = Vec::new();
            for state in repository.collections() {
                for record in &state.generation.records {
                    if record.file == file.id {
                        expected.push(super::RecordRef {
                            collection: state.generation.collection,
                            slot: record.slot,
                        });
                    }
                }
            }
            expected.sort_unstable();
            expected.dedup();
            if refs != expected {
                report.error(format!("path refs differ for file {}", file.id.0));
            }
        }

        for state in repository.collections() {
            let collection = state.generation.collection;
            for record in &state.generation.records {
                let Some(stable) = self.record_catalog_entry(collection, record.slot)? else {
                    report.error(format!(
                        "collection {} record {} is missing",
                        collection.0, record.slot.0
                    ));
                    continue;
                };
                report.records += 1;
                if !record_entry_equal(&stable.entry, record) || !stable.live {
                    report.error(format!(
                        "collection {} record {} metadata differs",
                        collection.0, record.slot.0
                    ));
                }
                let by_identity = self.records_for_identity(collection, &record.id)?;
                if !by_identity.contains(&record.slot) {
                    report.error(format!(
                        "identity {} does not map to slot {}",
                        record.id.0, record.slot.0
                    ));
                }
                report.identities += 1;
            }
            for (domain, expected, label) in [
                (codec::DOMAIN_LOGICAL, &state.logical_records, "logical"),
                (
                    codec::DOMAIN_EMPTY_ARRAY,
                    &state.empty_array_records,
                    "empty-array",
                ),
            ] {
                let actual = self.domain_posting(collection, domain)?;
                report.domains += 1;
                if &actual != expected {
                    report.error(format!(
                        "collection {} {label} domain differs",
                        collection.0
                    ));
                }
            }
            let raw_expected: RoaringBitmap = state
                .generation
                .records
                .iter()
                .filter(|record| record.class == RecordClass::RawFallback)
                .map(|record| record.slot.0)
                .collect();
            let raw_actual = self.domain_posting(collection, codec::DOMAIN_RAW_FALLBACK)?;
            if raw_actual != raw_expected {
                report.error(format!(
                    "collection {} raw-fallback domain differs",
                    collection.0
                ));
            }
            let live = self.live_slots(collection)?;
            let expected_live: RoaringBitmap = state
                .generation
                .records
                .iter()
                .map(|record| record.slot.0)
                .collect();
            if live != expected_live {
                report.error(format!("collection {} live domain differs", collection.0));
            }

            for (path, index) in &state.postings.fields {
                let key = FieldComponentKey {
                    path: path.clone(),
                    binding: None,
                    role: super::BindingRole::DeclaredField,
                };
                self.compare_posting(collection, &key, index, &mut report)?;
            }
            for ((path, name), custom) in &state.custom_postings {
                let key = FieldComponentKey {
                    path: path.clone(),
                    binding: Some(name.clone()),
                    role: super::BindingRole::NamedBinding,
                };
                self.compare_posting(collection, &key, &custom.index, &mut report)?;
            }

            self.compare_text(
                collection,
                &TextComponentKey {
                    path: Vec::new(),
                    binding: None,
                    role: super::BindingRole::RawText,
                },
                &state.raw,
                &mut report,
            )?;
            self.compare_text(
                collection,
                &TextComponentKey {
                    path: Vec::new(),
                    binding: None,
                    role: super::BindingRole::FallbackText,
                },
                &state.fallback_text,
                &mut report,
            )?;
            for ((path, name), index) in &state.custom_text {
                self.compare_text(
                    collection,
                    &TextComponentKey {
                        path: path.clone(),
                        binding: Some(name.clone()),
                        role: super::BindingRole::CustomText,
                    },
                    index,
                    &mut report,
                )?;
            }

            for (path, column) in &state.sorting.columns {
                let field = super::FieldIdentity {
                    path: path.segments.clone(),
                    binding: None,
                    role: super::BindingRole::DeclaredField,
                    logical_type: Some(column.kind.clone()),
                    multi: false,
                };
                let descriptor = self
                    .components
                    .values()
                    .find(|entry| {
                        entry.descriptor.kind == codec::ComponentKind::SortAscending
                            && entry.descriptor.collection.as_ref().map(|value| value.id)
                                == Some(collection)
                            && entry.descriptor.field.as_ref() == Some(&field)
                    })
                    .map(|entry| entry.descriptor.clone());
                let Some(descriptor) = descriptor else {
                    report.error(format!(
                        "collection {} sort {} is missing",
                        collection.0,
                        path.display()
                    ));
                    continue;
                };
                let sort_key = super::sort_component_key(&descriptor);
                let ascending = self.sort_slots(collection, &sort_key, false)?;
                let descending = self.sort_slots(collection, &sort_key, true)?;
                report.sort_entries += (ascending.len() + descending.len()) as u64;
                if ascending != column.ascending {
                    report.error(format!(
                        "collection {} ascending sort {} differs",
                        collection.0,
                        path.display()
                    ));
                }
                if descending != column.descending {
                    report.error(format!(
                        "collection {} descending sort {} differs",
                        collection.0,
                        path.display()
                    ));
                }
            }

            for relation in &state.relations {
                self.compare_relation(collection, relation, &mut report)?;
            }
        }
        Ok(report)
    }

    fn domain_posting(
        &mut self,
        collection: CollectionId,
        domain: u8,
    ) -> Result<RoaringBitmap, StableIndexError> {
        let component = self
            .collections
            .get(&collection)
            .ok_or_else(|| StableIndexError::Format("unknown collection".into()))?
            .domains;
        Ok(self
            .posting_value(component, ComponentKey::Domain(domain))?
            .unwrap_or_default())
    }

    fn compare_posting(
        &mut self,
        collection: CollectionId,
        field: &FieldComponentKey,
        index: &PostingIndex,
        report: &mut ValidationReport,
    ) -> Result<(), StableIndexError> {
        let Some(ids) = self.field_components(collection, field).cloned() else {
            report.error(format!(
                "collection {} field {} is missing",
                collection.0,
                field.path.join(".")
            ));
            return Ok(());
        };
        let values_component = ids
            .values
            .ok_or_else(|| StableIndexError::MissingDependency("typed values".into()))?;
        let stable_values = self.decode_scalar_map(values_component)?;
        report.posting_keys += stable_values.len() as u64;
        report.posting_members += stable_values.values().map(RoaringBitmap::len).sum::<u64>();
        if stable_values != index.values {
            report.error(format!(
                "collection {} field {} values differ",
                collection.0,
                field.path.join(".")
            ));
        }
        let lengths_component = ids
            .lengths
            .ok_or_else(|| StableIndexError::MissingDependency("lengths".into()))?;
        let stable_lengths = self.decode_number_map(lengths_component)?;
        if stable_lengths != index.lengths {
            report.error(format!(
                "collection {} field {} lengths differ",
                collection.0,
                field.path.join(".")
            ));
        }
        let states_component = ids
            .states
            .ok_or_else(|| StableIndexError::MissingDependency("states".into()))?;
        let stable_states = self.decode_state_map(states_component)?;
        let mut expected_states = BTreeMap::new();
        expected_states.insert(codec::STATE_PRESENCE, index.presence.clone());
        expected_states.insert(codec::STATE_EMPTY_SELECTION, index.empty_selections.clone());
        expected_states.insert(codec::STATE_LEGACY_NULL, index.nulls.clone());
        if let Some(facts) = &index.structural {
            for (key, posting) in &facts.postings {
                let state = match key {
                    StructuralKey::AnyNull => codec::STATE_ANY_NULL,
                    StructuralKey::NonEmpty => codec::STATE_NON_EMPTY,
                    StructuralKey::String => codec::STATE_STRING,
                    StructuralKey::Number => codec::STATE_NUMBER,
                    StructuralKey::Array => codec::STATE_ARRAY,
                    StructuralKey::Object => codec::STATE_OBJECT,
                    StructuralKey::EmptyObject => codec::STATE_EMPTY_OBJECT,
                };
                expected_states.insert(state, posting.clone());
            }
        }
        if stable_states != expected_states {
            report.error(format!(
                "collection {} field {} states differ",
                collection.0,
                field.path.join(".")
            ));
        }
        let descriptor = self
            .components
            .get(&values_component)
            .map(|entry| entry.descriptor.clone());
        if let Some(descriptor) = descriptor {
            let coverage = &descriptor.coverage;
            if coverage.value_complete != index.value_complete
                || coverage.length_complete != index.length_complete
                || coverage.empty_string_complete != index.empty_string_complete
                || coverage.values_indexed != index.values_indexed
                || coverage.covered_records != index.covered_records
            {
                report.error(format!(
                    "collection {} field {} coverage differs",
                    collection.0,
                    field.path.join(".")
                ));
            }
        }
        Ok(())
    }

    fn compare_text(
        &mut self,
        collection: CollectionId,
        key: &TextComponentKey,
        index: &BigramIndex,
        report: &mut ValidationReport,
    ) -> Result<(), StableIndexError> {
        let Some(ids) = self.text_components(collection, key).cloned() else {
            report.error(format!(
                "collection {} text {}:{} is missing",
                collection.0,
                key.path.join("."),
                key.binding.as_deref().unwrap_or("")
            ));
            return Ok(());
        };
        let normal = self.decode_gram_map(
            ids.normal
                .ok_or_else(|| StableIndexError::MissingDependency("normal grams".into()))?,
        )?;
        let skip = self.decode_gram_map(
            ids.skip
                .ok_or_else(|| StableIndexError::MissingDependency("skip grams".into()))?,
        )?;
        report.gram_keys += (normal.len() + skip.len()) as u64;
        report.gram_members += normal.values().map(RoaringBitmap::len).sum::<u64>()
            + skip.values().map(RoaringBitmap::len).sum::<u64>();
        let expected_normal = index
            .normal
            .iter()
            .map(|(key, value)| (*key, value.clone()))
            .collect::<BTreeMap<_, _>>();
        let expected_skip = index
            .skip
            .iter()
            .map(|(key, value)| (*key, value.clone()))
            .collect::<BTreeMap<_, _>>();
        if normal != expected_normal {
            report.error(format!(
                "collection {} normal grams {}:{} differ",
                collection.0,
                key.path.join("."),
                key.binding.as_deref().unwrap_or("")
            ));
        }
        if skip != expected_skip {
            report.error(format!(
                "collection {} skip grams {}:{} differ",
                collection.0,
                key.path.join("."),
                key.binding.as_deref().unwrap_or("")
            ));
        }
        Ok(())
    }

    fn compare_relation(
        &mut self,
        collection: CollectionId,
        relation: &RelationIndex,
        report: &mut ValidationReport,
    ) -> Result<(), StableIndexError> {
        let key = RelationComponentKey {
            path: relation.declaration.path.clone(),
            target_collection: relation.declaration.target_collection.clone(),
        };
        let Some(ids) = self.relation_components(collection, &key).cloned() else {
            report.error(format!(
                "collection {} relation {} is missing",
                collection.0,
                relation.declaration.path.join(".")
            ));
            return Ok(());
        };
        let mut expected_targets = BTreeMap::<String, RoaringBitmap>::new();
        for slot in 0..relation_max_slot(relation) {
            let source = RecordSlot(slot);
            let stable = self.relation_occurrences(collection, &key, source)?;
            let Some(native) = relation.edges(source) else {
                if !stable.is_empty() {
                    report.error(format!("relation source {} has unexpected edges", slot));
                }
                continue;
            };
            report.relation_occurrences += native.len() as u64;
            if stable.len() != native.len() {
                report.error(format!("relation source {} edge count differs", slot));
                continue;
            }
            for (stable, native) in stable.iter().zip(native) {
                if stable.target_id != native.target_id()
                    || stable.positions != native.positions
                    || stable.target_resolved
                        != matches!(native.target, RelationTarget::Resolved(_))
                {
                    report.error(format!("relation source {} edge differs", slot));
                }
            }
            for edge in native {
                expected_targets
                    .entry(edge.target_id().to_owned())
                    .or_default()
                    .insert(slot);
            }
        }
        let tree = self.tree(ids.targets)?;
        let mut actual_targets = BTreeMap::new();
        for entry in tree.iter() {
            if let ComponentKey::RelationTarget(RelationTargetKey { target_id, .. }) = entry.key() {
                actual_targets.insert(
                    target_id.clone(),
                    super::memory::decode_bitmap(&entry.value())
                        .map_err(StableIndexError::Format)?,
                );
            }
        }
        if actual_targets != expected_targets {
            report.error(format!(
                "collection {} relation {} target postings differ",
                collection.0,
                relation.declaration.path.join(".")
            ));
        }
        for (state, expected, label) in [
            (codec::RELATION_PRESENT, relation.present(), "present"),
            (codec::RELATION_NULLS, relation.nulls(), "nulls"),
            (
                codec::RELATION_EMPTY_ARRAY,
                relation.empty_arrays(),
                "empty-arrays",
            ),
            (
                codec::RELATION_UNAVAILABLE,
                relation.unavailable(),
                "unavailable",
            ),
        ] {
            let actual = self.relation_state(collection, &key, state)?;
            if &actual != expected {
                report.error(format!(
                    "collection {} relation {} {label} state differs",
                    collection.0,
                    relation.declaration.path.join(".")
                ));
            }
        }
        Ok(())
    }

    fn decode_scalar_map(
        &mut self,
        component: super::ComponentId,
    ) -> Result<BTreeMap<ScalarKey, RoaringBitmap>, StableIndexError> {
        let tree = self.tree(component)?;
        let mut result = BTreeMap::new();
        let mut decodes = 0u64;
        for entry in tree.iter() {
            if let ComponentKey::Scalar(key) = entry.key() {
                result.insert(
                    key.clone(),
                    super::memory::decode_bitmap(&entry.value())
                        .map_err(StableIndexError::Format)?,
                );
                decodes += 1;
            }
        }
        self.counters.bitmap_decodes += decodes;
        Ok(result)
    }

    fn decode_number_map(
        &mut self,
        component: super::ComponentId,
    ) -> Result<BTreeMap<crate::model::Number, RoaringBitmap>, StableIndexError> {
        let tree = self.tree(component)?;
        let mut result = BTreeMap::new();
        let mut decodes = 0u64;
        for entry in tree.iter() {
            if let ComponentKey::Length(key) = entry.key() {
                result.insert(
                    key.clone(),
                    super::memory::decode_bitmap(&entry.value())
                        .map_err(StableIndexError::Format)?,
                );
                decodes += 1;
            }
        }
        self.counters.bitmap_decodes += decodes;
        Ok(result)
    }

    fn decode_state_map(
        &mut self,
        component: super::ComponentId,
    ) -> Result<BTreeMap<u8, RoaringBitmap>, StableIndexError> {
        let tree = self.tree(component)?;
        let mut result = BTreeMap::new();
        let mut decodes = 0u64;
        for entry in tree.iter() {
            if let ComponentKey::State(state) = entry.key() {
                result.insert(
                    *state,
                    super::memory::decode_bitmap(&entry.value())
                        .map_err(StableIndexError::Format)?,
                );
                decodes += 1;
            }
        }
        self.counters.bitmap_decodes += decodes;
        Ok(result)
    }

    fn decode_gram_map(
        &mut self,
        component: super::ComponentId,
    ) -> Result<BTreeMap<u64, RoaringBitmap>, StableIndexError> {
        let tree = self.tree(component)?;
        let mut result = BTreeMap::new();
        let mut decodes = 0u64;
        for entry in tree.iter() {
            if let ComponentKey::Gram(gram) = entry.key() {
                result.insert(
                    *gram,
                    super::memory::decode_bitmap(&entry.value())
                        .map_err(StableIndexError::Format)?,
                );
                decodes += 1;
            }
        }
        self.counters.bitmap_decodes += decodes;
        Ok(result)
    }
}

fn relation_max_slot(relation: &RelationIndex) -> u32 {
    let mut all = relation.present().clone();
    all |= relation.nulls();
    all |= relation.empty_arrays();
    all |= relation.unavailable();
    all.max().map_or(0, |max| max.saturating_add(1))
}

fn record_entry_equal(
    left: &crate::inventory::RecordEntry,
    right: &crate::inventory::RecordEntry,
) -> bool {
    left.slot == right.slot
        && left.id == right.id
        && left.file == right.file
        && left.range == right.range
        && left.revision == right.revision
        && left.class == right.class
        && left.diagnostic_ref == right.diagnostic_ref
}
