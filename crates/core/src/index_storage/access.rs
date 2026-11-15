//! Borrowed typed-posting and bigram access over native or packed storage.
//!
//! Query planning uses one interface for a `PostingIndex` and a
//! `FieldPostingsView`. Operations return query-owned bitmaps; the access
//! itself never reconstructs a dictionary or caches decoded postings.

use roaring::RoaringBitmap;

use crate::bigram::{BigramIndex, BigramRequest};
use crate::model::{LogicalType, Number};
use crate::posting::{PostingIndex, ScalarKey, StructuralKey};
use crate::query::Operator;
use crate::reference::{BoundTerm, Schema};

use super::bigram::BigramSectionView;
use super::bigram_builder::{BigramBinding, BigramBindingKind};
use super::error::StorageResult;
use super::posting::FieldPostingsView;
use super::scalar::encode_scalar_key;
use super::segment::FrozenSegment;


/// One borrowed typed posting, native or packed.
#[derive(Clone, Copy)]
pub(crate) enum PostingAccess<'a> {
    Native(&'a PostingIndex),
    Packed(&'a FieldPostingsView),
}


impl std::fmt::Debug for PostingAccess<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PostingAccess")
            .field("kind", self.kind())
            .field("path", &self.path())
            .field("packed", &matches!(self, Self::Packed(_)))
            .finish()
    }
}


#[allow(dead_code)]
impl<'a> PostingAccess<'a> {
    pub(crate) fn path(&self) -> &[String] {
        match self {
            Self::Native(index) => &index.path,
            Self::Packed(index) => index.path(),
        }
    }

    pub(crate) fn kind(&self) -> &LogicalType {
        match self {
            Self::Native(index) => &index.kind,
            Self::Packed(index) => index.kind(),
        }
    }

    pub(crate) fn multi(&self) -> bool {
        match self {
            Self::Native(index) => index.multi,
            Self::Packed(index) => index.multi(),
        }
    }

    pub(crate) fn covered_records(&self) -> u32 {
        match self {
            Self::Native(index) => index.covered_records,
            Self::Packed(index) => index.covered_records(),
        }
    }

    pub(crate) fn values_indexed(&self) -> bool {
        match self {
            Self::Native(index) => index.values_indexed,
            Self::Packed(index) => index.values_indexed(),
        }
    }

    pub(crate) fn value_complete(&self) -> bool {
        match self {
            Self::Native(index) => index.value_complete,
            Self::Packed(index) => index.value_complete(),
        }
    }

    pub(crate) fn length_complete(&self) -> bool {
        match self {
            Self::Native(index) => index.length_complete,
            Self::Packed(index) => index.length_complete(),
        }
    }

    pub(crate) fn empty_string_complete(&self) -> bool {
        match self {
            Self::Native(index) => index.empty_string_complete,
            Self::Packed(index) => index.empty_string_complete(),
        }
    }

    pub(crate) fn presence(&self) -> StorageResult<RoaringBitmap> {
        match self {
            Self::Native(index) => Ok(index.presence.clone()),
            Self::Packed(index) => collect(index.presence()?),
        }
    }

    pub(crate) fn nulls(&self) -> StorageResult<RoaringBitmap> {
        match self {
            Self::Native(index) => Ok(index.nulls.clone()),
            Self::Packed(index) => collect(index.nulls()?),
        }
    }

    pub(crate) fn empty_selections(&self) -> StorageResult<RoaringBitmap> {
        match self {
            Self::Native(index) => Ok(index.empty_selections.clone()),
            Self::Packed(index) => collect(index.empty_selections()?),
        }
    }

    pub(crate) fn empty_string(&self) -> StorageResult<RoaringBitmap> {
        match self {
            Self::Native(index) => Ok(index
                .value_posting(&ScalarKey::String(String::new()))
                .cloned()
                .unwrap_or_default()),
            Self::Packed(index) => collect(index.empty_string()?),
        }
    }

