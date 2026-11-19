//! Typed query binding and independent, index-free reference evaluation.

pub mod filter;
pub mod fts;

pub(crate) mod relations;

pub(crate) use crate::query::binding::*;
pub(crate) use crate::query::semantics::*;
pub use filter::*;
pub use fts::*;

#[cfg(test)]
mod tests;
