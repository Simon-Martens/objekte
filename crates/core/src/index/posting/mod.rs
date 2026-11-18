//! Typed B-tree dictionaries with record-slot postings and special states.

use std::collections::{BTreeMap, HashMap, HashSet};

use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};

use crate::inventory::append_shifted_bitmap;
use crate::model::{LogicalType, Number, RecordSlot, Value};
use crate::query::semantics::record::data_values_small;
use crate::query::semantics::state::{StatePredicate, value_matches};
use crate::schema::{FieldDefinition, Schema};

mod lookup;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub enum ScalarKey {
    String(String),
    Id(String),
    Number(Number),
    Bool(bool),
    Date(Date),
    Datetime(OffsetDateTime),
    Unixtime(i64),
}

impl ScalarKey {
    pub fn from_value(value: &Value) -> Option<Self> {
        Some(match value {
            Value::String(s) => Self::String(s.clone()),
            Value::Id(s) => Self::Id(s.clone()),
            Value::Number(n) => Self::Number(n.clone()),
            Value::Bool(b) => Self::Bool(*b),
            Value::Date(d) => Self::Date(*d),
            Value::Datetime(t) => Self::Datetime(*t),
            Value::Unixtime(t) => Self::Unixtime(*t),
            _ => return None,
        })
    }
}

/// Ordered typed values and their record-slot postings.
#[derive(Clone, Debug)]
pub struct PostingIndex {
    pub path: Vec<String>,
    pub kind: LogicalType,
    pub multi: bool,
    pub presence: RoaringBitmap,
    /// Legacy path presence can include a traversal that selects no values.
    /// Only these exceptional records need a separate bitmap for `:exists`.
    pub empty_selections: RoaringBitmap,
    pub nulls: RoaringBitmap,
    /// Exact selected-occurrence facts for an admitted logical field. `None`
    /// preserves the narrower legacy structural-only or named value index.
    pub structural: Option<StructuralFacts>,
    pub values: BTreeMap<ScalarKey, RoaringBitmap>,
    pub lengths: BTreeMap<Number, RoaringBitmap>,
    /// Whether the declared field admits a complete typed value dictionary.
    /// Partial length and empty-string shortcuts are tracked separately.
    pub values_indexed: bool,
    /// A partial dictionary cannot prove that an absent scalar value is absent.
    pub value_complete: bool,
    pub length_complete: bool,
    pub empty_string_complete: bool,
    pub covered_records: u32,
    pub stats: PostingStats,
}

/// Sparse facts inside the owning field posting, never synthetic field paths.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum StructuralKey {
    AnyNull,
    NonEmpty,
    String,
    Number,
    Array,
    Object,
    EmptyObject,
}

#[derive(Clone, Debug, Default)]
pub struct StructuralFacts {
    pub postings: BTreeMap<StructuralKey, RoaringBitmap>,
}

impl StructuralFacts {
    fn insert(&mut self, key: StructuralKey, slot: RecordSlot) {
        self.postings.entry(key).or_default().insert(slot.0);
    }

