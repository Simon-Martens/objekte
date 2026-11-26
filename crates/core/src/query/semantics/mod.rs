//! Shared record-level query semantics.

use smallvec::SmallVec;
use std::borrow::Cow;
use std::cell::RefCell;
use std::cmp::Ordering;
pub(crate) use std::collections::HashMap;
pub(crate) use std::path::Path;
pub(crate) use std::sync::Arc;

pub(crate) use crate::inventory::FileEntry;
pub(crate) use crate::model::text::{fold, fold_into};
pub(crate) use crate::model::{FieldPath, LogicalType, MetadataField, Number, PathRoot, Value};
pub(crate) use crate::query::binding::filter::{
    BoundFieldOperand, BoundTerm, Derived, explicit_scope,
};
pub(crate) use crate::query::path::{PathStep, QueryPath};
pub(crate) use crate::query::{Operator, Quantifier};
pub(crate) use crate::schema::{BindingOperation, Transform};

pub(crate) mod boolean;
pub mod comparison;
pub(crate) mod filter;
pub mod folding;
pub mod record;
pub(crate) mod state;
#[cfg(test)]
pub(crate) mod test_matcher;

pub(crate) fn self_comparison_shape(term: &crate::query::binding::filter::BoundTerm) -> bool {
    use crate::model::LogicalType;
    use crate::query::binding::filter::Derived;
    use crate::query::{Operator, Quantifier};
    use crate::schema::operation::Transform;

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
