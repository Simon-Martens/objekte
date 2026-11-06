//! Typed B-tree dictionaries with record-slot postings and special states.

use std::collections::{BTreeMap, HashMap, HashSet};

use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};

use crate::model::{LogicalType, Number, RecordSlot, Value};
use crate::query::{Operator, Quantifier};
use crate::reference::{BoundTerm, Derived, Transform};
use crate::reference::{FieldDefinition, Schema, data_values};


#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub enum ScalarKey {
    String(String),
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
            Value::Number(n) => Self::Number(*n),
            Value::Bool(b) => Self::Bool(*b),
            Value::Date(d) => Self::Date(*d),
            Value::Datetime(t) => Self::Datetime(*t),
            Value::Unixtime(t) => Self::Unixtime(*t),
            _ => return None,
        })
    }
}


/// Ordered typed values and their record-slot postings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PostingIndex {
    pub path: Vec<String>,
    pub kind: LogicalType,
    pub multi: bool,
    pub presence: RoaringBitmap,
    pub nulls: RoaringBitmap,
    pub values: BTreeMap<ScalarKey, RoaringBitmap>,
    pub lengths: BTreeMap<Number, RoaringBitmap>,
    pub covered_records: u32,
    pub stats: PostingStats,
}


/// Coverage and cardinality summary for one posting index.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PostingStats {
    pub present_count: u64,
    pub null_count: u64,
    pub distinct_count: usize,
    pub min_key: Option<ScalarKey>,
    pub max_key: Option<ScalarKey>,
}


impl PostingIndex {
    /// Intersect borrowed broad postings with a restricted domain before any copy.
    pub fn exact_set_restricted(
        &self,
        term: &BoundTerm,
        domain: &RoaringBitmap,
        input: &RoaringBitmap,
    ) -> Option<RoaringBitmap> {
        if self.covered_records as u64 != domain.len() {
            return None;
        }
        let borrowed = match term.derived {
            Derived::Exists | Derived::Null => {
                let Value::Bool(value) = term.literal else {
                    return None;
                };
                let truth = match term.operator {
                    Operator::Eq => value,
                    Operator::Ne => !value,
                    _ => return None,
                };
                let source = if term.derived == Derived::Exists {
                    &self.presence
                } else {
                    &self.nulls
                };
                return Some(if truth {
                    input & source
                } else {
                    input - source
                });
            }
            Derived::Length if term.operator == Operator::Eq => {
                let Value::Number(number) = term.literal else {
                    return None;
                };
                return Some(
                    self.lengths
                        .get(&number)
                        .map(|set| input & set)
                        .unwrap_or_default(),
                );
            }
            Derived::None if matches!(term.literal, Value::Null) => {
                if term.multi && term.quantifier == Quantifier::Any {
                    return None;
                }
                return match term.operator {
                    Operator::Eq => Some(input & &self.nulls),
                    Operator::Ne => Some(&(input & &self.presence) - &self.nulls),
                    _ => None,
                };
            }
            Derived::None
                if term.transform == Transform::Identity
                    && (!term.multi || term.quantifier == Quantifier::Any)
                    && term.operator == Operator::Eq =>
            {
                let key = ScalarKey::from_value(&term.literal)?;
                self.values.get(&key)
            }
            _ => None,
        };
        if let Some(set) = borrowed {
            return Some(input & set);
        }
        self.exact_set(term, domain).map(|set| &set & input)
    }

