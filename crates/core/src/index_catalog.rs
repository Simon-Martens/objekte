//! Borrowed views of published search accesses. A catalogue never owns a
//! posting, bitmap, or dictionary: the owning generation/segment retains them.

use std::collections::HashMap;

use roaring::RoaringBitmap;

use crate::bigram::BigramIndex;
use crate::build::IndexedCollectionSegment;
use crate::inference::InferenceState;
use crate::model::{LogicalType, PathRoot, Value};
use crate::posting::ScalarKey;
use crate::posting::{CustomPostingIndex, PostingCollection, PostingIndex};
use crate::query::{Operator, Quantifier};
use crate::reference::{BoundTerm, Derived, Schema, Transform};
use crate::repository::CollectionState;

/// Physical identity is used only to resolve a selected borrowed access. It
/// does not change applicability, proof, or cost.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PostingId {
    Declared,
    Observed,
    Named,
}

#[derive(Clone, Copy)]
pub(crate) struct PostingAccess<'a> {
    pub id: PostingId,
    pub index: &'a PostingIndex,
    /// A named access without a completeness guarantee has no safe exclusion
    /// domain. The index may still exist for diagnostics and rebuilds.
    pub admitted: bool,
    pub empty_selections: Option<&'a RoaringBitmap>,
    pub short_string_coverage: Option<&'a RoaringBitmap>,
}

/// The same prepared operation is used by estimation, execution, and EXPLAIN.
/// It contains no handle that could outlive its published owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PostingOperation {
    Exact,
    EmptyStringExact,
    ObservedScalarExact,
    StringDictionaryExact,
    ShortStringCandidates,
    ValueCandidates,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PostingProposal {
    pub id: PostingId,
    pub operation: PostingOperation,
    pub exact: bool,
    pub lookup_work: f64,
    pub output: u64,
    pub coverage: u32,
}

