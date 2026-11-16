//! Stable posting, gram, sort and relation primitives.

use std::collections::BTreeSet;
use std::ops::Bound::Included;

use roaring::RoaringBitmap;

use crate::model::{CollectionId, RecordSlot};
use crate::posting::ScalarKey;

use super::codec::{
    self, ComponentKey, RELATION_EMPTY_ARRAY, RELATION_NULLS, RELATION_PRESENT,
    RELATION_UNAVAILABLE, RelationOccurrenceKey, RelationOccurrenceValue, RelationTargetKey,
};
use super::{
    FieldComponentIds, FieldComponentKey, RelationComponentKey, SortComponentKey, StableIndexError,
    StablePrototype, TextComponentKey,
};

#[derive(Clone, Debug)]
pub enum Restriction {
    NoRestriction,
    Complete(RoaringBitmap),
}

impl Restriction {
    pub fn complete(self) -> Result<RoaringBitmap, StableIndexError> {
        match self {
            Self::Complete(bitmap) => Ok(bitmap),
            Self::NoRestriction => Err(StableIndexError::Unsupported(
                "the selected index cannot prove a complete result".into(),
            )),
        }
    }
}

impl StablePrototype {
    fn component_coverage(
        &self,
        component: super::ComponentId,
    ) -> Option<&super::CoverageDescriptor> {
        self.components
            .get(&component)
            .map(|entry| &entry.descriptor.coverage)
    }
    pub fn field_components(
        &self,
        collection: CollectionId,
        key: &FieldComponentKey,
    ) -> Option<&FieldComponentIds> {
        self.collections
            .get(&collection)
            .and_then(|entry| entry.fields.get(key))
    }

    pub(crate) fn posting_value(
        &mut self,
        component: super::ComponentId,
        key: ComponentKey,
    ) -> Result<Option<RoaringBitmap>, StableIndexError> {
        let tree = self.tree(component)?;
        let value = tree.get(&key);
        self.counters.stable_gets += 1;
        let Some(value) = value else {
            return Ok(None);
        };
        self.counters.bitmap_decodes += 1;
        Ok(Some(
            super::memory::decode_bitmap(&value).map_err(StableIndexError::Format)?,
        ))
    }

    pub fn raw_posting(
        &mut self,
        component: super::ComponentId,
        key: &ComponentKey,
    ) -> Result<Option<RoaringBitmap>, StableIndexError> {
        self.posting_value(component, key.clone())
    }

    pub fn typed_posting(
        &mut self,
        collection: CollectionId,
        field: &FieldComponentKey,
        value: &ScalarKey,
    ) -> Result<Restriction, StableIndexError> {
        let component = self
            .field_components(collection, field)
            .and_then(|entry| entry.values)
            .ok_or_else(|| StableIndexError::MissingDependency("typed value index".into()))?;
        let coverage = self.component_coverage(component).cloned().ok_or_else(|| {
            StableIndexError::MissingDependency("typed value coverage descriptor".into())
        })?;
        if !coverage.values_indexed || !coverage.value_complete {
            return Ok(Restriction::NoRestriction);
        }
        let posting = self
            .posting_value(component, ComponentKey::Scalar(value.clone()))?
            .unwrap_or_default();
        let live = self.live_domain_shared(collection)?;
        Ok(Restriction::Complete(posting & &*live))
    }

