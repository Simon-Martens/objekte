# Assignment 6 repair specification — Lifecycle ownership and real worker admission

**Start** on `repair/core-lifecycle-coordination` from accepted R5. Merge frozen A6 `4df92f741782d8556448cac454478057d790416b` (original parent delivery A5 `a8a1db7d89186e7b34d2c18e25c621ba7f1b8cf6`). Preserve R4/R5 behavior and both delivery ancestries. Keep the thirteen original root removals and the existing synchronization protocol.

## Complete the module split

Use paths relative to crates/core/src/lifecycle/. At the reviewed head mod.rs still contains the job data, wait logic, mutation preparation implementation, and operation-specific public types. Move definitions and their impls together. Make public operation modules reachable and use explicit canonical imports in core, CLI, desktop, examples and tests. Eliminate the child's dependency on `use crate::lifecycle::*`; use the minimum visibility needed by siblings. Keep the public Index methods inherent, with their existing parameters and return semantics.

| Current item | Destination |
|---|---|
| ReconciliationState, report, progress, conversion, issue cap | reconciliation/mod.rs |
| MaintenanceState (the reconciliation slot/state), ReconciliationJob, ReconciliationTask and its wait/progress/cancel methods, wait_for_reconciliation from periodic.rs:97 | reconciliation/task.rs |
| PeriodicReconciliation and PeriodicSignal | reconciliation/periodic.rs |
| MaintenanceOperation, Phase, Progress, Report, Task, MaintenanceJob, task and job impls | maintenance/mod.rs |
| RebuildReport | maintenance/rebuild.rs |
| DeleteStrategy, DeleteResult, public delete_file currently in mutation/staging.rs:6 | mutation/mod.rs |
| PreparedAdd, PreparedReplacement, index_prepared_file, prepare_file_at, shared supplied-byte preparation currently in mutation/mod.rs:261 | mutation/preparation.rs |
| stage_prepared_add/replace/delete only | mutation/staging.rs |

Keep IndexInner as the single owner of locks, slots and current publication. Remove compatibility re-exports of inventory::FileVersion, index::segment::PublishedSegment and the initial task family from the lifecycle root; migrate all consumers to their owners. Preserve the specified public IndexView home. Audit remaining root definitions against the original responsibility table. Keep admission-time make_room_for_segment in publication.rs. Do not change worker count, synchronization primitives, cancellation checkpoints, retry limits, segment budgets, or introduce a deep copy. Correct start_compaction rustdoc: a conflicting operation returns the existing busy error immediately.

## Replace the admission test

At maintenance/tests.rs:38–84 the test constructs a Compact job with a Rebuild result and assigns the slot directly. Remove that setup. Add a cfg(test)-only per-index worker-entry hook with a reached signal and a release receiver, invoked by the real spawned maintenance worker before computation and outside admission/result/publication locks. Use a bounded wait for arrival and a cleanup guard that releases on unwind or disconnect. The worker must not panic on release-channel disconnection. No sleeps or global hook. Call start_compaction on an index with a deterministic compactable fixture. Wait for arrival, call start_compaction again, assert same job identity, then call start_rebuild and assert the existing busy error. Release and wait for both handles; assert both return Compact with matching revision/work totals and terminal Complete progress. Check only one worker entered and confirm a different maintenance operation can subsequently be admitted. Use a bounded harness for wait/join diagnostics. Keep or exercise reconciliation coalescing separately, showing its own slot can be observed independently of a held maintenance job. Verify stopping the timer lets a running reconciliation finish; quiesce must stop admission and cancel and wait for owned work.

## Validate the unchanged concurrency contract

Trace and record lock acquisition, scope and drop order at admission, install, notification and join. In particular distinguish the result mutex explicitly dropped before join from the handle-slot guard in an if-let scrutinee. Do not shorten the latter on the assumption that repeated/concurrent waiters already synchronize some other way. Keep joins outside locks the worker needs to complete. Check foreign instance, skipped and stale revision, shutdown refusal, retired-root drop after unlock and old-view catalogue/slot/segment visibility. Keep incomplete scans from inferring deletions and recheck absent paths. Run the named tests in §4 of the original specification, including shared-file atomic deletion, move identity, rebuild retry and unaffected-segment Arc identity. For retry evidence assert that the independent user mutation survives and pinned observations remain stable. Compare retained work, source reads and sharing against A5 if code moves change executable work.

