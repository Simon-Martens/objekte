# Desktop scheduler replacement test intermittently observes pending retirement

Category: `bug`. Originating task: `desktop-metadata-filters` (metadata column repair).

Observed on unmodified master `64f3cef393e475788629ead883cb77230d1d8d5d` and the repair integration worktree. Relevant locations: `crates/desktop/src/controller/runtime.test.rs`, test `rapid_replacements_and_toggle_keep_only_latest_scheduler`, and `DesktopController::transfer_check_policy` in `runtime.rs`.

The full desktop suite intermittently fails the assertion that the original index no longer has periodic reconciliation enabled, immediately after receiving the latest replacement's successful `AutomaticCheckChanged` event. Expected: the test's retirement assertions should observe completed retirement of all replaced schedulers.

Evidence from this repair session:

- The first repair desktop suite run failed that assertion; all new metadata regression tests passed.
- An isolated rerun passed.
- After cleaning the desktop package's shared target artifacts and rebuilding from unmodified master, 20 isolated runs passed. The full baseline desktop suite reproduced the same failure on repetition 7 (162 passed, 1 failed, 1 ignored). This confirms the failure exists without the metadata repair.
- Reproduction command: `cargo test --locked -p objekte-desktop`, repeated with normal parallel test execution. The baseline used `--target-dir /home/simon/source/objekte-integration-desktop-metadata-filters/target`; clean this package's artifacts when switching source worktrees so Cargo actually rebuilds the intended baseline.

Hypothesis from inspection: each transfer worker independently acquires `automatic_check_gate`, retires its immediate predecessor, then emits the latest event if still current. Gate acquisition order need not match creation order. The latest worker may emit its event before a superseded worker retires the original index, allowing the assertion to race with that retirement. This has not been established with worker-order instrumentation. A briefly overlapping old scheduler is a possible runtime consequence; only the intermittent test failure was measured.

Impact: nondeterministic validation failure. Resolve separately from metadata projection: decide whether the latest completion must guarantee retirement of the whole predecessor chain, or whether the test should explicitly await the separately scheduled retirements. Do not weaken the assertion without first verifying the intended lifecycle guarantee. No scheduler implementation or existing scheduler test was changed in this repair.
