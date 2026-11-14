# Agent specification: remove the legacy snapshot and lifecycle-cache stack

Task: `remove-legacy-snapshot-stack`
Repository: `Simon-Martens/objekte`
Plan date: 14 November 2026
Base revision: current `integration/hybrid-index-storage` head `dd3f8feb19` (or the merged master equivalent)

## 1. Objective

Delete the old snapshot and lifecycle-cache persistence stack completely. Production already uses the managed generation store. The old path survives only for tests and examples, yet accounts for roughly 3,100 lines of codec, cache, and loading logic plus several public options and status fields.

The goal is one persistence model:

```
IndexStoragePolicy + ManagedStore + Index::flush_storage / open_with_storage
```

Everything else is removed:

- legacy `OBJSNAP1` snapshot encoding/decoding;
- the old lifecycle cache state and segment codec;
- cache destination guards;
- snapshot open modes and policies;
- legacy cache synchronization workers;
- repository-layout cache convenience opening;
- associated options, stats, CLI output, benchmarks, tests and docs.

This is an intentional breaking API change. The repository is pre-1.0; keeping dead options and `cfg!(test)`-only public APIs is worse than removing them.

Non-goals:

- Do not change the managed store format, generation layout, validation stamps, or storage policy semantics.
- Do not remove the static in-memory `Repository` API unless listed below; it remains for direct library callers and tests.
- Do not remove the verified-result cache, inferred-order cache, or decoded-posting cache.
- Do not rewrite archived `bench/results/**` or historical `docs/ai/done/**` reports.
- Do not merge into `master`.

## 2. Base, branch and workflow

- New task branch: `integration/remove-legacy-snapshot-stack`.
- New worktree: `/home/simon/source/objekte-integration-remove-legacy-snapshot-stack`, sibling of the original checkout.
- Keep `/home/simon/source/objekte` on its existing branch; do not reuse the user's dirty checkout.
- Read `AGENTS.md` and `docs/ai/README.md` first.
- Push immediately after every commit. Do not force-push or modify remotes.
- Run `cargo fmt --all`, `python3 scripts/format-rust-spacing.py`, workspace tests and Clippy before each push. Report existing repository-wide formatting drift rather than committing a global reformat.

The user's current checkout contains an untracked `examples/musenalm/.objekte/indexes/` store. Leave it untouched.

## 3. Inventory: delete vs keep

### Delete

| File / item | Approx. size | Why |
| --- | ---: | --- |
| `crates/core/src/storage.rs` legacy codec | ~1,900 of 2,242 lines | `OBJSNAP1` snapshot and lifecycle cache encoding/decoding; production rejects it with `cfg!(test)` |
| `crates/core/src/lifecycle_segment_codec.rs` | 896 | Old lifecycle segment payload codec used only by snapshots |
| `crates/core/src/lifecycle_cache_guard.rs` | 252 | Cache destination safety for the old snapshot writer |
| `crates/core/src/lifecycle_repository.rs` | 145 | Repository-layout cache opener with `CacheOpenPolicy`/`RepositoryOpenOptions` |
| `SnapshotMode`, `SnapshotOpenPolicy`, `SnapshotAction` | — | Old persistence policy; production only uses `Disabled` |
| `SnapshotSaveStats`, `SnapshotSectionSaveStats`, `SnapshotLoadProfile`, `SnapshotSectionLoadProfile`, `SnapshotBuildConfiguration` | — | Old snapshot diagnostics/settings |
| `LifecycleCacheState`, `LifecycleSegmentCache`, `LifecycleCatalogFile`, `EncodedFile`, `EncodedCollection`, `EncodedCollectionRef`, `MetadataInterner` | — | Old lifecycle cache representation |
| `Repository::save_snapshot*`, `load_snapshot*`, `open_with_snapshot_policy`, `open_with_sources_and_snapshot_policy` | — | Public APIs that only work under `cfg!(test)` |
| `RepositoryOptions.snapshot_mode` | — | Dead production knob |
| `StartupStats` snapshot fields (`restored`, `snapshot_action`, `snapshot_restore_error`, `snapshot_decode_ns`, `snapshot_write_ns`; audit `reconciliation_ns`) | — | Snapshot-only status |
| `Index::load_cache*`, `flush_cache*`, `set_sync_cache`, `sync_cache_error`, `stop_sync_cache`, `protect_configuration_files`, `select_cache_destination`, `discard_invalid_cache` | — | Old lifecycle cache APIs |
| `PersistenceState`, `SyncCacheWorker`, `sync_cache`, `sync_cache_error` fields | — | Old cache writer state |
| `build_unsealed_for_legacy_snapshot_tests` | — | Exists only for snapshot tests |
| `crates/core/src/lifecycle_repository.test.rs`, `lifecycle_segment_codec.test.rs`, `lifecycle_cache_guard.test.rs`, `storage.test.rs` legacy cases | — | Test deleted formats |
| `crates/core/tests/legacy_snapshot_disabled.rs` | — | Tests that removed APIs return errors |
| `crates/core/examples/bench_snapshot_save.rs` | 100 | Legacy snapshot benchmark |
| Snapshot sections in `docs/search-core.md`, `docs/development/index-lifecycle-contracts.md`, `docs/future-whole-file-replacement.md`, `docs/cli/obj.md`, `bench/README.md` | — | Active docs must describe one persistence model |