    pub fn visit_typed_range(
        &mut self,
        collection: CollectionId,
        field: &FieldComponentKey,
        lower: std::ops::Bound<ScalarKey>,
        upper: std::ops::Bound<ScalarKey>,
        descending: bool,
        mut visit: impl FnMut(ScalarKey, RoaringBitmap) -> Result<(), String>,
    ) -> Result<(), StableIndexError> {
        let component = self
            .field_components(collection, field)
            .and_then(|entry| entry.values)
            .ok_or_else(|| StableIndexError::MissingDependency("typed value index".into()))?;
        let live = self.live_domain_shared(collection)?;
        let lower = map_scalar_bound(lower);
        let upper = map_scalar_bound(upper);
        let tree = self.tree(component)?;
        let mut decodes = 0u64;
        if descending {
            for entry in tree.range((lower, upper)).rev() {
                let ComponentKey::Scalar(key) = entry.key() else {
                    continue;
                };
                let posting = super::memory::decode_bitmap(&entry.value())
                    .map_err(StableIndexError::Format)?;
                decodes += 1;
                let posting = posting & &*live;
                if !posting.is_empty() {
                    visit(key.clone(), posting).map_err(StableIndexError::Format)?;
                }
            }
        } else {
            for entry in tree.range((lower, upper)) {
                let ComponentKey::Scalar(key) = entry.key() else {
                    continue;
                };
                let posting = super::memory::decode_bitmap(&entry.value())
                    .map_err(StableIndexError::Format)?;
                decodes += 1;
                let posting = posting & &*live;
                if !posting.is_empty() {
                    visit(key.clone(), posting).map_err(StableIndexError::Format)?;
                }
            }
        }
        self.counters.bitmap_decodes += decodes;
        Ok(())
    }

    pub fn typed_range_slots(
        &mut self,
        collection: CollectionId,
        field: &FieldComponentKey,
        lower: std::ops::Bound<ScalarKey>,
        upper: std::ops::Bound<ScalarKey>,
        descending: bool,
    ) -> Result<Restriction, StableIndexError> {
        let component = self
            .field_components(collection, field)
            .and_then(|entry| entry.values)
            .ok_or_else(|| StableIndexError::MissingDependency("typed value index".into()))?;
        let coverage = self.component_coverage(component).cloned().ok_or_else(|| {
            StableIndexError::MissingDependency("typed value coverage descriptor".into())
        })?;
        if !coverage.values_indexed || !coverage.value_complete {
            return Ok(Restriction::NoRestriction);
        }
        let mut result = RoaringBitmap::new();
        self.visit_typed_range(collection, field, lower, upper, descending, |_, posting| {
            result |= posting;
            Ok(())
        })?;
        Ok(Restriction::Complete(result))
    }

    pub fn length_posting(
        &mut self,
        collection: CollectionId,
        field: &FieldComponentKey,
        length: &crate::model::Number,
    ) -> Result<Restriction, StableIndexError> {
        let component = self
            .field_components(collection, field)
            .and_then(|entry| entry.lengths)
            .ok_or_else(|| StableIndexError::MissingDependency("length index".into()))?;
        let coverage = self.component_coverage(component).cloned().ok_or_else(|| {
            StableIndexError::MissingDependency("length coverage descriptor".into())
        })?;
        if !coverage.length_complete {
            return Ok(Restriction::NoRestriction);
        }
        let posting = self
            .posting_value(component, ComponentKey::Length(length.clone()))?
            .unwrap_or_default();
        let live = self.live_domain_shared(collection)?;
        Ok(Restriction::Complete(posting & &*live))
    }

    pub fn visit_length_range(
        &mut self,
        collection: CollectionId,
        field: &FieldComponentKey,
        lower: std::ops::Bound<crate::model::Number>,
        upper: std::ops::Bound<crate::model::Number>,
        mut visit: impl FnMut(crate::model::Number, RoaringBitmap) -> Result<(), String>,
    ) -> Result<(), StableIndexError> {
        let component = self
            .field_components(collection, field)
            .and_then(|entry| entry.lengths)
            .ok_or_else(|| StableIndexError::MissingDependency("length index".into()))?;
        let live = self.live_domain_shared(collection)?;
        let lower = map_length_bound(lower);
        let upper = map_length_bound(upper);
        let tree = self.tree(component)?;
        let mut decodes = 0u64;
        for entry in tree.range((lower, upper)) {
            let ComponentKey::Length(length) = entry.key() else {
                continue;
            };
            let posting =
                super::memory::decode_bitmap(&entry.value()).map_err(StableIndexError::Format)?;
            decodes += 1;
            let posting = posting & &*live;
            if !posting.is_empty() {
                visit(length.clone(), posting).map_err(StableIndexError::Format)?;
            }
        }
        self.counters.bitmap_decodes += decodes;
        Ok(())
    }

