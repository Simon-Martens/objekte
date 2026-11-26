//! Pinned relation lookup, work accounting and exact evaluation.

pub(crate) mod evaluation;
pub(crate) mod target_cache;
pub(crate) mod view;
pub(crate) mod work;

#[cfg(test)]
mod tests;