### Keep / extract

- `configuration_hash` and its direct helpers (`feed`, `path_bytes`, `CONFIGURATION_HASH_VERSION`, `SnapshotBuildConfiguration` inputs as needed) are still used by `index_storage`, `lifecycle`, `lifecycle_catalog`, and `repository_direct`.
  - Move them to a small `crates/core/src/configuration.rs` or into `repository_configuration.rs`.
  - Do not delete the hash inputs: model version, bigram version, folded representation, collation version, source readers/versions, schema fields, inference, sort settings, relations, custom operations/defaults.
- Managed-store code: `managed_store.rs`, `generation_catalog.rs`, `lifecycle_storage.rs`, validation stamps, `IndexStorageStatus`.
- `Repository` static in-memory construction/search and `CollectionState`.
- Verified-result cache, inferred-order cache, decoded-posting cache.
- `Index::flush_storage`, `open_with_storage`, `start_incremental_build_with_storage`, `attach_storage`, `storage_status`, `set_sync_storage`, `stop_sync_storage`, `quiesce`.
- `PublishedIndex.saved_revision` is still needed for managed flush status and `IndexStatus.dirty`; keep it.

## 4. Required changes

### 4.1 Extract configuration hashing

Before deleting `storage.rs`, move the parts still used:

1. `configuration_hash` and helpers.
2. Any `path_bytes` / `feed` / hash-version constants used by it.
3. Only the `RepositoryOptions` inputs it actually reads; keep the function signature stable where possible.

Update imports in:

- `crates/core/src/index_storage.rs`
- `crates/core/src/lifecycle.rs`
- `crates/core/src/lifecycle_catalog.rs`
- `crates/core/src/repository_direct.rs`

Add a focused test that the hash changes when model version, alphabet, inference, source reader version, schema field, sort setting, relation, custom binding or default changes, and does not change when only unrelated metadata changes. This preserves the identity contract after the move.

### 4.2 Simplify `Repository`

Remove:

- `SnapshotMode`, `SnapshotOpenPolicy`, `SnapshotAction`.
- `RepositoryOptions.snapshot_mode`.
- `Repository::open_with_snapshot_policy`, `open_with_sources_and_snapshot_policy`.
- The entire snapshot restore branch in the open path; opening always builds from source or receives a managed generation from `Index`.
- `save_snapshot`, `save_snapshot_with_stats`, `save_default_snapshot_for_test`.
- `StartupStats` snapshot fields.
- Imports of `storage::snapshot_path_in`, `sources_match`, `load_snapshot_checked`.

`Repository::open` and `open_with_options` become the simple fresh-build entry points. `empty_for_managed_generation` keeps `options.snapshot_mode = ...` removal as a no-op.

### 4.3 Simplify `Index`

Remove the old cache APIs and state:

