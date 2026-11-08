//! Desktop-only translation between editable controls and retained core work.

mod adapt;
pub mod filter_validation;
pub mod open_detection;
mod projection;
pub mod runtime;

pub use adapt::{open_spec, query_spec};
pub use projection::{detail_view, page_view, page_view_with_available};
