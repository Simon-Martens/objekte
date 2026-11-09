# Build warnings, cache recovery, and desktop list controls

## Task

Authorized in chat: remove Rust build warnings; delete and rebuild invalid caches; remove the redundant inspector close control; use icons for opening a query and opening a relation in a new tab; put expand indicators on the right of expandable inspector fields and metadata; show a scrollbar on the results list.

## Implementation report

- Task identifier: `build-warning-cleanup`
- Completed: 2026-11-09
- Base commit: `7486cebe00d6e55efd628c2e32d7a8afeb785d34`
- Integration branch: `integration/build-warning-cleanup`
- Integration worktree: `/home/simon/source/objekte-integration-build-warning-cleanup`
- Implementation commit: `2269ac93`
- Subtasks and contributing branches: none

Unused code confined to tests is compiled only for tests, an unused desktop state field was removed, and the standalone example support module permits its intentionally varying unused helpers. `cargo build --locked --workspace --all-targets` now completes without Rust warnings.

Configured core, CLI, and desktop opening replace a corrupt or incompatible selected cache after successfully rebuilding from source. Deletion checks the cache destination against indexed data and protected configuration files; access errors and unsafe destinations remain errors. A missing explicitly required cache still fails. Tests cover replacement, subsequent restoration, and a protected hardlink.

The inspector retains its record tab close control. Its redundant close button was removed. Open query and relation new tab use icons with tooltips and accessibility labels. Metadata and expandable fields have right aligned chevrons. The results table has an always visible vertical scrollbar connected to its virtualized list.

Validation:

- `cargo test --locked --workspace --all-targets`: passed.
- `cargo build --locked --workspace --all-targets`: passed with no Rust warnings.
- `git diff --check`: passed.
- Native desktop on headless Sway at 1920 × 1080, 100% scale, 60 Hz: opened a 131-record fixture; confirmed the results scrollbar moved with vertical scrolling while columns stayed fixed; confirmed inspector icons and metadata chevron. Screenshots are in the integration worktree's ignored `target/gui-validation/desktop/` directory.
- No existing performance benchmark targets warning removal, rejected-cache recovery, or these UI controls. The normal valid-cache and query paths are unchanged; no performance comparison was run.

No known remaining in-scope issue. The original `master` checkout was not changed. The integration worktree is retained for review; temporary task memory was removed after this report was pushed.