impl PostingAccess<'_> {
    pub fn assess(
        self,
        term: &BoundTerm,
        domain: &RoaringBitmap,
        visits: &mut u64,
    ) -> Result<Option<PostingProposal>, String> {
        let index = self.index;
        let structural = term.derived.state_predicate().is_some();
        if !self.admitted
            || term.path.root != PathRoot::Data
            || term.related.is_some()
            || term.rhs.is_some()
            || term.query_path.has_position
            || ((structural || term.derived == Derived::Length)
                && term.query_path.has_explicit_array_step)
            || term.transform != Transform::Identity
            || (term.period.is_some() && !index.value_complete)
            || index.covered_records as u64 != domain.len()
            || (term.kind != LogicalType::Unknown
                && term.kind != index.kind
                && !matches!(&index.kind, LogicalType::Array(inner) if inner.as_ref() == &term.kind)
                && !(structural && term.kind == LogicalType::Bool)
                && !(term.derived == Derived::Length && term.kind == LogicalType::Number))
        {
            return Ok(None);
        }
        let proposal = |operation, exact, lookup_work, output| PostingProposal {
            id: self.id,
            operation,
            exact,
            lookup_work,
            output,
            coverage: index.covered_records,
        };
        if let Some((work, output)) =
            index.estimate_exact_with_visits_checked(term, domain.len(), visits)?
        {
            return Ok(Some(proposal(PostingOperation::Exact, true, work, output)));
        }
        if index.empty_string_complete
            && !index.multi
            && !term.query_path.has_explicit_array_step
            && term.derived == Derived::None
            && term.operator == Operator::Eq
            && term.quantifier == Quantifier::Default
            && matches!(&term.literal, Value::String(value) if value.is_empty())
        {
            *visits += 1;
            let output = index.value_cardinality(&ScalarKey::String(String::new()))?;
            return Ok(Some(proposal(
                PostingOperation::EmptyStringExact,
                true,
                1.0,
                output,
            )));
        }
        // Opaque operations and custom matchers have no general implication
        // from a typed dictionary beyond the posting's own exact contract.
        if term.operation.is_some()
            || term.matcher.is_some()
            || term.period.is_some()
            || term.query_path.has_explicit_array_step
        {
            return Ok(None);
        }
        let complete_scalar = index.value_complete
            && !index.multi
            && self.empty_selections.is_none_or(RoaringBitmap::is_empty)
            && self.short_string_coverage.is_none_or(|covered| {
                index.presence.difference_len(covered) == index.nulls.difference_len(covered)
            });
        let compatible_literal = matches!(
            (&index.kind, &term.literal),
            (LogicalType::Number, Value::Number(_))
                | (LogicalType::Bool, Value::Bool(_))
                | (LogicalType::String, Value::String(_))
        );
        if complete_scalar && term.derived == Derived::None && compatible_literal {
            match term.operator {
                Operator::Eq
                | Operator::Ne
                | Operator::Gt
                | Operator::Ge
                | Operator::Lt
                | Operator::Le => {
                    let Some(key) = ScalarKey::from_value(&term.literal) else {
                        return Ok(None);
                    };
                    if let Some((work, output)) = index.estimate_observed_scalar_checked(
                        &key,
                        term.operator,
                        domain.len(),
                        visits,
                    )? {
                        return Ok(Some(proposal(
                            PostingOperation::ObservedScalarExact,
                            true,
                            work,
                            output,
                        )));
                    }
                }
                Operator::Contains | Operator::NotContains if index.kind == LogicalType::String => {
                    return Ok(Some(proposal(
                        PostingOperation::StringDictionaryExact,
                        true,
                        index.distinct_values() as f64,
                        domain.len() / 4,
                    )));
                }
                _ => {}
            }
        }
        if index.value_complete
            && term.derived == Derived::None
            && term.operator == Operator::Contains
            && matches!(term.literal, Value::String(_))
            && matches!(&index.kind, LogicalType::Array(inner) if inner.as_ref() == &LogicalType::String)
            && term.quantifier == Quantifier::Any
        {
            return Ok(Some(proposal(
                PostingOperation::StringDictionaryExact,
                true,
                index.distinct_values() as f64 * 2.0,
                domain.len() / 4,
            )));
        }
        if term.derived != Derived::None
            || !matches!(
                term.literal,
                Value::String(_) | Value::Number(_) | Value::Bool(_)
            )
            || !matches!(
                term.operator,
                Operator::Eq | Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le
            )
        {
            return Ok(None);
        }
        let Some(key) = ScalarKey::from_value(&term.literal) else {
            return Ok(None);
        };
        if let Some(covered) = self.short_string_coverage {
            if !matches!(term.literal, Value::String(_)) {
                return Ok(None);
            }
            let Some((work, count)) = index.estimate_observed_scalar_checked(
                &key,
                term.operator,
                domain.len(),
                visits,
            )?
            else {
                return Ok(None);
            };
            return Ok(Some(proposal(
                PostingOperation::ShortStringCandidates,
                false,
                work,
                count
                    .saturating_add(domain.len() - covered.len())
                    .min(domain.len()),
            )));
        }
        if !index.value_complete
            || (term.quantifier != Quantifier::Any && self.empty_selections.is_none())
        {
            return Ok(None);
        }
        let Some((work, count)) =
            index.estimate_observed_scalar_checked(&key, term.operator, domain.len(), visits)?
        else {
            return Ok(None);
        };
        let count = if term.quantifier != Quantifier::Any {
            count.saturating_add(self.empty_selections.map_or(0, RoaringBitmap::len))
        } else {
            count
        };
        Ok(Some(proposal(
            PostingOperation::ValueCandidates,
            false,
            work,
            count.min(domain.len()),
        )))
    }

    pub fn execute(
        self,
        term: &BoundTerm,
        domain: &RoaringBitmap,
        input: &RoaringBitmap,
        operation: PostingOperation,
    ) -> Result<Option<RoaringBitmap>, String> {
        let index = self.index;
        match operation {
            PostingOperation::Exact => index.exact_set_restricted_checked(term, domain, input),
            PostingOperation::EmptyStringExact => Ok(Some(
                index
                    .value_posting_restricted(&ScalarKey::String(String::new()), input)?
                    .unwrap_or_default(),
            )),
            PostingOperation::ObservedScalarExact => {
                let Some(key) = ScalarKey::from_value(&term.literal) else {
                    return Ok(None);
                };
                index.exact_observed_scalar_restricted_checked(&key, term.operator, domain, input)
            }
            PostingOperation::StringDictionaryExact => {
                let Value::String(pattern) = &term.literal else {
                    return Ok(None);
                };
                let mut result = RoaringBitmap::new();
                index.for_each_value(|key, slots| {
                    let ScalarKey::String(value) = key else {
                        return Err("string posting dictionary has non-string key".into());
                    };
                    let contains = term.prepared_contains.as_ref().map_or_else(
                        || crate::reference::contains_pattern(value, pattern),
                        |prepared| prepared.matches(value, pattern),
                    );
                    if contains == (term.operator == Operator::Contains) {
                        result |= slots & input;
                    }
                    Ok(())
                })?;
                Ok(Some(result))
            }
            PostingOperation::ShortStringCandidates | PostingOperation::ValueCandidates => {
                let Some(key) = ScalarKey::from_value(&term.literal) else {
                    return Ok(None);
                };
                let mut result = match term.operator {
                    Operator::Eq => index
                        .value_posting_restricted(&key, input)?
                        .unwrap_or_default(),
                    Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le => {
                        index.union_scalar_range(&key, term.operator, Some(input))?
                    }
                    _ => return Ok(None),
                };
                if operation == PostingOperation::ShortStringCandidates {
                    let Some(coverage) = self.short_string_coverage else {
                        return Ok(None);
                    };
                    result |= input - coverage;
                } else if term.quantifier != Quantifier::Any {
                    let Some(empty) = self.empty_selections else {
                        return Ok(None);
                    };
                    result |= empty;
                }
                Ok(Some(result & input))
            }
        }
    }
}

