use std::collections::{BTreeMap, HashSet};
use std::ops::Bound::{Excluded, Included, Unbounded};

use roaring::RoaringBitmap;

use super::{PostingIndex, ScalarKey, StructuralKey};
use crate::model::{LogicalType, Number, Value};
use crate::query::{Operator, Quantifier};
use crate::query::{binding::filter::BoundTerm, binding::filter::Derived};
use crate::schema::operation::Transform;

/// Borrowed ordered traversal over one posting dictionary's values.
pub(crate) enum PostingValueCursor<'a> {
    Forward(std::collections::btree_map::Range<'a, ScalarKey, RoaringBitmap>),
    Reverse(std::iter::Rev<std::collections::btree_map::Range<'a, ScalarKey, RoaringBitmap>>),
}

impl<'a> PostingValueCursor<'a> {
    pub(crate) fn next(&mut self) -> Option<(ScalarKey, &'a RoaringBitmap)> {
        match self {
            Self::Forward(range) => range.next(),
            Self::Reverse(range) => range.next(),
        }
        .map(|(key, posting)| (key.clone(), posting))
    }
}

impl PostingIndex {
    /// Borrow the ordered value keys of this posting index. The native
    /// B-tree is the completed query representation; callers never decode it.
    pub(crate) fn value_range(
        &self,
        lower: std::ops::Bound<ScalarKey>,
        upper: std::ops::Bound<ScalarKey>,
        descending: bool,
    ) -> PostingValueCursor<'_> {
        if descending {
            PostingValueCursor::Reverse(self.values.range((lower, upper)).rev())
        } else {
            PostingValueCursor::Forward(self.values.range((lower, upper)))
        }
    }

    pub(crate) fn value_cardinality(&self, key: &ScalarKey) -> u64 {
        self.values.get(key).map_or(0, RoaringBitmap::len)
    }

    pub(crate) fn value_posting(&self, key: &ScalarKey) -> Option<&RoaringBitmap> {
        self.values.get(key)
    }

    /// A broad direct relation can match thousands of target IDs. Scan each
    /// ordered source dictionary once when that costs less than thousands of
    /// independent seeks and repeated key decoding.
    pub(crate) fn union_id_postings(&self, ids: &HashSet<String>) -> (RoaringBitmap, u64, u64) {
        let mut union = RoaringBitmap::new();
        let mut lists = 0u64;
        let mut members = 0u64;
        if ids.len().saturating_mul(8) < self.values.len() {
            for id in ids {
                if let Some(slots) = self.values.get(&ScalarKey::Id(id.clone())) {
                    lists += 1;
                    members += slots.len();
                    union |= slots;
                }
            }
        } else {
            for (key, slots) in &self.values {
                if let ScalarKey::Id(id) = key
                    && ids.contains(id)
                {
                    lists += 1;
                    members += slots.len();
                    union |= slots;
                }
            }
        }
        (union, lists, members)
    }

    pub(crate) fn value_posting_restricted(
        &self,
        key: &ScalarKey,
        input: &RoaringBitmap,
    ) -> Option<RoaringBitmap> {
        self.values.get(key).map(|bitmap| input & bitmap)
    }

    pub(crate) fn for_each_value(
        &self,
        mut visit: impl FnMut(&ScalarKey, &RoaringBitmap) -> Result<(), String>,
    ) -> Result<(), String> {
        for (key, posting) in &self.values {
            visit(key, posting)?;
        }
        Ok(())
    }

    pub(crate) fn for_each_length(
        &self,
        mut visit: impl FnMut(&Number, &RoaringBitmap) -> Result<(), String>,
    ) -> Result<(), String> {
        for (key, posting) in &self.lengths {
            visit(key, posting)?;
        }
        Ok(())
    }

    pub(crate) fn union_scalar_range(
        &self,
        key: &ScalarKey,
        operator: Operator,
        input: Option<&RoaringBitmap>,
    ) -> RoaringBitmap {
        scalar_range_postings(&self.values, key, operator, input).0
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
    fn scalar_equality_complete(&self, term: &BoundTerm) -> bool {
        self.value_complete
            || (self.empty_string_complete
                && !term.multi
                && !term.query_path.has_explicit_array_step
                && self.kind == LogicalType::String
                && matches!(&term.literal, Value::String(value) if value.is_empty()))
    }
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

#[cfg(test)]
#[path = "lookup.test.rs"]
mod tests;
