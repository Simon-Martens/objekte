# Abandoned: hybrid redb index work (plans 1-6)

- Status: **abandoned / unfinished** by user instruction on 2026-11-15.
- Abandonment branch: `abandoned/hybrid-redb`.
- Base: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9` (`master`).
- Branch tip before this document commit: `954c1836e1`.
- This branch contains all 37 implementation commits from the six planned
  tasks plus a work-in-progress start on plan 6. It is **not ready to merge**
  and must not be merged into `master`.

The user stopped the programme before plan 6 was implemented. Plans 1-5 were
completed and validated on their integration branches; plan 6 was only begun.
The branches are stacked: each plan branch starts at the previous plan's tip,
because the implementing agents were not allowed to merge into `master`.

## Status by plan

| Plan | Task | Branch | Tip | Status |
| --- | --- | --- | --- | --- |
| 1 | `posting-map-interface` | `integration/posting-map-interface` | `5d509aecf1` | complete, validated |
| 2 | `chunked-shared-postings` | `integration/chunked-shared-postings` | `6e369cda37` | complete, validated |
| 3 | `appendable-shards` | `integration/appendable-shards` | `9fd8838db9` | complete, validated with one disclosed test gap |
| 4 | `redb-index-store` | `integration/redb-index-store` | `693f8e0a4b` | complete, validated; store stays dormant unless called explicitly |
| 5 | `main-shard-vacuum` | `integration/main-shard-vacuum` | `bdaf194eb9` | complete, validated |
| 6 | `index-sync-frontends` | `integration/index-sync-frontends` | `954c1836e1` | **INCOMPLETE**: C1 committed, C2 partial WIP, C3-C10 not implemented |

Completion records for plans 1-5 are under `docs/ai/done/`:

- `btree-plan-1-posting-map-interface.md`
- `btree-plan-2-chunked-shared-postings.md`
- `btree-plan-3-appendable-shards.md`
- `btree-plan-4-redb-index-store.md`
- `btree-plan-5-main-shard-vacuum.md`

Plan 6's specification is still at
`docs/ai/tasks/btree-plan-6-index-sync-frontends.md` in the original checkout
(it was never moved to `done/`).

## What plan 6 has so far

Committed on `abandoned/hybrid-redb`:

- `e61013ffba` Resolve index sync store locations: core `IndexSync`,
  `store_file`, `system_cache_dir` and their tests.
- `954c1836e1` WIP: begin loading and attaching index stores: a compilation-
  clean start on `lifecycle_sync.rs` (`StoreLoad`, `StoreStatus`,
  `configuration_identity_for`, `load_from_store`, `attach_store`,
  `detach_store`, `store_status`, `checkpoint_if_changed`), a `Repository`
  root accessor, and the `open_persistent` refactor.

Not implemented:

- C2 tests (`lifecycle_sync.test.rs` does not exist).
- C3 background sync worker, `start_store_sync`, `stop_store_sync`, `quiesce`
  and `shutdown` integration, and its tests.
- C4 CLI flags (`--index-sync`, `--index-sync-dir`), the saved setting,
  `SyncTarget`/`SyncOutcome`/`resolve_target`/`try_load`/`attach_after_build`/
  `start_sync`/`apply_choice`, and their tests.
- C5 CLI batch output and behaviour, C6 TUI actions/status/quit, C7 desktop
  backend/controller, C8 desktop Index panel, C9 documentation, C10
  validation.
- TUI PTY and GUI validation evidence.
- The `.gitignore` entry `**/.objekte/.index/`.

The partial `lifecycle_sync` code compiles as a library (`cargo check
-p objekte-search-core --lib`) but has not been tested and is not wired into any
frontend. It is not an acceptable merge candidate.

## Validation recorded for plans 1-5

At the plan 5/plan 6 START commit (`bdaf194eb9`):

- Default workspace tests: 907 passed, 0 failed, 20 ignored.
- Forced `objekte-search-core/force-shared-backing` tests: 907 passed,
  0 failed, 20 ignored.
- Clippy: 235 warning lines (plan 5 baseline was 236; plans 1-4 ended at 236).
- Build gate: signatures unchanged. ASCII 0.968 s / 491.5 MB
  (+2.9% time, +1.0% RSS against plan 4); Unicode 0.986 s / 510.2 MB
  (-1.2% time, -0.4% RSS).
- `accept_musenalm` passed for ASCII/Unicode with default and forced features.
- `accept_musenalm_store` passed for ASCII/Unicode.
- The quick query gate still reports the same three path-sensitive baseline
  failures seen since plan 1 (`corpus_fingerprint differs`, missing
  `schema-manifest.json`, `semantic registrations differ`); no latency
  confirmation failed.

Plan 5 measurement: 20,000 churn updates held the main shard at 3,064 physical
slots / 2,040 live records; 2,500 folds, 19 vacuums, 0 fold failures, RSS
+0.5%, query probe p95 0.001 ms.

## Known limitations and risks

- Plan 3 disclosed one missing heavy test, `main_shard_dissolves_when_half_retired`.
- Plan 4 left `CheckpointReport.bytes_written` at zero and did not add
  `loaded_index_equals_checkpointed_shards`,
  `store_survives_main_shard_dissolve`, or the raw corrupted-record-row
  sub-case of `damaged_store_rebuilds`.
- Plan 5's churn test runs 4,500 updates and omits the delete/re-add branch
  because verified deletion compacts the built shard before the 75% retirement
  window opens. Migration is covered separately.
- The plan 6 WIP stores are not synced in the background, not attached by any
  frontend, and not shut down through the store.
- Memory-only frontend behaviour is still the default because plan 6 never
  changed a frontend.

## How to resume

1. Read `docs/ai/README.md`, `docs/ai/tasks/btree-plan-6-index-sync-frontends.md`
   and this document.
2. Start from `abandoned/hybrid-redb` or from the plan 6 WIP commit
   `954c1836e1`; do not merge the abandoned branch as-is.
3. Add the missing `lifecycle_sync.test.rs` and finish C2 before moving to C3.
4. Continue with C3-C10 in the order of the specification.
5. Re-run the full plan 6 gates (workspace tests default and forced, clippy,
   build gate, acceptance, C10 CLI runs, TUI PTY and GUI validation) before
   calling the work complete.

## Cleanup

The six task worktrees were removed after this document was committed. The
integration branches and this abandonment branch were retained. Temporary
memory for the tasks remains under `docs/ai/memory/` in the original checkout
for reference; it is not committed.
