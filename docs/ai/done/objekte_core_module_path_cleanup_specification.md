# Interim assignment — Remove compatibility façades and relocate core helpers

**Task** `core-module-path-cleanup`  
**Post-Assignment 3 reference tree** `integration/core-physical-indexes` at `c982344f87c9ff01d1316bba4b9b3d7c5bde53e7`  
**Starting branch** `origin/master` after the owner merges Assignment 3  
**Integration branch** `integration/core-module-path-cleanup`  
**Sibling worktree** `/home/simon/source/objekte-integration-core-module-path-cleanup`

## Delivery decision

Start this work after Assignment 3 has merged. Use the merged `origin/master` in the sibling worktree and record its SHA. The physical families already live under `src/index/`. If the merge was squashed, verify that the same files and operations are present. Deliver this cleanup for review before Assignment 4 starts.

**Rust module paths may break.** Give each moved item one canonical path and migrate every maintained caller in the same branch. Delete the old file and its module declaration. The same rule applies to the remaining roadmap. An inline module alias in `lib.rs`, a deprecated re-export, a Cargo feature restoring the old path or a second forwarding file would reproduce the original problem. A module root that declares real children is useful. An intentional public type export can be the sole public name of a type whose implementation module is private.

At the inspected commit, `crates/core/src/` has **107 direct Rust files**, including **50 `*.test.rs` companions**. There are two pure forwarding files: `run.rs` and `build.rs`. The disposition below removes **33** direct entries: two forwarders and 31 real source/test files relocated to an owner. On this base the result is **74 direct Rust files, including 35 root test companions**. If the merge contains additional unrelated changes, record their root-file delta separately. Count tracked files, including tests. [Baseline tree](https://github.com/Simon-Martens/objekte/tree/c982344f87c9ff01d1316bba4b9b3d7c5bde53e7/crates/core/src)

## 1. Delete the two forwarding modules

| Remove | Migrate by symbol |
| --- | --- |
| `src/run.rs` and `pub mod run;` | `QueryPlanTrace`, `PlanActual`, `ExecutionObservation`, `ObservationStatus`, `BoundTermDiagnostic` and `RelationHopDiagnostic` belong to `query_diagnostics::trace`. Functions defined by the runner, such as `search`, `search_stream`, `explain` and `search_analyze`, belong to `query_execution::runner`. `DirectLookupResult` currently belongs there too. For items merely forwarded by the runner, such as cache operations, follow the definition. Change the caller directly to the defining owner. Replace runner-side public forwarding imports of trace types with ordinary imports. |
| `src/build.rs` and `mod build;` | Route `BatchContribution` to `construction::batch`, `PreparedFileBuild` and `prepare_file_build` to `construction::file_preparation`, worker selection to `construction::scheduling`, and `SegmentAdmission` and `CollectionSegment` to `construction::segment_admission`. `IndexedCollectionSegment`, `SegmentFile` and `SegmentId` belong to `index::segment`. Remove their named forwarding import in `construction/mod.rs`. `lifecycle_partition.rs` already imports `index::segment::PartitionRouting`, the canonical crate-internal segment operation. Keep that path and its private projection implementation. |

Expose the relevant construction children at the minimum required visibility and eliminate the four wildcard re-exports in `construction/mod.rs`. Keep real borrowed/moved values and signatures. For runner functions with external workspace callers, use the canonical public runner module or the caller’s existing `IndexView` method where the semantics match. Make `query_diagnostics::trace` public for its externally used diagnostic types. If downstream code imports a runner function, make `query_execution::runner` public. Keep `domain` and `request` crate-visible. Expose cache controls only where current maintained callers require them. Keep access-provider, planning and cache implementation details private or crate-visible. Simply replacing `run::*` with another wildcard would leave the ownership ambiguous. [Original runner façade](https://github.com/Simon-Martens/objekte/blob/c982344f87c9ff01d1316bba4b9b3d7c5bde53e7/crates/core/src/run.rs) · [Construction façade](https://github.com/Simon-Martens/objekte/blob/c982344f87c9ff01d1316bba4b9b3d7c5bde53e7/crates/core/src/build.rs) · [Construction declarations](https://github.com/Simon-Martens/objekte/blob/c982344f87c9ff01d1316bba4b9b3d7c5bde53e7/crates/core/src/construction/mod.rs)

## 2. Remove the Assignment 3 path aliases

1. Change `mod index;` to `pub mod index;` in `lib.rs`. Its five family modules are already public. Delete `pub use index::{bigram, posting, segment as native_segment, sorting};`. Migrate all internal and downstream imports to `index::bigram`, `index::posting`, `index::segment` and `index::sorting`. This includes signatures, tests and physical code that uses the old paths internally.
2. Root `relation.rs` owns real declarations and stays. Remove its two forwarding imports of `RelationEdge`, `RelationIndex`, `RelationTarget`, `RelationContribution` and `validate_indexes`. Use `index::relation` for these symbols and `relation` for `RelationDefinition`, `RelationCardinality` and `RelationId`. Update the module documentation. Physical internals keep their present visibility.
3. Leave private implementation modules and intentional exports from the owning physical family in place. For example, a caller can use `index::segment::PartitionRouting` without exposing `projection` or `routing`. Remove aliases that duplicate an owner at a different path. [Root aliases](https://github.com/Simon-Martens/objekte/blob/c982344f87c9ff01d1316bba4b9b3d7c5bde53e7/crates/core/src/lib.rs#L72) · [Relation aliases](https://github.com/Simon-Martens/objekte/blob/c982344f87c9ff01d1316bba4b9b3d7c5bde53e7/crates/core/src/relation.rs#L11)

## 3. Move real code into the following owners

All paths below are relative to `crates/core/src/`. A test listed alongside a file moves with its owner. Preserve its assertions and feature gates. Prefer `#[cfg(test)] mod tests;` and a module-local `tests.rs`. Where two suites belong to one owner, use distinct child names.

| Current root file(s) | Destination | Reason |
| --- | --- | --- |
| `query.rs`, `query.test.rs` | `query/mod.rs`, `query/tests.rs` | The actual parser and request types form the root of the query language module. Move the implementation into the directory. |
| `sort_binding.rs`, `sort_binding.test.rs` | `query/sort_binding.rs`, `query/sort_binding/tests.rs` | Binds requested sort terms and prepares query-scoped keys. The ICU columns and stored order remain the physical index owner’s responsibility. |
| `query_path.rs`, `query_validation.rs`, `regex_query.rs`, `temporal_query.rs` and their four companions | `query/path.rs`, `query/validation.rs`, `query/regex.rs`, `query/temporal.rs`, each with `<module>/tests.rs` | Query paths, definition validation, regex compilation and temporal operands. Use an absolute external-crate import if the local `regex` name shadows it. |
| `byte_size_literal.rs`, `clock_context.rs`, `parameters.rs` and their three companions | `query/size_literal.rs`, `query/clock.rs`, `query/parameters.rs`, each with `<module>/tests.rs` | FILTER literals, frozen query time and parameter binding/transport. Preserve lexical and time semantics. |
| `prepared.rs` | `query/prepared.rs` | Binds a request for execution. Keep its inline tests, if present, beside the implementation. |
| `executor.rs`, `executor.test.rs`, `dynamic_filter.test.rs` | `query_execution/restrictions.rs`, `query_execution/restrictions/tests.rs`, `query_execution/restrictions/dynamic_filter_tests.rs` | Actual candidate restriction, access estimates and residual execution. Move the bodies mechanically. Update references to public estimate types at their defining path. |
| `materialization.rs`, `fts_preview.rs` | `query_results/payload.rs`, `query_results/fts_preview.rs` | Decode selected source bytes and present bounded FTS evidence. `query_results/materialize.rs` already owns page-level materialization. Keep their distinct duties and shared call path. |
| `verified.rs`, `verified.test.rs` | `query_results/verified.rs`, `query_results/verified/tests.rs` | Verified-result storage and ordering. Existing `query_results/cache.rs` orchestrates use of that storage. |
| `index_catalog.rs` | `query_access/catalog.rs` | Borrows native access paths and assesses their capabilities. Physical storage stays behind this adapter. The adapter continues to mediate native index access. |
| `lifecycle_caller.rs`, `lifecycle_caller.test.rs` | `lifecycle/session.rs`, `lifecycle/session/tests.rs` | `IndexedRepository` owns an `Index` and chooses live or pinned reads. Keep that behavior and the owner’s lifetime. Migrate CLI and desktop imports to `lifecycle::session::IndexedRepository`. |
| `build.test.rs`, `run.test.rs` | `construction/tests.rs`, `query_execution/runner/tests.rs` | The tests belong to the implementations whose old forwarding files were deleted. |

The query packages already present as directories can keep their names. For the runner’s private children, move the existing `query_execution/runner.rs` implementation to `query_execution/runner/mod.rs`. Move its `relations.rs`, `segment_scan.rs`, `source_scan.rs` and the runner-owned `query_results/ordered.rs` to sibling files inside `runner/`. Replace their `#[path]` attributes with ordinary child declarations. `ordered.rs` already compiles as a child of the runner and uses runner-private names. Preserve that relationship and avoid widening its internals. The pre-index source-search API in root `source_scan.rs` is a separate implementation. [Runner module wiring](https://github.com/Simon-Martens/objekte/blob/c982344f87c9ff01d1316bba4b9b3d7c5bde53e7/crates/core/src/query_execution/runner.rs) · [Sort binding](https://github.com/Simon-Martens/objekte/blob/c982344f87c9ff01d1316bba4b9b3d7c5bde53e7/crates/core/src/sort_binding.rs)

## 4. Migrate every maintained caller

- Search the full workspace for qualified paths, brace-group imports, aliases, wildcard imports, `pub use`, rustdoc links, examples, doctests, feature-gated code and `#[path]` attributes. Cover core, CLI, desktop and active usage documentation. Preserve completed historical reports as records of their revisions.
- Remove old-path forwarding from mixed modules touched by this change. For example, `workload.rs` implements real workload types. Keep those definitions there. Change callers of `workload::{ReturnMode, SortField, SortExpression, SortReducer, BigramAlphabet}` to `execution_options`, and callers of `workload::WorkCounters` to `measurement`. Change the re-exports to private imports only where the workload code itself needs them. A type used by query diagnostics should have one public home.
- Keep the actual filter/FTS provider contract, domain checks, term selection, residual verification and pinned read behavior. Introducing a module boundary is no reason to clone a bitmap, collect a membership iterator, reconstruct results, introduce a dispatch layer, or route a query through repository metadata.

## Commit and acceptance sequence

1. Record the base SHA, root inventory and Cargo test inventory. Commit the deletion of the two forwarding modules with the migration of their callers and the wiring of both former companion test suites. In a second compiling commit, remove the Assignment 3 aliases and migrate their callers. Keep the tests active in every commit.
2. Move query-language helpers and tests, then runner/result/access/session files. Each commit updates real call sites and removes old declarations. Review moved function bodies with `git diff --color-moved`. Make only import, module declaration and essential visibility adjustments.
3. Count direct Rust files with `git ls-files -- ':(glob)crates/core/src/*.rs' | wc -l`. Report the before/after paths as well as the count. Compare against the mapped list so moving another unrelated file cannot mask a missed deletion. Inspect `lib.rs` and module roots for new inline redirects. As an initial search use `rg -n 'pub(\([^)]*\))? use .*::\*;' crates/core/src`, then inspect named aliases and renamed imports as well. Any remaining public export in the touched surface must have a current API purpose other than preserving a superseded name.
4. Run `cargo fmt --all -- --check`, `cargo check --locked --workspace --all-targets`, `cargo test --locked --workspace`, `cargo test --locked -p objekte-search-core --all-features` and `RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links" cargo doc --locked --workspace --no-deps`. Compare test inventories before and after in both default and all-feature modes, allowing module-prefix changes. Confirm the executor, dynamic filter, runner, construction and sort-binding suites actually run. Reuse the independent scalar/gram provider and selective FTS/FILTER tests. Source-only relocations need no corpus benchmark.
5. Follow `docs/ai/README.md`. Push the integration branch and record the tested SHA, root counts, canonical path map and test results in its completion report. The owner reviews and merges the cleanup before Assignment 4.

## Implementation report

- **Task identifier:** `core-module-path-cleanup`
- **Completion date:** 2026-11-18
- **Base commit:** `c982344f87c9ff01d1316bba4b9b3d7c5bde53e7`
- **Integration branch:** `integration/core-module-path-cleanup`
- **Integration worktree:** `/home/simon/source/objekte-integration-core-module-path-cleanup`

### Implemented changes

The two forwarding modules, `build.rs` and `run.rs`, and their declarations were removed. Maintained callers now import each item from its defining owner. Construction children expose only the required crate-visible surface, diagnostic trace types have a public canonical module, and runner functions and children now live under `query_execution::runner`.

The Assignment 3 aliases were removed. `index` is a public module with the canonical `index::{bigram, posting, segment, sorting}` paths. Relation symbols imported from physical indexes now use `index::relation`; relation definitions continue to use the root `relation` owner.

The query language, execution, result, access, session, construction-test, and runner files listed by the specification were moved into their owner modules. The old declarations and forwarding paths were deleted. Runner-private `relations`, `segment_scan`, `source_scan`, and `ordered` are ordinary children of `runner`. Construction integration tests are wired as `construction::tests`.

The remaining compatibility exports in `workload.rs` were removed. Runtime options now use `execution_options` and work counters use `measurement`; all maintained core, CLI, desktop, example, integration-test, doctest, and active-source callers were migrated to those canonical paths. Historical completed reports and benchmark snapshots were left unchanged.

The all-feature test build also exposed one feature-gated construction test import that still depended on the deleted `build` forwarding surface. `process_batch_profiled` is now imported directly from `construction::batch`.

### Canonical path map

| Former root path | Canonical owner |
| --- | --- |
| `build.rs` / `build.test.rs` | `construction/*` and `construction/tests.rs` |
| `run.rs` / `run.test.rs` | `query_diagnostics::trace`, `query_execution::runner/*` |
| `query.rs` | `query/mod.rs` |
| `clock_context.rs` | `query/clock.rs` |
| `parameters.rs` | `query/parameters.rs` |
| `query_path.rs` | `query/path.rs` |
| `query_validation.rs` | `query/validation.rs` |
| `regex_query.rs` | `query/regex.rs` |
| `byte_size_literal.rs` | `query/size_literal.rs` |
| `sort_binding.rs` | `query/sort_binding.rs` |
| `temporal_query.rs` | `query/temporal.rs` |
| `prepared.rs` | `query/prepared.rs` |
| `executor.rs` | `query_execution/restrictions.rs` |
| `fts_preview.rs` | `query_results/fts_preview.rs` |
| `materialization.rs` | `query_results/payload.rs` |
| `verified.rs` | `query_results/verified.rs` |
| `index_catalog.rs` | `query_access/catalog.rs` |
| `lifecycle_caller.rs` | `lifecycle/session.rs` |
| `query_execution/runner.rs` | `query_execution/runner/mod.rs` |

Each moved source retained its companion assertions and feature gates under the mapped `tests.rs` path.

### Implementation commits

- `641570a723` — remove `build` and `run` compatibility façades
- `1cd45a8d82` — use canonical index and relation paths
- `c064a24226` — move query language helpers under `query`
- `bed1f83026` — move restriction execution into `query_execution`
- `780d4729a3` — move result payload and access adapters to owners
- `5ae7864e19` — move `IndexedRepository` to `lifecycle::session`
- `47453fa798` — move query runner children into the canonical module
- `7877575a74` — move construction integration tests to the construction owner
- `8c478819c9` — remove workload compatibility re-exports
- `0097d6f213` — import the feature-gated profiling helper from `construction::batch`

The implementation head used for final validation was `0097d6f213fc27475f7b0fb00db2502fe1a67f69`.

### Inventory and API audit

The direct `crates/core/src/*.rs` inventory changed from 107 files, including 50 root test companions, to 74 files, including 35 root test companions. This matches the specification's expected post-move inventory. The moved-path diff matches the mapped list; no unrelated core source file was removed to reach the count.

The audit found no public wildcard re-exports under `crates/core/src`, no `native_segment` or other Assignment 3 compatibility aliases, and no maintained `workload::{ReturnMode, SortField, SortExpression, SortReducer, BigramAlphabet, WorkCounters}` imports. Remaining named `pub use` declarations export definitions from explicit implementation children for current API use. `#[path]` attributes that remain connect active companion tests or the existing split inference implementation, not superseded root paths.

### Validation

The following commands passed at the implementation head:

- `cargo fmt --all -- --check`
- `cargo check --locked --workspace --all-targets`
- `cargo test --locked --workspace` — 885 passed, 18 ignored
- `cargo test --locked -p objekte-search-core --all-features` — 611 passed, 16 ignored
- `RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links" cargo doc --locked --workspace --no-deps`

The initial all-feature run found the missing `process_batch_profiled` import described above. Commit `0097d6f213` corrected it, and both the all-feature and workspace test commands were rerun successfully afterward.

The core test inventory remained 626 tests with default features and 627 tests with all features. Normalizing module prefixes leaves the same set of assertions before and after; only the documented repository compile-fail example moves from line 229 to line 230. The inventories confirm that the construction, sort-binding, restrictions, dynamic-filter, runner, lifecycle-session, and result suites are active. The workspace run additionally exercised CLI, desktop, integration, and doctest suites.

No corpus benchmark was run because this task is a source-only relocation and API-path migration. Function bodies were retained, and no query hot-path allocation, copying, dispatch, or execution behavior was intentionally changed.

There were no subordinate worktrees for this task. The integration branch and worktree are retained for review. Temporary task memory is removed after this report is pushed; the task implementation and report have no unresolved limitations.
