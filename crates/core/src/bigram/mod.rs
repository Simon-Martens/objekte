//! Content-fed bigram indexing adapted from FFF (MIT license).
//!
//! The builder, compressed filter, overlay, and original tests live in this
//! module so Objékte does not depend on FFF's file walker or path storage.
//! See `LICENSE` and `NOTICE.md` for provenance.

mod codec;
mod column_slab;
mod filter;
mod snapshot;

use column_slab::ColumnSlab;
pub use filter::{
    BIGRAM_KEY_SLOTS, BigramEncoding, BigramFilter, BigramIndexBuilder, BigramOverlay,
    SKIP_INDEX_MIN_DENSITY_PCT, build_bigram_index_from_content,
    build_bigram_index_from_content_with_encoding, extract_bigrams, extract_bigrams_with_encoding,
    release_thread_buffers,
};
pub use snapshot::{BigramFile, BigramSnapshot};