    pub fn state_posting(
        &mut self,
        collection: CollectionId,
        field: &FieldComponentKey,
        state: u8,
    ) -> Result<Restriction, StableIndexError> {
        let component = self
            .field_components(collection, field)
            .and_then(|entry| entry.states)
            .ok_or_else(|| StableIndexError::MissingDependency("state index".into()))?;
        let coverage = self.component_coverage(component).cloned().ok_or_else(|| {
            StableIndexError::MissingDependency("state coverage descriptor".into())
        })?;
        if !coverage.structural_complete {
            return Ok(Restriction::NoRestriction);
        }
        let posting = self
            .posting_value(component, ComponentKey::State(state))?
            .unwrap_or_default();
        let live = self.live_domain_shared(collection)?;
        Ok(Restriction::Complete(posting & &*live))
    }

    pub fn text_components(
        &self,
        collection: CollectionId,
        key: &TextComponentKey,
    ) -> Option<&super::TextComponentIds> {
        self.collections
            .get(&collection)
            .and_then(|entry| entry.texts.get(key))
    }

    pub fn gram_posting(
        &mut self,
        component: super::ComponentId,
        gram: u64,
    ) -> Result<RoaringBitmap, StableIndexError> {
        Ok(self
            .posting_value(component, ComponentKey::Gram(gram))?
            .unwrap_or_default())
    }

    /// Intersect one text binding's normal or skip grams.  `None` means no
    /// restriction is available, not an empty result.
    pub fn gram_candidates(
        &mut self,
        collection: CollectionId,
        key: &TextComponentKey,
        grams: &[u64],
        skip: bool,
    ) -> Result<Option<RoaringBitmap>, StableIndexError> {
        if grams.is_empty() {
            return Ok(None);
        }
        let entry = self
            .text_components(collection, key)
            .ok_or_else(|| StableIndexError::MissingDependency("text gram index".into()))?
            .clone();
        let check_component = if skip { entry.skip } else { entry.normal }
            .ok_or_else(|| StableIndexError::MissingDependency("text gram index".into()))?;
        if !self
            .component_coverage(check_component)
            .is_some_and(|coverage| coverage.complete_for_field)
        {
            return Ok(None);
        }
        let component = if skip {
            entry
                .skip
                .ok_or_else(|| StableIndexError::MissingDependency("skip-gram index".into()))?
        } else {
            entry
                .normal
                .ok_or_else(|| StableIndexError::MissingDependency("normal-gram index".into()))?
        };
        let live = self.live_domain_shared(collection)?;
        let tree = self.tree(component)?;
        let mut candidates: Option<RoaringBitmap> = None;
        let mut decodes = 0u64;
        for gram in grams {
            let posting = tree
                .get(&ComponentKey::Gram(*gram))
                .map(|value| super::memory::decode_bitmap(&value))
                .transpose()
                .map_err(StableIndexError::Format)?
                .unwrap_or_default();
            decodes += 1;
            candidates = Some(match candidates {
                None => posting,
                Some(current) => current & posting,
            });
            if candidates.as_ref().is_some_and(RoaringBitmap::is_empty) {
                break;
            }
        }
        self.counters.bitmap_decodes += decodes;
        Ok(candidates.map(|posting| posting & &*live))
    }

    /// Prepare a literal with master's existing bigram folding and intersect
    /// both normal and skip gram columns. `None` means no gram restriction is
    /// available, not an empty result.
    pub fn text_candidates(
        &mut self,
        collection: CollectionId,
        key: &TextComponentKey,
        literal: &str,
        alphabet: crate::workload::BigramAlphabet,
    ) -> Result<Option<RoaringBitmap>, StableIndexError> {
        let request = crate::bigram::BigramRequest::new(literal);
        let skip_grams = request
            .grams(alphabet)
            .iter()
            .filter(|(skip, _)| *skip)
            .map(|(_, key)| *key)
            .collect::<Vec<_>>();
        let normal_grams = request
            .grams(alphabet)
            .iter()
            .filter(|(skip, _)| !*skip)
            .map(|(_, key)| *key)
            .collect::<Vec<_>>();
        let mut candidates: Option<RoaringBitmap> = None;
        if !normal_grams.is_empty() {
            candidates = self.gram_candidates(collection, key, &normal_grams, false)?;
        }
        if !skip_grams.is_empty() {
            let skip = self.gram_candidates(collection, key, &skip_grams, true)?;
            candidates = Some(match (candidates, skip) {
                (Some(current), Some(skip)) => current & skip,
                (Some(current), None) => current,
                (None, Some(skip)) => skip,
                (None, None) => return Ok(None),
            });
        }
        Ok(candidates)
    }

