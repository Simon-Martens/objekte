//! Query syntax binding for FILTER, FTS, relations, and prepared matchers.

pub(crate) use memchr::memmem::Finder;
use std::collections::HashMap;
pub(crate) use std::sync::Arc;

pub(crate) use crate::index::bigram::BigramRequest;
pub(crate) use crate::model::text::fold;
pub(crate) use crate::model::{
    CollectionId, FieldPath, LogicalType, MetadataField, Number, PathRoot, Value,
};
pub(crate) use crate::query::path::{PathStep, QueryPath};
pub(crate) use crate::query::regex::CompiledRegex;
pub(crate) use crate::query::semantics::comparison::{comparable_logical_types, compare_scalar};
pub(crate) use crate::query::semantics::folding::{
    with_folded_candidate, with_folded_candidate_str,
};
pub(crate) use crate::query::semantics::state::StatePredicate;
pub(crate) use crate::query::temporal::TemporalBounds;
pub(crate) use crate::query::{
    CaseSensitivity, FilterExpr, FtsField, FtsMode, FtsQuery, Literal, Modifier, Operand, Operator,
    Quantifier, Term,
};
pub(crate) use crate::relation::{RelationCardinality, RelationId};
pub(crate) use crate::repository::Repository;
pub(crate) use crate::schema::{
    BindingOperation, FieldDefinition, PreparedFilterMatcher, PreparedTextMatcher, Schema,
    Transform,
};
pub(crate) use crate::source::TextDerivation;
pub(crate) use crate::source::TextSegment;

pub(crate) const MAX_RELATION_HOPS: usize = 8;
pub(crate) const MAX_PATH_STEPS: usize = 64;

pub mod filter;
pub mod fts;
pub(crate) mod matcher;
pub mod relation;

pub(crate) use filter::*;
pub(crate) use fts::*;
pub(crate) use matcher::*;
