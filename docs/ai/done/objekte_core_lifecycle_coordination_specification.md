# Assignment 6 — Index lifecycle and maintenance coordination

**Task** `core-lifecycle-coordination`
**Branch** `integration/core-lifecycle-coordination`
**Worktree** `/home/simon/source/objekte-integration-core-lifecycle-coordination`
**Starting commit** **A5**, the exact pushed and validated delivery head of Assignment 5. The source inventory below was checked on master `1eef51b12c9dad55c2b9b7e603b888412cb51ad5`. Use the layout and import paths actually present at A5.

## Overnight handoff

Complete Assignments 4 and 5 first. Record the original master base **B**, the frozen Assignment 4 head **A4**, and the frozen Assignment 5 head **A5**. After 5 passes its acceptance checks and its completion report is pushed, create this assignment’s branch and sibling worktree from A5. Proceed without waiting for a master merge. For a new branch, set `A5` to that full SHA and run from the original checkout:

```sh
git worktree add -b integration/core-lifecycle-coordination ../objekte-integration-core-lifecycle-coordination "$A5"
```

Resume an existing task branch by inspecting its history, worktree and pending work. Leave both frozen parent branches and the original checkout intact. Use `docs/ai/README.md` and push after each commit. If 5 is blocked, report the dependency and the last passing SHA; 6 remains pending. If morning arrives mid-task, record the checked commits and outstanding validation. The owner reviews and merges in order.

## Outcome

A maintainer investigating a file replacement should be able to follow preparation, staging and publication. Someone changing whether a background job starts, coalesces or stops should work in the task and scheduling modules. The existing `Index` handle, pinned `IndexView` and shared published root continue to supply the lifetime and synchronization model.