- `flush_cache`, `flush_cache_with_stats`, `flush_cache_copy_with_stats`, `flush_cache_with_stats_select`
- `load_cache`, `load_cache_profiled`, `load_cache_or_rebuild_invalid`, `load_cache_with_sources`, `load_cache_with_sources_inner`
- `select_cache_destination`, `protect_configuration_files`, `discard_invalid_cache`
- `set_sync_cache`, `enable_sync_cache`, `stop_sync_cache`, `sync_cache_error`, `notify_sync_cache`
- `PersistenceState`, `SyncCacheWorker`, `sync_cache`, `sync_cache_error` fields
- `sync_cache` calls in `shutdown`, `quiesce`, mutation notification paths
- `build_unsealed_for_legacy_snapshot_tests`

Keep:

- managed `sync_storage`, `storage_syncing`, `storage_status_snapshot`
- `saved_revision` and status/dirty behavior
- managed shutdown/flush paths
- all `IndexStorageStatus`, validation stamp fields

Audit `IndexInner` for fields that become unused and prune them (`selected_cache`, `saved`, `protected_files`, etc.).

### 4.4 Remove the repository-layout convenience opener

Delete `lifecycle_repository.rs` and its cache policy types:

- `RepositoryOpenOptions`
- `CacheOpenPolicy`
- `CacheFailurePolicy`
- `Index::open_repository`
- `Index::open_configured_repository`

Rationale: it exists to select/load/write the legacy snapshot cache. The managed store opener already accepts `IndexStoragePolicy`, and the CLI/TUI/desktop share a loader. If a repository-layout convenience entry point is still required, add it later as a thin wrapper over:

```
IndexStoragePolicy::prepare(...)
→ Index::open_with_storage(...)
→ optional source-free / reconciliation behavior
```

Do not reintroduce cache-open policies. Update `docs/search-core.md`, `docs/future-whole-file-replacement.md`, and `docs/development/index-lifecycle-contracts.md` to point to `Index::open_with_storage` or the CLI loader.

### 4.5 Delete `storage.rs` legacy code

After extracting configuration hashing, remove:

- snapshot and lifecycle cache codecs, section readers/writers, metadata interner, encoded collections;
- `SNAPSHOT_VERSION`, `SnapshotBuildConfiguration`, `SnapshotSaveStats`, `SnapshotSectionSaveStats`, `SnapshotLoadProfile`, `SnapshotSectionLoadProfile`, `SnapshotLoadError`;
- `snapshot_path`, `snapshot_path_in`, `sources_match`, `snapshot_build_configuration*`;
- `save_lifecycle_snapshot_with_stats`, `load_snapshot*`, `load_lifecycle_snapshot*`;
- legacy path/bytes helpers that are no longer referenced.

Delete `lifecycle_segment_codec.rs` and `lifecycle_cache_guard.rs` after all references are gone. Remove their modules from `lib.rs`.

### 4.6 CLI and NDJSON output

`crates/cli/src/execution.rs`:

- Remove `SnapshotSaveStats` imports and `snapshot_save` fields/parameters.
- Remove the legacy `snapshot_action` helper and `report.snapshot_action` inputs.
- Replace the output field:

```json
"index_storage_action": if memory { "built_in_memory" }
                        else if restored { "managed_restored" }
                        else { "managed_built" }
```

- Update `execution.test.rs` expectations.
- Update `docs/cli/obj.md` if it documents `snapshot_action` or snapshot modes.

No other frontend uses `SnapshotAction`; remove imports in `lifecycle_caller.rs` and adjust `IndexedRepository::new` to stop setting snapshot startup fields.

### 4.7 Benchmark examples and scripts

Update or delete:

- `bench_snapshot_save.rs` — delete.
- `bench_build_musenalm.rs` — remove snapshot save measurement or switch to `flush_storage`; do not keep a snapshot-specific benchmark.
- `bench_case_modes.rs`, `bench_corrective_cases.rs`, `bench_index_construction.rs`, `bench_musenalm.rs`, `bench_optional_features.rs` — remove `snapshot_mode` from `RepositoryOptions`; remove snapshot save/restore profiles.
- Any remaining `SnapshotMode::Disabled` / `IfValid` / `Required` uses.
- `bench/README.md` references to snapshots.

Archived results and scripts that only read them must remain untouched.

### 4.8 Tests and migration

There are about 20 core test files using `save_default_snapshot_for_test`, and 34 files with snapshot-related options. Migrate them before deleting the code so each commit stays green.

