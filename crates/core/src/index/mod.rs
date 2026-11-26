//! Logical index configuration, identities, and inspection types.
//!
//! Storage layouts stay private to their backend.
//!
//! ```compile_fail
//! use objekte_search_core::index::memory;
//! ```
//!
//! ```compile_fail
//! use objekte_search_core::index::bigram::BigramIndex;
//! ```
//!
//! ```compile_fail
//! use objekte_search_core::inventory::FileInventory;
//! ```
//!
//! ```compile_fail
//! fn posting_dictionary(collection: &objekte_search_core::repository::state::CollectionState) {
//!     let _ = &collection.postings;
//! }
//! ```

#[cfg(not(test))]
mod memory;
#[cfg(test)]
pub(crate) mod memory;
pub(crate) mod storage;

#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub use memory::bigram::assert_equivalent_logical_bigrams;

#[cfg(test)]
pub use memory::{bigram, posting, relation, segment, sorting};

#[cfg(not(test))]
pub mod bigram {
    pub use super::storage::text::{BIGRAM_VERSION, BigramAlphabet, FOLDED_REPRESENTATION};
}

#[cfg(not(test))]
pub mod posting {
    pub use super::storage::posting::{ScalarKey, SelectionSummary};
}

#[cfg(not(test))]
pub mod relation {
    pub use super::storage::relation::{RelationEdge, RelationTarget};
}

#[cfg(not(test))]
pub mod segment {
    pub use super::storage::component::{
        ComponentDescriptor as NativeComponentDescriptor, ComponentKind as NativeComponentKind,
    };
    pub(crate) use super::storage::segment::SegmentRecordAddress;
    pub use super::storage::segment::{NativeSegmentSummary, SegmentId, SegmentPolicy};
}

#[cfg(not(test))]
pub mod sorting {
    pub(crate) use super::storage::order::simple_composite_part;
    pub use super::storage::order::{
        COLLATION_VERSION, CollationStrength, CompositeSortPart, Placement, SortConfig, SortKey,
        SortOptions,
    };
}