pub(crate) struct IndexCatalog<'a> {
    postings: &'a PostingCollection,
    observed: Option<&'a InferenceState>,
    named: &'a HashMap<(Vec<String>, String), CustomPostingIndex>,
    text: &'a HashMap<(Vec<String>, String), BigramIndex>,
    text_bindings: &'a TextBindingCatalog,
    pub logical: &'a RoaringBitmap,
    pub raw_text: &'a BigramIndex,
    pub fallback_text: &'a BigramIndex,
}

/// Derived once when a collection is published or restored. It contains only
/// small binding names; postings and membership remain in their owners.
#[derive(Clone, Debug, Default)]
pub(crate) struct TextBindingCatalog {
    canonical: HashMap<Vec<String>, String>,
}

impl TextBindingCatalog {
    pub fn new(schema: &Schema) -> Self {
        let mut canonical = HashMap::new();
        for (path, name) in schema.custom.keys() {
            canonical
                .entry(path.clone())
                .and_modify(|current: &mut String| {
                    if name < current {
                        current.clone_from(name);
                    }
                })
                .or_insert_with(|| name.clone());
        }
        Self { canonical }
    }
}

impl<'a> IndexCatalog<'a> {
    pub fn collection(state: &'a CollectionState) -> Self {
        Self {
            postings: &state.postings,
            observed: state.inference.as_ref(),
            named: &state.custom_postings,
            text: &state.custom_text,
            text_bindings: &state.text_bindings,
            logical: &state.logical_records,
            raw_text: &state.raw,
            fallback_text: &state.fallback_text,
        }
    }

