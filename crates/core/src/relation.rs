//! Declared, collection-scoped links and immutable forward reference access.

use std::collections::{BTreeMap, HashMap};

use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};

use crate::inventory::{RecordClass, append_shifted_bitmap};
use crate::model::{CollectionId, LogicalType, RecordSlot, Value};
use crate::posting::ScalarKey;
use crate::reference::{FieldDefinition, Schema};
use crate::repository::{CollectionDefinition, CollectionState};

#[cfg(test)]
thread_local! {
    static REVERSE_RECONSTRUCTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}


/// Declared cardinality of a source relation field.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RelationCardinality {
    One,
    Many,
}


/// Format-independent declaration of one source field's target namespace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelationDefinition {
    pub path: Vec<String>,
    pub target_collection: String,
    pub cardinality: RelationCardinality,
    pub reverse_alias: Option<String>,
}


/// Stable relation identity within a repository configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct RelationId {
    pub source: CollectionId,
    pub ordinal: u32,
}


/// Resolved target slot or an ID that has no target in its declared namespace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RelationTarget {
    Resolved(RecordSlot),
    Unresolved(String),
}


/// One source occurrence of a declared ID, with array indexes from outer to inner.
/// The logical target ID remains available when the resolved slot is retired.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelationEdge {
    pub target: RelationTarget,
    target_id: String,
    pub positions: Vec<u32>,
}

impl RelationEdge {
    pub fn target_id(&self) -> &str {
        &self.target_id
    }
}


/// Immutable CSR adjacency for one declaration. A source's file provenance is
/// read from its `CollectionGeneration` record; no file ID is duplicated here.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RelationIndex {
    pub id: RelationId,
    pub declaration: RelationDefinition,
    pub target_collection: CollectionId,
    offsets: Vec<u32>,
    edges: Vec<RelationEdge>,
    present: RoaringBitmap,
    nulls: RoaringBitmap,
    empty_arrays: RoaringBitmap,
    unavailable: RoaringBitmap,
    unresolved_count: u64,
}


impl RelationIndex {
    pub fn edges(&self, source: RecordSlot) -> Option<&[RelationEdge]> {
        let start = *self.offsets.get(source.0 as usize)? as usize;
        let end = *self.offsets.get(source.0 as usize + 1)? as usize;
        self.edges.get(start..end)
    }

    pub fn present(&self) -> &RoaringBitmap {
        &self.present
    }

    pub fn nulls(&self) -> &RoaringBitmap {
        &self.nulls
    }

    pub fn empty_arrays(&self) -> &RoaringBitmap {
        &self.empty_arrays
    }

    pub fn unavailable(&self) -> &RoaringBitmap {
        &self.unavailable
    }

    pub fn unresolved_count(&self) -> u64 {
        self.unresolved_count
    }

