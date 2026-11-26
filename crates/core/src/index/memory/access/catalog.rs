//! Borrowed views of published search accesses. A catalogue never owns a
//! posting, bitmap, or dictionary: the owning generation/segment retains them.

use std::collections::HashMap;

use roaring::RoaringBitmap;

use crate::index::bigram::BigramIndex;
use crate::index::memory::inference::InferenceState;
use crate::index::posting::ScalarKey;
use crate::index::posting::{CustomPostingIndex, PostingCollection, PostingIndex};
use crate::index::segment::IndexedCollectionSegment;
use crate::index::storage::text::TextBindingCatalog;
use crate::model::{LogicalType, PathRoot, Value};
use crate::query::path::PathStep;
use crate::query::{Operator, Quantifier};
use crate::query::{binding::filter::BoundTerm, binding::filter::Derived};
use crate::repository::state::CollectionState;
use crate::schema::{Schema, operation::Transform};

pub(crate) use crate::query_access::contract::{PostingId, PostingOperation};

#[derive(Clone, Copy)]
pub(crate) struct PostingAccess<'a> {
    pub id: PostingId,
    pub index: &'a PostingIndex,
    schema: &'a Schema,
    /// A named access without a completeness guarantee has no safe exclusion
    /// domain. The index may still exist for diagnostics and rebuilds.
    pub admitted: bool,
    pub short_string_coverage: Option<&'a RoaringBitmap>,
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
        // A non-scalar occurrence can compare false (or raise an ambiguity),
        // and must keep the source evaluator.
        if self.admitted
            && self.id == PostingId::Declared
            && term.path.root == PathRoot::Data
            && self_comparison_shape(term)
            && term.path.segments.len() == 1
            && term.source_kind == index.kind
            && (!term.query_path.has_explicit_array_step
                || declared_path_has_equivalent_terminal_selection(self.schema, term))
            && index.covered_records as u64 == domain.len()
            && index.summary.is_some_and(|summary| {
                summary.only_matches_scalar_or_null() && !summary.may_select_multiple()
            })
        {
            *visits += 1;
            return Ok(Some(PostingProposal {
                id: self.id,
                operation: PostingOperation::SelfComparePresence,
                exact: true,
                lookup_work: 1.0,
                output: index.presence.len(),
                coverage: index.covered_records,
            }));
        }
        if !self.admitted
            || term.path.root != PathRoot::Data
            || term.related.is_some()
            || term.rhs.is_some()
            || term.query_path.has_position
            || (term.query_path.has_explicit_array_step
                && (term.derived.state_predicate().is_none()
                    || self.id != PostingId::Declared
                    || !declared_path_has_equivalent_terminal_selection(self.schema, term)))
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
        if let Some((work, output)) = index.estimate_exact_with_visits(term, domain.len(), visits) {
            return Ok(Some(proposal(PostingOperation::Exact, true, work, output)));
        }
        if structural && index.summary.is_some() && PostingIndex::state_truth(term) == Some(true) {
            let (work, output) = index
                .estimate_state_candidates(term.derived, domain.len(), visits)
                .unwrap_or_else(|| {
                    *visits += 1;
                    (1.0, index.presence.len())
                });
            return Ok(Some(proposal(
                PostingOperation::StateCandidates,
                false,
                work,
                output,
            )));
        }
        if index.empty_string_equality_is_exact(term) {
            *visits += 1;
            let output = index.value_cardinality(&ScalarKey::String(String::new()));
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
            && index.summary.is_none_or(|summary| {
                !summary.may_have_empty_traversal() || term.quantifier == Quantifier::Any
            })
            && self.short_string_coverage.is_none_or(|covered| {
                index.null_posting().map_or_else(
                    || index.presence.difference_len(covered) == 0,
                    |nulls| index.presence.difference_len(covered) == nulls.difference_len(covered),
                )
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
                    if let Some((work, output)) =
                        index.estimate_observed_scalar(&key, term.operator, domain.len(), visits)
                    {
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
            let Some((work, count)) =
                index.estimate_observed_scalar(&key, term.operator, domain.len(), visits)
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
        if !index.value_complete || term.quantifier != Quantifier::Any {
            return Ok(None);
        }
        let Some((work, count)) =
            index.estimate_observed_scalar(&key, term.operator, domain.len(), visits)
        else {
            return Ok(None);
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
            PostingOperation::Exact => Ok(index.exact_set_restricted(term, domain, input)),
            PostingOperation::SelfComparePresence => Ok(Some(input & &index.presence)),
            PostingOperation::EmptyStringExact => Ok(Some(
                index
                    .value_posting_restricted(&ScalarKey::String(String::new()), input)
                    .unwrap_or_default(),
            )),
            PostingOperation::ObservedScalarExact => {
                let Some(key) = ScalarKey::from_value(&term.literal) else {
                    return Ok(None);
                };
                Ok(index.exact_observed_scalar_restricted(&key, term.operator, domain, input))
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
                        || crate::query::semantics::comparison::contains_pattern(value, pattern),
                        |prepared| prepared.matches(value, pattern),
                    );
                    if contains == (term.operator == Operator::Contains) {
                        result |= slots & input;
                    }
                    Ok(())
                })?;
                Ok(Some(result))
            }
            PostingOperation::StateCandidates => Ok(Some(
                index
                    .state_candidate_postings(term.derived, input)
                    .unwrap_or_else(|| input & &index.presence),
            )),
            PostingOperation::ShortStringCandidates | PostingOperation::ValueCandidates => {
                let Some(key) = ScalarKey::from_value(&term.literal) else {
                    return Ok(None);
                };
                let mut result = match term.operator {
                    Operator::Eq => index
                        .value_posting_restricted(&key, input)
                        .unwrap_or_default(),
                    Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le => {
                        index.union_scalar_range(&key, term.operator, Some(input))
                    }
                    _ => return Ok(None),
                };
                if operation == PostingOperation::ShortStringCandidates {
                    let Some(coverage) = self.short_string_coverage else {
                        return Ok(None);
                    };
                    result |= input - coverage;
                }
                Ok(Some(result & input))
            }
        }
    }
}

