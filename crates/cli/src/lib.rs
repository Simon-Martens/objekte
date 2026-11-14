//! Input and output boundaries shared by the `obj` command and its tests.

pub mod arguments;
pub mod execution;
pub mod loader;
pub mod output;
pub mod query_input;
pub mod storage_view;
#[cfg(feature = "tui")]
pub mod tui;
