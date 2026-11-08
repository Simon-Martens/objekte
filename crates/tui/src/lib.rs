//! Reusable terminal screen for an already configured repository loader.

mod tui;

pub use tui::export;
pub use tui::settings;
pub use tui::{TuiOptions, run_with_loader};
