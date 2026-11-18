# Assignment 1 — Lifecycle publication and pinned read views

**Task ID** `lifecycle-publication-and-read-views`  
**Reviewed base** `master` at `1bafff6fa53be3215f0b48731550cce569f699c3`  
**Delivery branch** `integration/lifecycle-publication-and-read-views`  
**Sibling worktree** `/home/simon/source/objekte-integration-lifecycle-publication-and-read-views`

## Outcome

A maintainer following `Index::view()` can find the pinned reader in `lifecycle/view.rs`. A maintainer following a publication conflict can find the revision checks and root installation in `lifecycle/publication.rs`. Current callers continue to import `objekte_search_core::lifecycle::{Index, IndexView, IndexRevision}`. This is a mechanical, core-only refactoring. Keep the query engine and remaining roadmap assignments outside this change.

Create the branch and sibling worktree from current `origin/master` at assignment time and record that SHA. The SHA above anchors the symbols and source links. Use `AGENTS.md` and `docs/ai/README.md` for task handling, explicit staging, pushing each commit and delivery. The original checkout stays on its current branch. One implementer can supervise this task because both extractions touch the same original file. [Repository workflow](https://github.com/Simon-Martens/objekte/blob/1bafff6fa53be3215f0b48731550cce569f699c3/docs/ai/README.md)

## File and symbol assignments

All paths below are relative to `crates/core/src/`. Keep the ordinary `pub mod lifecycle;` declaration in `lib.rs`.

| Destination | Exact work |
| --- | --- |
| `lifecycle/mod.rs` | Move `lifecycle.rs` here. Retain `Index`, `IndexInner`, `IndexRevision`, `FileHandle`, `IndexError`, status/report types, tasks, counters, mutation staging, reconciliation, the verified-cache owner and handle constructors. Keep the existing `FileVersion`, initial-build types and `PublishedSegment` re-exports at their current visibility. Add private `mod view;` and `mod publication;`, `pub use view::IndexView;`, and `pub(crate) use publication::PublishedIndex;`. Keep the index-family estimation helpers here in this assignment. |
| `lifecycle/view.rs` | Move the `IndexView` tuple struct, its derive and its entire current inherent implementation, including its private helpers, stream wrappers and diagnostic remapping. Move just `Index::view` into an `impl Index` block in this file. It can then construct the tuple directly. Keep field 0 crate-visible for existing users, and field 1 private as `Weak<IndexInner>`. |
| `lifecycle/publication.rs` | Move `PublishedIndex`, its entire existing inherent implementation, `cheapest_consolidation_pair`, and `prepare_initial_segment`. In a separate `impl Index` block in this file move `publish_initial_segment`, `publish_initial_segments`, `finish_initial_construction`, `publish_prepared` and `publish_prepared_inner`. Move their existing documentation and attributes with them. |
| `lifecycle/tests.rs` | Move `lifecycle.test.rs` here. Replace the old path attribute with `#[cfg(test)] mod tests;` in `mod.rs`. This keeps the test module at `lifecycle::tests` and preserves its `super::` references. |
| `lifecycle_mutation.test.rs` | Reuse or add the public revision-pinning regression described below next to the existing update tests. This file remains with its current module. |

Keep all crate-root `lifecycle_*.rs` modules in their current public or private locations for this assignment. Existing callers include incremental construction, mutation, compaction, posting inspection and relation execution. Use explicit imports in the extracted modules. Retain crate-visible symbols at their original module paths. For previously private methods now called from the parent or its tests, use the narrowest sufficient visibility. Current examples are `PublishedIndex::status`, `retire_file_records` and `make_room_for_segment_with_limits`, for which `pub(super)` is sufficient. Keep `IndexInner` and its fields private to the lifecycle hierarchy. [Published root](https://github.com/Simon-Martens/objekte/blob/1bafff6fa53be3215f0b48731550cce569f699c3/crates/core/src/lifecycle.rs#L411) · [View acquisition](https://github.com/Simon-Martens/objekte/blob/1bafff6fa53be3215f0b48731550cce569f699c3/crates/core/src/lifecycle.rs#L1742) · [Pinned reader](https://github.com/Simon-Martens/objekte/blob/1bafff6fa53be3215f0b48731550cce569f699c3/crates/core/src/lifecycle.rs#L2720)

## Behavior to preserve

1. **Pinning.** `Index::view` takes the publication read lock, clones the existing root's `Arc`, and stores a weak reference to the index owner. Queries run after the read guard is released. Preserve the same ownership and allocation operations, including query behavior when the weak owner cannot be upgraded. Never clone a root, bitmap, catalog or result just to cross a module boundary.
2. **Installation.** Preserve the existing validation and error ordering. Check the instance, calculate the successor revision and validate the staged revision before installing. Under the existing write lock, check quiescence and compare the current revision with the expected predecessor. Replace the root once and drop the retired `Arc` after releasing the guard. Preserve the existing writer synchronization and atomic memory orderings. [Installation sequence](https://github.com/Simon-Martens/objekte/blob/1bafff6fa53be3215f0b48731550cce569f699c3/crates/core/src/lifecycle.rs#L2666)
3. **Initial construction.** Preserve grouped publication, timing instrumentation, completion checks, inference finalization and the revision at which a group becomes visible. Leave staging, filesystem work and heavyweight construction outside the installation lock as they are today. Move existing bodies intact, including their current optimizations.
4. **Compatibility.** Preserve public Rust paths and signatures, inherent-method availability, serde derives, revision/file-handle values, error variants and returned generations. Keep the existing static/segmented search dispatch, cache identity and source-availability semantics. Defer algorithm changes and further module splits to their own assignments.

## Acceptance test through the public API

Start from the existing mutation fixtures and run the test against the pre-move implementation. It documents a behavior that must already pass. Retain existing coverage for stale, skipped and foreign publications. The existing pinned-view test publishes a prepared root after adding a second file. The test below exercises a replacement of the **same** file using the public writer API. [Existing pinning test](https://github.com/Simon-Martens/objekte/blob/1bafff6fa53be3215f0b48731550cce569f699c3/crates/core/src/lifecycle.test.rs#L378) · [Public update](https://github.com/Simon-Martens/objekte/blob/1bafff6fa53be3215f0b48731550cce569f699c3/crates/core/src/lifecycle_mutation.rs#L410)

1. Build one JSONL collection with intrinsic record ID `id`, an explicitly indexed numeric `rank` field and a disabled verified-result cache. Write a single record with ID `a` and rank `10`. Acquire `old_view` and its file handle. Confirm an exact indexed count for rank 10 is one.
2. Replace the file contents with the same ID and rank `2002`. The changed byte length makes this deterministic across filesystem timestamp resolutions. Call `index.update_file(handle, &path)` and acquire `new_view`. Keep both views alive.
3. After publication, execute the same two queries through `IndexView::count_exact` on **both** views. Use evidence disabled and locator return mode. Expect the following results.

| View | `rank = 10` | `rank = 2002` | Revision |
| --- | ---: | ---: | --- |
| `old_view` | 1 | 0 | Exactly the revision captured before the update |
| `new_view` | 0 | 1 | The revision returned by the update and the successor of the old revision for this isolated fixture |

4. Verify the old view still returns the original file handle, and the new view returns the version from the successful update. Use the fixture's normal shutdown and cleanup. The test queries pinned index membership. Source files belong to the application and may already contain the replacement bytes. Avoid asserting that reading an old locator returns historical file contents.

## Commit and validation sequence

| Commit | Work and check |
| --- | --- |
| `Test pinned index membership across file replacement` | Run the new or extended regression on the recorded base. If an existing test already exercises all four post-update cells through the public API, cite and reuse it. |
| `Establish the lifecycle module directory` | Move the module and its companion test file. Update the test declaration. Run the lifecycle tests before extracting either owner. |
| `Extract pinned index view acquisition and reads` | Move the reader and acquisition method as specified. Compile all core targets and rerun the pinned-view and replacement tests. |
| `Extract index revision publication` | Move the published root and installation methods. Review `git diff --color-moved` against the base for changes to bodies, lock scopes, clones, error ordering and visibility. Run the final checks below. |

```sh
cargo fmt -p objekte-search-core -- --check
cargo check --locked -p objekte-search-core --all-targets --all-features
cargo test --locked -p objekte-search-core
cargo check --locked -p objekte-cli
```

The CLI check exercises a downstream consumer without changing its code. The core tests cover lifecycle publication conflicts, quiescence, mutation, initial construction and public imports. In the completion report identify the two-view regression by name, list the files and methods moved, and record the base, tested implementation SHA and command results. Include any material behavioral difference or unexpected allocation as a finding to resolve before delivery. A full Musenalm timing suite adds little evidence for a source move with identical hot-path bodies. Deliver the tested integration branch for owner review.

## Implementation report

- Task identifier: `lifecycle-publication-and-read-views`
- Completion date: 2026-11-18
- Assignment base: `0a0e0c3507af94164fce2387070cfcb6b5277d23`
- Integration branch: `integration/lifecycle-publication-and-read-views`
- Tested implementation: `b14234cf5cc8ef9ac8beb20e05d3b9a0e0efe774`

### Implemented changes

The core lifecycle implementation now has a stable module directory while preserving the ordinary `pub mod lifecycle;` crate declaration and all existing public paths.

- Moved `crates/core/src/lifecycle.rs` to `crates/core/src/lifecycle/mod.rs`.
- Moved `crates/core/src/lifecycle.test.rs` to `crates/core/src/lifecycle/tests.rs` and retained the test module name as `lifecycle::tests`.
- Moved `IndexView`, its complete inherent implementation, private helpers, stream wrappers and diagnostic remapping to `crates/core/src/lifecycle/view.rs`.
- Moved `Index::view` to a separate `impl Index` block in `view.rs` and preserved the publication read lock, `Arc` clone, weak owner acquisition and query behavior.
- Moved `PublishedIndex`, its complete inherent implementation, `cheapest_consolidation_pair` and `prepare_initial_segment` to `crates/core/src/lifecycle/publication.rs`.
- Moved `Index::publish_initial_segment`, `publish_initial_segments`, `finish_initial_construction`, `publish_prepared` and `publish_prepared_inner` to a separate `impl Index` block in `publication.rs`.
- Re-exported `IndexView` with `pub use view::IndexView` and `PublishedIndex` with `pub(crate) use publication::PublishedIndex`.
- Narrowed only the required cross-module helper visibility: `PublishedIndex::status`, `retire_file_records` and `make_room_for_segment_with_limits` use `pub(super)`.

Normalized source comparisons against the pre-move implementation were exact for every moved range after accounting for explicit imports, the separate `impl Index` wrapper and the three required visibility changes. No lock scope, clone, allocation, revision check, error ordering or initial-construction body changed.

The public-API regression `pinned_views_keep_membership_and_file_handles_across_same_file_replacement` was added to `lifecycle_mutation.test.rs`. It verifies both pinned views after replacing the same file through `Index::update_file`, including the four `rank = 10` / `rank = 2002` membership cells, unchanged old revision, successor new revision and old/new file handles.

### Commits

- `8de5161510172516e77c527ccbf873761e802d55` — Test pinned index membership across file replacement
- `70a07a4dcc3fc095755baeee0762a4b4e6089605` — Establish the lifecycle module directory
- `67072117c9` — Extract pinned index view acquisition and reads
- `b14234cf5cc8ef9ac8beb20e05d3b9a0e0efe774` — Extract index revision publication

No subtask branches were created. The task was implemented and supervised on its integration branch.

### Validation

| Command | Result |
| --- | --- |
| `cargo test -p objekte-search-core pinned_views_keep_membership_and_file_handles_across_same_file_replacement -- --nocapture` on the recorded base | Passed before the module move. |
| `cargo test --locked -p objekte-search-core lifecycle:: -- --nocapture` | Passed: 25 lifecycle tests. |
| `cargo check --locked -p objekte-search-core --all-targets` | Passed. |
| `cargo test --locked -p objekte-search-core pinned_ -- --nocapture` | Passed: 9 pinned-view tests after reader extraction. |
| `cargo fmt -p objekte-search-core -- --check` | Passed. |
| `cargo test --locked -p objekte-search-core` | Passed: 602 tests passed, 16 ignored, 0 failed. |
| `cargo check --locked -p objekte-cli` | Passed. |
| `cargo check --locked -p objekte-search-core --all-targets --all-features` | Failed on both the assignment base and the integration branch because `query_results/cache.rs` references the missing `crate::profiling` module when `detailed-profiling` is enabled. |

The all-feature failure is pre-existing and is recorded in [`detailed_profiling_missing_module.md`](../issues/bug/detailed_profiling_missing_module.md). It was not fixed here because this task is a mechanical lifecycle-only source move and changing query-result cache code would exceed that scope.

No benchmark was run: this task moves unchanged bodies between modules, has no behavioral hot-path change, and the specification notes that a full Musenalm timing suite would add little evidence. No unexpected allocation or behavioral difference was found by source comparison or tests.

### Limitations and cleanup

The only unresolved validation limitation is the baseline all-feature compile failure linked above. No subordinate worktree was created, so there was no subordinate cleanup. The integration worktree is intentionally retained for owner review at `/home/simon/source/objekte-integration-lifecycle-publication-and-read-views`.