    pub(crate) fn structural(&self, key: StructuralKey) -> StorageResult<RoaringBitmap> {
        match self {
            Self::Native(index) => Ok(index
                .structural
                .as_ref()
                .and_then(|facts| facts.postings.get(&key))
                .cloned()
                .unwrap_or_default()),
            Self::Packed(index) => Ok(index
                .structural(structural_code(key))?
                .map(collect)
                .transpose()?
                .unwrap_or_default()),
        }
    }

    pub(crate) fn distinct_values(&self) -> usize {
        match self {
            Self::Native(index) => index.distinct_values(),
            Self::Packed(index) => index.values().entry_count() as usize,
        }
    }

    /// Owned value page for posting-value enumeration. Native bitmaps are
    /// cloned; packed postings are decoded only for returned keys.
    pub(crate) fn value_range_owned(
        &self,
        lower: std::ops::Bound<ScalarKey>,
        upper: std::ops::Bound<ScalarKey>,
        descending: bool,
    ) -> StorageResult<Vec<(ScalarKey, RoaringBitmap)>> {
        let encode =
            |bound: std::ops::Bound<ScalarKey>| -> StorageResult<std::ops::Bound<Vec<u8>>> {
                Ok(match bound {
                    std::ops::Bound::Included(key) => {
                        std::ops::Bound::Included(encode_scalar_key(&key)?)
                    }
                    std::ops::Bound::Excluded(key) => {
                        std::ops::Bound::Excluded(encode_scalar_key(&key)?)
                    }
                    std::ops::Bound::Unbounded => std::ops::Bound::Unbounded,
                })
            };
        match self {
            Self::Native(index) => {
                let mut cursor = index.value_range(lower, upper, descending);
                let mut result = Vec::new();
                while let Some((key, bitmap)) = cursor.next() {
                    result.push((key, bitmap.clone()));
                }
                Ok(result)
            }
            Self::Packed(index) => {
                let lower = encode(lower)?;
                let upper = encode(upper)?;
                let entries = if descending {
                    index
                        .values()
                        .range_reverse(owned_bound_ref(&lower), owned_bound_ref(&upper))
                        .collect::<StorageResult<Vec<_>>>()?
                } else {
                    index
                        .values()
                        .range(owned_bound_ref(&lower), owned_bound_ref(&upper))
                        .collect::<StorageResult<Vec<_>>>()?
                };
                let mut result = Vec::with_capacity(entries.len());
                for (key, ref_bytes) in entries {
                    let key = super::scalar::decode_scalar_key(&key)?;
                    let reference = super::record_set::RecordSetRef::decode(&ref_bytes)?;
                    result.push((key, collect(index.view(reference)?)?));
                }
                Ok(result)
            }
        }
    }

    pub(crate) fn value_cardinality(&self, key: &ScalarKey) -> StorageResult<u64> {
        match self {
            Self::Native(index) => Ok(index.value_cardinality(key)),
            Self::Packed(index) => Ok(index.value(key)?.map_or(0, |view| view.cardinality())),
        }
    }

    /// Exact typed equality, without an input restriction.
    pub(crate) fn exact(&self, key: &ScalarKey) -> StorageResult<RoaringBitmap> {
        match self {
            Self::Native(index) => Ok(index.value_posting(key).cloned().unwrap_or_default()),
            Self::Packed(index) => index.exact(key),
        }
    }

    /// All presence except nulls and the equal value.
    pub(crate) fn not_equal(&self, key: &ScalarKey) -> StorageResult<RoaringBitmap> {
        match self {
            Self::Native(index) => {
                let mut result = index.presence.clone();
                result -= &index.nulls;
                if let Some(equal) = index.value_posting(key) {
                    result -= equal;
                }
                Ok(result)
            }
            Self::Packed(index) => index.not_equal(key),
        }
    }

    /// Scalar comparison with the engine's same-logical-type boundary.
    pub(crate) fn range(
        &self,
        key: &ScalarKey,
        operator: Operator,
    ) -> StorageResult<RoaringBitmap> {
        match self {
            Self::Native(index) => match operator {
                Operator::Eq => Ok(index.value_posting(key).cloned().unwrap_or_default()),
                Operator::Ne => {
                    let mut result = index.presence.clone();
                    result -= &index.nulls;
                    if let Some(equal) = index.value_posting(key) {
                        result -= equal;
                    }
                    Ok(result)
                }
                Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le => {
                    Ok(index.union_scalar_range(key, operator, None))
                }
                _ => Ok(RoaringBitmap::new()),
            },
            Self::Packed(index) => index.range(key, operator),
        }
    }

