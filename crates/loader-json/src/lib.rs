//! JSON document sources and JSON Schema configuration for Objekte.

pub mod configuration;
pub mod parameters;
pub mod schema;

use schema as json_schema;

// Core includes these sources in its unit tests, where private query internals
// and their instrumentation are available.
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

use std::path::PathBuf;

use objekte_search_core::repository::configuration::{CollectionDefinition, IdentityPolicy};
use objekte_search_core::source::SourceFormat;

/// Select the JSON document reader with its historical configuration identity.
pub const JSON_DOCUMENTS: SourceFormat = SourceFormat::with_compatibility_encoding("json", "Json");

/// Select the JSON reader that indexes decoded lexical scalar text.
pub const JSON_DECODED_SCALARS: SourceFormat =
    SourceFormat::with_compatibility_encoding("json_decoded_scalars", "JsonDecodedScalars");

/// Define a JSON folder with relative-path record IDs.
pub fn folder(name: impl Into<String>, source_path: impl Into<PathBuf>) -> CollectionDefinition {
    CollectionDefinition::folder_with_identity(
        name,
        source_path,
        JSON_DOCUMENTS,
        IdentityPolicy::RelativePath,
    )
}

/// Define a decoded-scalar JSON folder with relative-path record IDs.
pub fn decoded_scalar_folder(
    name: impl Into<String>,
    source_path: impl Into<PathBuf>,
) -> CollectionDefinition {
    CollectionDefinition::folder_with_identity(
        name,
        source_path,
        JSON_DECODED_SCALARS,
        IdentityPolicy::RelativePath,
    )
}

pub use document::value::{number_from_json, value_from_json};

#[cfg(test)]
extern crate self as objekte_loader_json;