    pub fn sort_components(
        &self,
        collection: CollectionId,
        key: &SortComponentKey,
    ) -> Option<&super::SortComponentIds> {
        self.collections
            .get(&collection)
            .and_then(|entry| entry.sorts.get(key))
    }

    pub fn sort_page(
        &mut self,
        collection: CollectionId,
        key: &SortComponentKey,
        descending: bool,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<RecordSlot>, StableIndexError> {
        let sort = self
            .sort_components(collection, key)
            .ok_or_else(|| StableIndexError::MissingDependency("configured sort".into()))?
            .clone();
        let component = if descending {
            sort.descending
        } else {
            sort.ascending
        };
        let live = self.live_domain_shared(collection)?;
        let tree = self.tree(component)?;
        let mut skipped = 0usize;
        let mut result = Vec::new();
        for entry in tree.iter() {
            let ComponentKey::Sort(sort_key) = entry.key() else {
                continue;
            };
            if !live.contains(sort_key.slot.0) {
                continue;
            }
            if skipped < offset {
                skipped += 1;
                continue;
            }
            result.push(sort_key.slot);
            if result.len() == limit {
                break;
            }
        }
        Ok(result)
    }

    pub fn sort_slots(
        &mut self,
        collection: CollectionId,
        key: &SortComponentKey,
        descending: bool,
    ) -> Result<Vec<RecordSlot>, StableIndexError> {
        self.sort_page(collection, key, descending, 0, usize::MAX)
    }

    pub fn composite_component(
        &self,
        collection: CollectionId,
        key: &str,
    ) -> Option<super::ComponentId> {
        self.collections
            .get(&collection)
            .and_then(|entry| entry.composites.get(key).copied())
    }

    pub fn sort_composite_page(
        &mut self,
        collection: CollectionId,
        key: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<RecordSlot>, StableIndexError> {
        let component = self.composite_component(collection, key).ok_or_else(|| {
            StableIndexError::MissingDependency("configured composite sort".into())
        })?;
        let live = self.live_domain_shared(collection)?;
        let tree = self.tree(component)?;
        let mut skipped = 0usize;
        let mut result = Vec::new();
        for entry in tree.iter() {
            let ComponentKey::CompositeSort(sort_key) = entry.key() else {
                continue;
            };
            if !live.contains(sort_key.slot.0) {
                continue;
            }
            if skipped < offset {
                skipped += 1;
                continue;
            }
            result.push(sort_key.slot);
            if result.len() == limit {
                break;
            }
        }
        Ok(result)
    }

    pub fn sort_composite_slots(
        &mut self,
        collection: CollectionId,
        key: &str,
    ) -> Result<Vec<RecordSlot>, StableIndexError> {
        self.sort_composite_page(collection, key, 0, usize::MAX)
    }

    pub fn relation_components(
        &self,
        collection: CollectionId,
        key: &RelationComponentKey,
    ) -> Option<&super::RelationComponentIds> {
        self.collections
            .get(&collection)
            .and_then(|entry| entry.relations.get(key))
    }

    pub fn relation_occurrences(
        &mut self,
        collection: CollectionId,
        key: &RelationComponentKey,
        source: RecordSlot,
    ) -> Result<Vec<RelationOccurrenceValue>, StableIndexError> {
        let relation = self
            .relation_components(collection, key)
            .ok_or_else(|| StableIndexError::MissingDependency("relation index".into()))?
            .clone();
        let tree = self.tree(relation.occurrences)?;
        let lower = ComponentKey::Occurrence(RelationOccurrenceKey { source, ordinal: 0 });
        let upper = ComponentKey::Occurrence(RelationOccurrenceKey {
            source,
            ordinal: u32::MAX,
        });
        let mut values = Vec::new();
        for entry in tree.range((Included(lower), Included(upper))) {
            let ComponentKey::Occurrence(_) = entry.key() else {
                continue;
            };
            values.push(codec::decode_meta(&entry.value()).map_err(StableIndexError::Format)?);
        }
        Ok(values)
    }

    pub fn relation_state(
        &mut self,
        collection: CollectionId,
        key: &RelationComponentKey,
        state: u8,
    ) -> Result<RoaringBitmap, StableIndexError> {
        let relation = self
            .relation_components(collection, key)
            .ok_or_else(|| StableIndexError::MissingDependency("relation index".into()))?
            .clone();
        let posting = self
            .posting_value(relation.states, ComponentKey::RelationState(state))?
            .unwrap_or_default();
        let live = self.live_domain_shared(collection)?;
        Ok(posting & &*live)
    }

    pub fn relation_present(
        &mut self,
        collection: CollectionId,
        key: &RelationComponentKey,
    ) -> Result<RoaringBitmap, StableIndexError> {
        self.relation_state(collection, key, RELATION_PRESENT)
    }

    pub fn relation_nulls(
        &mut self,
        collection: CollectionId,
        key: &RelationComponentKey,
    ) -> Result<RoaringBitmap, StableIndexError> {
        self.relation_state(collection, key, RELATION_NULLS)
    }

    pub fn relation_empty_arrays(
        &mut self,
        collection: CollectionId,
        key: &RelationComponentKey,
    ) -> Result<RoaringBitmap, StableIndexError> {
        self.relation_state(collection, key, RELATION_EMPTY_ARRAY)
    }

    pub fn relation_unavailable(
        &mut self,
        collection: CollectionId,
        key: &RelationComponentKey,
    ) -> Result<RoaringBitmap, StableIndexError> {
        self.relation_state(collection, key, RELATION_UNAVAILABLE)
    }

    pub fn relation_target_sources(
        &mut self,
        collection: CollectionId,
        key: &RelationComponentKey,
        target_collection: CollectionId,
        target_id: &str,
    ) -> Result<RoaringBitmap, StableIndexError> {
        let relation = self
            .relation_components(collection, key)
            .ok_or_else(|| StableIndexError::MissingDependency("relation index".into()))?
            .clone();
        let target_key = RelationTargetKey {
            target_collection,
            target_id: target_id.to_owned(),
        };
        let posting = self
            .posting_value(relation.targets, ComponentKey::RelationTarget(target_key))?
            .unwrap_or_default();
        let live = self.live_domain_shared(collection)?;
        Ok(posting & &*live)
    }

    /// Sorted distinct gram keys in one text component, used by diagnostics
    /// and fixture comparisons without materializing a native gram map.
    pub fn gram_keys(
        &mut self,
        component: super::ComponentId,
    ) -> Result<BTreeSet<u64>, StableIndexError> {
        let tree = self.tree(component)?;
        let mut keys = BTreeSet::new();
        for entry in tree.iter() {
            if let ComponentKey::Gram(gram) = entry.key() {
                keys.insert(*gram);
            }
        }
        Ok(keys)
    }
}

fn map_scalar_bound(bound: std::ops::Bound<ScalarKey>) -> std::ops::Bound<ComponentKey> {
    match bound {
        std::ops::Bound::Included(value) => std::ops::Bound::Included(ComponentKey::Scalar(value)),
        std::ops::Bound::Excluded(value) => std::ops::Bound::Excluded(ComponentKey::Scalar(value)),
        std::ops::Bound::Unbounded => std::ops::Bound::Unbounded,
    }
}

fn map_length_bound(bound: std::ops::Bound<crate::model::Number>) -> std::ops::Bound<ComponentKey> {
    match bound {
        std::ops::Bound::Included(value) => std::ops::Bound::Included(ComponentKey::Length(value)),
        std::ops::Bound::Excluded(value) => std::ops::Bound::Excluded(ComponentKey::Length(value)),
        std::ops::Bound::Unbounded => std::ops::Bound::Unbounded,
    }
}
