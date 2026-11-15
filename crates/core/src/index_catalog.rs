//! Borrowed views of published search accesses. A catalogue never owns a
//! posting, bitmap, or dictionary: the owning generation/segment retains them.

use std::collections::HashMap;

use roaring::RoaringBitmap;

use crate::bigram::BigramIndex;
use crate::build::IndexedCollectionSegment;
use crate::inference::InferenceState;
use crate::model::{LogicalType, PathRoot, Value};
use crate::posting::{CustomPostingIndex, PostingCollection, PostingIndex};
use crate::posting::{ScalarKey, StructuralKey};
use crate::query::{Operator, Quantifier};
use crate::query_path::PathStep;
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
    pub access: crate::index_storage::access::PostingAccess<'a>,
    schema: &'a Schema,
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
    SelfComparePresence,
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
    fn storage_error(error: crate::index_storage::error::StorageError) -> String {
        error.to_string()
    }

    pub fn assess(
        self,
        term: &BoundTerm,
        domain: &RoaringBitmap,
        visits: &mut u64,
    ) -> Result<Option<PostingProposal>, String> {
        let index = self.access;
        let structural = term.derived.state_predicate().is_some();
        let kind = index.kind();
        let covered_records = index.covered_records();
        // The paired evaluator treats a present empty selection as vacuously
        // true. `:exists` excludes that selection, so this uses raw presence.
        // A non-scalar occurrence can compare false (or raise an ambiguity),
        // and must keep the source evaluator.
        if self.admitted
            && self.id == PostingId::Declared
            && term.path.root == PathRoot::Data
            && self_comparison_shape(term)
            && &term.source_kind == kind
            && (!term.query_path.has_explicit_array_step || declared_array_steps(self.schema, term))
            && covered_records as u64 == domain.len()
            && !index
                .structural(StructuralKey::Array)
                .map_err(Self::storage_error)?
                .is_empty()
                == false
            && !index
                .structural(StructuralKey::Object)
                .map_err(Self::storage_error)?
                .is_empty()
                == false
        {
            *visits += 1;
            let presence = index.presence().map_err(Self::storage_error)?;
            return Ok(Some(PostingProposal {
                id: self.id,
                operation: PostingOperation::SelfComparePresence,
                exact: true,
                lookup_work: 1.0,
                output: presence.len(),
                coverage: covered_records,
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
                    || !declared_array_steps(self.schema, term)))
            || term.transform != Transform::Identity
            || (term.period.is_some() && !index.value_complete())
            || covered_records as u64 != domain.len()
            || (term.kind != LogicalType::Unknown
                && term.kind != *kind
                && !matches!(kind, LogicalType::Array(inner) if inner.as_ref() == &term.kind)
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
            coverage: covered_records,
        };
        if let Some((work, output)) = index.estimate_exact_with_visits(term, domain.len(), visits) {
            return Ok(Some(proposal(PostingOperation::Exact, true, work, output)));
        }
        if index.empty_string_complete()
            && !index.multi()
            && !term.query_path.has_explicit_array_step
            && term.derived == Derived::None
            && term.operator == Operator::Eq
            && term.quantifier == Quantifier::Default
            && matches!(&term.literal, Value::String(value) if value.is_empty())
        {
            *visits += 1;
            let output = index
                .value_cardinality(&ScalarKey::String(String::new()))
                .map_err(Self::storage_error)?;
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
        let presence = index.presence().map_err(Self::storage_error)?;
        let nulls = index.nulls().map_err(Self::storage_error)?;
        let complete_scalar = index.value_complete()
            && !index.multi()
            && self.empty_selections.is_none_or(RoaringBitmap::is_empty)
            && self.short_string_coverage.is_none_or(|covered| {
                presence.difference_len(covered) == nulls.difference_len(covered)
            });
        let compatible_literal = matches!(
            (kind, &term.literal),
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
                    if let Some((work, output)) = index
                        .estimate_observed_scalar(&key, term.operator, domain.len(), visits)
                        .map_err(Self::storage_error)?
                    {
                        return Ok(Some(proposal(
                            PostingOperation::ObservedScalarExact,
                            true,
                            work,
                            output,
                        )));
                    }
                }
                Operator::Contains | Operator::NotContains if *kind == LogicalType::String => {
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
        if index.value_complete()
            && term.derived == Derived::None
            && term.operator == Operator::Contains
            && matches!(term.literal, Value::String(_))
            && matches!(kind, LogicalType::Array(inner) if inner.as_ref() == &LogicalType::String)
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
            let Some((work, count)) = index
                .estimate_observed_scalar(&key, term.operator, domain.len(), visits)
                .map_err(Self::storage_error)?
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
        if !index.value_complete()
            || (term.quantifier != Quantifier::Any && self.empty_selections.is_none())
        {
            return Ok(None);
        }
        let Some((work, count)) = index
            .estimate_observed_scalar(&key, term.operator, domain.len(), visits)
            .map_err(Self::storage_error)?
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
        let index = self.access;
        match operation {
            PostingOperation::Exact => Ok(index.exact_set_restricted(term, domain, input)),
            PostingOperation::SelfComparePresence => Ok(Some(
                &index.presence().map_err(Self::storage_error)? & input,
            )),
            PostingOperation::EmptyStringExact => Ok(Some(
                index
                    .value_posting_restricted(&ScalarKey::String(String::new()), input)
                    .map_err(Self::storage_error)?,
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
                index
                    .for_each_value(|key, slots| {
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
                    })
                    .map_err(Self::storage_error)?;
                Ok(Some(result))
            }
            PostingOperation::ShortStringCandidates | PostingOperation::ValueCandidates => {
                let Some(key) = ScalarKey::from_value(&term.literal) else {
                    return Ok(None);
                };
                let mut result = match term.operator {
                    Operator::Eq => index
                        .value_posting_restricted(&key, input)
                        .map_err(Self::storage_error)?,
                    Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le => index
                        .union_scalar_range(&key, term.operator, Some(input))
                        .map_err(Self::storage_error)?,
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


fn declared_array_steps(schema: &Schema, term: &BoundTerm) -> bool {
    let mut prefix = Vec::new();
    for step in &term.query_path.steps {
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
            }
            PathStep::ArrayIndex(_) => return false,
        }
    }
    true
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
    packed: Option<&'a crate::index_storage::segment::FrozenSegment>,
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
            schema: &state.definition.schema,
            postings: &state.postings,
            observed: state.inference.as_ref(),
            named: &state.custom_postings,
            text: &state.custom_text,
            text_bindings: &state.text_bindings,
            logical: &state.logical_records,
            raw_text: &state.raw,
            fallback_text: &state.fallback_text,
            packed: state.packed.as_deref(),
        }
    }

    pub fn segment(state: &'a IndexedCollectionSegment, collection: &'a CollectionState) -> Self {
        Self {
            schema: &collection.definition.schema,
            postings: &state.postings,
            observed: state.inference.as_ref(),
            named: &state.custom_postings,
            text: &state.custom_text,
            text_bindings: &collection.text_bindings,
            logical: &state.logical_records,
            raw_text: &state.raw,
            fallback_text: &state.fallback_text,
            packed: state.packed.as_deref().or(collection.packed.as_deref()),
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
        if let Some(packed) = self.packed {
            use crate::index_storage::posting_builder::FieldSource;
            if named_operation {
                let named = packed
                    .postings()
                    .field_with_source(path, &FieldSource::Named(binding.to_owned()))
                    .map(|field| PostingAccess {
                        id: PostingId::Named,
                        access: crate::index_storage::access::PostingAccess::Packed(field),
                        schema: self.schema,
                        admitted: field.complete(),
                        empty_selections: None,
                        short_string_coverage: None,
                    });
                return [named, None];
            }
            let declared = packed
                .postings()
                .field_with_source(path, &FieldSource::Declared)
                .map(|field| PostingAccess {
                    id: PostingId::Declared,
                    access: crate::index_storage::access::PostingAccess::Packed(field),
                    schema: self.schema,
                    admitted: true,
                    empty_selections: None,
                    short_string_coverage: None,
                });
            let observed = packed
                .postings()
                .field_with_source(path, &FieldSource::Observed)
                .map(|field| PostingAccess {
                    id: PostingId::Observed,
                    access: crate::index_storage::access::PostingAccess::Packed(field),
                    schema: self.schema,
                    admitted: true,
                    empty_selections: self
                        .observed
                        .and_then(|state| state.empty_selections.get(path)),
                    short_string_coverage: self
                        .observed
                        .and_then(|state| state.short_string_coverage.get(path)),
                });
            return [declared, observed];
        }
        if named_operation {
            let named = self.named.get(&(path.to_vec(), binding.to_owned()));
            return [
                named.map(|value| PostingAccess {
                    id: PostingId::Named,
                    access: crate::index_storage::access::PostingAccess::Native(&value.index),
                    schema: self.schema,
                    admitted: value.complete,
                    empty_selections: None,
                    short_string_coverage: None,
                }),
                None,
            ];
        }
        let declared = self.postings.fields.get(path).map(|index| PostingAccess {
            id: PostingId::Declared,
            access: crate::index_storage::access::PostingAccess::Native(index),
            schema: self.schema,
            admitted: true,
            empty_selections: None,
            short_string_coverage: None,
        });
        let observed = self.observed.and_then(|state| {
            state.indexes.get(path).map(|index| PostingAccess {
                id: PostingId::Observed,
                access: crate::index_storage::access::PostingAccess::Native(index),
                schema: self.schema,
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
    #[allow(dead_code)]
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

    /// Raw-corpus candidate provider for this collection or segment.
    pub fn raw_text_access(&self) -> crate::index_storage::access::BigramAccess<'a> {
        use crate::index_storage::access::BigramAccess;
        if let Some(packed) = self.packed
            && packed
                .bigrams()
                .binding(crate::index_storage::bigram_builder::BigramBindingKind::Raw)
                .is_some()
        {
            return BigramAccess::Packed {
                section: packed.bigrams(),
                binding: crate::index_storage::bigram_builder::BigramBinding::Raw,
            };
        }
        BigramAccess::Native {
            index: self.raw_text,
            binding: crate::index_storage::bigram_builder::BigramBinding::Raw,
        }
    }

    /// Fallback-corpus candidate provider for this collection or segment.
    pub fn fallback_text_access(&self) -> crate::index_storage::access::BigramAccess<'a> {
        use crate::index_storage::access::BigramAccess;
        if let Some(packed) = self.packed
            && packed
                .bigrams()
                .binding(crate::index_storage::bigram_builder::BigramBindingKind::Fallback)
                .is_some()
        {
            return BigramAccess::Packed {
                section: packed.bigrams(),
                binding: crate::index_storage::bigram_builder::BigramBinding::Fallback,
            };
        }
        BigramAccess::Native {
            index: self.fallback_text,
            binding: crate::index_storage::bigram_builder::BigramBinding::Fallback,
        }
    }

    /// Resolve a field text binding to a native or packed candidate provider.
    pub fn field_text_access(
        &self,
        schema: &Schema,
        path: &[String],
        binding: &str,
        named_operation: bool,
        logical_text_complete: bool,
        raw_field_compatible: bool,
    ) -> Option<crate::index_storage::access::BigramAccess<'a>> {
        use crate::index_storage::access::BigramAccess;
        use crate::index_storage::bigram_builder::BigramBinding;
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
        if logical_text_complete && let Some(name) = name {
            let custom = BigramBinding::Custom {
                path: path.to_vec(),
                name: name.to_owned(),
            };
            if let Some(packed) = self.packed
                && packed.bigrams().custom(path, name).is_some()
            {
                return Some(BigramAccess::Packed {
                    section: packed.bigrams(),
                    binding: custom,
                });
            }
            if let Some(index) = self.field_text(path, name) {
                return Some(BigramAccess::Native {
                    index,
                    binding: custom,
                });
            }
        }
        if built_in && !named_operation && raw_field_compatible {
            let binding = BigramBinding::Raw;
            if let Some(packed) = self.packed
                && packed
                    .bigrams()
                    .binding(crate::index_storage::bigram_builder::BigramBindingKind::Raw)
                    .is_some()
            {
                return Some(BigramAccess::Packed {
                    section: packed.bigrams(),
                    binding,
                });
            }
            return Some(BigramAccess::Native {
                index: self.raw_text,
                binding,
            });
        }
        None
    }

    /// An ordered value access needs one complete scalar key or a known
    /// missing/null state for every logical record. This rule is independent
    /// of the producer that populated the posting.
    pub fn sortable_scalar(&self, path: &[String]) -> Option<&'a PostingIndex> {
        self.field_postings(path, "default", false)
            .into_iter()
            .flatten()
            .find(|access| {
                access.admitted
                    && access.access.covered_records() as u64 == self.logical.len()
                    && access.access.value_complete()
                    && !access.access.multi()
                    && matches!(
                        access.access.kind(),
                        LogicalType::Number | LogicalType::Bool | LogicalType::String
                    )
                    && access.empty_selections.is_none_or(RoaringBitmap::is_empty)
                    && access.short_string_coverage.is_none_or(|covered| {
                        let presence = access.access.presence().unwrap_or_default();
                        let nulls = access.access.nulls().unwrap_or_default();
                        presence.difference_len(covered) == nulls.difference_len(covered)
                    })
            })
            .and_then(|access| access.access.native())
    }
}