The current crate root has seven lifecycle implementation files and six companion tests outside `lifecycle/`. Move those thirteen files into their owners and remove the former module declarations. On the planned stack this takes the direct Rust file count from **61 to 48**. Count the named removals even if unrelated changes shift the baseline. Rewrite the stale `lifecycle/mod.rs` module documentation to describe the operations now implemented. [Current lifecycle module](https://github.com/Simon-Martens/objekte/blob/1eef51b12c9dad55c2b9b7e603b888412cb51ad5/crates/core/src/lifecycle/mod.rs) · [Publication boundary](https://github.com/Simon-Martens/objekte/blob/1eef51b12c9dad55c2b9b7e603b888412cb51ad5/crates/core/src/lifecycle/publication.rs)

## 1. Move cohesive files first

Destinations are relative to `crates/core/src/lifecycle/`. Update core, maintained downstream callers, examples, tests and current rustdoc in each compiling commit. Use ordinary module declarations and direct imports. Delete old names rather than installing compatibility aliases.

| Existing root module | Canonical destination | Responsibility |
| --- | --- | --- |
| `lifecycle_catalog.rs` | `catalog.rs` | `FileCatalog`, path/ID lookup, stamps, configuration identity, live/tombstoned memberships and persistent sharing. |
| `lifecycle_initial.rs` | `initial.rs` | `InitialBuildTask`, cursor/progress/wait/cancel, early catalogue publication and `Index::start_build`. Construction workers remain under `construction/`. |
| `lifecycle_mutation.rs` | `mutation/mod.rs` | Public add, update, move, old-content delete, strategy selection, retries and mutation result types. |
| `lifecycle_verified.rs` | `mutation/verified.rs` | Old-content verification, fallback reasons at existing API boundaries, slot-preserving replacement and removal in a private candidate root. |
| `lifecycle_scan.rs` | `reconciliation/scan.rs` | Batched metadata traversal, file stamps, change classification, cancellation and completeness of each collection scan. |
| `lifecycle_compaction.rs` | `maintenance/compaction.rs` | Explicit compaction, budgets, work accounting, progress and bounded optimistic retry. |
| `lifecycle_partition.rs` | `maintenance/compaction/partition.rs` | Partition orchestration, current file membership, bounds and cancellation. Physical slot transforms remain under `index/segment/`. |

Move each existing companion suite to `<owner>/tests.rs` (for `mutation/mod.rs` use `mutation/tests.rs`). Verified mutation already has coverage in the mutation suite. Group existing cross-owner public regressions in `lifecycle/tests.rs`; put tests requiring private worker state beside its owner. Preserve all ignored tests, feature gates, fixtures and assertions. Keep `session.rs`, `view.rs` and `publication.rs` as the owners already established.

## 2. Split the remaining responsibilities in lifecycle/mod.rs

| Destination | Move these existing operations |
| --- | --- |
| `mod.rs` | Keep `Index`, `IndexInner`, revision and file-handle identities, `IndexError`, top-level status and small handle/accessor methods. `IndexInner` remains the sole owner of the published root, writer gate, quiescing flag, task slots, cache and scheduling locks. Keep public result/observation types with their real operation owners. |
| `open.rs` | `Index::from_repository`, private `from_published` and the test-only consuming conversion `into_repository`. Preserve instance IDs, adoption of completed segments, timing initialization and exclusive ownership errors. This file initializes an index from existing state. |
| `mutation/preparation.rs` | `PreparedAdd`, `PreparedReplacement`, `prepare_file_at`, `index_prepared_file`, `prepare_add_file`, `prepare_replace_file` and the supplied-byte preparation helper where its callers share that work. Prepare against a captured view. Keep source reads and extraction outside the publication lock. |
| `mutation/staging.rs` | `stage_prepared_add`, `stage_prepared_replace` and `stage_prepared_delete`. Return the existing private candidate and expected revision/results. The public `delete_file` method belongs in `mutation/mod.rs` with its current writer-lock scope. |
| `reconciliation/mod.rs` | Reconciliation state, progress/report types, issue cap, `reconcile_pass_with_progress`, its test wrapper and `apply_reconciliation_change`. The pass determines what change to apply, calls the mutation/publication operations and records each outcome. |
| `reconciliation/task.rs` and `reconciliation/periodic.rs` | Put `ReconciliationJob`, `ReconciliationTask`, its current-task slot/state and `start_reconcile`, current/wait methods in `task.rs`. Put `PeriodicReconciliation`, signals, start/replace/stop, enabled query and timer thread in `periodic.rs`. Keep the test tick available to the corresponding suite. |
| `maintenance/mod.rs` and `maintenance/rebuild.rs` | The parent owns `MaintenanceOperation`, `MaintenancePhase`, progress/report, `MaintenanceTask`, private job state, `start_compaction`, `start_rebuild` and `start_maintenance`. Put `RebuildReport`, synchronous rebuild and its control/before-publication helpers in `rebuild.rs`. The scheduler starts work; the rebuild and compaction implementations prepare and attempt publication. |
| `service.rs` | `Index::quiesce`, `resume_after_quiesce` and `shutdown`. Preserve their ordering across the publication flag, periodic scheduler and active tasks. Keep `shutdown`’s existing delegation to `quiesce`. |

Use inherent methods on `Index` across the child modules. The external lifecycle operations retain their call signatures. Namespaced types have one canonical import, e.g. `lifecycle::initial::InitialBuildTask` and `lifecycle::maintenance::MaintenanceTask`. Preserve the existing single public home for `IndexView` when the implementation module is private. Use `inventory::FileVersion` and `index::segment::PublishedSegment` directly in migrated callers. Restrict visibility for sibling access to `pub(super)` or `pub(in crate::lifecycle)` where sufficient. Keep the admission-time consolidation policy in `publication.rs` with `make_room_for_segment`. It is invoked during staging and has different triggers from an explicit compaction job.

## 3. Concurrency and work bounds

1. **Publication.** Stage a complete root against a captured revision. Preserve foreign-instance and next-revision validation. Inside the publication write lock, recheck quiescence and the expected revision, then swap the root. Drop the retired root after unlocking. A pinned view continues to read exactly its prior catalogue, active slots and segments. A shared file changing in several collections publishes once for that file. No additional deep copy of segment payloads, caches, query results or postings is justified by this split. [Root installation](https://github.com/Simon-Martens/objekte/blob/1eef51b12c9dad55c2b9b7e603b888412cb51ad5/crates/core/src/lifecycle/publication.rs) · [Mutation retries](https://github.com/Simon-Martens/objekte/blob/1eef51b12c9dad55c2b9b7e603b888412cb51ad5/crates/core/src/lifecycle_mutation.rs)
2. **Task admission.** A second request for the same active compaction/rebuild shares its task. A request for the other maintenance operation receives the existing busy error. Reconciliation has its own coalesced job, progress, report, cancellation flag, condition variable and join handle. Preserve the separate slots, panic/error conversion, terminal progress, wakeups and repeated waits. Keep initial build’s distinct cursor and early-index semantics. [Job admission](https://github.com/Simon-Martens/objekte/blob/1eef51b12c9dad55c2b9b7e603b888412cb51ad5/crates/core/src/lifecycle/mod.rs#L981) · [Reconciliation admission](https://github.com/Simon-Martens/objekte/blob/1eef51b12c9dad55c2b9b7e603b888412cb51ad5/crates/core/src/lifecycle/mod.rs#L1184)
3. **Periodic work and shutdown.** The timer holds a weak index reference. Replacing/stopping a schedule joins the timer; stopping the schedule lets an already-started pass finish. Quiesce sets its flag under the publication write lock, releases that lock, stops the timer, requests cancellation, and waits for owned jobs. Preserve the existing error/report precedence. Resume reopens publication and leaves restoration of recurring policies to the caller. Keep worker joins outside the publication lock and outside job locks needed by the completing worker. [Scheduler and shutdown](https://github.com/Simon-Martens/objekte/blob/1eef51b12c9dad55c2b9b7e603b888412cb51ad5/crates/core/src/lifecycle/mod.rs#L1274)
4. **Mutations and reconciliation.** Preserve the existing distinction between an unrelated publication conflict that a caller retries and a stale file handle that reports a file conflict. Keep the actual per-operation retry limits, hash/version/membership checks and fallback strategies. A compatible move reuses index contributions. After a partial or cancelled scan, infer removals only under the existing completeness rules; recheck a missing path before applying deletion. An unchanged pass must leave the revision alone. Preserve batch size, issue cap, progress and source-read accounting. [Reconciliation pass](https://github.com/Simon-Martens/objekte/blob/1eef51b12c9dad55c2b9b7e603b888412cb51ad5/crates/core/src/lifecycle/mod.rs#L1646) · [Verified old content](https://github.com/Simon-Martens/objekte/blob/1eef51b12c9dad55c2b9b7e603b888412cb51ad5/crates/core/src/lifecycle_verified.rs)
5. **Compaction and rebuild.** Retain budgets, cancellation checkpoints, revision-conflict retry and unaffected segment sharing. Keep source-free compaction source-free. Rebuild continues to use `Index::start_build` and revalidates its candidate before publication, so a concurrent user update survives. Keep existing timing and worker behavior. This assignment changes ownership of existing code; preserve synchronization primitives, worker count and lock scopes. [Compaction](https://github.com/Simon-Martens/objekte/blob/1eef51b12c9dad55c2b9b7e603b888412cb51ad5/crates/core/src/lifecycle_compaction.rs) · [Controlled rebuild](https://github.com/Simon-Martens/objekte/blob/1eef51b12c9dad55c2b9b7e603b888412cb51ad5/crates/core/src/lifecycle/mod.rs#L1103)

## 4. Commit and acceptance sequence

1. Capture A5, root-file inventory, default/all-features test names and relevant baseline results. Move the seven modules, their six test companions and all imports in compiling commits. Check CLI/desktop callers as well as core.
2. Separate adoption, mutation preparation/staging and reconciliation passes. Preserve the public pinned-view and foreign/stale revision tests. Compare moved bodies using `git diff --color-moved`.
3. Extract task admission, periodic scheduling, rebuild control and shutdown in a distinct commit. Provide a before/after lock ownership note listing which locks cover admission, root installation, periodic replacement, completion notification and join. Call out any changed lock scope for explicit review. Use existing test synchronization points, channels and the test timer tick when exercising a specific interleaving.
4. Retain and run the named tests below. If coverage does not exercise concurrent admission after the split, add one deterministic test: hold a compaction or rebuild worker at a test-only barrier, request the same operation twice and the other operation once, release the worker, and verify shared completion for the first two callers and the existing busy error for the other. Use bounded waits for test failure diagnostics and guarantee barrier release on failure. Never establish ordering with sleeps.
5. Run `cargo fmt --all -- --check`, `cargo check --locked --workspace --all-targets`, `cargo test --locked --workspace`, `cargo test --locked -p objekte-search-core --all-features` and `RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links" cargo doc --locked --workspace --no-deps`. Account for all baseline test names, allowing path changes. Re-run the focused concurrency tests after the final extraction. For pure moves use existing counters and identity/sharing assertions. If lock scope, I/O, copying or critical-path work changes, isolate and measure that change on identical inputs. Prioritize the lifecycle suite over a full Musenalm benchmark.

| Required behavior | Existing regression tests |
| --- | --- |
| Publication and pins | `pinned_view_stays_stable_when_a_new_root_is_published`, `publication_rejects_foreign_and_skipped_revisions_without_advancing_status`, `quiescence_rejects_prepared_publications_until_close_is_cancelled`, `deleting_shared_multi_record_file_retires_every_collection_atomically` |
| Initial construction and ownership | `start_build_is_the_single_scan_and_repeated_waits_do_not_rebuild`, `repository_conversion_rejects_retained_owner_and_view_promptly`, `late_private_invalidation_and_worker_failure_publish_nothing` |
| Reconciliation and scheduling | `stopping_periodic_schedule_does_not_cancel_current_reconciliation`, `periodic_tick_detects_change_after_initial_index_build`, `background_reconciliation_reports_completion_and_joins`, `reconciliation_applies_metadata_changes_without_revising_unchanged_files` |
| Concurrency and maintenance | `rebuild_retries_after_concurrent_file_commit_without_losing_it`, `background_rebuild_waits_and_cancelled_preparation_never_publishes`, `background_compaction_preserves_pinned_reader_and_reports_completion`, `large_source_free_compaction_cancels_and_retries_a_concurrent_commit` |
| Reuse and file identity | `changed_paths_and_tombstones_do_not_copy_or_rewrite_base_inventory`, `compatible_move_keeps_index_contributions_and_advances_file_version`, `one_record_shared_file_preserves_every_collection_slot_atomically`, `compaction_reuses_unaffected_native_segment`, `pinned_adapter_keeps_batch_queries_on_one_publication` |

The large source-free compaction regression is ignored by the default suite. Run it explicitly at its existing default size of 262,144 records with `cargo test --locked --release -p objekte-search-core --lib large_source_free_compaction_cancels_and_retries_a_concurrent_commit -- --ignored`. Check the summary shows one passing test. The name filter still works after the module move. Apply the same matched-test count check to any individually selected regression.

## Morning review packet

Append the actual path map, thirteen removed root paths, tested code SHA, lock note, commands and results to the task’s completion report. After pushing it, verify the remote A6 delivery SHA and its ancestry. Report B, A4, A5 and A6 as full SHAs, with separate GitHub comparisons **B…A4**, **A4…A5**, **A5…A6**. Mark each branch ready, partial or blocked with reasons. For an unfinished assignment give the last passing SHA, failing command and next concrete action. Leave the merge decisions to the owner. Assignment 7 is outside tonight’s work.

## Implementation report

- **Task:** `core-lifecycle-coordination`
- **Completion date:** 2026-11-19
- **Base A5:** `a8a1db7d89186e7b34d2c18e25c621ba7f1b8cf6`
- **Integration branch:** `integration/core-lifecycle-coordination`
- **Integration worktree:** `/home/simon/source/objekte-integration-core-lifecycle-coordination`
- **Tested code revision:** `161597bf2dc2a8aa91e12e2bca977f406675ec9c`
- **Implementation commits:** `3989b16acd218a1280931999b91bb35b4f56a13f`, `0785c0f28ddc4f57856cc60c3946dff3086fde1b`, `161597bf2dc2a8aa91e12e2bca977f406675ec9c`
- **Status:** complete and ready for review; no merge into `master` was performed

### Summary

The seven former lifecycle root modules and their six companion suites now live
under their owning lifecycle modules. The direct core Rust-file count is 48,
down from 61 at A5 as required.

`lifecycle/mod.rs` now retains `Index`, `IndexInner`, revisions, file handles,
top-level status and accessors, and the shared published-root, writer,
quiescence, task, cache and scheduling ownership. Opening, preparation,
staging, reconciliation passes and tasks, periodic scheduling, maintenance
admission and rebuild control, and lifecycle service operations have focused
module homes. Former module declarations and compatibility paths were removed,
and maintained core, CLI, desktop, example, test and rustdoc callers use the
canonical module names.

Publication remains a captured-view candidate construction followed by a
revision- and quiescence-checked root swap under the existing publication write
lock. Rebuild still uses `Index::start_build` and revalidates its candidate, so
a concurrent file update survives. Compaction remains source-free, and pinned
views continue to retain their original catalogue, slots and segments.

### Path map

| Former core-root path | Canonical path |
| --- | --- |
| `crates/core/src/lifecycle_catalog.rs` | `crates/core/src/lifecycle/catalog.rs` |
| `crates/core/src/lifecycle_catalog.test.rs` | `crates/core/src/lifecycle/catalog/tests.rs` |
| `crates/core/src/lifecycle_initial.rs` | `crates/core/src/lifecycle/initial.rs` |
| `crates/core/src/lifecycle_initial.test.rs` | `crates/core/src/lifecycle/initial/tests.rs` |
| `crates/core/src/lifecycle_mutation.rs` | `crates/core/src/lifecycle/mutation/mod.rs` |
| `crates/core/src/lifecycle_mutation.test.rs` | `crates/core/src/lifecycle/mutation/tests.rs` |
| `crates/core/src/lifecycle_verified.rs` | `crates/core/src/lifecycle/mutation/verified.rs` |
| `crates/core/src/lifecycle_scan.rs` | `crates/core/src/lifecycle/reconciliation/scan/mod.rs` |
| `crates/core/src/lifecycle_scan.test.rs` | `crates/core/src/lifecycle/reconciliation/scan/tests.rs` |
| `crates/core/src/lifecycle_compaction.rs` | `crates/core/src/lifecycle/maintenance/compaction/mod.rs` |
| `crates/core/src/lifecycle_compaction.test.rs` | `crates/core/src/lifecycle/maintenance/compaction/tests.rs` |
| `crates/core/src/lifecycle_partition.rs` | `crates/core/src/lifecycle/maintenance/compaction/partition.rs` |
| `crates/core/src/lifecycle_partition.test.rs` | `crates/core/src/lifecycle/maintenance/compaction/partition/tests.rs` |

Responsibilities extracted from the former `lifecycle/mod.rs` have these
additional canonical homes:

| Canonical path | Contents |
| --- | --- |
| `lifecycle/open.rs` | Existing-state open/adoption, private published conversion and the test-only consuming repository conversion |
| `lifecycle/mutation/preparation.rs` | Prepared file values and reads outside the publication lock |
| `lifecycle/mutation/staging.rs` | Candidate preparation for add, replace and delete, returning the expected revision and results |
| `lifecycle/reconciliation/mod.rs` | Reconciliation state, progress/report types and the reconciled pass |
| `lifecycle/reconciliation/task.rs` | Coalesced reconciliation job and task admission/waits |
| `lifecycle/reconciliation/periodic.rs` | Periodic weak-index timer, replacement, stopping and periodic wait behavior |
| `lifecycle/maintenance/mod.rs` | Maintenance operation/progress/report/task types and compaction/rebuild admission |
| `lifecycle/maintenance/rebuild.rs` | Synchronous rebuild control and candidate publication helpers |
| `lifecycle/service.rs` | Quiesce, resume and shutdown ordering across recurring and active workers |

### Lock ownership note

No production lock primitive or critical-section scope changed. `IndexInner`
remains the sole owner of `published`, `writer`, `quiescing`,
`maintenance_operation`, `maintenance`, `periodic_transition`, `periodic` and
the publication cache.

- Maintenance admission holds `maintenance_operation`, checks `quiescing`, and
  briefly inspects the active job's result before sharing or rejecting a task.
- Reconciliation admission holds `maintenance` and uses the separate current
  job slot; the worker does not hold an admission lock during its scan.
- Root installation rechecks quiescence and the expected revision under
  `published.write()`, swaps the root, releases the guard, and only then drops
  the retired root.
- Periodic replacement and stopping serialize transitions with
  `periodic_transition`; the periodic slot is taken and its guard released
  before joining the timer worker.
- Completion writes progress/result under the existing job mutex and notifies
  the existing condition variable. Task waits take and drop the worker mutex
  before joining it.
- Quiesce sets `quiescing` under `published.write()`, releases that lock, stops
  the periodic timer, cancels and joins regular maintenance and reconciliation,
  and performs joins outside publication and worker-completion locks. Resume
  clears the flag under `published.write()`. Shutdown still delegates to
  quiesce.

The existing coverage plus a deterministic channel-barrier regression now
checks maintenance admission after the split. The test holds a worker, requests
the same compaction twice, verifies both handles share one task, rejects a
rebuild with the existing busy `ResourceLimit` error, releases the worker, and
joins both successful waits. Channel disconnection guarantees release if an
assertion fails; no sleeps establish ordering.

### Validation

The following commands passed at tested code revision
`161597bf2dc2a8aa91e12e2bca977f406675ec9c`:

```text
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
cargo test --locked -p objekte-search-core --all-features
RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc --locked --workspace --no-deps
cargo test --locked -p objekte-search-core --lib lifecycle::
cargo test --locked --release -p objekte-search-core --lib \
  large_source_free_compaction_cancels_and_retries_a_concurrent_commit -- --ignored
```

The workspace test run passed the CLI (81 tests), desktop (161 tests), CLI
process integration tests (33 passed, one ignored), core integration tests and
doc tests. Default core results were 584 passed and 16 ignored. Core
all-features results were 585 passed and 16 ignored. The focused lifecycle run
reported 88 passed, six ignored and no failures. The explicitly ignored
release-mode compaction regression reported exactly one passing test:

```text
running 1 test
test lifecycle::maintenance::compaction::tests::large_source_free_compaction_cancels_and_retries_a_concurrent_commit ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 599 filtered out
```

An A5-to-A6 all-feature test-function inventory comparison, normalizing module
prefixes to account for moved suites, reports exactly one addition:
`maintenance_admission_shares_same_operation_until_worker_release`. No existing
test function was removed or renamed, and ignored/feature-gated registrations
remain.

The move was reviewed with rename detection and `git diff --color-moved`.
Because this task changed ownership without changing lock scope, worker count,
I/O scheduling, data layout or critical-path work, no additional lifecycle
benchmark was required.

### Review notes

No unresolved functional issue or blocked check remains. The final report is a
documentation-only commit after tested code revision `161597bf2dc2a8aa91e12e2bca977f406675ec9c`.
The branch and integration worktree remain for the owner's separate merge
decision.

### Repair amendment

Repair branch `repair/core-lifecycle-coordination` tested code revision
`d1170ac5b6429783b05e508c3e40c5dc69dd828b`. It completes the lifecycle type,
task, preparation, staging and scheduling ownership split, removes the
compatibility root re-exports, and replaces the synthetic maintenance admission
test with a bounded real-worker entry barrier.

Detailed path ownership, synchronization ordering and validation are recorded
in `docs/ai/done/assignment_6_lifecycle_coordination_repairs.md`. R6 retains
both R5 and frozen A6 ancestry, all original root removals, and the unchanged
publication/locking behavior.