Run the ignored regression with an explicit default-size environment and retain the output including the built count, attempts and final summary:

```sh
env -u OBJEKTE_LARGE_COMPACT_RECORDS cargo test --locked --release -p objekte-search-core --lib large_source_free_compaction_cancels_and_retries_a_concurrent_commit -- --ignored --nocapture
```

Require records=262144, two attempts on the injected conflict and exactly one passing test. Run the focused lifecycle suite in addition to the workspace gates. Amend the completion report with the actual paths, lock note, qualified test names, and exact code/delivery SHAs.

## Integration and acceptance

Use a sibling worktree under /home/simon/source and a new repair branch. Preserve master and all three frozen delivery refs. Record the actual starting commit, original comparison, implementation commits and tested code SHA. Keep the original specification authoritative for unchanged behavior. Use commits that build through normal workspace entry points. Deliver to the owner for review, without merging into master.

Run the following on the final code tree, retain command summaries and exit status, and compare default and all-feature test inventories by function name and multiplicity, accounting for module moves. For every individually filtered test, show the fully qualified test name and the positive number executed. Fix errors within scope before marking the repair ready. In the report distinguish results actually run from historical results. After a report-only commit verify that its only delta is the report and record the final full SHA.

```sh
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
cargo test --locked -p objekte-search-core --all-features
RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc --locked --workspace --no-deps
```

## Implementation report