    /// Report dangling references without changing the source record's parse status.
    pub fn unresolved_references(&self) -> impl Iterator<Item = (RecordSlot, &str, &[u32])> {
        self.offsets
            .windows(2)
            .enumerate()
            .flat_map(|(source, pair)| {
                self.edges[pair[0] as usize..pair[1] as usize]
                    .iter()
                    .filter_map(move |edge| match &edge.target {
                        RelationTarget::Unresolved(id) => Some((
                            RecordSlot(source as u32),
                            id.as_str(),
                            edge.positions.as_slice(),
                        )),
                        RelationTarget::Resolved(_) => None,
                    })
            })
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn estimated_heap_bytes(&self) -> usize {
        self.offsets.capacity() * std::mem::size_of::<u32>()
            + self.edges.capacity() * std::mem::size_of::<RelationEdge>()
            + self
                .edges
                .iter()
                .map(|edge| {
                    edge.positions.capacity() * 4
                        + edge.target_id.capacity()
                        + match &edge.target {
                            RelationTarget::Unresolved(id) => id.capacity(),
                            _ => 0,
                        }
                })
                .sum::<usize>()
            + self.present.serialized_size()
            + self.nulls.serialized_size()
            + self.empty_arrays.serialized_size()
            + self.unavailable.serialized_size()
    }

    pub(crate) fn validate(
        &self,
        source: &CollectionState,
        target: &CollectionState,
    ) -> Result<(), String> {
        if self.id.source != source.generation.collection
            || self.target_collection != target.generation.collection
            || self.declaration.target_collection != target.definition.name
            || source
                .definition
                .schema
                .relations()
                .get(&self.declaration.path)
                != Some(&self.declaration)
            || self.offsets.len() != source.generation.records.len() + 1
            || self.offsets.first() != Some(&0)
            || self.offsets.last().copied() != u32::try_from(self.edges.len()).ok()
        {
            return Err("relation identity or adjacency coverage mismatch".into());
        }
        let fallback: RoaringBitmap = source
            .generation
            .records
            .iter()
            .filter(|record| record.class == RecordClass::RawFallback)
            .map(|record| record.slot.0)
            .collect();
        if self.unavailable != fallback
            || !self.present.is_subset(&source.logical_records)
            || !self.nulls.is_subset(&self.present)
            || !self.empty_arrays.is_subset(&self.present)
        {
            return Err("relation source state mismatch".into());
        }
        let posting = source
            .postings
            .fields
            .get(&self.declaration.path)
            .ok_or("relation reverse posting missing")?;
        if posting.presence != self.present {
            return Err("relation reverse presence mismatch".into());
        }

        #[cfg(test)]
        REVERSE_RECONSTRUCTIONS.with(|count| count.set(count.get() + 1));
        let mut reverse = BTreeMap::<ScalarKey, RoaringBitmap>::new();
        let mut unresolved = 0u64;
        let mut previous_end = 0u32;
        for (source_slot, pair) in self.offsets.windows(2).enumerate() {
            let (start, end) = (pair[0], pair[1]);
            if start != previous_end || start > end || end as usize > self.edges.len() {
                return Err("relation adjacency offsets out of bounds".into());
            }
            previous_end = end;
            if start < end && !self.present.contains(source_slot as u32) {
                return Err("relation edge without present source".into());
            }
            for edge in &self.edges[start as usize..end as usize] {
                let id = match &edge.target {
                    RelationTarget::Resolved(slot) => {
                        let record = target
                            .generation
                            .records
                            .get(slot.0 as usize)
                            .ok_or("relation target slot out of bounds")?;
                        if record.slot != *slot {
                            return Err("relation target slot mismatch".into());
                        }
                        if edge.target_id() != record.id.0.as_str() {
                            return Err("relation resolved target ID mismatch".into());
                        }
                        &record.id.0
                    }
                    RelationTarget::Unresolved(id) => {
                        if edge.target_id() != id.as_str() {
                            return Err("relation unresolved target ID mismatch".into());
                        }
                        if target.id_lookup.contains_key(id.as_str()) {
                            return Err("relation unresolved target exists".into());
                        }
                        unresolved += 1;
                        id
                    }
                };
                reverse
                    .entry(ScalarKey::Id(id.clone()))
                    .or_default()
                    .insert(source_slot as u32);
            }
        }
        if reverse != posting.values || unresolved != self.unresolved_count {
            return Err("relation forward/reverse membership mismatch".into());
        }
        Ok(())
    }
}


/// One local ID occurrence before target inventories are available.
struct PendingReference {
    source: RecordSlot,
    target_id: String,
    positions: Vec<u32>,
}


/// Construction-only relation state, merged in deterministic batch order.
pub(crate) struct RelationContribution {
    declaration: RelationDefinition,
    record_count: u32,
    references: Vec<PendingReference>,
    present: RoaringBitmap,
    nulls: RoaringBitmap,
    empty_arrays: RoaringBitmap,
    unavailable: RoaringBitmap,
}


impl RelationContribution {
    pub(crate) fn declaration(&self) -> &RelationDefinition {
        &self.declaration
    }

