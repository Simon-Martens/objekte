//! JSON document sources and JSON Schema configuration for Objekte.

pub mod configuration;
pub mod schema;

use schema as json_schema;

#[cfg(test)]
#[macro_export]
macro_rules! objekte_json_unit_tests {
    ($($module:tt)*) => {};
}

mod document;
pub use document::{JsonDecodedScalarsReader, JsonReader, register};

/// Record decoder and projection primitives shared with the JSON Lines loader.
pub mod records {
    pub use super::document::document::{
        json_reader_failure, parse_json, parse_json_for_query, parse_json_for_query_diagnosed,
        parse_json_for_query_trusted,
    };
    pub use super::document::scan_record;
}

/// Borrowed decoding for complete JSON string tokens.
pub use document::scalars::json_string::{with_next_quoted_text, with_quoted_text};
