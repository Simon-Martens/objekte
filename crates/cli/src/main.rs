mod execution;

use std::io;
use std::time::{Instant, SystemTime};

use clap::Parser;
use objekte_search_core::streaming::CancellationToken;

use objekte_cli::arguments::Arguments;

fn main() {
    let args = Arguments::parse();
    let started = Instant::now();
    let token = CancellationToken::new();
    let signal_token = token.clone();
    if let Err(error) = ctrlc::set_handler(move || signal_token.cancel()) {
        eprintln!("obj: signal handler: {error}");
        std::process::exit(2);
    }
    let stdout = io::stdout();
    let exit = execution::run(&args, stdout.lock(), &token);
    if args.diagnostic {
        let timestamp_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis());
        let success = matches!(exit, execution::Exit::Success | execution::Exit::PipeClosed);
        let diagnostic = serde_json::json!({
            "timestamp_ms": timestamp_ms,
            "elapsed_ms": started.elapsed().as_millis(),
            "level": if success { "info" } else { "error" },
            "message": if success { "obj command completed" } else { "obj command failed" },
        });
        if let Ok(text) = serde_json::to_string_pretty(&diagnostic) {
            eprintln!("{text}");
        }
    }
    std::process::exit(match exit {
        execution::Exit::PipeClosed => 0,
        execution::Exit::OutputError => 1,
        other => other as i32,
    });
}