    pub(crate) fn record_count(&self) -> u32 {
        self.record_count
    }

    pub(crate) fn references(&self) -> impl Iterator<Item = (RecordSlot, &str, &[u32])> {
        self.references.iter().map(|reference| {
            (
                reference.source,
                reference.target_id.as_str(),
                reference.positions.as_slice(),
            )
        })
    }

    pub(crate) fn present(&self) -> &RoaringBitmap {
        &self.present
    }

    pub(crate) fn nulls(&self) -> &RoaringBitmap {
        &self.nulls
    }

    pub(crate) fn empty_arrays(&self) -> &RoaringBitmap {
        &self.empty_arrays
    }

    pub(crate) fn unavailable(&self) -> &RoaringBitmap {
        &self.unavailable
    }

    pub(crate) fn for_schema(schema: &Schema) -> Vec<Self> {
        schema
            .relations()
            .values()
            .cloned()
            .map(|declaration| Self {
                declaration,
                record_count: 0,
                references: Vec::new(),
                present: RoaringBitmap::new(),
                nulls: RoaringBitmap::new(),
                empty_arrays: RoaringBitmap::new(),
                unavailable: RoaringBitmap::new(),
            })
            .collect()
    }

    pub(crate) fn add_logical(&mut self, slot: RecordSlot, value: &Value) -> Result<(), String> {
        if slot.0 != self.record_count {
            return Err("relation slots out of order".into());
        }
        self.record_count = self
            .record_count
            .checked_add(1)
            .ok_or("relation coverage overflow")?;
        let mut positions = Vec::new();
        let mut null = false;
        let mut empty = false;
        let references = &mut self.references;
        let found = visit_reference(
            value,
            &self.declaration.path,
            &mut positions,
            &mut null,
            &mut empty,
            &mut |target_id, positions| {
                references.push(PendingReference {
                    source: slot,
                    target_id: target_id.to_owned(),
                    positions: positions.to_vec(),
                });
            },
        )?;
        if found {
            self.present.insert(slot.0);
        }
        if null {
            self.nulls.insert(slot.0);
        }
        if empty {
            self.empty_arrays.insert(slot.0);
        }
        Ok(())
    }

    pub(crate) fn add_unavailable(&mut self, slot: RecordSlot) -> Result<(), String> {
        if slot.0 != self.record_count {
            return Err("relation slots out of order".into());
        }
        self.record_count = self
            .record_count
            .checked_add(1)
            .ok_or("relation coverage overflow")?;
        self.unavailable.insert(slot.0);
        Ok(())
    }

    pub(crate) fn merge_shifted(
        &mut self,
        batch: Self,
        base: u32,
        count: u32,
    ) -> Result<(), String> {
        if self.declaration != batch.declaration
            || self.record_count != base
            || batch.record_count != count
        {
            return Err("relation batch representation or coverage mismatch".into());
        }
        base.checked_add(count).ok_or("relation slot overflow")?;
        append_shifted_bitmap(&mut self.present, &batch.present, base)?;
        append_shifted_bitmap(&mut self.nulls, &batch.nulls, base)?;
        append_shifted_bitmap(&mut self.empty_arrays, &batch.empty_arrays, base)?;
        append_shifted_bitmap(&mut self.unavailable, &batch.unavailable, base)?;
        for mut reference in batch.references {
            reference.source.0 = reference
                .source
                .0
                .checked_add(base)
                .ok_or("relation source slot overflow")?;
            self.references.push(reference);
        }
        self.record_count = self
            .record_count
            .checked_add(count)
            .ok_or("relation coverage overflow")?;
        Ok(())
    }