fn declared_path_has_equivalent_terminal_selection(schema: &Schema, term: &BoundTerm) -> bool {
    let mut prefix = Vec::new();
    let steps = &term.query_path.steps;
    for (index, step) in steps.iter().enumerate() {
        match step {
            PathStep::Field(name) => prefix.push(name.clone()),
            PathStep::EachElement => {
                if !schema
                    .fields
                    .get(&prefix)
                    .is_some_and(|field| matches!(field.kind, LogicalType::Array(_)))
                {
                    return false;
                }
                // A wildcard selects the array elements, while the posting for
                // the same field sequence selects the container. It is only
                // equivalent when a subsequent field step selects the same
                // terminal values reached by normal array traversal.
                if !matches!(steps.get(index + 1), Some(PathStep::Field(_))) {
                    return false;
                }
            }
            PathStep::ArrayIndex(_) => return false,
        }
    }
    matches!(steps.last(), Some(PathStep::Field(_)))
}

pub(crate) fn self_comparison_shape(term: &BoundTerm) -> bool {
    let Some(rhs) = &term.rhs else { return false };
    term.operator == Operator::Eq
        && term.quantifier == Quantifier::Default
        && term.derived == Derived::None
        && rhs.derived == Derived::None
        && term.path == rhs.path
        && term.query_path == rhs.query_path
        && term.kind == rhs.kind
        && term.multi == rhs.multi
        && !term.implicit_array
        && !term.query_path.has_position
        && matches!(
            term.source_kind,
            LogicalType::String
                | LogicalType::Id
                | LogicalType::Number
                | LogicalType::Bool
                | LogicalType::Date
                | LogicalType::Datetime
                | LogicalType::Unixtime
        )
        && term.transform == Transform::Identity
        && rhs.transform == Transform::Identity
        && term.operation.is_none()
        && rhs.operation.is_none()
        && term.matcher.is_none()
        && term.related.is_none()
        && term.period.is_none()
}

pub(crate) struct IndexCatalog<'a> {
    schema: &'a Schema,
    postings: &'a PostingCollection,
    observed: Option<&'a InferenceState>,
    named: &'a HashMap<(Vec<String>, String), CustomPostingIndex>,
    text: &'a HashMap<(Vec<String>, String), BigramIndex>,
    text_bindings: &'a TextBindingCatalog,
    pub logical: &'a RoaringBitmap,
    pub raw_text: &'a BigramIndex,
    pub fallback_text: &'a BigramIndex,
}

impl<'a> IndexCatalog<'a> {
    pub fn collection(state: &'a CollectionState) -> Self {
        Self {
            schema: &state.definition.schema,
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

    pub(crate) fn segment_with_bindings(
        state: &'a IndexedCollectionSegment,
        schema: &'a Schema,
        bindings: &'a TextBindingCatalog,
    ) -> Self {
        Self {
            schema,
            postings: &state.postings,
            observed: state.inference.as_ref(),
            named: &state.custom_postings,
            text: &state.custom_text,
            text_bindings: bindings,
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
                    schema: self.schema,
                    admitted: value.complete,
                    short_string_coverage: None,
                }),
                None,
            ];
        }
        let declared = self.postings.fields.get(path).map(|index| PostingAccess {
            id: PostingId::Declared,
            index,
            schema: self.schema,
            admitted: true,
            short_string_coverage: None,
        });
        let observed = self.observed.and_then(|state| {
            state.indexes.get(path).map(|index| PostingAccess {
                id: PostingId::Observed,
                index,
                schema: self.schema,
                admitted: true,
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
}

/// An ordered value access needs one complete scalar key or a known
/// missing/null state for every logical record.
pub(in crate::index::memory) fn sortable_segment_posting<'a>(
    segment: &'a IndexedCollectionSegment,
    path: &[String],
) -> Option<&'a PostingIndex> {
    let complete = |index: &PostingIndex, short_string_coverage: Option<&RoaringBitmap>| {
        index.covered_records as u64 == segment.logical_records.len()
            && index.value_complete
            && !index.multi
            && matches!(
                index.kind,
                LogicalType::Number | LogicalType::Bool | LogicalType::String
            )
            && short_string_coverage.is_none_or(|covered| {
                index.null_posting().map_or_else(
                    || index.presence.difference_len(covered) == 0,
                    |nulls| index.presence.difference_len(covered) == nulls.difference_len(covered),
                )
            })
    };
    segment
        .postings
        .fields
        .get(path)
        .filter(|index| complete(index, None))
        .or_else(|| {
            let observed = segment.inference.as_ref()?;
            observed
                .indexes
                .get(path)
                .filter(|index| complete(index, observed.short_string_coverage.get(path)))
        })
}
