//! Access planning boundary between semantic predicates and index storage.

#![allow(
    dead_code,
    reason = "the internal provider contract carries the complete capability and set-algebra surface"
)]

pub(crate) mod base;
pub(crate) mod contract;
pub(crate) mod membership;
pub(crate) mod registry;
pub(crate) mod relations;
pub(crate) mod text;

#[cfg(test)]
pub(crate) mod independent;