    pub(crate) fn resolve(
        self,
        source: CollectionId,
        ordinal: u32,
        collections: &[CollectionState],
    ) -> Result<RelationIndex, String> {
        let source_state = collections
            .get(source.0 as usize)
            .ok_or("relation source collection missing")?;
        if self.record_count as usize != source_state.generation.records.len() {
            return Err("relation record coverage mismatch".into());
        }
        let target = collections
            .iter()
            .find(|collection| collection.definition.name == self.declaration.target_collection)
            .ok_or("relation target collection missing")?;
        let mut offsets = Vec::with_capacity(self.record_count as usize + 1);
        let mut edges = Vec::with_capacity(self.references.len());
        let mut references = self.references.into_iter().peekable();
        let mut unresolved_count = 0u64;
        offsets.push(0);
        for slot in 0..self.record_count {
            while references
                .peek()
                .is_some_and(|reference| reference.source.0 == slot)
            {
                let reference = references.next().unwrap();
                let target_ref =
                    if let Some(target_slot) = target.id_lookup.get(reference.target_id.as_str()) {
                        RelationTarget::Resolved(*target_slot)
                    } else {
                        unresolved_count = unresolved_count
                            .checked_add(1)
                            .ok_or("relation unresolved count overflow")?;
                        RelationTarget::Unresolved(reference.target_id.clone())
                    };
                edges.push(RelationEdge {
                    target: target_ref,
                    target_id: reference.target_id,
                    positions: reference.positions,
                });
            }
            offsets.push(u32::try_from(edges.len()).map_err(|_| "too many relation edges")?);
        }
        if references.next().is_some() {
            return Err("relation source order mismatch".into());
        }
        // The complete publication validation checks this index after every
        // contribution has been installed. Validating here would rebuild its
        // reverse dictionary a second time on every fresh construction.
        Ok(RelationIndex {
            id: RelationId { source, ordinal },
            declaration: self.declaration,
            target_collection: target.generation.collection,
            offsets,
            edges,
            present: self.present,
            nulls: self.nulls,
            empty_arrays: self.empty_arrays,
            unavailable: self.unavailable,
            unresolved_count,
        })
    }
}


fn visit_reference(
    value: &Value,
    path: &[String],
    positions: &mut Vec<u32>,
    null: &mut bool,
    empty: &mut bool,
    emit: &mut impl FnMut(&str, &[u32]),
) -> Result<bool, String> {
    if path.is_empty() {
        match value {
            Value::Id(id) => emit(id, positions),
            Value::Null => *null = true,
            Value::Array(array) => {
                if array.values.is_empty() {
                    *empty = true;
                }
                for (index, item) in array.values.iter().enumerate() {
                    positions
                        .push(u32::try_from(index).map_err(|_| "relation array index overflow")?);
                    visit_reference(item, path, positions, null, empty, emit)?;
                    positions.pop();
                }
            }
            _ => return Err("relation field has non-ID value".into()),
        }
        return Ok(true);
    }
    match value {
        Value::Object(fields) => fields.get(&path[0]).map_or(Ok(false), |next| {
            visit_reference(next, &path[1..], positions, null, empty, emit)
        }),
        Value::Array(array) => {
            if array.values.is_empty() {
                *empty = true;
                return Ok(true);
            }
            let mut found = false;
            for (index, item) in array.values.iter().enumerate() {
                positions.push(u32::try_from(index).map_err(|_| "relation array index overflow")?);
                found |= visit_reference(item, path, positions, null, empty, emit)?;
                positions.pop();
            }
            Ok(found)
        }
        _ => Ok(false),
    }
}


impl Schema {
    pub fn relations(&self) -> &BTreeMap<Vec<String>, RelationDefinition> {
        &self.relations
    }

