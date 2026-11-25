//! Index storage and backend-independent index types.

pub(crate) mod memory;
pub(crate) mod storage;

// Transitional exports during the storage consumer migration.
pub use memory::{bigram, posting, relation, segment, sorting};
