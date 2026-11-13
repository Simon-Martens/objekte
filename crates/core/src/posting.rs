//! Typed B-tree dictionaries with record-slot postings and special states.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Bound::{Excluded, Included, Unbounded};

use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};

use crate::field_state::{StatePredicate, value_matches};
use crate::inventory::append_shifted_bitmap;
use crate::model::{LogicalType, Number, RecordSlot, Value};
use crate::query::{Operator, Quantifier};
use crate::reference::{BoundTerm, Derived, Transform};
use crate::reference::{FieldDefinition, Schema, data_values_small};

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
#[derive(Clone, Debug, Serialize, Deserialize)]
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
    /// A partial dictionary cannot prove that an absent scalar value is absent.
    pub value_complete: bool,
    pub length_complete: bool,
    pub empty_string_complete: bool,
    pub covered_records: u32,
    pub stats: PostingStats,
}

/// Sparse facts inside the owning field posting, never synthetic field paths.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum StructuralKey {
    AnyNull,
    NonEmpty,
    String,
    Number,
    Array,
    Object,
    EmptyObject,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CustomPostingIndex {
    pub index: PostingIndex,
    pub complete: bool,
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
    fn state_source(&self, derived: Derived) -> Option<Option<&RoaringBitmap>> {
        let key = match derived {
            Derived::Exists => return None,
            Derived::Null if !self.multi => return Some(Some(&self.nulls)),
            Derived::Null => StructuralKey::AnyNull,
            Derived::IsNonEmpty => StructuralKey::NonEmpty,
            Derived::IsString => StructuralKey::String,
            Derived::IsNumber => StructuralKey::Number,
            Derived::IsArray => StructuralKey::Array,
            Derived::IsObject => StructuralKey::Object,
            Derived::None | Derived::Length => return None,
        };
        Some(self.structural.as_ref()?.postings.get(&key))
    }

    fn state_truth(term: &BoundTerm) -> Option<bool> {
        let Value::Bool(literal) = term.literal else {
            return None;
        };
        match term.operator {
            Operator::Eq => Some(literal),
            Operator::Ne => Some(!literal),
            _ => None,
        }
    }

    /// Inferred dictionary costing without materializing or unioning postings.
    pub(crate) fn estimate_observed_scalar(
        &self,
        key: &ScalarKey,
        operator: Operator,
        domain_len: u64,
        visits: &mut u64,
    ) -> Option<(f64, u64)> {
        if self.covered_records as u64 != domain_len {
            return None;
        }
        let equal = || self.values.get(key).map(RoaringBitmap::len).unwrap_or(0);
        match operator {
            Operator::Eq => {
                *visits += 1;
                Some((1.0, equal()))
            }
            Operator::Ne if !self.multi => {
                *visits += 1;
                Some((
                    2.0,
                    self.stats.present_count - self.stats.null_count - equal(),
                ))
            }
            Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le => {
                let keys = scalar_range_key_probe(&self.values, key, operator, 33);
                *visits += keys as u64;
                let count = if keys <= 32 {
                    keys as u64 * self.stats.present_count / self.stats.distinct_count.max(1) as u64
                } else {
                    domain_len / 4
                };
                let work = if keys <= 32 {
                    keys as f64
                } else {
                    self.stats.distinct_count as f64 / 8.0
                };
                Some((1.0 + work, count))
            }
            _ => None,
        }
    }

    /// An observed scalar field can prove a dynamic comparison even when the
    /// schema-free binder conservatively marks its term as multivalued.
    pub(crate) fn exact_observed_scalar_restricted(
        &self,
        key: &ScalarKey,
        operator: Operator,
        domain: &RoaringBitmap,
        input: &RoaringBitmap,
    ) -> Option<RoaringBitmap> {
        if !self.value_complete || self.multi || self.covered_records as u64 != domain.len() {
            return None;
        }
        match operator {
            Operator::Eq => Some(
                self.values
                    .get(key)
                    .map(|slots| input & slots)
                    .unwrap_or_default(),
            ),
            Operator::Ne => {
                let mut result = input & &self.presence;
                result -= &self.nulls;
                if let Some(equal) = self.values.get(key) {
                    result -= equal;
                }
                Some(result)
            }
            Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le => {
                Some(scalar_range_postings(&self.values, key, operator, Some(input)).0)
            }
            _ => None,
        }
    }

    pub(crate) fn merge_shifted(&mut self, batch: Self, base: u32) -> Result<(), String> {
        if self.path != batch.path
            || self.kind != batch.kind
            || self.multi != batch.multi
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

    /// Intersect borrowed broad postings with a restricted domain before any copy.
    pub fn exact_set_restricted(
        &self,
        term: &BoundTerm,
        domain: &RoaringBitmap,
        input: &RoaringBitmap,
    ) -> Option<RoaringBitmap> {
        if term
            .matcher
            .as_ref()
            .is_some_and(|matcher| !matcher.posting_compatible())
        {
            return None;
        }
        if self.covered_records as u64 != domain.len() {
            return None;
        }
        if term.period.is_some() {
            if !self.value_complete {
                return None;
            }
            return self.temporal_postings(term, Some(input));
        }
        if term.derived == Derived::Exists {
            let truth = Self::state_truth(term)?;
            if self.empty_selections.is_empty() {
                return Some(if truth {
                    input & &self.presence
                } else {
                    input - &self.presence
                });
            }
            let mut present = input & &self.presence;
            present -= &self.empty_selections;
            return Some(if truth { present } else { input - &present });
        }
        if let Some(source) = self.state_source(term.derived) {
            return Some(match (Self::state_truth(term)?, source) {
                (true, Some(slots)) => input & slots,
                (false, Some(slots)) => input - slots,
                (true, None) => RoaringBitmap::new(),
                (false, None) => input.clone(),
            });
        }
        let borrowed = match term.derived {
            Derived::Exists | Derived::Null => return None,
            Derived::Length if !self.length_complete || !length_posting_is_exact(term) => {
                return None;
            }
            Derived::Length if term.operator == Operator::Eq => {
                let Value::Number(number) = &term.literal else {
                    return None;
                };
                return Some(
                    self.lengths
                        .get(number)
                        .map(|set| input & set)
                        .unwrap_or_default(),
                );
            }
            Derived::Length if is_range(term.operator) => {
                let Value::Number(number) = &term.literal else {
                    return None;
                };
                let restriction = (input.len() < domain.len()).then_some(input);
                return Some(range_postings(&self.lengths, number, term.operator, restriction).0);
            }
            Derived::Length if term.operator == Operator::Ne => {
                let Value::Number(number) = &term.literal else {
                    return None;
                };
                return Some(
                    union_postings(
                        self.lengths.iter().filter(|(key, _)| *key != number),
                        Some(input),
                    )
                    .0,
                );
            }
            Derived::None if matches!(term.literal, Value::Null) => {
                // The null posting tracks only an explicit null field. It cannot
                // prove any/all predicates over an array's elements.
                if term.multi {
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
                    && term.operator == Operator::Eq
                    && self.scalar_equality_complete(term) =>
            {
                let key = ScalarKey::from_value(&term.literal)?;
                self.values.get(&key)
            }
            Derived::None
                if term.transform == Transform::Identity
                    && !term.multi
                    && self.value_complete
                    && is_range(term.operator) =>
            {
                let key = ScalarKey::from_value(&term.literal)?;
                let restriction = (input.len() < domain.len()).then_some(input);
                return Some(
                    scalar_range_postings(&self.values, &key, term.operator, restriction).0,
                );
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
            empty_selections: RoaringBitmap::new(),
            nulls: RoaringBitmap::new(),
            structural: None,
            values: BTreeMap::new(),
            lengths: BTreeMap::new(),
            value_complete: true,
            length_complete: matches!(field.kind, LogicalType::Array(_)),
            empty_string_complete: field.kind == LogicalType::String && !field.multi,
            covered_records: 0,
            stats: PostingStats::default(),
        }
    }

    fn structural(field: &FieldDefinition) -> Self {
        let mut index = Self::new(field);
        index.value_complete = field.indexed;
        index.structural = field.indexed.then(StructuralFacts::default);
        index
    }

    fn scalar_equality_complete(&self, term: &BoundTerm) -> bool {
        self.value_complete
            || (self.empty_string_complete
                && !term.multi
                && !term.query_path.has_explicit_array_step
                && self.kind == LogicalType::String
                && matches!(&term.literal, Value::String(value) if value.is_empty()))
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
            || self.stats.distinct_count != self.values.len()
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

    /// Cheap cardinality/work estimate; no dictionary traversal or bitmap copy.
    pub fn estimate_exact(&self, term: &BoundTerm, domain_len: u64) -> Option<(f64, u64)> {
        self.estimate_exact_with_visits(term, domain_len, &mut 0)
    }

    pub(crate) fn estimate_exact_with_visits(
        &self,
        term: &BoundTerm,
        domain_len: u64,
        visits: &mut u64,
    ) -> Option<(f64, u64)> {
        if term
            .matcher
            .as_ref()
            .is_some_and(|matcher| !matcher.posting_compatible())
        {
            return None;
        }
        if self.covered_records as u64 != domain_len {
            return None;
        }
        if term.period.is_some() {
            if !self.value_complete {
                return None;
            }
            if term.multi || term.transform != Transform::Identity {
                return None;
            }
            let keys = self.temporal_key_probe(term, 33)?;
            *visits += keys as u64;
            let count = if keys <= 32 {
                keys as u64 * self.stats.present_count / self.stats.distinct_count.max(1) as u64
            } else {
                domain_len / 4
            };
            return Some((1.0 + keys as f64, count));
        }
        if term.derived == Derived::Exists {
            *visits += 1;
            let yes = self
                .presence
                .len()
                .saturating_sub(self.empty_selections.len());
            return Some((
                1.0,
                if Self::state_truth(term)? {
                    yes
                } else {
                    domain_len - yes
                },
            ));
        }
        if let Some(source) = self.state_source(term.derived) {
            *visits += 1;
            let yes = source.map_or(0, RoaringBitmap::len);
            return Some((
                1.0,
                if Self::state_truth(term)? {
                    yes
                } else {
                    domain_len - yes
                },
            ));
        }
        match term.derived {
            Derived::Exists | Derived::Null => return None,
            Derived::Length => {
                if !self.length_complete || !length_posting_is_exact(term) {
                    return None;
                }
                let Value::Number(number) = &term.literal else {
                    return None;
                };
                let range_keys = is_range(term.operator)
                    .then(|| range_key_probe(&self.lengths, number, term.operator, 33));
                *visits += range_keys.unwrap_or(1) as u64;
                let count = if term.operator == Operator::Eq {
                    self.lengths
                        .get(number)
                        .map(RoaringBitmap::len)
                        .unwrap_or(0)
                } else if let Some(keys) = range_keys {
                    if keys <= 32 {
                        keys as u64 * domain_len / self.lengths.len().max(1) as u64
                    } else {
                        domain_len / 4
                    }
                } else {
                    domain_len / 4
                };
                let work = if let Some(keys) = range_keys {
                    if keys <= 32 {
                        keys as f64
                    } else {
                        self.lengths.len() as f64
                    }
                } else if term.operator == Operator::Eq {
                    0.0
                } else {
                    self.lengths.len() as f64
                };
                return Some((1.0 + work, count));
            }
            Derived::None => {}
            Derived::IsNonEmpty
            | Derived::IsString
            | Derived::IsNumber
            | Derived::IsArray
            | Derived::IsObject => return None,
        }
        if matches!(term.literal, Value::Null) {
            if term.multi {
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
        if term.operator == Operator::Eq && !self.scalar_equality_complete(term) {
            return None;
        }
        if term.operator != Operator::Eq && !self.value_complete {
            return None;
        }
        match term.operator {
            Operator::Eq => {
                *visits += 1;
                Some((
                    1.0,
                    self.values.get(&key).map(RoaringBitmap::len).unwrap_or(0),
                ))
            }
            Operator::Ne if !term.multi => {
                *visits += 1;
                Some((
                    2.0,
                    self.stats.present_count
                        - self.stats.null_count
                        - self.values.get(&key).map(RoaringBitmap::len).unwrap_or(0),
                ))
            }
            Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le if !term.multi => {
                let keys = scalar_range_key_probe(&self.values, &key, term.operator, 33);
                *visits += keys as u64;
                let estimate = if keys <= 32 {
                    keys as u64 * self.stats.present_count / self.stats.distinct_count.max(1) as u64
                } else {
                    domain_len / 4
                };
                let work = if keys <= 32 {
                    keys as f64
                } else {
                    self.stats.distinct_count as f64 / 8.0
                };
                Some((1.0 + work, estimate))
            }
            _ => None,
        }
    }

    /// An exact answer over this index's complete record domain, or `None`
    /// when quantification/transformation needs independent verification.
    pub fn exact_set(&self, term: &BoundTerm, domain: &RoaringBitmap) -> Option<RoaringBitmap> {
        if term
            .matcher
            .as_ref()
            .is_some_and(|matcher| !matcher.posting_compatible())
        {
            return None;
        }
        if self.covered_records as u64 != domain.len() {
            return None;
        }
        if term.period.is_some() {
            if !self.value_complete {
                return None;
            }
            return self.temporal_postings(term, None);
        }
        if term.derived == Derived::Exists {
            let truth = Self::state_truth(term)?;
            if self.empty_selections.is_empty() {
                return Some(if truth {
                    self.presence.clone()
                } else {
                    domain - &self.presence
                });
            }
            let mut present = self.presence.clone();
            present -= &self.empty_selections;
            return Some(if truth { present } else { domain - &present });
        }
        if let Some(source) = self.state_source(term.derived) {
            return Some(match (Self::state_truth(term)?, source) {
                (true, Some(slots)) => slots.clone(),
                (false, Some(slots)) => domain - slots,
                (true, None) => RoaringBitmap::new(),
                (false, None) => domain.clone(),
            });
        }
        match term.derived {
            Derived::Exists | Derived::Null => return None,
            Derived::Length => {
                if !self.length_complete || !length_posting_is_exact(term) {
                    return None;
                }
                let Value::Number(number) = &term.literal else {
                    return None;
                };
                return match term.operator {
                    Operator::Eq => Some(self.lengths.get(number).cloned().unwrap_or_default()),
                    Operator::Ne => Some(
                        union_postings(self.lengths.iter().filter(|(key, _)| *key != number), None)
                            .0,
                    ),
                    op if is_range(op) => Some(range_postings(&self.lengths, &number, op, None).0),
                    _ => None,
                };
            }
            Derived::None => {}
            Derived::IsNonEmpty
            | Derived::IsString
            | Derived::IsNumber
            | Derived::IsArray
            | Derived::IsObject => return None,
        }
        if matches!(term.literal, Value::Null) {
            if term.multi {
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
        if term.operator == Operator::Eq && !self.scalar_equality_complete(term) {
            return None;
        }
        if term.operator != Operator::Eq && !self.value_complete {
            return None;
        }
        if term.multi && term.quantifier != Quantifier::Any {
            return None;
        }
        if term.multi && term.operator != Operator::Eq {
            return None;
        }
        match term.operator {
            Operator::Eq => Some(self.values.get(&key).cloned().unwrap_or_default()),
            Operator::Ne if !term.multi => Some(
                &(&self.presence - &self.nulls)
                    - &self.values.get(&key).cloned().unwrap_or_default(),
            ),
            Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le if !term.multi => {
                Some(scalar_range_postings(&self.values, &key, term.operator, None).0)
            }
            _ => None,
        }
    }

    fn temporal_postings(
        &self,
        term: &BoundTerm,
        input: Option<&RoaringBitmap>,
    ) -> Option<RoaringBitmap> {
        if term.multi || term.transform != Transform::Identity || term.derived != Derived::None {
            return None;
        }
        let period = term.period.as_ref()?;
        let start = ScalarKey::from_value(&period.start)?;
        let end = ScalarKey::from_value(&period.end)?;
        let type_tag = std::mem::discriminant(&start);
        if std::mem::discriminant(&end) != type_tag || start >= end {
            return None;
        }
        let same_type = |key: &ScalarKey| std::mem::discriminant(key) == type_tag;
        let values = &self.values;
        let lower = |bound: &ScalarKey| {
            values
                .range(..bound.clone())
                .rev()
                .take_while(|(key, _)| same_type(key))
        };
        let upper = |bound: &ScalarKey| {
            values
                .range(bound.clone()..)
                .take_while(|(key, _)| same_type(key))
        };
        Some(match term.operator {
            Operator::Eq => union_postings(values.range(start..end), input).0,
            Operator::Ne => {
                let mut result = union_postings(lower(&start), input).0;
                result |= union_postings(upper(&end), input).0;
                result
            }
            Operator::Lt => union_postings(lower(&start), input).0,
            Operator::Le => union_postings(lower(&end), input).0,
            Operator::Gt => union_postings(upper(&end), input).0,
            Operator::Ge => union_postings(upper(&start), input).0,
            _ => return None,
        })
    }

    fn temporal_key_probe(&self, term: &BoundTerm, limit: usize) -> Option<usize> {
        let period = term.period.as_ref()?;
        let start = ScalarKey::from_value(&period.start)?;
        let end = ScalarKey::from_value(&period.end)?;
        let type_tag = std::mem::discriminant(&start);
        if std::mem::discriminant(&end) != type_tag || start >= end {
            return None;
        }
        let same_type = |key: &ScalarKey| std::mem::discriminant(key) == type_tag;
        let values = &self.values;
        Some(match term.operator {
            Operator::Eq => values.range(start..end).take(limit).count(),
            Operator::Ne => {
                let below = values
                    .range(..start)
                    .rev()
                    .take_while(|(key, _)| same_type(key))
                    .take(limit)
                    .count();
                below
                    + values
                        .range(end..)
                        .take_while(|(key, _)| same_type(key))
                        .take(limit.saturating_sub(below))
                        .count()
            }
            Operator::Lt => values
                .range(..start)
                .rev()
                .take_while(|(key, _)| same_type(key))
                .take(limit)
                .count(),
            Operator::Le => values
                .range(..end)
                .rev()
                .take_while(|(key, _)| same_type(key))
                .take(limit)
                .count(),
            Operator::Gt => values
                .range(end..)
                .take_while(|(key, _)| same_type(key))
                .take(limit)
                .count(),
            Operator::Ge => values
                .range(start..)
                .take_while(|(key, _)| same_type(key))
                .take(limit)
                .count(),
            _ => return None,
        })
    }
}

/// A length posting is a union of records containing at least one reached
/// array of that length. It proves existential comparisons. A one-segment
/// field path selects at most one array from a record root, so its default
/// universal comparison is also exact. Nested universal comparisons require
/// a scanner because the posting does not retain every reached length.
fn length_posting_is_exact(term: &BoundTerm) -> bool {
    !term.query_path.has_explicit_array_step
        && (term.quantifier == Quantifier::Any || term.path.segments.len() == 1)
}

fn is_range(operator: Operator) -> bool {
    matches!(
        operator,
        Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le
    )
}

fn union_postings<'a, K: 'a>(
    entries: impl Iterator<Item = (&'a K, &'a RoaringBitmap)>,
    restriction: Option<&RoaringBitmap>,
) -> (RoaringBitmap, usize) {
    let mut result = RoaringBitmap::new();
    let mut visited = 0;
    for (_, posting) in entries {
        visited += 1;
        if let Some(input) = restriction {
            result |= input & posting;
        } else {
            result |= posting;
        }
    }
    (result, visited)
}

fn range_postings<K: Ord>(
    entries: &BTreeMap<K, RoaringBitmap>,
    key: &K,
    operator: Operator,
    restriction: Option<&RoaringBitmap>,
) -> (RoaringBitmap, usize) {
    match operator {
        Operator::Gt => union_postings(entries.range((Excluded(key), Unbounded)), restriction),
        Operator::Ge => union_postings(entries.range((Included(key), Unbounded)), restriction),
        Operator::Lt => union_postings(entries.range((Unbounded, Excluded(key))), restriction),
        Operator::Le => union_postings(entries.range((Unbounded, Included(key))), restriction),
        _ => (RoaringBitmap::new(), 0),
    }
}

fn range_key_probe<K: Ord>(
    entries: &BTreeMap<K, RoaringBitmap>,
    key: &K,
    operator: Operator,
    limit: usize,
) -> usize {
    match operator {
        Operator::Gt => entries
            .range((Excluded(key), Unbounded))
            .take(limit)
            .count(),
        Operator::Ge => entries
            .range((Included(key), Unbounded))
            .take(limit)
            .count(),
        Operator::Lt => entries
            .range((Unbounded, Excluded(key)))
            .take(limit)
            .count(),
        Operator::Le => entries
            .range((Unbounded, Included(key)))
            .take(limit)
            .count(),
        _ => 0,
    }
}

fn scalar_range_key_probe(
    entries: &BTreeMap<ScalarKey, RoaringBitmap>,
    key: &ScalarKey,
    operator: Operator,
    limit: usize,
) -> usize {
    let same_type =
        |candidate: &ScalarKey| std::mem::discriminant(candidate) == std::mem::discriminant(key);
    match operator {
        Operator::Gt => entries
            .range((Excluded(key), Unbounded))
            .take_while(|(candidate, _)| same_type(candidate))
            .take(limit)
            .count(),
        Operator::Ge => entries
            .range((Included(key), Unbounded))
            .take_while(|(candidate, _)| same_type(candidate))
            .take(limit)
            .count(),
        Operator::Lt => entries
            .range((Unbounded, Excluded(key)))
            .rev()
            .take_while(|(candidate, _)| same_type(candidate))
            .take(limit)
            .count(),
        Operator::Le => entries
            .range((Unbounded, Included(key)))
            .rev()
            .take_while(|(candidate, _)| same_type(candidate))
            .take(limit)
            .count(),
        _ => 0,
    }
}

fn scalar_range_postings(
    entries: &BTreeMap<ScalarKey, RoaringBitmap>,
    key: &ScalarKey,
    operator: Operator,
    restriction: Option<&RoaringBitmap>,
) -> (RoaringBitmap, usize) {
    let same_type =
        |candidate: &ScalarKey| std::mem::discriminant(candidate) == std::mem::discriminant(key);
    match operator {
        Operator::Gt => union_postings(
            entries
                .range((Excluded(key), Unbounded))
                .take_while(|(candidate, _)| same_type(candidate)),
            restriction,
        ),
        Operator::Ge => union_postings(
            entries
                .range((Included(key), Unbounded))
                .take_while(|(candidate, _)| same_type(candidate)),
            restriction,
        ),
        Operator::Lt => union_postings(
            entries
                .range((Unbounded, Excluded(key)))
                .rev()
                .take_while(|(candidate, _)| same_type(candidate)),
            restriction,
        ),
        Operator::Le => union_postings(
            entries
                .range((Unbounded, Included(key)))
                .rev()
                .take_while(|(candidate, _)| same_type(candidate)),
            restriction,
        ),
        _ => (RoaringBitmap::new(), 0),
    }
}

/// Posting indexes registered for one collection.
#[derive(Clone, Debug, Serialize, Deserialize)]
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
            .filter(|f| {
                f.indexed
                    || (!schema.is_auto_index_prohibited(&f.path)
                        && (matches!(f.kind, LogicalType::Array(_))
                            || (f.kind == LogicalType::String && !f.multi)))
            })
            .map(|f| (f.path.clone(), PostingIndex::structural(f)))
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

#[cfg(test)]
#[path = "posting.test.rs"]
mod tests;