    pub(crate) fn length(&self, length: &Number) -> StorageResult<RoaringBitmap> {
        match self {
            Self::Native(index) => Ok(index.lengths.get(length).cloned().unwrap_or_default()),
            Self::Packed(index) => Ok(index
                .length(length)?
                .map(collect)
                .transpose()?
                .unwrap_or_default()),
        }
    }

    fn estimate_length(&self, term: &BoundTerm, domain_len: u64) -> StorageResult<Option<u64>> {
        if !self.length_complete() || !crate::posting::length_posting_is_exact(term) {
            return Ok(None);
        }
        let crate::model::Value::Number(number) = &term.literal else {
            return Ok(None);
        };
        let mut total = 0u64;
        let mut keys = 0u64;
        self.for_each_length(|length, bitmap| {
            keys = keys.saturating_add(1);
            let matches = match term.operator {
                Operator::Eq => length == number,
                Operator::Ne => length != number,
                Operator::Gt => length > number,
                Operator::Ge => length >= number,
                Operator::Lt => length < number,
                Operator::Le => length <= number,
                _ => return Ok(()),
            };
            if matches {
                total = total.saturating_add(bitmap.len());
            }
            Ok(())
        })?;
        let _ = domain_len;
        Ok(Some(total.min(domain_len)))
    }

    fn exact_length(
        &self,
        term: &BoundTerm,
        input: &RoaringBitmap,
    ) -> StorageResult<Option<RoaringBitmap>> {
        if !self.length_complete() || !crate::posting::length_posting_is_exact(term) {
            return Ok(None);
        }
        let crate::model::Value::Number(number) = &term.literal else {
            return Ok(None);
        };
        let mut result = RoaringBitmap::new();
        self.for_each_length(|length, bitmap| {
            let matches = match term.operator {
                Operator::Eq => length == number,
                Operator::Ne => length != number,
                Operator::Gt => length > number,
                Operator::Ge => length >= number,
                Operator::Lt => length < number,
                Operator::Le => length <= number,
                _ => false,
            };
            if matches {
                result |= bitmap & input;
            }
            Ok(())
        })?;
        Ok(Some(result))
    }

    fn estimate_derived(&self, term: &BoundTerm, domain_len: u64) -> StorageResult<Option<u64>> {
        if term.derived == crate::reference::Derived::None {
            return Ok(None);
        }
        let Some(truth) = state_truth(term) else {
            return Ok(None);
        };
        let output = if term.derived == crate::reference::Derived::Exists {
            let yes = self.presence()?.difference_len(&self.empty_selections()?);
            if truth {
                yes
            } else {
                domain_len.saturating_sub(yes)
            }
        } else {
            let Some(key) = derived_structural_key(term.derived, self.multi()) else {
                return Ok(None);
            };
            let value = if term.derived == crate::reference::Derived::Null && !self.multi() {
                self.nulls()?.len()
            } else {
                self.structural(key)?.len()
            };
            if truth {
                value
            } else {
                domain_len.saturating_sub(value)
            }
        };
        Ok(Some(output))
    }

    fn exact_derived(
        &self,
        term: &BoundTerm,
        _domain: &RoaringBitmap,
        input: &RoaringBitmap,
    ) -> StorageResult<Option<RoaringBitmap>> {
        if term.derived == crate::reference::Derived::None {
            return Ok(None);
        }
        let Some(truth) = state_truth(term) else {
            return Ok(None);
        };
        let value = if term.derived == crate::reference::Derived::Exists {
            let mut present = input & &self.presence()?;
            present -= &self.empty_selections()?;
            present
        } else {
            let Some(key) = derived_structural_key(term.derived, self.multi()) else {
                return Ok(None);
            };
            if term.derived == crate::reference::Derived::Null && !self.multi() {
                input & &self.nulls()?
            } else {
                input & &self.structural(key)?
            }
        };
        let result = if truth { value } else { input - &value };
        Ok(Some(result))
    }

