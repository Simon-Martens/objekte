//! Shared record-level query semantics.

use smallvec::SmallVec;
use std::borrow::Cow;
use std::cell::RefCell;
use std::cmp::Ordering;
pub(crate) use std::collections::HashMap;
pub(crate) use std::path::Path;
pub(crate) use std::sync::Arc;

pub(crate) use crate::inventory::{FileEntry, RecordEntry};
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