Add a reusable test helper (in `crates/core/src/test_support.rs` or an existing test helper module):

```
fn restore_through_managed_store(repo: Repository) -> Repository
```

or, preferably, return `Index`:

1. create a temp custom base directory;
2. `Index::from_repository(repo)` or `Index::build` with `IndexStoragePolicy::CustomDirectory(temp)`;
3. `attach_storage`, `flush_storage`;
4. drop the index;
5. `Index::open_with_storage(...)` with the same policy;
6. return the reopened index or its pinned view.

Use this helper for tests that currently do:

- `repo.save_default_snapshot_for_test(); Repository::open_with_snapshot_policy(...)`;
- `SnapshotMode::IfValid`/`Required` round trips;
- old lifecycle `load_cache`/`flush_cache` round trips.

For tests that specifically exercise old snapshot formats, corruption, section layouts, or cache guards, delete them. The managed-store tests cover generation manifests, catalogs, segments, validation stamps, corrupt bytes, migration and reopen.

Delete:

- `prepared.test.rs`-style cache tests for the old lifecycle cache (`lifecycle_cache_guard.test.rs`, `lifecycle_segment_codec.test.rs`, snapshot cases in `storage.test.rs`);
- `legacy_snapshot_disabled.rs`;
- `lifecycle_repository.test.rs` (or rewrite to the managed opener if a replacement API exists).

Update tests that only set `snapshot_mode: Disabled`; removing the field is mechanical.

### 4.9 Docs

Update active docs:

- `docs/search-core.md`: replace the “Static `Repository` compatibility” and snapshot sections with the managed `Index` lifecycle; state that `Repository` is an in-memory builder/API with no persistence. Remove the old `open_repository` description.
- `docs/development/index-lifecycle-contracts.md`: mark the old phase-0 record as historical or update it; do not leave instructions that reference removed APIs.
- `docs/future-whole-file-replacement.md`: update `open_repository` references to `open_with_storage`.
- `docs/cli/obj.md`: update the output field name if it was documented.
- `bench/README.md`: remove snapshot references.
- Historical `docs/ai/done/**` and archived `bench/results/**` remain unchanged.

## 5. Validation

Run:

```
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked
cargo fmt --all -- --check
python3 scripts/format-rust-spacing.py --check
```

Search for leftovers:

```
git grep -n 'SnapshotMode\|SnapshotOpenPolicy\|SnapshotAction\|SnapshotSaveStats\|SnapshotLoadProfile\|SnapshotBuildConfiguration'
git grep -n 'LifecycleCacheState\|LifecycleSegmentCache\|EncodedCollection\|EncodedFile'
git grep -n 'save_snapshot\|load_snapshot\|load_cache\|flush_cache\|set_sync_cache\|sync_cache_error'
git grep -n 'lifecycle_segment_codec\|lifecycle_cache_guard\|open_repository\|RepositoryOpenOptions'
git grep -n 'snapshot_action\|snapshot_save'
```

Only historical `docs/ai/done/**` and `bench/results/**` may still contain them.

Correctness/performance checks:

- Managed store tests: generation publish/reopen, hard-link reuse, validation stamps, migration, corruption, concurrent readers, maintenance.
- Query campaign: 220-case extended ASCII/Unicode, verified-result cache disabled; all result digests/counts/evidence match the base.
- Build/open: construction and managed open unchanged; no source reads added.
- CLI batch output: `index_storage_action` values correct for memory, managed-built and managed-restored.
- TUI/desktop smoke: storage selection, status, flush-on-close still work.
- Record lines removed and files deleted in the report.

Expected effect: no production behavior change; a large reduction in code, public options and test-only branches. No meaningful performance change.

## 6. Commit sequence

Keep each commit green. Suggested:

1. **Extract configuration hash** into a small module; add identity tests; no behavior change.
2. **Add managed-store test helper and migrate core snapshot tests** in batches; tests pass while legacy code still exists.
3. **Remove `Repository` snapshot options/API and `StartupStats` fields**; update callers.
4. **Remove `Index` legacy cache APIs/state and `lifecycle_repository`**, then delete `lifecycle_cache_guard.rs`.
5. **Delete `storage.rs` legacy codec and `lifecycle_segment_codec.rs`**, update remaining tests.
6. **Update CLI/bench/examples** (`index_storage_action`, snapshot benchmark deletion, option removals).
7. **Update docs and write the completion report**; final validation and campaign.