    pub(crate) fn native(&self) -> Option<&'a PostingIndex> {
        match self {
            Self::Native(index) => Some(index),
            Self::Packed(_) => None,
        }
    }

    /// Native planner estimate. Packed execution falls back to a conservative
    /// candidate operation until the packed planner costs are persisted.
    pub(crate) fn estimate_exact_with_visits(
        &self,
        term: &BoundTerm,
        domain_len: u64,
        visits: &mut u64,
    ) -> StorageResult<Option<(f64, u64)>> {
        if let Some(index) = self.native() {
            return Ok(index.estimate_exact_with_visits(term, domain_len, visits));
        }
        if term
            .matcher
            .as_ref()
            .is_some_and(|matcher| !matcher.posting_compatible())
        {
            return Ok(None);
        }
        if term.derived == crate::reference::Derived::Length {
            let Some(output) = self.estimate_length(term, domain_len)? else {
                return Ok(None);
            };
            *visits = visits.saturating_add(1);
            return Ok(Some((1.0 + output as f64 / 8.0, output)));
        }
        if term.derived != crate::reference::Derived::None {
            let Some(output) = self.estimate_derived(term, domain_len)? else {
                return Ok(None);
            };
            *visits = visits.saturating_add(1);
            return Ok(Some((1.0, output)));
        }
        if self.covered_records() as u64 != domain_len
            || term.period.is_some()
            || term.related.is_some()
            || term.rhs.is_some()
            || term.operation.is_some()
            || term.derived != crate::reference::Derived::None
            || term.transform != crate::reference::Transform::Identity
        {
            return Ok(None);
        }
        if term.multi
            && (term.operator != Operator::Eq || term.quantifier != crate::query::Quantifier::Any)
        {
            return Ok(None);
        }
        let Some(key) = ScalarKey::from_value(&term.literal) else {
            return Ok(None);
        };
        if !self.value_complete() {
            return Ok(None);
        }
        *visits = visits.saturating_add(1);
        let output = match term.operator {
            Operator::Eq => self.value_cardinality(&key)?,
            Operator::Ne if !term.multi => {
                let presence = self.presence()?.len();
                let nulls = self.nulls()?.len();
                presence
                    .saturating_sub(nulls)
                    .saturating_sub(self.value_cardinality(&key)?)
            }
            Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le if !term.multi => {
                self.range(&key, term.operator)?.len()
            }
            _ => return Ok(None),
        };
        Ok(Some((1.0 + output as f64 / 8.0, output)))
    }

    pub(crate) fn estimate_observed_scalar(
        &self,
        key: &ScalarKey,
        operator: Operator,
        domain_len: u64,
        visits: &mut u64,
    ) -> StorageResult<Option<(f64, u64)>> {
        if let Some(index) = self.native() {
            return Ok(index.estimate_observed_scalar(key, operator, domain_len, visits));
        }
        // Packed: use stored cardinalities and a bounded range scan. This is a
        // conservative cost, not a byte estimate of source verification.
        *visits = visits.saturating_add(1);
        let output = match operator {
            Operator::Eq => self.value_cardinality(key)?,
            Operator::Ne if !self.multi() => {
                let presence = self.presence()?.len();
                let nulls = self.nulls()?.len();
                presence
                    .saturating_sub(nulls)
                    .saturating_sub(self.value_cardinality(key)?)
            }
            Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le => {
                self.range(key, operator)?.len()
            }
            _ => return Ok(None),
        };
        Ok(Some((1.0 + output as f64 / 8.0, output)))
    }

    /// Native exact executor. Packed supports ordinary scalar predicates; a
    /// packaged operation with a custom matcher or transform still requires
    /// the source evaluator.
    pub(crate) fn exact_set_restricted(
        &self,
        term: &BoundTerm,
        domain: &RoaringBitmap,
        input: &RoaringBitmap,
    ) -> StorageResult<Option<RoaringBitmap>> {
        match self {
            Self::Native(index) => Ok(index.exact_set_restricted(term, domain, input)),
            Self::Packed(_) => {
                if term.derived == crate::reference::Derived::Length {
                    return self.exact_length(term, input);
                }
                if let Some(matches) = self.exact_derived(term, domain, input)? {
                    return Ok(Some(matches));
                }
                if !self.values_indexed() {
                    return Ok(None);
                }
                if term.operation.is_some()
                    || term.matcher.is_some()
                    || term.rhs.is_some()
                    || term.related.is_some()
                    || term.period.is_some()
                    || term.transform != crate::reference::Transform::Identity
                    || term.derived != crate::reference::Derived::None
                    || (term.multi && term.operator != crate::query::Operator::Eq)
                {
                    return Ok(None);
                }
                let Some(key) = ScalarKey::from_value(&term.literal) else {
                    return Ok(None);
                };
                let matches = self.range(&key, term.operator)?;
                Ok(Some(&matches & input))
            }
        }
    }

    /// Exact observed scalar operation used by the executor.
    pub(crate) fn exact_observed_scalar_restricted(
        &self,
        key: &ScalarKey,
        operator: Operator,
        domain: &RoaringBitmap,
        input: &RoaringBitmap,
    ) -> StorageResult<Option<RoaringBitmap>> {
        if let Some(index) = self.native() {
            return Ok(index.exact_observed_scalar_restricted(key, operator, domain, input));
        }
        if self.covered_records() as u64 != domain.len() || self.multi() {
            return Ok(None);
        }
        let result = match operator {
            Operator::Eq
            | Operator::Ne
            | Operator::Gt
            | Operator::Ge
            | Operator::Lt
            | Operator::Le => self.range(key, operator)?,
            _ => return Ok(None),
        };
        Ok(Some(&result & input))
    }

    pub(crate) fn value_posting_restricted(
        &self,
        key: &ScalarKey,
        input: &RoaringBitmap,
    ) -> StorageResult<RoaringBitmap> {
        Ok(&self.exact(key)? & input)
    }

    pub(crate) fn union_scalar_range(
        &self,
        key: &ScalarKey,
        operator: Operator,
        input: Option<&RoaringBitmap>,
    ) -> StorageResult<RoaringBitmap> {
        let result = self.range(key, operator)?;
        Ok(input.map_or(result.clone(), |input| &result & input))
    }

    /// Union the ID postings for a relation reverse probe.
    pub(crate) fn union_id_postings(
        &self,
        ids: &std::collections::HashSet<String>,
    ) -> StorageResult<(RoaringBitmap, u64, u64)> {
        match self {
            Self::Native(index) => Ok(index.union_id_postings(ids)),
            Self::Packed(_) => {
                let mut union = RoaringBitmap::new();
                let mut lists = 0u64;
                let mut members = 0u64;
                for id in ids {
                    let posting = self.exact(&crate::posting::ScalarKey::Id(id.clone()))?;
                    if !posting.is_empty() {
                        lists += 1;
                        members += posting.len();


                        union |= posting;
                    }
                }
                Ok((union, lists, members))
            }
        }
    }

    pub(crate) fn for_each_value(
        &self,
        mut visit: impl FnMut(&ScalarKey, &RoaringBitmap) -> Result<(), String>,
    ) -> StorageResult<()> {
        match self {
            Self::Native(index) => index
                .for_each_value(|key, bitmap| visit(key, bitmap))
                .map_err(super::error::StorageError::corrupt)?,
            Self::Packed(index) => {
                for entry in index
                    .values()
                    .range(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded)
                {
                    let (key, value) = entry?;
                    let key = super::scalar::decode_scalar_key(&key)?;
                    let reference = super::record_set::RecordSetRef::decode(&value)?;
                    let bitmap = collect(index.view(reference)?)?;
                    visit(&key, &bitmap).map_err(super::error::StorageError::corrupt)?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn for_each_length(
        &self,
        mut visit: impl FnMut(&Number, &RoaringBitmap) -> Result<(), String>,
    ) -> StorageResult<()> {
        match self {
            Self::Native(index) => index
                .for_each_length(|length, bitmap| visit(length, bitmap))
                .map_err(super::error::StorageError::corrupt)?,
            Self::Packed(index) => {
                let Some(lengths) = index.lengths() else {
                    return Ok(());
                };
                for entry in lengths.range(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded) {
                    let (key, value) = entry?;
                    let ScalarKey::Number(length) = super::scalar::decode_scalar_key(&key)? else {
                        return Err(super::error::StorageError::corrupt(
                            "length dictionary contains a non-number key",
                        ));
                    };
                    let reference = super::record_set::RecordSetRef::decode(&value)?;
                    let bitmap = collect(index.view(reference)?)?;
                    visit(&length, &bitmap).map_err(super::error::StorageError::corrupt)?;
                }
            }
        }
        Ok(())
    }
}


fn owned_bound_ref(bound: &std::ops::Bound<Vec<u8>>) -> std::ops::Bound<&[u8]> {
    match bound {
        std::ops::Bound::Included(value) => std::ops::Bound::Included(value.as_slice()),
        std::ops::Bound::Excluded(value) => std::ops::Bound::Excluded(value.as_slice()),
        std::ops::Bound::Unbounded => std::ops::Bound::Unbounded,
    }
}

fn collect(view: super::record_set::RecordSetView<'_>) -> StorageResult<RoaringBitmap> {
    let mut bitmap = RoaringBitmap::new();
    view.union_into(&mut bitmap)?;
    Ok(bitmap)
}

fn state_truth(term: &BoundTerm) -> Option<bool> {
    let crate::model::Value::Bool(literal) = term.literal else {
        return None;
    };
    match term.operator {
        Operator::Eq => Some(literal),
        Operator::Ne => Some(!literal),
        _ => None,
    }
}

fn derived_structural_key(
    derived: crate::reference::Derived,
    multi: bool,
) -> Option<StructuralKey> {
    Some(match derived {
        crate::reference::Derived::None | crate::reference::Derived::Exists => return None,
        crate::reference::Derived::Null if !multi => return Some(StructuralKey::AnyNull),
        crate::reference::Derived::Null => StructuralKey::AnyNull,
        crate::reference::Derived::IsNonEmpty => StructuralKey::NonEmpty,
        crate::reference::Derived::IsString => StructuralKey::String,
        crate::reference::Derived::IsNumber => StructuralKey::Number,
        crate::reference::Derived::IsArray => StructuralKey::Array,
        crate::reference::Derived::IsObject => StructuralKey::Object,
        crate::reference::Derived::Length => return None,
    })
}

fn structural_code(key: StructuralKey) -> u8 {
    match key {
        StructuralKey::AnyNull => 0,
        StructuralKey::NonEmpty => 1,
        StructuralKey::String => 2,
        StructuralKey::Number => 3,
        StructuralKey::Array => 4,
        StructuralKey::Object => 5,
        StructuralKey::EmptyObject => 6,
    }
}


/// One borrowed bigram posting access.
#[derive(Clone)]
pub(crate) enum BigramAccess<'a> {
    Native {
        index: &'a BigramIndex,
        binding: BigramBinding,
    },
    Packed {
        section: &'a BigramSectionView,
        binding: BigramBinding,
    },
}


impl std::fmt::Debug for BigramAccess<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BigramAccess")
            .field("binding", &self.binding())
            .field("packed", &matches!(self, Self::Packed { .. }))
            .finish()
    }
}


