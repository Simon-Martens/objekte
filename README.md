# Objekte

Objekte is being rebuilt as a Rust library with a command line application. The library is intentionally empty for now. The CLI is the first interface for developing and exercising library functions before a new UI is built.

The `examples/` directory contains sample file repositories with JSON Schema and UI configuration. The previous Rust backend, its web assets, and their documentation are preserved in [`deprecated/`](deprecated/README.md) as design and behavior references. The former `frontend/` directory contains generated artifacts only; its source was already absent from the repository.

## Current workspace

| Path | Purpose |
| --- | --- |
| `crates/core` | New Rust library; no domain code yet |
| `crates/cli` | `objekte` command line entry point |
| `examples` | Sample repositories for future CLI commands |
| `deprecated` | Archived backend, web interface, and documentation |

Run the CLI scaffold with `cargo run -p objekte-cli -- --help`. Check the new workspace with `cargo check --workspace`.
