//! Access planning boundary between semantic predicates and index storage.

pub(crate) mod contract;
pub(crate) mod membership;
pub(crate) mod native;
pub(crate) mod registry;
pub(crate) mod relations;
pub(crate) mod segment;
pub(crate) mod text;

#[cfg(test)]
pub(crate) mod independent;