#[allow(dead_code)]
impl<'a> BigramAccess<'a> {
    pub(crate) fn binding(&self) -> &BigramBinding {
        match self {
            Self::Native { binding, .. } | Self::Packed { binding, .. } => binding,
        }
    }

    pub(crate) fn alphabet(&self) -> u8 {
        match self {
            Self::Native { index, .. } => match index.alphabet {
                crate::execution_options::BigramAlphabet::Ascii => 0,
                crate::execution_options::BigramAlphabet::Unicode => 1,
            },
            Self::Packed { section, binding } => {
                packed_binding(section, binding).map_or(0, |view| view.alphabet())
            }
        }
    }

    pub(crate) fn record_count(&self) -> u32 {
        match self {
            Self::Native { index, .. } => index.record_count,
            Self::Packed { section, binding } => {
                packed_binding(section, binding).map_or(0, |view| view.record_count())
            }
        }
    }

    pub(crate) fn normal(&self, gram: u64) -> StorageResult<Option<RoaringBitmap>> {
        match self {
            Self::Native { index, .. } => Ok(index.normal.get(&gram).cloned()),
            Self::Packed { section, binding } => {
                let Some(view) = packed_binding(section, binding) else {
                    return Ok(None);
                };
                Ok(view.normal(gram)?.map(collect).transpose()?)
            }
        }
    }