    fn observe(&mut self, slot: RecordSlot, value: &Value, multi: bool) {
        if value_matches(value, StatePredicate::NonEmpty) {
            self.insert(StructuralKey::NonEmpty, slot);
        }
        match value {
            Value::Null if multi => self.insert(StructuralKey::AnyNull, slot),
            Value::String(_) | Value::Id(_) => self.insert(StructuralKey::String, slot),
            Value::Number(_) => self.insert(StructuralKey::Number, slot),
            Value::Array(_) => self.insert(StructuralKey::Array, slot),
            Value::Object(map) => {
                self.insert(StructuralKey::Object, slot);
                if map.is_empty() {
                    self.insert(StructuralKey::EmptyObject, slot);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn merge_shifted(&mut self, batch: Self, base: u32) -> Result<(), String> {
        for (key, posting) in batch.postings {
            append_shifted_bitmap(self.postings.entry(key).or_default(), &posting, base)?;
        }
        Ok(())
    }

    pub(crate) fn estimated_bytes(&self) -> usize {
        self.postings
            .values()
            .map(RoaringBitmap::serialized_size)
            .sum()
    }
}

/// Optional named-binding posting and its exact coverage guarantee.
#[derive(Clone, Debug)]
pub struct CustomPostingIndex {
    pub index: PostingIndex,
    pub complete: bool,
}

/// Coverage and cardinality summary for one posting index.
#[derive(Clone, Debug, Default)]
pub struct PostingStats {
    pub present_count: u64,
    pub null_count: u64,
    pub distinct_count: usize,
    pub min_key: Option<ScalarKey>,
    pub max_key: Option<ScalarKey>,
}

impl PostingIndex {
    pub(crate) fn merge_shifted(&mut self, batch: Self, base: u32) -> Result<(), String> {
        if self.path != batch.path
            || self.kind != batch.kind
            || self.multi != batch.multi
            || self.values_indexed != batch.values_indexed
            || self.value_complete != batch.value_complete
            || self.length_complete != batch.length_complete
            || self.empty_string_complete != batch.empty_string_complete
            || self.structural.is_some() != batch.structural.is_some()
        {
            return Err("posting batch representation mismatch".into());
        }
        let covered = self
            .covered_records
            .checked_add(batch.covered_records)
            .ok_or("posting coverage overflow")?;
        append_shifted_bitmap(&mut self.presence, &batch.presence, base)?;
        append_shifted_bitmap(&mut self.empty_selections, &batch.empty_selections, base)?;
        append_shifted_bitmap(&mut self.nulls, &batch.nulls, base)?;
        if let (Some(target), Some(source)) = (&mut self.structural, batch.structural) {
            target.merge_shifted(source, base)?;
        }
        for (key, posting) in batch.values {
            append_shifted_bitmap(self.values.entry(key).or_default(), &posting, base)?;
        }
        for (key, posting) in batch.lengths {
            append_shifted_bitmap(self.lengths.entry(key).or_default(), &posting, base)?;
        }
        self.covered_records = covered;
        Ok(())
    }

    pub fn validate_domain(&self, allowed: &RoaringBitmap) -> Result<(), String> {
        if !self.presence.is_subset(allowed)
            || !self.empty_selections.is_subset(allowed)
            || !self.nulls.is_subset(allowed)
            || self.structural.as_ref().is_some_and(|facts| {
                facts
                    .postings
                    .values()
                    .any(|slots| !slots.is_subset(allowed))
            })
            || self
                .values
                .values()
                .chain(self.lengths.values())
                .any(|slots| !slots.is_subset(allowed))
        {
            return Err("posting outside logical eligible domain".into());
        }
        Ok(())
    }

    pub fn new(field: &FieldDefinition) -> Self {
        Self {
            path: field.path.clone(),
            kind: field.kind.clone(),
            multi: field.multi,
            presence: RoaringBitmap::new(),
            empty_selections: RoaringBitmap::new(),
            nulls: RoaringBitmap::new(),
            structural: None,
            values: BTreeMap::new(),
            lengths: BTreeMap::new(),
            values_indexed: true,
            value_complete: true,
            length_complete: matches!(field.kind, LogicalType::Array(_)),
            empty_string_complete: field.kind == LogicalType::String && !field.multi,
            covered_records: 0,
            stats: PostingStats::default(),
        }
    }

    fn structural(field: &FieldDefinition) -> Self {
        let mut index = Self::new(field);
        index.values_indexed = field.indexed;
        index.value_complete = field.indexed;
        index.structural = Some(StructuralFacts::default());
        index
    }

    pub fn finish_stats(&mut self) {
        self.stats = PostingStats {
            present_count: self.presence.len(),
            null_count: self.nulls.len(),
            distinct_count: self.values.len(),
            min_key: self.values.first_key_value().map(|(key, _)| key.clone()),
            max_key: self.values.last_key_value().map(|(key, _)| key.clone()),
        };
    }

    pub fn add(&mut self, slot: RecordSlot, record: &Value) -> Result<(), String> {
        let selected = data_values_small(record, &self.path);
        self.add_values(slot, selected.as_deref())
    }

    pub fn add_values(
        &mut self,
        slot: RecordSlot,
        selected: Option<&[&Value]>,
    ) -> Result<(), String> {
        self.covered_records = self
            .covered_records
            .checked_add(1)
            .ok_or("posting coverage overflow")?;
        let Some(selected) = selected else {
            return Ok(());
        };
        self.presence.insert(slot.0);
        if selected.is_empty() {
            self.empty_selections.insert(slot.0);
            return Ok(());
        }
        if let Some(facts) = &mut self.structural {
            for value in selected {
                facts.observe(slot, value, self.multi);
            }
        }
        if selected.len() == 1 && matches!(selected[0], Value::Null) {
            self.nulls.insert(slot.0);
            return Ok(());
        }
        if let [value] = selected {
            if !matches!(value, Value::Array(_)) {
                if let Value::String(text) = value
                    && text.is_empty()
                    && self.empty_string_complete
                    && !self.value_complete
                {
                    self.values
                        .entry(ScalarKey::String(String::new()))
                        .or_default()
                        .insert(slot.0);
                } else if self.value_complete
                    && let Some(key) = ScalarKey::from_value(value)
                {
                    self.values.entry(key).or_default().insert(slot.0);
                }
                return Ok(());
            }
        }
        if !self.value_complete {
            if self.length_complete {
                for value in selected {
                    if let Value::Array(array) = value {
                        self.lengths
                            .entry(Number::integer(array.values.len() as i128))
                            .or_default()
                            .insert(slot.0);
                    }
                }
            }
            return Ok(());
        }
        let mut unique = HashSet::new();
        for value in selected {
            if let Value::Array(array) = value {
                if self.length_complete {
                    self.lengths
                        .entry(Number::integer(array.values.len() as i128))
                        .or_default()
                        .insert(slot.0);
                }
                for item in &array.values {
                    if let Some(key) = ScalarKey::from_value(item) {
                        unique.insert(key);
                    }
                }
            } else if let Some(key) = ScalarKey::from_value(value) {
                unique.insert(key);
            }
        }
        for key in unique {
            self.values.entry(key).or_default().insert(slot.0);
        }
        Ok(())
    }

    pub fn validate(&self, record_count: usize) -> Result<(), String> {
        self.validate_eligible(record_count, record_count)
    }

    /// Check complete logical coverage while retaining the full slot bound.
    pub fn validate_eligible(
        &self,
        slot_count: usize,
        eligible_count: usize,
    ) -> Result<(), String> {
        if !self.values_indexed && self.value_complete {
            return Err("structural-only posting has complete values".into());
        }
        if self.covered_records as usize != eligible_count {
            return Err("incomplete posting coverage".into());
        }
        let valid = |set: &RoaringBitmap| set.max().is_none_or(|s| (s as usize) < slot_count);
        if !valid(&self.presence)
            || !valid(&self.empty_selections)
            || !valid(&self.nulls)
            || self
                .structural
                .as_ref()
                .is_some_and(|facts| facts.postings.values().any(|slots| !valid(slots)))
            || !self.values.values().all(valid)
            || !self.lengths.values().all(valid)
        {
            return Err("posting contains out-of-domain record slot".into());
        }
        if !self.nulls.is_subset(&self.presence) {
            return Err("null posting is not present".into());
        }
        if !self.empty_selections.is_subset(&self.presence) {
            return Err("empty selection is not present".into());
        }
        if self.structural.as_ref().is_some_and(|facts| {
            facts
                .postings
                .values()
                .any(|slots| !slots.is_subset(&self.presence))
                || facts
                    .postings
                    .get(&StructuralKey::EmptyObject)
                    .is_some_and(|empty| {
                        !empty.is_subset(
                            facts
                                .postings
                                .get(&StructuralKey::Object)
                                .unwrap_or(&RoaringBitmap::new()),
                        )
                    })
        }) {
            return Err("structural posting is inconsistent with presence or type".into());
        }
        if self.stats.present_count != self.presence.len()
            || self.stats.null_count != self.nulls.len()
            || self.stats.distinct_count != self.distinct_values()
        {
            return Err("posting statistics mismatch".into());
        }
        if !self.length_complete && !self.lengths.is_empty() {
            return Err("posting capability and stored facts mismatch".into());
        }
        Ok(())
    }

    pub fn distinct_values(&self) -> usize {
        self.values.len()
    }

    pub(crate) fn distinct_lengths(&self) -> usize {
        self.lengths.len()
    }
}

/// Posting indexes registered for one collection.
#[derive(Clone, Debug)]
pub struct PostingCollection {
    pub fields: HashMap<Vec<String>, PostingIndex>,
}

impl PostingCollection {
    pub(crate) fn merge_shifted(&mut self, batch: Self, base: u32) -> Result<(), String> {
        if self.fields.len() != batch.fields.len() {
            return Err("posting batch field coverage mismatch".into());
        }
        for (path, index) in batch.fields {
            self.fields
                .get_mut(&path)
                .ok_or("posting batch field missing")?
                .merge_shifted(index, base)?;
        }
        Ok(())
    }

    pub(crate) fn merge_shifted_in_pool(
        &mut self,
        batch: Self,
        base: u32,
        pool: &rayon::ThreadPool,
    ) -> Result<(), String> {
        if self.fields.len() != batch.fields.len() {
            return Err("posting batch field coverage mismatch".into());
        }
        let mut global = std::mem::take(&mut self.fields)
            .into_iter()
            .collect::<Vec<_>>();
        global.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        let mut incoming = batch.fields;
        let mut jobs = Vec::with_capacity(global.len());
        for (path, index) in global {
            let batch = incoming
                .remove(&path)
                .ok_or("posting batch field missing")?;
            jobs.push((path, index, batch));
        }
        if !incoming.is_empty() {
            return Err("posting batch field coverage mismatch".into());
        }
        let reduce = |jobs: Vec<(Vec<String>, PostingIndex, PostingIndex)>| {
            jobs.into_iter()
                .map(|(path, mut index, batch)| {
                    index.merge_shifted(batch, base)?;
                    Ok::<_, String>((path, index))
                })
                .collect::<Result<Vec<_>, _>>()
        };
        let mut first = jobs;
        let split = first.len().saturating_add(1) / 2;
        let second = first.split_off(split);
        let (left, right) = pool.join(|| reduce(first), || reduce(second));
        self.fields = left?.into_iter().chain(right?).collect();
        Ok(())
    }

    pub fn validate_domain(&self, allowed: &RoaringBitmap) -> Result<(), String> {
        for index in self.fields.values() {
            index.validate_domain(allowed)?;
        }
        Ok(())
    }

    pub fn new(schema: &Schema) -> Self {
        let fields = schema
            .fields
            .values()
            .map(|field| {
                let mut index = PostingIndex::structural(field);
                if !field.indexed && schema.is_auto_index_prohibited(&field.path) {
                    index.length_complete = false;
                    index.empty_string_complete = false;
                }
                (field.path.clone(), index)
            })
            .collect();
        Self { fields }
    }

    pub fn add(&mut self, slot: RecordSlot, record: &Value) -> Result<(), String> {
        for index in self.fields.values_mut() {
            index.add(slot, record)?;
        }
        Ok(())
    }

    pub fn finish_stats(&mut self) {
        for index in self.fields.values_mut() {
            index.finish_stats();
        }
    }

    pub fn validate(&self, record_count: usize) -> Result<(), String> {
        self.validate_eligible(record_count, record_count)
    }

    pub fn validate_eligible(
        &self,
        slot_count: usize,
        eligible_count: usize,
    ) -> Result<(), String> {
        for index in self.fields.values() {
            index.validate_eligible(slot_count, eligible_count)?;
        }
        Ok(())
    }
}
