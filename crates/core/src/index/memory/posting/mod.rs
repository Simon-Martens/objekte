//! Typed B-tree dictionaries with record-slot postings and special states.

use std::collections::{BTreeMap, HashMap, HashSet};

use roaring::RoaringBitmap;

use crate::inventory::append_shifted_bitmap;
use crate::model::{LogicalType, Number, RecordSlot, Value};
use crate::query::semantics::record::data_values_small;
use crate::schema::{FieldDefinition, Schema};

#[cfg(test)]
mod benchmark;
mod lookup;

const DIRECT_MULTIVALUE_LIMIT: usize = 8;

pub use crate::index::storage::posting::{ScalarKey, SelectionSummary};

/// Ordered typed values and their record-slot postings.
#[derive(Clone, Debug)]
pub(crate) struct PostingIndex {
    pub path: Vec<String>,
    pub kind: LogicalType,
    // eg. {"items": [{"price": 23}]}. Then the path items.price can match many different
    // concrete paths in a cocrete structure. For two keys in an object with the same path
    // see summary.multiple
    pub multi: bool,
    pub presence: RoaringBitmap,

    /// Complete terminal observation when construction can certify it.
    pub summary: Option<SelectionSummary>,
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

/// Optional named-binding posting and its exact coverage guarantee.
#[derive(Clone, Debug)]
pub(crate) struct CustomPostingIndex {
    pub index: PostingIndex,
    pub complete: bool,
}

/// Coverage and cardinality summary for one posting index.
#[derive(Clone, Debug)]
pub(crate) struct PostingStats {
    pub present_count: u64,
    pub null_count: u64,
    pub distinct_count: usize,
    pub min_key: Option<ScalarKey>,
    pub max_key: Option<ScalarKey>,
    /// Established posting-values meanings can use terminal presence when no
    /// successful zero-value traversal was observed in the build domain.
    pub legacy_present_equivalent: bool,
    /// Any-null postings can answer singleton-null statistics when no record
    /// selected null together with a sibling or multiple null occurrences.
    pub legacy_null_equivalent: bool,
}

impl Default for PostingStats {
    fn default() -> Self {
        Self {
            present_count: 0,
            null_count: 0,
            distinct_count: 0,
            min_key: None,
            max_key: None,
            legacy_present_equivalent: true,
            legacy_null_equivalent: true,
        }
    }
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
            || self.summary.is_some() != batch.summary.is_some()
        {
            return Err("posting batch representation mismatch".into());
        }
        let covered = self
            .covered_records
            .checked_add(batch.covered_records)
            .ok_or("posting coverage overflow")?;
        append_shifted_bitmap(&mut self.presence, &batch.presence, base)?;
        if let (Some(target), Some(source)) = (&mut self.summary, batch.summary) {
            target.merge_shifted(source);
        }
        for (key, posting) in batch.values {
            append_shifted_bitmap(self.values.entry(key).or_default(), &posting, base)?;
        }
        for (key, posting) in batch.lengths {
            append_shifted_bitmap(self.lengths.entry(key).or_default(), &posting, base)?;
        }
        self.stats.legacy_present_equivalent &= batch.stats.legacy_present_equivalent;
        self.stats.legacy_null_equivalent &= batch.stats.legacy_null_equivalent;
        self.covered_records = covered;
        Ok(())
    }

    pub fn validate_domain(&self, allowed: &RoaringBitmap) -> Result<(), String> {
        if !self.presence.is_subset(allowed)
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
            summary: None,
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
        index.summary = Some(SelectionSummary::default());
        index
    }

    pub fn finish_stats(&mut self) {
        let null_count = self.value_cardinality(&ScalarKey::Null);
        let min_key = self
            .values
            .iter()
            .find(|(key, _)| !matches!(key, ScalarKey::Null))
            .map(|(key, _)| key.clone());
        let max_key = self
            .values
            .iter()
            .rev()
            .find(|(key, _)| !matches!(key, ScalarKey::Null))
            .map(|(key, _)| key.clone());
        self.stats = PostingStats {
            present_count: self.presence.len(),
            null_count,
            distinct_count: self.distinct_values(),
            min_key,
            max_key,
            legacy_present_equivalent: self.stats.legacy_present_equivalent,
            legacy_null_equivalent: self.stats.legacy_null_equivalent,
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
        if selected.is_empty() {
            self.stats.legacy_present_equivalent = false;
            if let Some(summary) = &mut self.summary {
                summary.note_empty_traversal();
            }
            return Ok(());
        }
        self.presence.insert(slot.0);
        if let Some(summary) = &mut self.summary {
            if selected.len() > 1 {
                summary.note_multiple();
            }
            for value in selected {
                summary.observe(value);
            }
        }
        let has_null = selected.iter().any(|value| matches!(value, Value::Null));
        if has_null && selected.len() != 1 {
            self.stats.legacy_null_equivalent = false;
        }
        if has_null {
            self.values
                .entry(ScalarKey::Null)
                .or_default()
                .insert(slot.0);
        }
        if selected.iter().all(|value| matches!(value, Value::Null)) {
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
        let flattened_count = selected
            .iter()
            .try_fold(0usize, |total, value| {
                let count = match value {
                    Value::Array(array) => array.values.len(),
                    _ => 1,
                };
                total.checked_add(count)
            })
            .unwrap_or(usize::MAX);
        if flattened_count <= DIRECT_MULTIVALUE_LIMIT {
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
                            self.values.entry(key).or_default().insert(slot.0);
                        }
                    }
                } else if let Some(key) = ScalarKey::from_value(value) {
                    self.values.entry(key).or_default().insert(slot.0);
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

    #[cfg(test)]
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
            || !self.values.values().all(valid)
            || !self.lengths.values().all(valid)
        {
            return Err("posting contains out-of-domain record slot".into());
        }
        if self
            .values
            .values()
            .chain(self.lengths.values())
            .any(|slots| !slots.is_subset(&self.presence))
        {
            return Err("value or length posting is not present".into());
        }
        if self.stats.present_count != self.presence.len()
            || self.stats.null_count != self.value_cardinality(&ScalarKey::Null)
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
        self.values.len() - usize::from(self.values.contains_key(&ScalarKey::Null))
    }

    pub(crate) fn distinct_lengths(&self) -> usize {
        self.lengths.len()
    }
}

/// Posting indexes registered for one collection.
#[derive(Clone, Debug)]
pub(crate) struct PostingCollection {
    pub fields: HashMap<Vec<String>, PostingIndex>,
}

impl PostingCollection {
    pub(crate) fn transfer_pristine(&mut self, batch: Self) -> Result<(), String> {
        if self.fields.len() != batch.fields.len()
            || self
                .fields
                .keys()
                .any(|path| !batch.fields.contains_key(path))
        {
            return Err("posting batch field coverage mismatch".into());
        }
        *self = batch;
        Ok(())
    }

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