    pub(crate) fn skip(&self, gram: u64) -> StorageResult<Option<RoaringBitmap>> {
        match self {
            Self::Native { index, .. } => Ok(index.skip.get(&gram).cloned()),
            Self::Packed { section, binding } => {
                let Some(view) = packed_binding(section, binding) else {
                    return Ok(None);
                };
                Ok(view.skip(gram)?.map(collect).transpose()?)
            }
        }
    }

    pub(crate) fn union_normal(&self, grams: &[u64]) -> StorageResult<RoaringBitmap> {
        match self {
            Self::Native { index, .. } => {
                let mut result = RoaringBitmap::new();
                for gram in grams {
                    if let Some(posting) = index.normal.get(gram) {
                        result |= posting;
                    }
                }
                Ok(result)
            }
            Self::Packed { section, binding } => {
                let Some(view) = packed_binding(section, binding) else {
                    return Ok(RoaringBitmap::new());
                };
                view.union_normal(grams)
            }
        }
    }

    pub(crate) fn intersect_normal(&self, grams: &[u64]) -> StorageResult<Option<RoaringBitmap>> {
        match self {
            Self::Native { index, .. } => {
                let mut result: Option<RoaringBitmap> = None;
                for gram in grams {
                    let Some(posting) = index.normal.get(gram) else {
                        return Ok(Some(RoaringBitmap::new()));
                    };
                    result = Some(match result {
                        Some(previous) => &previous & posting,
                        None => posting.clone(),
                    });
                }
                Ok(result)
            }
            Self::Packed { section, binding } => {
                let Some(view) = packed_binding(section, binding) else {
                    return Ok(Some(RoaringBitmap::new()));
                };
                view.intersect_normal(grams)
            }
        }
    }