    pub fn segment(state: &'a IndexedCollectionSegment, collection: &'a CollectionState) -> Self {
        Self {
            postings: &state.postings,
            observed: state.inference.as_ref(),
            named: &state.custom_postings,
            text: &state.custom_text,
            text_bindings: &collection.text_bindings,
            logical: &state.logical_records,
            raw_text: &state.raw,
            fallback_text: &state.fallback_text,
        }
    }

    /// Constant-time borrowed lookup of every posting representation for one
    /// bound field. No catalogue is rebuilt by traversing the schema.
    pub fn field_postings(
        &self,
        path: &[String],
        binding: &str,
        named_operation: bool,
    ) -> [Option<PostingAccess<'a>>; 2] {
        if named_operation {
            let named = self.named.get(&(path.to_vec(), binding.to_owned()));
            return [
                named.map(|value| PostingAccess {
                    id: PostingId::Named,
                    index: &value.index,
                    admitted: value.complete,
                    empty_selections: None,
                    short_string_coverage: None,
                }),
                None,
            ];
        }
        let declared = self.postings.fields.get(path).map(|index| PostingAccess {
            id: PostingId::Declared,
            index,
            admitted: true,
            empty_selections: None,
            short_string_coverage: None,
        });
        let observed = self.observed.and_then(|state| {
            state.indexes.get(path).map(|index| PostingAccess {
                id: PostingId::Observed,
                index,
                admitted: true,
                empty_selections: state.empty_selections.get(path),
                short_string_coverage: state.short_string_coverage.get(path),
            })
        });
        [declared, observed]
    }

    pub fn posting(
        &self,
        path: &[String],
        binding: &str,
        named_operation: bool,
        id: PostingId,
    ) -> Option<PostingAccess<'a>> {
        self.field_postings(path, binding, named_operation)
            .into_iter()
            .flatten()
            .find(|access| access.id == id)
    }

    pub fn field_text(&self, path: &[String], binding: &str) -> Option<&'a BigramIndex> {
        self.text.get(&(path.to_vec(), binding.to_owned()))
    }

    /// Resolve a field text binding with the same representation/coverage rule
    /// for FILTER, FTS, full generations, and incremental segments. A raw
    /// corpus is candidate-only for a field: occurrence elsewhere in a record
    /// never proves that the selected field matched.
    pub fn field_text_for(
        &self,
        schema: &Schema,
        path: &[String],
        binding: &str,
        named_operation: bool,
        logical_text_complete: bool,
        raw_field_compatible: bool,
    ) -> Option<&'a BigramIndex> {
        let built_in = matches!(binding, "default" | "lower");
        let canonical = self.text_bindings.canonical.get(path);
        let name = if built_in
            || schema
                .custom
                .contains_key(&(path.to_vec(), binding.to_owned()))
        {
            canonical.map(String::as_str)
        } else {
            Some(binding)
        };
        if logical_text_complete
            && let Some(index) = name.and_then(|name| self.field_text(path, name))
        {
            return Some(index);
        }
        (built_in && !named_operation && raw_field_compatible).then_some(self.raw_text)
    }

    /// An ordered value access needs one complete scalar key or a known
    /// missing/null state for every logical record. This rule is independent
    /// of the producer that populated the posting.
    pub fn sortable_scalar(&self, path: &[String]) -> Option<&'a PostingIndex> {
        self.field_postings(path, "default", false)
            .into_iter()
            .flatten()
            .find(|access| {
                let index = access.index;
                access.admitted
                    && index.covered_records as u64 == self.logical.len()
                    && index.value_complete
                    && !index.multi
                    && matches!(
                        index.kind,
                        LogicalType::Number | LogicalType::Bool | LogicalType::String
                    )
                    && access.empty_selections.is_none_or(RoaringBitmap::is_empty)
                    && access.short_string_coverage.is_none_or(|covered| {
                        index.presence.difference_len(covered)
                            == index.nulls.difference_len(covered)
                    })
            })
            .map(|access| access.index)
    }
}
