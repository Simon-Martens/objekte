//! Command-line entry point.
mod args;
mod commands;
mod diagnostics;
mod output;

fn main() {
    let started = std::time::Instant::now();
    if let Err(e) = commands::run() {
        if std::env::args().any(|arg| arg == "--diagnostic" || arg == "--diagnostics") {
            diagnostics::write_error(&e.to_string(), started.elapsed().as_secs_f64() * 1_000.0);
        } else {
            eprintln!("{e}");
        }
        std::process::exit(2)
    }
}
