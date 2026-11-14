# Remove legacy snapshot stack: implementation results

**Task:** `remove-legacy-snapshot-stack`.
**Date:** 14 November 2026.
**Base:** `0c63651458` (`integration/direct-snapshot-access`; the task plan named the earlier hybrid head `dd3f8feb19`).
**Branch/worktree:** `integration/remove-legacy-snapshot-stack`, `/home/simon/source/objekte-integration-remove-legacy-snapshot-stack`.
**Commits before this report:** `e5d6f250bd` (configuration hash extraction), `eb98d7ef18` (merge completed `remove-binding-cache`), `da608b2799` (legacy stack removal). The binding-cache work was separately completed and pushed on `integration/remove-binding-cache` before its merge here.

## Changes

The old `OBJSNAP1` snapshot, lifecycle cache codec, destination guard, repository-layout cache opener, background cache writer, associated tests, and snapshot benchmark were deleted. The 107-file removal commit added 543 lines and deleted 8,680 lines, a net reduction of 8,137 lines. Ten files were deleted outright, including `storage.rs`, `lifecycle_segment_codec.rs`, `lifecycle_cache_guard.rs`, `lifecycle_repository.rs`, their legacy tests, `legacy_snapshot_disabled.rs`, and `bench_snapshot_save.rs`. `prepared.test.rs` was deleted as part of the separately merged binding-cache task. Historical reports and archived benchmark files remain intact.

`configuration_hash` and its identity inputs moved to `crates/core/src/configuration.rs` with a focused test covering model version, alphabet, inference, source reader version, schema fields, sort settings, relations, custom bindings/defaults, and unrelated metadata. The hash algorithm and managed-store format were not changed. A managed-store roundtrip helper in `test_support.rs` replaced snapshot roundtrips where persistence behavior matters. Tests of the deleted wire format and cache guards were removed; static semantic tests now reopen fresh repositories when no persistence behavior is needed.

`Repository` is a fresh in-memory builder and search API. `Index` retains managed persistence through `IndexStoragePolicy`, `open_with_storage`, `flush_storage`, and `ManagedStore`. Snapshot modes, snapshot open/save methods and stats, `RepositoryOpenOptions`, and `Index::load_cache`/`flush_cache`/`sync_cache` are intentionally removed public APIs. `RepositoryConfiguration.protected_files` was removed because it only guarded the old writer. The CLI NDJSON field is now `index_storage_action`, with values `built_in_memory`, `managed_built`, and `managed_restored`. Active CLI, core, TUI/desktop integration, benchmark examples, scripts, and lifecycle docs were updated; historical generated design records retain historical API descriptions.

## Validation

- `cargo test --workspace --locked`: passed after the combined implementation (CLI 95 tests, desktop core 173 tests, core 620 tests, plus the remaining workspace and integration suites). Managed generation publication/reopen, validation stamp, corruption, maintenance, concurrency, and migrated roundtrip tests were included.
- `cargo clippy --workspace --all-targets --locked`: passed with existing warnings. `cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked` also passed.
- `cargo fmt --all -- --check` and `python3 scripts/format-rust-spacing.py --check`: both report pre-existing repository-wide formatting drift (about 1,495 and 529 diff chunks respectively, including untouched files). The new `test_support.rs` passes local rustfmt; `git diff --check` passes. A global formatting rewrite was excluded from this task.
- `python3 -m py_compile` on the three changed `bench/` scripts passed. Source and active documentation searches found no remaining references to the deleted snapshot/cache APIs. Historical `docs/ai/generated/**`, `docs/ai/done/**`, and archived `bench/results/**` still contain references as records of prior behavior.
- CLI batch tests cover the new storage-action field. CLI, TUI, and desktop workspace suites compile and pass against managed storage; no interactive native GUI session was run.

The 220-case extended Musenalm campaign used 148,486 records, ASCII and Unicode, five repetitions, 32 query workers, and zero verified-result cache. A disposable custom managed store was primed before the on-demand measurement. All 220 per-case digests match the earlier binding-cache candidate in both memory and on-demand modes, with all 440 case/profile counts and returned-record summaries matching in each mode. The on-demand reopen has **zero source reads and zero build parses** for both alphabets. Both runs recorded zero query failures. Raw compressed samples, exact manifests, counters, commands, and comparison ratios are in [`bench/results/remove-legacy-snapshot-stack/final-20261114/README.md`](../../../bench/results/remove-legacy-snapshot-stack/final-20261114/README.md). Single-run build/open timings and shared-host query ratios are recorded there as context, not a precise speedup claim.

Managed generations remain the sole persistence path. The old snapshot format has no reader or writer in production or tests. The old encoded-segment-buffer issue and the resolved collection-wide verified-base rewrite issue were moved to `done/resolved-issues/`. The remaining bounded verified-base update latency is separately recorded in `issues/perf/verified-base-update-bounded-path-latency.md`; all other open issue reports retain their matching `bug`, `feat`, or `perf` category. Issue reports are for review, with no additional implementation authorized by their presence.

## Limits

The change intentionally breaks callers of removed snapshot and cache APIs. The benchmark uses one shared-host campaign rather than controlled, repeated process pairings; its strongest evidence is query result parity and source-free managed reopen. Formatting checks do not pass because of pre-existing drift, which was left outside this task. The task is complete on its integration branch and is intended to be merged into `integration/direct-snapshot-access` under the user's explicit follow-up instruction.