    pub fn register_relation(&mut self, declaration: RelationDefinition) -> Result<(), String> {
        if declaration.path.is_empty() || declaration.target_collection.is_empty() {
            return Err("relation path or target collection is empty".into());
        }
        if let Some(alias) = &declaration.reverse_alias
            && !valid_alias(alias)
        {
            return Err("invalid relation reverse alias".into());
        }
        for depth in 1..declaration.path.len() {
            if let Some(ancestor) = self.fields.get(&declaration.path[..depth])
                && !traversable_kind(&ancestor.kind)
            {
                return Err(format!(
                    "relation path crosses non-object field: {}",
                    ancestor.path.join(".")
                ));
            }
        }
        if self
            .fields
            .keys()
            .any(|path| path.len() > declaration.path.len() && path.starts_with(&declaration.path))
        {
            return Err("relation field is also registered as an embedded object".into());
        }
        let field = self
            .fields
            .get_mut(&declaration.path)
            .ok_or("relation field is not registered")?;
        let expected = if field.multi || matches!(field.kind, LogicalType::Array(_)) {
            RelationCardinality::Many
        } else {
            RelationCardinality::One
        };
        let valid_type = field.kind == LogicalType::Id
            || field.kind == LogicalType::Array(Box::new(LogicalType::Id));
        if !valid_type || declaration.cardinality != expected {
            return Err("relation requires matching ID field cardinality".into());
        }
        if self.relations.contains_key(&declaration.path) {
            return Err("duplicate relation declaration".into());
        }
        field.indexed = true; // Typed reverse membership is required even without an ordinary index request.
        self.relations.insert(declaration.path.clone(), declaration);
        Ok(())
    }
}


pub(crate) fn validate_field_against_relations(
    field: &FieldDefinition,
    relations: &BTreeMap<Vec<String>, RelationDefinition>,
) -> Result<(), String> {
    for relation in relations.values() {
        if relation.path.len() > field.path.len()
            && relation.path.starts_with(&field.path)
            && !traversable_kind(&field.kind)
        {
            return Err("relation path crosses non-object field".into());
        }
        if field.path.len() > relation.path.len() && field.path.starts_with(&relation.path) {
            return Err("relation field is also registered as an embedded object".into());
        }
    }
    Ok(())
}

fn traversable_kind(kind: &LogicalType) -> bool {
    match kind {
        LogicalType::Object => true,
        LogicalType::Array(inner) => traversable_kind(inner),
        _ => false,
    }
}

fn valid_alias(alias: &str) -> bool {
    let mut bytes = alias.bytes();
    matches!(bytes.next(), Some(first) if first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

pub(crate) fn validate_definitions(definitions: &[CollectionDefinition]) -> Result<(), String> {
    let mut names = HashMap::new();
    for (index, definition) in definitions.iter().enumerate() {
        if names.insert(&definition.name, index).is_some() {
            return Err(format!("duplicate collection: {}", definition.name));
        }
    }
    let mut aliases = HashMap::<(String, String), (String, Vec<String>)>::new();
    for definition in definitions {
        for relation in definition.schema.relations().values() {
            if !names.contains_key(&relation.target_collection) {
                return Err(format!(
                    "relation target collection is unknown: {}",
                    relation.target_collection
                ));
            }
            if let Some(alias) = &relation.reverse_alias {
                let key = (relation.target_collection.clone(), alias.clone());
                if aliases
                    .insert(key, (definition.name.clone(), relation.path.clone()))
                    .is_some()
                {
                    return Err(format!("duplicate reverse relation alias: {alias}"));
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_indexes(collections: &[CollectionState]) -> Result<(), String> {
    for source in collections {
        if source.relations.len() != source.definition.schema.relations().len() {
            return Err("relation declaration coverage mismatch".into());
        }
        for (ordinal, (declaration, relation)) in source
            .definition
            .schema
            .relations()
            .values()
            .zip(&source.relations)
            .enumerate()
        {
            if relation.declaration != *declaration
                || relation.id
                    != (RelationId {
                        source: source.generation.collection,
                        ordinal: u32::try_from(ordinal).map_err(|_| "too many relations")?,
                    })
            {
                return Err("relation declaration order mismatch".into());
            }
            let target = collections
                .get(relation.target_collection.0 as usize)
                .ok_or("relation target collection out of bounds")?;
            relation.validate(source, target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "relation.test.rs"]
mod tests;
