//! The engine and external adapters share one Unicode fold implementation.

#[cfg(test)]
pub(crate) use crate::adapter::helpers::case_fold::folded_chars;
pub(crate) use crate::adapter::helpers::case_fold::{fold, fold_into};