    pub(crate) fn intersection_count_normal(&self, grams: &[u64]) -> StorageResult<u64> {
        Ok(self
            .intersect_normal(grams)?
            .map_or(0, |bitmap| bitmap.len()))
    }

    pub(crate) fn union_skip(&self, grams: &[u64]) -> StorageResult<RoaringBitmap> {
        match self {
            Self::Native { index, .. } => {
                let mut result = RoaringBitmap::new();
                for gram in grams {
                    if let Some(posting) = index.skip.get(gram) {
                        result |= posting;
                    }
                }
                Ok(result)
            }
            Self::Packed { section, binding } => {
                let Some(view) = packed_binding(section, binding) else {
                    return Ok(RoaringBitmap::new());
                };
                view.union_skip(grams)
            }
        }
    }

    pub(crate) fn native_index(&self) -> Option<&'a BigramIndex> {
        match self {
            Self::Native { index, .. } => Some(index),
            Self::Packed { .. } => None,
        }
    }

    /// Candidate intersection for one prepared literal request.
    pub(crate) fn candidates_prepared(
        &self,
        request: &BigramRequest,
    ) -> StorageResult<Option<RoaringBitmap>> {
        let alphabet = match self.alphabet() {
            0 => crate::execution_options::BigramAlphabet::Ascii,
            _ => crate::execution_options::BigramAlphabet::Unicode,
        };
        let grams = request.grams(alphabet);
        if grams.is_empty() {
            return Ok(None);
        }
        if let Some(index) = self.native_index() {
            return Ok(index.candidates_prepared(request));
        }
        let mut columns = Vec::with_capacity(grams.len());
        for &(skip, key) in grams {
            let column = if skip {
                self.skip(key)?
            } else {
                self.normal(key)?
            };
            let Some(column) = column else {
                return Ok(Some(RoaringBitmap::new()));
            };
            columns.push(column);
        }
        columns.sort_by_key(RoaringBitmap::len);
        let Some(first) = columns.first() else {
            return Ok(None);
        };
        let mut result = first.clone();
        for column in columns.iter().skip(1) {
            result &= column;
            if result.is_empty() {
                break;
            }
        }
        Ok(Some(result))
    }

    pub(crate) fn estimate_prepared(&self, request: &BigramRequest) -> StorageResult<Option<u64>> {
        let alphabet = match self.alphabet() {
            0 => crate::execution_options::BigramAlphabet::Ascii,
            _ => crate::execution_options::BigramAlphabet::Unicode,
        };
        let mut minimum: Option<u64> = None;
        for (skip, key) in request.grams(alphabet).iter() {
            let bitmap = if *skip {
                self.skip(*key)?
            } else {
                self.normal(*key)?
            };
            let count = bitmap.map_or(0, |bitmap| bitmap.len());
            minimum = Some(minimum.map_or(count, |current| current.min(count)));
        }
        Ok(minimum)
    }

    pub(crate) fn normal_gram_count(&self) -> u64 {
        match self {
            Self::Native { index, .. } => index.normal.len() as u64,
            Self::Packed { section, binding } => {
                packed_binding(section, binding).map_or(0, |view| view.normal_gram_count())
            }
        }
    }

    pub(crate) fn skip_gram_count(&self) -> u64 {
        match self {
            Self::Native { index, .. } => index.skip.len() as u64,
            Self::Packed { section, binding } => {
                packed_binding(section, binding).map_or(0, |view| view.skip_gram_count())
            }
        }
    }
}


