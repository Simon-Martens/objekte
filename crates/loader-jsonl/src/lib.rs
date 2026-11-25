//! JSON Lines source registration for Objekte.

mod framing;
mod reader;
use framing as jsonl_framing;
pub use reader::{JsonLinesReader, register};

use objekte_search_core::source::SourceFormat;

/// Select JSON Lines with its historical configuration identity.
pub const JSON_LINES: SourceFormat =
    SourceFormat::with_compatibility_encoding("jsonl", "JsonLines");
