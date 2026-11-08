# Desktop index lifecycle validation

Task: `desktop-index-lifecycle`

Date: 2026-11-08

Branch: `integration/desktop-index-lifecycle`

Final implementation revision: `c74268725c`

Desktop UI base: `9a94bc90d0`
Core API prerequisite: `integration/index-lifecycle` at `a39434ca00`

## Automated checks

| Command | Result |
| --- | --- |
| `cargo test --locked -p objekte-desktop --lib --quiet` | 109 passed |
| `cargo test --locked --workspace --all-targets` | Passed, including core, CLI, TUI, and desktop targets |
| `cargo check --locked -p objekte-desktop --bin objekte-desktop --quiet` | Passed |
| `git diff --check` | Passed |

The repository-wide `cargo fmt --all --check` and `python3 scripts/format-rust-spacing.py --check` find thousands of pre-existing layout differences, including files outside this task. The touched Rust files were formatted with `rustfmt --edition 2024 --config skip_children=true`; no blanket formatting commit was made.

## Native interaction evidence

Ran the actual GPUI binary in the maintained headless Sway session on Fedora 44, x86_64. The output was verified at **1920 × 1080, scale 1.0, 60 Hz**, with loopback WayVNC. Screenshots are raw output captures under [screenshots](screenshots/). The application was also tested as a floating **1100 × 700** window inside that output. Generated fixtures were stored under the ignored `target/gui-validation/desktop/` directory, not in repository source data.

| Action and state | Evidence |
| --- | --- |
| Detect a plain folder with read-only, in-memory defaults; open 180 JSON records | [open-plain-detected.png](screenshots/open-plain-detected.png) |
| Detect a configured 3,000-file repository, its collection schema and selected cache path | [open-configured.png](screenshots/open-configured.png) |
| Search a cached 3,000-record view before the initial file check finishes | [cached-open-immediate.png](screenshots/cached-open-immediate.png) |
| Inspect resolved load/save paths, recurring check/save controls, and maintenance actions | [configured-index-scrolled.png](screenshots/configured-index-scrolled.png), [auto-save-click2.png](screenshots/auto-save-click2.png) |
| Change source data and observe an outdated record tab, then refresh by canonical identity | [stale-tab.png](screenshots/stale-tab.png), [tab-refreshed-done.png](screenshots/tab-refreshed-done.png) |
| Remove a source record and retain the tab with an unavailable-record state | [tab-removed.png](screenshots/tab-removed.png) |
| Make the source directory unreadable, run Check for changes, and keep the searchable workspace with a partial-check warning | [partial-dir-actual.png](screenshots/partial-dir-actual.png) |
| Make the derived-cache directory unwritable during automatic final save; show a local Retry/Cancel/Close without saving choice while retaining the workspace; restore permissions and retry successfully | [close-save-error.png](screenshots/close-save-error.png) |
| Inspect the scrollable Index panel in a smaller window in light and dark themes | [small-light-index.png](screenshots/small-light-index.png), [small-menu-theme.png](screenshots/small-menu-theme.png) |
| Reopen the 3,000-record fixture in the final binary and use the Index panel | [final-revision-workspace.png](screenshots/final-revision-workspace.png), [final-revision-index.png](screenshots/final-revision-index.png) |
| Build the 148,486-record Musenalm fixture in memory and query it in the native app | [musenalm-in-memory-workspace.png](screenshots/musenalm-in-memory-workspace.png) |
| Expand the disposable configured fixture to 100,000 JSON files, start Rebuild, execute the `Configured` query while maintenance remains active, and reopen the panel to observe both the completed query and running rebuild | [concurrent-start.png](screenshots/concurrent-start.png), [concurrent-query.png](screenshots/concurrent-query.png), [concurrent-status.png](screenshots/concurrent-status.png) |

The configured fixture began with 3,000 files and was later expanded to 100,000 generated JSON files for the concurrent rebuild check. Its source data was changed only to exercise refresh, deletion, and failure paths; it was disposable. The unreadable directory and cache permissions were restored afterward. Screenshot capture latency is excluded from query timing. The cached view screenshot reports a 1.6 ms query while the footer still reads “Checking files...”. The 100,000-file screenshot reports a 357.2 ms debug query with 99,999 matches while the Index panel still reports a running rebuild. These are interaction observations, not comparable release benchmarks.

## Release benchmark comparison

Built with `cargo build --locked --release -p objekte-search-core --example bench_lifecycle_mutations`, then ran the existing `target/release/examples/bench_lifecycle_mutations small` three times from each worktree: core prerequisite `a39434ca00` and desktop integration `94cbba9b25`. The final `c74268725c` commit changes only desktop status notifications, so it does not change this core benchmark's code. Same host, profile, fixture generation, and command; raw TSVs are in [benchmark](benchmark/). The fixture has three initial records and is useful as a smoke comparison, not a desktop interaction benchmark.

| Phase | Core base median | Desktop integration median |
| --- | ---: | ---: |
| Build | 1.591 ms | 1.657 ms |
| Add | 0.139 ms | 0.140 ms |
| Append update | 0.088 ms | 0.082 ms |
| Verified update | 0.092 ms | 0.092 ms |
| Move | 0.076 ms | 0.073 ms |
| Verified delete | 0.029 ms | 0.027 ms |
| Compact | 1.334 ms | 1.243 ms |
| Flush | 1.197 ms | 1.204 ms |
| Restore to searchable | 0.732 ms | 0.614 ms |
| No-change reconciliation | 0.033 ms | 0.039 ms |

Peak measured RSS over the tiny example runs was 9,172 KiB at the core base and 9,272 KiB at the desktop revision. These differences are small relative to run-to-run noise. The core task's separate [large-fixture performance report](../index-lifecycle-performance-validation.md) covers the full Musenalm lifecycle and query suite. The previous desktop UI has no equivalent maintenance controls, so there is no directly comparable native “query during maintenance” timing from the UI base. Native 3,000-record query times observed in the debug app were about 1.6–4.6 ms, with no SSH screenshot time included.

## Coverage limits

Native captures validate visible interaction states. Deterministic unit tests cover publication identities, pinned query/output views, stale tab resolution, cancellation, save ownership, and shutdown errors. The 148,486-record debug run exposed repeated GPUI redraws from an unchanged status emitted every 100 ms. Commit `c74268725c` suppresses that event; a focused test verifies that an unchanged status produces no update while a changed status still does. The final binary was reopened and used on the 3,000-record and expanded 100,000-record fixtures, including a query during rebuild. A second 148,486-record debug build was not run after the redraw fix. The toolkit's screen-reader announcements could not be inspected through the headless Sway protocol; the controls use GPUI button, checkbox, and accessibility labels where exposed. No claim is made that a forced process kill completes an automatic final save.
