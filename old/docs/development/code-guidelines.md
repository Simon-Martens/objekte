# Code guidelines

These conventions apply to the active Rust workspace.

- Keep vertical space: one empty line between functions and two empty lines before and after an `impl` block. Run `python3 scripts/format-rust-spacing.py` after formatting, then check with `python3 scripts/format-rust-spacing.py --check`.
- Give every struct a short comment explaining its role. Comment difficult code paths, invariants, and gotchas where the reason is not clear from the code.
- Keep tests in files separate from production code. Unit tests can be loaded with `#[cfg(test)]` and `#[path = "name.test.rs"]`; integration tests belong under a crate's `tests/` directory.
- Split subsystems into sensible modules. Keep distinct parts of a subsystem in separate files when that makes their responsibilities easier to follow.
- `--diagnostic` and `--diagnostics` emit diagnostics to stderr as pretty-printed JSON. Each diagnostic event includes a Unix millisecond timestamp, elapsed command time in milliseconds, and a log level. Successful commands report `info`; failures report `error` and a message. Keep stdout for command results.

Run the workspace tests and Clippy before committing changes that affect behavior.