Push each commit immediately.

## 7. Acceptance criteria

- No production, test, example or active-doc references to the deleted snapshot/lifecycle-cache types or APIs remain.
- `RepositoryOptions` has no `snapshot_mode`; repository opening always builds from source or receives a managed generation.
- `Index` exposes only managed persistence APIs; no `flush_cache`/`load_cache`/`sync_cache`.
- `lifecycle_repository.rs`, `lifecycle_segment_codec.rs`, `lifecycle_cache_guard.rs`, and the legacy portion of `storage.rs` are gone.
- CLI emits `index_storage_action`, not `snapshot_action`.
- Workspace tests, Clippy and formatting checks pass.
- Query digests and managed storage behavior are unchanged.
- The report records the removed public API surface, line/file reduction, migrated tests, and the absence of any remaining legacy persistence path.
- `master` is untouched; the integration branch is ready for the user's merge.

## 8. Report

Write `docs/ai/generated/remove_legacy_snapshot_stack_results.md` with:

- base/final revisions, branch/worktree, commits;
- deleted files and approximate line counts;
- extracted/moved configuration-hash code and tests;
- test migration summary (files migrated/deleted);
- CLI output change and TUI/desktop smoke results;
- query/build/open validation results and digest parity;
- remaining API breaks (`RepositoryOpenOptions`, snapshot open/save APIs, `Index::flush_cache`/`load_cache`, `sync_cache`);
- confirmation that managed storage is the only persistence path;
- any limitation or follow-up.

## Implementation report

- **Task/date:** `remove-legacy-snapshot-stack`, completed 14 November 2026.
- **Base and integration:** `0c63651458` (`integration/direct-snapshot-access`, descended from the plan's hybrid base); `integration/remove-legacy-snapshot-stack` in `/home/simon/source/objekte-integration-remove-legacy-snapshot-stack`.
- **Contributing work:** `e5d6f250bd` extracted and tested stable configuration hashing; `eb98d7ef18` merged the separately completed and pushed `integration/remove-binding-cache`; `da608b2799` deleted the legacy snapshot and lifecycle-cache stack, migrated tests and clients, and updated active documentation. Ten files were deleted outright. The removal commit reduced the code by 8,137 net lines.
- **Behavior:** The managed generation store is the only remaining persistence implementation. Static `Repository` opening builds from source. The CLI now reports `index_storage_action`; snapshot/cache APIs and their options are intentionally removed.
- **Validation:** `cargo test --workspace --locked` and `cargo clippy --workspace --all-targets --locked` passed, as did the detailed-profiling all-targets check, changed Python script compilation, and `git diff --check`. The 220-case extended ASCII/Unicode campaign, five repetitions each in memory and managed on-demand modes, returned zero failures and exact parity with the prior binding-cache candidate's 220 digests in each mode. The managed reopen performed zero source reads and zero build parses. CLI, TUI, and desktop workspace tests passed; no interactive GUI run was made. Full commands, environment, raw measurements, build/open counters, and timing ratios are in [the detailed report](../generated/remove_legacy_snapshot_stack_results.md) and [the benchmark archive](../../../bench/results/remove-legacy-snapshot-stack/final-20261114/README.md).
- **Formatting exception:** The repository-wide `cargo fmt --all -- --check` and `python3 scripts/format-rust-spacing.py --check` report pre-existing broad drift, including untouched files. This task did not rewrite unrelated formatting.
- **Issue sorting:** The obsolete snapshot-buffer issue and resolved collection-wide verified-base rewrite issue moved to `done/resolved-issues/`; the remaining bounded verified-base update latency has its own `issues/perf/` report. Other open issues remain in their matching category folders.
- **Cleanup:** The task's temporary memory is to be deleted after the completion commit is pushed. The integration worktree is retained for the user's review and the explicitly authorized merge into `integration/direct-snapshot-access`. The untracked source task document in the original checkout is to be removed only after its content preservation and push are verified.