fn packed_binding<'a>(
    section: &'a BigramSectionView,
    binding: &BigramBinding,
) -> Option<super::bigram::BigramBindingView<'a>> {
    match binding {
        BigramBinding::Raw => section.binding(BigramBindingKind::Raw),
        BigramBinding::Fallback => section.binding(BigramBindingKind::Fallback),
        BigramBinding::Custom { path, name } => section.custom(path, name),
    }
}


/// Borrowed packed field/bigram catalogue used by a future packed execution
/// path. It offers the same access objects as the native catalogue without
/// owning a reconstructed dictionary.
#[allow(dead_code)]
pub(crate) struct PackedIndexCatalog<'a> {
    segment: &'a FrozenSegment,
    schema: &'a Schema,
}


impl std::fmt::Debug for PackedIndexCatalog<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PackedIndexCatalog")
            .field("collection", &self.segment.stats().collection)
            .field("fields", &self.segment.postings().fields().len())
            .finish_non_exhaustive()
    }
}


#[allow(dead_code)]
impl<'a> PackedIndexCatalog<'a> {
    pub(crate) fn new(segment: &'a FrozenSegment, schema: &'a Schema) -> Self {
        Self { segment, schema }
    }

    pub(crate) fn schema(&self) -> &'a Schema {
        self.schema
    }

    pub(crate) fn segment(&self) -> &'a FrozenSegment {
        self.segment
    }

    pub(crate) fn field(&self, path: &[String]) -> Option<&'a FieldPostingsView> {
        self.segment.postings().field(path)
    }

    pub(crate) fn field_postings(&self, path: &[String]) -> Option<PostingAccess<'a>> {
        self.field(path).map(PostingAccess::Packed)
    }

    pub(crate) fn bigram(&self, binding: &BigramBinding) -> Option<BigramAccess<'a>> {
        Some(BigramAccess::Packed {
            section: self.segment.bigrams(),
            binding: binding.clone(),
        })
    }

    pub(crate) fn logical_records(&self) -> StorageResult<RoaringBitmap> {
        collect(self.segment.coverage().logical_records()?)
    }

    pub(crate) fn active_records(&self) -> StorageResult<RoaringBitmap> {
        collect(self.segment.coverage().active_records()?)
    }
}
