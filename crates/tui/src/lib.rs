//! Reusable terminal screen for an already configured repository loader.

mod tui;

pub use tui::export;
pub use tui::relations;
pub use tui::settings;
pub use tui::{TuiOptions, run_with_index_loader, run_with_loader};
pub use tui::{record_json_value, record_raw_text};