    pub fn new(field: &FieldDefinition) -> Self {
        Self {
            path: field.path.clone(),
            kind: field.kind.clone(),
            multi: field.multi,
            presence: RoaringBitmap::new(),
            nulls: RoaringBitmap::new(),
            values: BTreeMap::new(),
            lengths: BTreeMap::new(),
            covered_records: 0,
            stats: PostingStats::default(),
        }
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
        self.covered_records = self
            .covered_records
            .checked_add(1)
            .ok_or("posting coverage overflow")?;
        let Some(selected) = data_values(record, &self.path) else {
            return Ok(());
        };
        self.presence.insert(slot.0);
        if selected.len() == 1 && matches!(selected[0], Value::Null) {
            self.nulls.insert(slot.0);
            return Ok(());
        }
        let mut unique = HashSet::new();
        for value in selected {
            if let Value::Array(array) = value {
                self.lengths
                    .entry(Number::integer(array.values.len() as i128))
                    .or_default()
                    .insert(slot.0);
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
        if self.covered_records as usize != record_count {
            return Err("incomplete posting coverage".into());
        }
        let valid = |set: &RoaringBitmap| set.max().is_none_or(|s| (s as usize) < record_count);
        if !valid(&self.presence)
            || !valid(&self.nulls)
            || !self.values.values().all(valid)
            || !self.lengths.values().all(valid)
        {
            return Err("posting contains out-of-domain record slot".into());
        }
        if !self.nulls.is_subset(&self.presence) {
            return Err("null posting is not present".into());
        }
        if self.stats.present_count != self.presence.len()
            || self.stats.null_count != self.nulls.len()
            || self.stats.distinct_count != self.values.len()
        {
            return Err("posting statistics mismatch".into());
        }
        Ok(())
    }

    pub fn distinct_values(&self) -> usize {
        self.values.len()
    }

    /// Cheap cardinality/work estimate; no dictionary traversal or bitmap copy.
    pub fn estimate_exact(&self, term: &BoundTerm, domain_len: u64) -> Option<(f64, u64)> {
        if self.covered_records as u64 != domain_len {
            return None;
        }
        match term.derived {
            Derived::Exists | Derived::Null => {
                let Value::Bool(literal) = term.literal else {
                    return None;
                };
                let yes = if term.derived == Derived::Exists {
                    self.stats.present_count
                } else {
                    self.stats.null_count
                };
                let matches_true = match term.operator {
                    Operator::Eq => literal,
                    Operator::Ne => !literal,
                    _ => return None,
                };
                return Some((1.0, if matches_true { yes } else { domain_len - yes }));
            }
            Derived::Length => {
                let Value::Number(number) = term.literal else {
                    return None;
                };
                let count = if term.operator == Operator::Eq {
                    self.lengths
                        .get(&number)
                        .map(RoaringBitmap::len)
                        .unwrap_or(0)
                } else {
                    domain_len / 4
                };
                return Some((
                    1.0 + if term.operator == Operator::Eq {
                        0.0
                    } else {
                        self.lengths.len() as f64
                    },
                    count,
                ));
            }
            Derived::None => {}
        }
        if matches!(term.literal, Value::Null) {
            if term.multi && term.quantifier == Quantifier::Any {
                return None;
            }
            return match term.operator {
                Operator::Eq => Some((1.0, self.stats.null_count)),
                Operator::Ne => Some((2.0, self.stats.present_count - self.stats.null_count)),
                _ => None,
            };
        }
        if term.transform != Transform::Identity
            || (term.multi && (term.quantifier != Quantifier::Any || term.operator != Operator::Eq))
        {
            return None;
        }
        let key = ScalarKey::from_value(&term.literal)?;
        match term.operator {
            Operator::Eq => Some((
                1.0,
                self.values.get(&key).map(RoaringBitmap::len).unwrap_or(0),
            )),
            Operator::Ne if !term.multi => Some((
                2.0,
                self.stats.present_count
                    - self.stats.null_count
                    - self.values.get(&key).map(RoaringBitmap::len).unwrap_or(0),
            )),
            Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le if !term.multi => {
                let estimate = if self.stats.min_key.as_ref().is_some_and(|min| min > &key)
                    && matches!(term.operator, Operator::Lt | Operator::Le)
                {
                    0
                } else if self.stats.max_key.as_ref().is_some_and(|max| max < &key)
                    && matches!(term.operator, Operator::Gt | Operator::Ge)
                {
                    0
                } else {
                    domain_len / 4
                };
                Some((1.0 + self.stats.distinct_count as f64 / 8.0, estimate))
            }
            _ => None,
        }
    }

    /// An exact answer over this index's complete record domain, or `None`
    /// when quantification/transformation needs independent verification.
    pub fn exact_set(&self, term: &BoundTerm, domain: &RoaringBitmap) -> Option<RoaringBitmap> {
        if self.covered_records as u64 != domain.len() {
            return None;
        }
        match term.derived {
            Derived::Exists | Derived::Null => {
                let yes = if term.derived == Derived::Exists {
                    &self.presence
                } else {
                    &self.nulls
                };
                let Value::Bool(literal) = term.literal else {
                    return None;
                };
                let matches_true = match term.operator {
                    Operator::Eq => literal,
                    Operator::Ne => !literal,
                    _ => return None,
                };
                return Some(if matches_true {
                    yes.clone()
                } else {
                    domain - yes
                });
            }
            Derived::Length => {
                let Value::Number(number) = term.literal else {
                    return None;
                };
                let mut out = RoaringBitmap::new();
                for (key, posting) in &self.lengths {
                    if ordered_match(key.cmp(&number), term.operator) {
                        out |= posting;
                    }
                }
                return Some(out);
            }
            Derived::None => {}
        }
        if matches!(term.literal, Value::Null) {
            if term.multi && term.quantifier == Quantifier::Any {
                return None;
            }
            return Some(match term.operator {
                Operator::Eq => self.nulls.clone(),
                Operator::Ne => &self.presence - &self.nulls,
                _ => return None,
            });
        }
        if term.transform != Transform::Identity {
            return None;
        }
        let key = ScalarKey::from_value(&term.literal)?;
        if term.multi && term.quantifier != Quantifier::Any {
            return None;
        }
        if term.multi && term.operator != Operator::Eq {
            return None;
        }
        let equal = self.values.get(&key).cloned().unwrap_or_default();
        match term.operator {
            Operator::Eq => Some(equal),
            Operator::Ne if !term.multi => Some(&(&self.presence - &self.nulls) - &equal),
            Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le if !term.multi => {
                let mut out = RoaringBitmap::new();
                for (candidate, posting) in &self.values {
                    if std::mem::discriminant(candidate) == std::mem::discriminant(&key)
                        && ordered_match(candidate.cmp(&key), term.operator)
                    {
                        out |= posting;
                    }
                }
                Some(out)
            }
            _ => None,
        }
    }
}


fn ordered_match(order: std::cmp::Ordering, op: Operator) -> bool {
    match op {
        Operator::Eq => order.is_eq(),
        Operator::Ne => !order.is_eq(),
        Operator::Gt => order.is_gt(),
        Operator::Ge => order.is_ge(),
        Operator::Lt => order.is_lt(),
        Operator::Le => order.is_le(),
        _ => false,
    }
}


/// Posting indexes registered for one collection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PostingCollection {
    pub fields: HashMap<Vec<String>, PostingIndex>,
}


impl PostingCollection {
    pub fn new(schema: &Schema) -> Self {
        let fields = schema
            .fields
            .values()
            .filter(|f| f.indexed)
            .map(|f| (f.path.clone(), PostingIndex::new(f)))
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
        for index in self.fields.values() {
            index.validate(record_count)?;
        }
        Ok(())
    }
}


#[cfg(test)]
#[path = "posting.test.rs"]
mod tests;
