//! Desktop-only translation between editable controls and retained core work.

mod adapt;
pub mod filter_validation;
mod lifecycle_navigation;
pub mod open_detection;
mod projection;
pub mod runtime;

pub use adapt::{open_spec, query_spec};
pub use lifecycle_navigation::{PublicationKey, ResultContextKey, TabJobKey};
pub use projection::{
    detail_view, initial_columns, page_view, page_view_with_available, page_view_with_prior,
};