- **Task:** `core-lifecycle-coordination-repairs`
- **Completion date:** 2026-11-19
- **Starting R5:** `e81a0711417503d55ddd707735b8a31bd72c9338`
- **Final accepted R5 merged:** `18f877b11d3b2565212b41a495d4904a5996d2a8`
- **Frozen A6 merged:** `4df92f741782d8556448cac454478057d790416b`
- **Ordinary merge commits:** `160a17582f52810eb5b745dacb124a161f136856` (A6), `aa3b7628ce2e7d77fd4daa58bca1b9276e787894` (final R5 probe)
- **Repair branch:** `repair/core-lifecycle-coordination`
- **Repair worktree:** `/home/simon/source/objekte-repair-core-lifecycle-coordination`
- **Tested code revision:** `aa3b7628ce2e7d77fd4daa58bca1b9276e787894`
- **Implementation commits:** `160a17582f52810eb5b745dacb124a161f136856`, `d1170ac5b6429783b05e508c3e40c5dc69dd828b`, `aa3b7628ce2e7d77fd4daa58bca1b9276e787894`
- **Original comparison:** [final R5 to tested R6 code](https://github.com/Simon-Martens/objekte/compare/18f877b11d3b2565212b41a495d4904a5996d2a8...aa3b7628ce2e7d77fd4daa58bca1b9276e787894)
- **Status:** complete and ready for review; no merge into `master` was performed

R5 and frozen A6 are both ancestors of this branch. The A6 merge completed
without conflict. The repair preserved the thirteen original root removals,
publication/locking ownership and the specified public `IndexView` home.

The final accepted R5 tip was merged after the A6 ownership repair. Its
reference-entry regression now uses thread-local production prepare/planner
probes and exercises both forward and backlink source truth through
`reference::relations::search_segmented`. Because the R5 follow-up is test-only,
it does not change the lifecycle synchronization contract or production work.

### Completed lifecycle ownership

`lifecycle/mod.rs` is now 407 lines and retains only `Index`, `IndexInner`,
revision/file identity, `IndexError`, top-level status/access observations and
small handle/accessor methods. `IndexInner` remains the sole owner of the
published root, writer gate, quiescing flag, maintenance/reconciliation task
slots, cache and scheduling locks. The direct core Rust-file count remains 48.

| Canonical path | Ownership |
| --- | --- |
| `lifecycle/reconciliation/mod.rs` | `ReconciliationState`, report/progress, issue cap and reconciled pass |
| `lifecycle/reconciliation/task.rs` | `MaintenanceState`, `ReconciliationJob`, `ReconciliationTask`, admission, waits, progress/cancel and `wait_for_reconciliation` |
| `lifecycle/reconciliation/periodic.rs` | `PeriodicReconciliation`, `PeriodicSignal`, weak-index timer and schedule replacement/stop |
| `lifecycle/maintenance/mod.rs` | operation, phase, progress, report, task/job types, compaction/rebuild admission |
| `lifecycle/maintenance/rebuild.rs` | `RebuildReport` and synchronous controlled rebuild |
| `lifecycle/mutation/mod.rs` | public mutations, `DeleteStrategy`, `DeleteResult` and public `delete_file` |
| `lifecycle/mutation/preparation.rs` | `PreparedAdd`, `PreparedReplacement`, source/byte preparation and prepared indexing |
| `lifecycle/mutation/staging.rs` | only add, replace and delete candidate staging |

The former root paths remain removed:

- `lifecycle_catalog.rs` and `.test.rs`
- `lifecycle_initial.rs` and `.test.rs`
- `lifecycle_mutation.rs` and `.test.rs`
- `lifecycle_verified.rs`
- `lifecycle_scan.rs` and `.test.rs`
- `lifecycle_compaction.rs` and `.test.rs`
- `lifecycle_partition.rs` and `.test.rs`

Compatibility re-exports for `inventory::FileVersion`,
`index::segment::PublishedSegment` and the initial task family were removed.
Core, CLI, desktop, examples and tests now import those types from their
canonical module owners.

### Real-worker admission regression

The former synthetic `MaintenanceJob` construction was removed. A cfg(test)-only
per-index worker-entry hook is invoked by the real maintenance thread before
computation and outside admission, result and publication locks. It reports
arrival through a bounded channel and waits for release; channel disconnection
is treated as release rather than a worker panic. The test fixture uses a
deleted source file to create a deterministic retired slot for compaction.

`lifecycle::maintenance::tests::maintenance_admission_shares_only_the_real_worker`
performs the required sequence:

1. Start a real compaction and wait with a 10-second bound for entry.
2. Start the same compaction again and assert `Arc` task identity.
3. Request rebuild while compaction is held and assert the existing
   `ResourceLimit` busy error.
4. Start reconciliation independently, observe its separate current-task slot,
   and wait for it while maintenance remains held.
5. Clear the one-shot hook and release the worker on success or unwinding.
6. Join both waiters through a bounded harness. Both receive `Compact` with the
   same revision, slot total, examined work and byte budget; task progress is
   terminal `Complete`.
7. Confirm no second maintenance worker entered.
8. Start and complete rebuild to prove a different operation is admitted after
   release.

### Synchronization review

Production synchronization primitives, worker count, cancellation checkpoints,
retry limits and segment budgets were retained.

- Maintenance admission holds `maintenance_operation`, checks `quiescing`, then
  briefly locks the active result to distinguish shareable and conflicting
  jobs. The conflicting-operation error is returned immediately.
- Reconciliation admission holds `maintenance` and uses its separate current
  job. The reconciliation worker does not hold admission or publication locks
  during its scan.
- Root installation checks quiescence and expected revision under
  `published.write()`, swaps the root, drops the guard, then retires the prior
  root after unlocking.
- Periodic start/replace/stop serializes through `periodic_transition`. The
  schedule is removed from `periodic`, that handle guard is dropped, and only
  then is the timer worker joined.
- Completion writes terminal progress/result under the existing job mutex and
  notifies the existing condition variable.
- Both task waits explicitly drop the result mutex before joining. The worker
  handle guard in the `if let Some(worker) = ...lock()?.take()` scrutinee retains
  its established scope through the join body; it was not shortened or moved
  earlier.
- Quiesce sets `quiescing` under `published.write()`, releases publication,
  stops the timer, cancels and joins maintenance and reconciliation, and keeps
  joins outside locks required by completing workers. Resume and shutdown keep
  their prior ordering and error/report precedence.

### Validation actually run

The following commands passed at tested code revision
`aa3b7628ce2e7d77fd4daa58bca1b9276e787894`:

```text
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
cargo test --locked -p objekte-search-core --all-features
RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc --locked --workspace --no-deps
cargo test --locked -p objekte-search-core --lib lifecycle::
```

Workspace results included CLI 81 passed; process integration 33 passed and
one ignored; desktop 161 passed and one ignored; all core integration suites;
and one passing compile-fail doc test. Default core was 586 passed and 16
ignored. All-feature core was 587 passed and 16 ignored. The focused lifecycle
suite was 88 passed, six ignored and no failures.

The ignored release regression was run with the environment override removed:

```text
env -u OBJEKTE_LARGE_COMPACT_RECORDS cargo test --locked --release \
  -p objekte-search-core --lib \
  large_source_free_compaction_cancels_and_retries_a_concurrent_commit \
  -- --ignored --nocapture
```

It reported `large compaction build records=262144`, a second attempt at
`records=262144`, `attempts=2`, one retired slot, and:

```text
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 601 filtered out
```

The following existing named tests were each run exactly and each reported one
positive test:

- `lifecycle::tests::pinned_view_stays_stable_when_a_new_root_is_published`
- `lifecycle::tests::publication_rejects_foreign_and_skipped_revisions_without_advancing_status`
- `lifecycle::tests::quiescence_rejects_prepared_publications_until_close_is_cancelled`
- `lifecycle::tests::deleting_shared_multi_record_file_retires_every_collection_atomically`
- `lifecycle::initial::tests::start_build_is_the_single_scan_and_repeated_waits_do_not_rebuild`
- `lifecycle::initial::tests::repository_conversion_rejects_retained_owner_and_view_promptly`
- `lifecycle::initial::tests::late_private_invalidation_and_worker_failure_publish_nothing`
- `lifecycle::tests::stopping_periodic_schedule_does_not_cancel_current_reconciliation`
- `lifecycle::tests::periodic_tick_detects_change_after_initial_index_build`
- `lifecycle::tests::background_reconciliation_reports_completion_and_joins`
- `lifecycle::tests::reconciliation_applies_metadata_changes_without_revising_unchanged_files`
- `lifecycle::tests::rebuild_retries_after_concurrent_file_commit_without_losing_it`
- `lifecycle::tests::background_rebuild_waits_and_cancelled_preparation_never_publishes`
- `lifecycle::maintenance::compaction::tests::background_compaction_preserves_pinned_reader_and_reports_completion`
- `lifecycle::catalog::tests::changed_paths_and_tombstones_do_not_copy_or_rewrite_base_inventory`
- `lifecycle::mutation::tests::compatible_move_keeps_index_contributions_and_advances_file_version`
- `lifecycle::mutation::tests::one_record_shared_file_preserves_every_collection_slot_atomically`
- `lifecycle::maintenance::compaction::tests::compaction_reuses_unaffected_native_segment`
- `lifecycle::session::tests::pinned_adapter_keeps_batch_queries_on_one_publication`

Frozen A6 and tested R6 contain 629 versus 631 default inventory entries and
630 versus 632 all-feature entries. Function-name and multiplicity comparison
shows:

- the intentionally replaced admission test is absent and
  `maintenance_admission_shares_only_the_real_worker` is present once;
- the two accepted R5 regressions remain present once each;
- the rustdoc compile-fail name changed only by its source line from 232 to 233;
- no unrelated test was added, removed or duplicated.

No production benchmark applies: this repair changes ownership and a test hook
without changing executable lock scope, source I/O, copying, worker count,
cancellation, retry or segment policy. The existing shared-segment identity,
pinned-view and rebuild-conflict tests cover the unchanged work contract.

### Review notes

No unresolved functional issue or blocked check remains. The report-only commit
follows tested code revision
`aa3b7628ce2e7d77fd4daa58bca1b9276e787894`; its only delta is completion
documentation. The branch and worktree remain for the owner's review.
