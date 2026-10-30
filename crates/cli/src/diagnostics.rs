//! Structured CLI diagnostic output.

use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};


/// Current Unix epoch time in milliseconds for diagnostic events.
pub(super) fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// Report a command failure in the same pretty JSON envelope as successful diagnostics.
pub(super) fn write_error(message: &str, command_elapsed_ms: f64) {
    let event = json!({
        "type": "diagnostics",
        "level": "error",
        "timestamp_unix_ms": now_unix_ms(),
        "command_elapsed_ms": command_elapsed_ms,
        "message": message,
    });
    eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
}
