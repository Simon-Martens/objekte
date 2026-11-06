//! Terminal entry point for a repository described by optional schema files.

mod schema;
mod tui;

use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn main() {
    let started = Instant::now();
    let diagnostic = std::env::args().any(|arg| arg == "--diagnostic" || arg == "--diagnostics");
    match run() {
        Ok(()) if diagnostic => write_diagnostic("info", "command completed", started),
        Ok(()) => {}
        Err(error) => {
            if diagnostic {
                write_diagnostic("error", &error, started);
            } else {
                eprintln!("{error}");
            }
            std::process::exit(2);
        }
    }
}

fn write_diagnostic(level: &str, message: &str, started: Instant) {
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_millis());
    let diagnostic = serde_json::json!({
        "timestamp_ms": timestamp_ms,
        "elapsed_ms": started.elapsed().as_secs_f64() * 1_000.0,
        "level": level,
        "message": message,
    });
    eprintln!("{}", serde_json::to_string_pretty(&diagnostic).unwrap());
}

fn run() -> Result<(), String> {
    let mut root = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--help" | "-h" => {
                println!(
                    "Usage: objekte [REPOSITORY] [tui] [--diagnostic]\nOpen the interactive search UI for a repository with .objekte/collections/*.json schemas."
                );
                return Ok(());
            }
            "--version" | "-V" => {
                println!("objekte {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "tui" | "--diagnostic" | "--diagnostics" => {}
            value if value.starts_with('-') => return Err(format!("unknown option {value}")),
            value if root.is_none() => root = Some(PathBuf::from(value)),
            value => return Err(format!("unexpected argument {value}")),
        }
    }
    let root = root.unwrap_or_else(|| PathBuf::from("."));
    let definitions = schema::load_definitions(&root)?;
    if definitions.is_empty() {
        return Err(format!("no collection schemas found in {}", root.display()));
    }
    tui::run(root, definitions).map_err(|error| error.to_string())
}
