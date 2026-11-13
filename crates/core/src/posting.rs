//! Typed B-tree dictionaries with record-slot postings and special states.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Bound::{Excluded, Included, Unbounded};
use std::sync::Arc;

use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};

use crate::field_state::{StatePredicate, value_matches};
use crate::inventory::append_shifted_bitmap;
use crate::model::{LogicalType, Number, RecordSlot, Value};
use crate::posting_dictionary::SealedPostingDictionary;
use crate::query::{Operator, Quantifier};
use crate::reference::{BoundTerm, Derived, Transform};
use crate::reference::{FieldDefinition, Schema, data_values_small};
use crate::segment_format::PostingDescriptor;

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
    /// Immutable dictionary readers replace the completed builder maps.
    #[serde(skip)]
    pub(crate) sealed: Option<SealedPostingPair>,
    /// A partial dictionary cannot prove that an absent scalar value is absent.
    pub value_complete: bool,
    pub length_complete: bool,
    pub empty_string_complete: bool,
    pub covered_records: u32,
    pub stats: PostingStats,
}

#[derive(Clone)]
pub(crate) struct SealedPostingPair {
    values: Arc<SealedPostingDictionary<ScalarKey>>,
    lengths: Arc<SealedPostingDictionary<Number>>,
}

impl std::fmt::Debug for SealedPostingPair {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SealedPostingPair")
            .field("values", &self.values.len())
            .field("lengths", &self.lengths.len())
            .finish()
    }
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
    /// Install checked immutable readers and release completed B-tree posting
    /// maps. The segment reader retains the owning mapped or resident bytes.
    pub(crate) fn install_sealed(
        &mut self,
        values: SealedPostingDictionary<ScalarKey>,
        lengths: SealedPostingDictionary<Number>,
    ) -> Result<(), String> {
        if self.sealed.is_some()
            || self.stats.distinct_count != values.len()
            || (!self.values.is_empty() && self.values.len() != values.len())
            || (!self.lengths.is_empty() && self.lengths.len() != lengths.len())
            || (!self.length_complete && lengths.len() != 0)
        {
            return Err("sealed posting dictionary metadata mismatch".into());
        }
        self.sealed = Some(SealedPostingPair {
            values: Arc::new(values),
            lengths: Arc::new(lengths),
        });
        self.values.clear();
        self.lengths.clear();
        Ok(())
    }

    /// Restore mutable maps only for source-free merging or compaction.
    pub(crate) fn materialize_mutable(&mut self) -> Result<(), String> {
        if let Some(sealed) = &self.sealed {
            let values = sealed.values.materialize_all()?;
            let lengths = sealed.lengths.materialize_all()?;
            self.values = values;
            self.lengths = lengths;
            self.sealed = None;
        }
        Ok(())
    }

    pub(crate) fn sealed_values(&self) -> Option<&SealedPostingDictionary<ScalarKey>> {
        self.sealed.as_ref().map(|pair| pair.values.as_ref())
    }

    pub(crate) fn sealed_lengths(&self) -> Option<&SealedPostingDictionary<Number>> {
        self.sealed.as_ref().map(|pair| pair.lengths.as_ref())
    }

    /// Secondary APIs that need ordered traversal may borrow builder maps or
    /// materialize a temporary map from immutable bytes. The published index
    /// never retains both representations.
    pub(crate) fn values_view(
        &self,
    ) -> Result<Cow<'_, BTreeMap<ScalarKey, RoaringBitmap>>, String> {
        match self.sealed_values() {
            Some(sealed) => Ok(Cow::Owned(sealed.materialize_all()?)),
            None => Ok(Cow::Borrowed(&self.values)),
        }
    }

    pub(crate) fn value_cardinality(&self, key: &ScalarKey) -> Result<u64, String> {
        if let Some(sealed) = self.sealed_values() {
            Ok(sealed.estimated_cardinality(key)?.unwrap_or(0) as u64)
        } else {
            Ok(self.values.get(key).map_or(0, RoaringBitmap::len))
        }
    }

    pub(crate) fn value_posting(&self, key: &ScalarKey) -> Result<Option<RoaringBitmap>, String> {
        if let Some(sealed) = self.sealed_values() {
            sealed.get(key)
        } else {
            Ok(self.values.get(key).cloned())
        }
    }

    pub(crate) fn value_posting_restricted(
        &self,
        key: &ScalarKey,
        input: &RoaringBitmap,
    ) -> Result<Option<RoaringBitmap>, String> {
        if let Some(sealed) = self.sealed_values() {
            sealed.get_restricted(key, input)
        } else {
            Ok(self.values.get(key).map(|bitmap| input & bitmap))
        }
    }

    pub(crate) fn length_cardinality(&self, key: &Number) -> Result<u64, String> {
        if let Some(sealed) = self.sealed_lengths() {
            Ok(sealed.estimated_cardinality(key)?.unwrap_or(0) as u64)
        } else {
            Ok(self.lengths.get(key).map_or(0, RoaringBitmap::len))
        }
    }

    pub(crate) fn length_posting(&self, key: &Number) -> Result<Option<RoaringBitmap>, String> {
        if let Some(sealed) = self.sealed_lengths() {
            sealed.get(key)
        } else {
            Ok(self.lengths.get(key).cloned())
        }
    }

    pub(crate) fn for_each_value(
        &self,
        mut visit: impl FnMut(&ScalarKey, &RoaringBitmap) -> Result<(), String>,
    ) -> Result<(), String> {
        if let Some(sealed) = self.sealed_values() {
            sealed.visit_entries(|key, descriptor| {
                let posting = sealed.materialize_descriptor(descriptor)?;
                visit(key, &posting)?;
                Ok(true)
            })
        } else {
            for (key, posting) in &self.values {
                visit(key, posting)?;
            }
            Ok(())
        }
    }

    pub(crate) fn for_each_length(
        &self,
        mut visit: impl FnMut(&Number, &RoaringBitmap) -> Result<(), String>,
    ) -> Result<(), String> {
        if let Some(sealed) = self.sealed_lengths() {
            sealed.visit_entries(|key, descriptor| {
                let posting = sealed.materialize_descriptor(descriptor)?;
                visit(key, &posting)?;
                Ok(true)
            })
        } else {
            for (key, posting) in &self.lengths {
                visit(key, posting)?;
            }
            Ok(())
        }
    }

    pub(crate) fn union_scalar_range(
        &self,
        key: &ScalarKey,
        operator: Operator,
        input: Option<&RoaringBitmap>,
    ) -> Result<RoaringBitmap, String> {
        if let Some(sealed) = self.sealed_values() {
            let Some((lower, upper)) = ordered_range_bounds(key, operator) else {
                return Ok(RoaringBitmap::new());
            };
            let mut result = RoaringBitmap::new();
            sealed.visit_bounds(lower, upper, |candidate, descriptor| {
                if std::mem::discriminant(candidate) == std::mem::discriminant(key) {
                    result |= if let Some(input) = input {
                        sealed.intersect_descriptor(descriptor, input)?
                    } else {
                        sealed.materialize_descriptor(descriptor)?
                    };
                    Ok(true)
                } else {
                    Ok(candidate < key)
                }
            })?;
            Ok(result)
        } else {
            Ok(scalar_range_postings(&self.values, key, operator, input).0)
        }
    }

    pub(crate) fn count_scalar_range(
        &self,
        key: &ScalarKey,
        operator: Operator,
        limit: usize,
    ) -> Result<usize, String> {
        if let Some(sealed) = self.sealed_values() {
            let Some((lower, upper)) = ordered_range_bounds(key, operator) else {
                return Ok(0);
            };
            let mut count = 0;
            sealed.visit_bounds(lower, upper, |candidate, _| {
                if std::mem::discriminant(candidate) == std::mem::discriminant(key) {
                    count += 1;
                    Ok(count < limit)
                } else {
                    Ok(candidate < key)
                }
            })?;
            Ok(count)
        } else {
            Ok(scalar_range_key_probe(&self.values, key, operator, limit))
        }
    }

    fn visit_temporal_descriptors(
        &self,
        start: &ScalarKey,
        end: &ScalarKey,
        operator: Operator,
        mut visit: impl FnMut(PostingDescriptor) -> Result<bool, String>,
    ) -> Result<(), String> {
        let sealed = self
            .sealed_values()
            .ok_or("temporal sealed dictionary missing")?;
        let mut keep_going = true;
        for (lower, upper) in temporal_bounds(start, end, operator) {
            sealed.visit_bounds(lower, upper, |candidate, descriptor| {
                if std::mem::discriminant(candidate) == std::mem::discriminant(start) {
                    keep_going = visit(descriptor)?;
                    Ok(keep_going)
                } else {
                    Ok(candidate < start)
                }
            })?;
            if !keep_going {
                break;
            }
        }
        Ok(())
    }

    fn union_temporal_range(
        &self,
        start: &ScalarKey,
        end: &ScalarKey,
        operator: Operator,
        input: Option<&RoaringBitmap>,
    ) -> Result<RoaringBitmap, String> {
        let sealed = self
            .sealed_values()
            .ok_or("temporal sealed dictionary missing")?;
        let mut result = RoaringBitmap::new();
        self.visit_temporal_descriptors(start, end, operator, |descriptor| {
            result |= if let Some(input) = input {
                sealed.intersect_descriptor(descriptor, input)?
            } else {
                sealed.materialize_descriptor(descriptor)?
            };
            Ok(true)
        })?;
        Ok(result)
    }

    fn count_temporal_range(
        &self,
        start: &ScalarKey,
        end: &ScalarKey,
        operator: Operator,
        limit: usize,
    ) -> Result<usize, String> {
        let mut count = 0;
        self.visit_temporal_descriptors(start, end, operator, |_| {
            count += 1;
            Ok(count < limit)
        })?;
        Ok(count)
    }

    pub(crate) fn union_lengths_matching(
        &self,
        mut matches: impl FnMut(&Number) -> bool,
        input: Option<&RoaringBitmap>,
    ) -> Result<RoaringBitmap, String> {
        let mut result = RoaringBitmap::new();
        if let Some(sealed) = self.sealed_lengths() {
            sealed.visit_entries(|key, descriptor| {
                if matches(key) {
                    result |= if let Some(input) = input {
                        sealed.intersect_descriptor(descriptor, input)?
                    } else {
                        sealed.materialize_descriptor(descriptor)?
                    };
                }
                Ok(true)
            })?;
        } else {
            for (key, posting) in &self.lengths {
                if matches(key) {
                    result |= if let Some(input) = input {
                        input & posting
                    } else {
                        posting.clone()
                    };
                }
            }
        }
        Ok(result)
    }

    fn union_length_range(
        &self,
        key: &Number,
        operator: Operator,
    ) -> Result<RoaringBitmap, String> {
        let Some((lower, upper)) = ordered_range_bounds(key, operator) else {
            return Ok(RoaringBitmap::new());
        };
        if let Some(sealed) = self.sealed_lengths() {
            sealed.union_bounds(lower, upper, None)
        } else {
            Ok(range_postings(&self.lengths, key, operator, None).0)
        }
    }

    fn count_length_range(
        &self,
        key: &Number,
        operator: Operator,
        limit: usize,
    ) -> Result<usize, String> {
        let Some((lower, upper)) = ordered_range_bounds(key, operator) else {
            return Ok(0);
        };
        if let Some(sealed) = self.sealed_lengths() {
            let mut count = 0;
            sealed.visit_bounds(lower, upper, |_, _| {
                count += 1;
                Ok(count < limit)
            })?;
            Ok(count)
        } else {
            Ok(range_key_probe(&self.lengths, key, operator, limit))
        }
    }

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

    pub(crate) fn estimate_observed_scalar_checked(
        &self,
        key: &ScalarKey,
        operator: Operator,
        domain_len: u64,
        visits: &mut u64,
    ) -> Result<Option<(f64, u64)>, String> {
        if self.sealed.is_none() {
            return Ok(self.estimate_observed_scalar(key, operator, domain_len, visits));
        }
        if self.covered_records as u64 != domain_len {
            return Ok(None);
        }
        Ok(match operator {
            Operator::Eq => {
                *visits += 1;
                Some((1.0, self.value_cardinality(key)?))
            }
            Operator::Ne if !self.multi => {
                *visits += 1;
                Some((
                    2.0,
                    self.stats.present_count
                        - self.stats.null_count
                        - self.value_cardinality(key)?,
                ))
            }
            op if is_range(op) => {
                let count = self.count_scalar_range(key, op, 33)?;
                *visits += count as u64;
                let output = if count <= 32 {
                    count as u64 * self.stats.present_count
                        / self.stats.distinct_count.max(1) as u64
                } else {
                    domain_len / 4
                };
                let work = if count <= 32 {
                    count as f64
                } else {
                    self.stats.distinct_count as f64 / 8.0
                };
                Some((1.0 + work, output))
            }
            _ => None,
        })
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

    pub(crate) fn exact_observed_scalar_restricted_checked(
        &self,
        key: &ScalarKey,
        operator: Operator,
        domain: &RoaringBitmap,
        input: &RoaringBitmap,
    ) -> Result<Option<RoaringBitmap>, String> {
        if self.sealed.is_none() {
            return Ok(self.exact_observed_scalar_restricted(key, operator, domain, input));
        }
        if !self.value_complete || self.multi || self.covered_records as u64 != domain.len() {
            return Ok(None);
        }
        Ok(match operator {
            Operator::Eq => Some(
                self.value_posting_restricted(key, input)?
                    .unwrap_or_default(),
            ),
            Operator::Ne => {
                let mut result = input & &self.presence;
                result -= &self.nulls;
                result -= &self
                    .value_posting_restricted(key, input)?
                    .unwrap_or_default();
                Some(result)
            }
            op if is_range(op) => Some(self.union_scalar_range(key, op, Some(input))?),
            _ => None,
        })
    }

    pub(crate) fn merge_shifted(&mut self, mut batch: Self, base: u32) -> Result<(), String> {
        self.materialize_mutable()?;
        batch.materialize_mutable()?;
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

    /// Checked counterpart used by published query generations. A corrupt
    /// immutable posting is an error, never an absent or empty result.
    pub(crate) fn exact_set_restricted_checked(
        &self,
        term: &BoundTerm,
        domain: &RoaringBitmap,
        input: &RoaringBitmap,
    ) -> Result<Option<RoaringBitmap>, String> {
        if self.sealed.is_none() {
            return Ok(self.exact_set_restricted(term, domain, input));
        }
        if term.period.is_none()
            && term.derived == Derived::None
            && term.transform == Transform::Identity
            && term.operator == Operator::Eq
            && (!term.multi || term.quantifier == Quantifier::Any)
            && self.scalar_equality_complete(term)
            && self.covered_records as u64 == domain.len()
            && !matches!(term.literal, Value::Null)
            && term
                .matcher
                .as_ref()
                .is_none_or(|matcher| matcher.posting_compatible())
        {
            if let Some(key) = ScalarKey::from_value(&term.literal) {
                return Ok(Some(
                    self.value_posting_restricted(&key, input)?
                        .unwrap_or_default(),
                ));
            }
        }
        Ok(self
            .exact_set_checked(term, domain)?
            .map(|set| &set & input))
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
            sealed: None,
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
            || self.stats.distinct_count != self.distinct_values()
        {
            return Err("posting statistics mismatch".into());
        }
        if !self.length_complete
            && (!self.lengths.is_empty()
                || self
                    .sealed_lengths()
                    .is_some_and(|lengths| lengths.len() != 0))
        {
            return Err("posting capability and stored facts mismatch".into());
        }
        Ok(())
    }

    pub fn distinct_values(&self) -> usize {
        self.sealed_values()
            .map_or(self.values.len(), |values| values.len())
    }

    pub(crate) fn distinct_lengths(&self) -> usize {
        self.sealed_lengths()
            .map_or(self.lengths.len(), |lengths| lengths.len())
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

    pub(crate) fn estimate_exact_with_visits_checked(
        &self,
        term: &BoundTerm,
        domain_len: u64,
        visits: &mut u64,
    ) -> Result<Option<(f64, u64)>, String> {
        if self.sealed.is_none() {
            return Ok(self.estimate_exact_with_visits(term, domain_len, visits));
        }
        if term
            .matcher
            .as_ref()
            .is_some_and(|matcher| !matcher.posting_compatible())
            || self.covered_records as u64 != domain_len
        {
            return Ok(None);
        }
        if let Some(period) = &term.period {
            if !self.value_complete || term.multi || term.transform != Transform::Identity {
                return Ok(None);
            }
            let Some(start) = ScalarKey::from_value(&period.start) else {
                return Ok(None);
            };
            let Some(end) = ScalarKey::from_value(&period.end) else {
                return Ok(None);
            };
            if std::mem::discriminant(&start) != std::mem::discriminant(&end) || start >= end {
                return Ok(None);
            }
            let keys = self.count_temporal_range(&start, &end, term.operator, 33)?;
            *visits += keys as u64;
            let output = if keys <= 32 {
                keys as u64 * self.stats.present_count / self.stats.distinct_count.max(1) as u64
            } else {
                domain_len / 4
            };
            return Ok(Some((1.0 + keys as f64, output)));
        }
        if term.derived == Derived::Exists
            || self.state_source(term.derived).is_some()
            || (term.derived == Derived::None && matches!(term.literal, Value::Null))
        {
            return Ok(self.estimate_exact_with_visits(term, domain_len, visits));
        }
        if term.derived == Derived::Length {
            if !self.length_complete || !length_posting_is_exact(term) {
                return Ok(None);
            }
            let Value::Number(number) = &term.literal else {
                return Ok(None);
            };
            let range_keys = if is_range(term.operator) {
                Some(self.count_length_range(number, term.operator, 33)?)
            } else {
                None
            };
            *visits += range_keys.unwrap_or(1) as u64;
            let count = if term.operator == Operator::Eq {
                self.length_cardinality(number)?
            } else if let Some(keys) = range_keys {
                if keys <= 32 {
                    keys as u64 * domain_len / self.sealed_lengths().unwrap().len().max(1) as u64
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
                    self.sealed_lengths().unwrap().len() as f64
                }
            } else if term.operator == Operator::Eq {
                0.0
            } else {
                self.sealed_lengths().unwrap().len() as f64
            };
            return Ok(Some((1.0 + work, count)));
        }
        if term.derived != Derived::None
            || term.transform != Transform::Identity
            || (term.multi && (term.quantifier != Quantifier::Any || term.operator != Operator::Eq))
        {
            return Ok(None);
        }
        let Some(key) = ScalarKey::from_value(&term.literal) else {
            return Ok(None);
        };
        if term.operator == Operator::Eq && !self.scalar_equality_complete(term)
            || term.operator != Operator::Eq && !self.value_complete
        {
            return Ok(None);
        }
        Ok(match term.operator {
            Operator::Eq => {
                *visits += 1;
                Some((1.0, self.value_cardinality(&key)?))
            }
            Operator::Ne if !term.multi => {
                *visits += 1;
                Some((
                    2.0,
                    self.stats.present_count
                        - self.stats.null_count
                        - self.value_cardinality(&key)?,
                ))
            }
            op if is_range(op) && !term.multi => {
                let keys = self.count_scalar_range(&key, op, 33)?;
                *visits += keys as u64;
                let output = if keys <= 32 {
                    keys as u64 * self.stats.present_count / self.stats.distinct_count.max(1) as u64
                } else {
                    domain_len / 4
                };
                let work = if keys <= 32 {
                    keys as f64
                } else {
                    self.stats.distinct_count as f64 / 8.0
                };
                Some((1.0 + work, output))
            }
            _ => None,
        })
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

    pub(crate) fn exact_set_checked(
        &self,
        term: &BoundTerm,
        domain: &RoaringBitmap,
    ) -> Result<Option<RoaringBitmap>, String> {
        if self.sealed.is_none() {
            return Ok(self.exact_set(term, domain));
        }
        if term
            .matcher
            .as_ref()
            .is_some_and(|matcher| !matcher.posting_compatible())
            || self.covered_records as u64 != domain.len()
        {
            return Ok(None);
        }
        if let Some(period) = &term.period {
            if !self.value_complete
                || term.multi
                || term.transform != Transform::Identity
                || term.derived != Derived::None
            {
                return Ok(None);
            }
            let Some(start) = ScalarKey::from_value(&period.start) else {
                return Ok(None);
            };
            let Some(end) = ScalarKey::from_value(&period.end) else {
                return Ok(None);
            };
            if std::mem::discriminant(&start) != std::mem::discriminant(&end) || start >= end {
                return Ok(None);
            }
            return Ok(Some(self.union_temporal_range(
                &start,
                &end,
                term.operator,
                None,
            )?));
        }
        if term.derived == Derived::Exists
            || self.state_source(term.derived).is_some()
            || (term.derived == Derived::None && matches!(term.literal, Value::Null))
        {
            return Ok(self.exact_set(term, domain));
        }
        if term.derived == Derived::Length {
            if !self.length_complete || !length_posting_is_exact(term) {
                return Ok(None);
            }
            let Value::Number(number) = &term.literal else {
                return Ok(None);
            };
            return Ok(match term.operator {
                Operator::Eq => Some(self.length_posting(number)?.unwrap_or_default()),
                Operator::Ne => Some(self.union_lengths_matching(|key| key != number, None)?),
                op if is_range(op) => Some(self.union_length_range(number, op)?),
                _ => None,
            });
        }
        if term.derived != Derived::None || term.transform != Transform::Identity {
            return Ok(None);
        }
        let Some(key) = ScalarKey::from_value(&term.literal) else {
            return Ok(None);
        };
        if term.operator == Operator::Eq && !self.scalar_equality_complete(term)
            || term.operator != Operator::Eq && !self.value_complete
            || term.multi && (term.quantifier != Quantifier::Any || term.operator != Operator::Eq)
        {
            return Ok(None);
        }
        Ok(match term.operator {
            Operator::Eq => Some(self.value_posting(&key)?.unwrap_or_default()),
            Operator::Ne if !term.multi => Some(
                &(&self.presence - &self.nulls) - &self.value_posting(&key)?.unwrap_or_default(),
            ),
            op if is_range(op) && !term.multi => Some(self.union_scalar_range(&key, op, None)?),
            _ => None,
        })
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

fn ordered_range_bounds<K>(
    key: &K,
    operator: Operator,
) -> Option<(std::ops::Bound<&K>, std::ops::Bound<&K>)> {
    match operator {
        Operator::Gt => Some((Excluded(key), Unbounded)),
        Operator::Ge => Some((Included(key), Unbounded)),
        Operator::Lt => Some((Unbounded, Excluded(key))),
        Operator::Le => Some((Unbounded, Included(key))),
        _ => None,
    }
}

fn temporal_bounds<'a>(
    start: &'a ScalarKey,
    end: &'a ScalarKey,
    operator: Operator,
) -> Vec<(
    std::ops::Bound<&'a ScalarKey>,
    std::ops::Bound<&'a ScalarKey>,
)> {
    match operator {
        Operator::Eq => vec![(Included(start), Excluded(end))],
        Operator::Ne => vec![(Unbounded, Excluded(start)), (Included(end), Unbounded)],
        Operator::Lt => vec![(Unbounded, Excluded(start))],
        Operator::Le => vec![(Unbounded, Excluded(end))],
        Operator::Gt => vec![(Included(end), Unbounded)],
        Operator::Ge => vec![(Included(start), Unbounded)],
        _ => Vec::new(),
    }
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
