//! Content-fed bigram indexing: builder, compressed filter, changed-file
//! overlay, and snapshots.
//!
//! `column_slab.rs` is adapted from FFF and remains under the MIT license;
//! see `LICENSE` and `NOTICE.md`.

mod codec;
mod column_slab;
mod filter;
mod snapshot;

use column_slab::ColumnSlab;
pub(crate) use filter::PostingChunkAccumulator;
pub use filter::{
    BIGRAM_KEY_SLOTS, BigramEncoding, BigramFilter, BigramIndexBuilder, BigramOverlay,
    SKIP_INDEX_MIN_DENSITY_PCT, build_bigram_index_from_content,
    build_bigram_index_from_content_with_encoding, extract_bigrams, extract_bigrams_with_encoding,
    release_thread_buffers,
};
pub use snapshot::{BigramFile, BigramSnapshot};
pub(crate) use snapshot::{BigramFileRef, CompactSnapshot, SnapshotFiles};
