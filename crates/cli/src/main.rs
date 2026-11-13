use std::io;
#[cfg(feature = "detailed-profiling")]
use std::time::Instant;
use std::time::SystemTime;

use clap::Parser;
use objekte_search_core::streaming::CancellationToken;

use objekte_cli::arguments::Arguments;
use objekte_cli::execution;
use objekte_cli::output::milliseconds_value;

fn main() {
    let args = Arguments::parse();
    #[cfg(feature = "detailed-profiling")]
    let started = Instant::now();
    let token = CancellationToken::new();
    let signal_token = token.clone();
    if let Err(error) = ctrlc::set_handler(move || signal_token.cancel()) {
        eprintln!("obj: signal handler: {error}");
        std::process::exit(2);
    }
    let exit = if args.tui {
        #[cfg(feature = "tui")]
        {
            execution::run_tui(&args, &token)
        }

        #[cfg(not(feature = "tui"))]
        {
            eprintln!("obj: --tui support was not built");
            execution::Exit::Arguments
        }
    } else {
        let stdout = io::stdout();
        execution::run(&args, stdout.lock(), &token)
    };
    if args.diagnostic {
        let timestamp_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or_else(
                |_| milliseconds_value(0),
                |duration| milliseconds_value(duration.as_nanos()),
            );
        let success = matches!(exit, execution::Exit::Success | execution::Exit::PipeClosed);
        #[allow(unused_mut)]
        let mut diagnostic = serde_json::json!({
            "timestamp_ms": timestamp_ms,
            "level": if success { "info" } else { "error" },
            "message": if success { "obj command completed" } else { "obj command failed" },
        });
        #[cfg(feature = "detailed-profiling")]
        {
            diagnostic["elapsed_ms"] = milliseconds_value(started.elapsed().as_nanos());
        }
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
