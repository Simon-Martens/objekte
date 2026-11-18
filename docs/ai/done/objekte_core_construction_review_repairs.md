# Assignment 2 — Construction review repairs

**Repository** `Simon-Martens/objekte`  
**Integration branch** `integration/core-index-construction`  
**Reviewed tip** `0eeffe49605fe08758eea3a8314995b8a288e195`  
**Comparison base** `407410cbc28ae2a2c6fa89ba0e6af2458157c472`  
**Delivery** Repair the existing integration branch in its sibling worktree. The owner decides the merge.

Keep the four-argument `Index::start_build` as the sole public initial-build operation. Keep progressive publication, repeated waits on the same task, the registered-reader path, and the completed detailed-profiling compilation fix. The following findings concern the committed implementation and its callers. [Reviewed changes](https://github.com/Simon-Martens/objekte/compare/407410cbc28ae2a2c6fa89ba0e6af2458157c472...0eeffe49605fe08758eea3a8314995b8a288e195)

## R1. Retain the index owner for the full benchmark or query session

`bench_musenalm` defaults to `SearchBackend::Direct(IndexView)`. It constructs the view inside `.and_then(|index| index.view())`, dropping the caller’s only strong `Index`. All profiles subsequently call `configure_verified_cache`. That method upgrades the view’s weak owner and returns “index owner is no longer available” once the owner has gone. A brief race with worker cleanup is possible. Both cache-off and warm profiles require cache configuration. The same error exists in `bench_case_modes` and the range workload of `bench_corrective_cases`. Ordinary queries that allow an absent owner can also silently run without lifecycle caching. Compiling examples never exercises these paths. [Default benchmark](https://github.com/Simon-Martens/objekte/blob/0eeffe49605fe08758eea3a8314995b8a288e195/crates/core/examples/bench_musenalm.rs#L295) · [Case modes](https://github.com/Simon-Martens/objekte/blob/0eeffe49605fe08758eea3a8314995b8a288e195/crates/core/examples/bench_case_modes.rs#L213) · [Corrective cases](https://github.com/Simon-Martens/objekte/blob/0eeffe49605fe08758eea3a8314995b8a288e195/crates/core/examples/bench_corrective_cases.rs#L514)

- Bind the completed `Index` to a variable or store it in the benchmark session. Derive the pinned view from that owner. Configure the cache through `Index::configure_verified_cache` and query through the pinned view. Keep the owner through the final query. For `bench_musenalm`, share this ownership model across both command-line modes. Preserve the existing differences in reported milestones or measured workload.
- Audit the other migrated `index.view()` closures, including helpers returning an `IndexView`. Retain an owner wherever a caller measures caching or another owner-backed facility. A deliberately detached view remains valid for its established read-only behavior. Keep `IndexView`’s weak back-reference. Remove the newly introduced public cache setter on the view once callers use the owner.
- Run a real cache-off and warm case through the default `bench_musenalm` command without `--incremental`, and the same case with that flag. Use one existing case ID and one repetition. Run the cache-configuration paths in the other two examples or a focused smoke fixture invoking their actual setup. Assert success, matching IDs and existing cache-hit evidence for one repeated cache-eligible query. Use a local fixture for that last assertion if the chosen corpus query has no cacheable verification.

## R2. Eliminate the unbounded exclusive-ownership wait

`Index::into_repository` retries `Arc::try_unwrap` and calls `yield_now()` until both the index and its published root have no other owners. A retained task owns an index and a view. A retained pinned view holds the published root. Either can prevent return indefinitely, including when the same thread is responsible for dropping it. The implementation cannot distinguish a transient worker reference from a legitimate long-lived reader. Numerous migrated tests now rely on this bridge. [Unwrap loops](https://github.com/Simon-Martens/objekte/blob/0eeffe49605fe08758eea3a8314995b8a288e195/crates/core/src/lifecycle/mod.rs#L866) · [Task ownership](https://github.com/Simon-Martens/objekte/blob/0eeffe49605fe08758eea3a8314995b8a288e195/crates/core/src/lifecycle_initial.rs#L156)

- Migrate ordinary test setup to retain the completed index and query its view. Borrow repository-private build metadata from the pinned view for tests explicitly checking that metadata. Do not clone a repository, segment, posting, bitmap or search result to solve ownership. The counting-reader test can call the maintenance initializer through its borrowed repository.
- Keep genuine repository-adoption tests. If these need an owned `Repository`, isolate the consuming fixture behind `#[cfg(test)]`. A test can use the existing private construction worker with its completion return, without task-held owners, then perform one checked unwrap per layer. Alternatively eliminate the conversion if existing adoption fixtures already supply a real repository. No production or test conversion may wait indefinitely for callers to release a handle. Do not change normal `wait_complete()` into an ownership transfer.
- If any conversion remains, test a deliberately retained owner and a retained view separately. The caller must get a prompt typed error. Keep a successful exclusive-adoption test with the original allocation-reclamation assertions. Do not replace the loop with an arbitrary sleep, retry count or timeout.

## R3. Validate segment limits at the single public entry

The old synchronous entry checked `options.segment_policy.validate()`. The new entry checks inference and relations and then spawns a worker. `SegmentPolicy` has public fields, so a caller can supply a nonzero record or file limit above `u32::MAX` on a 64-bit target. The admission constructor stores it directly. [Previous validation](https://github.com/Simon-Martens/objekte/blob/407410cbc28ae2a2c6fa89ba0e6af2458157c472/crates/core/src/repository.rs#L1699) · [New entry](https://github.com/Simon-Martens/objekte/blob/0eeffe49605fe08758eea3a8314995b8a288e195/crates/core/src/lifecycle_initial.rs#L284)

Call the existing validator at `Index::start_build` alongside option validation. Map its existing error to `IndexError::Build` before spawning the worker. On a 64-bit target, test the record limit and file limit independently using a valid temporary root and directly constructed options. Assert that the start call itself fails with the corresponding message and no reader is invoked. Keep valid-limit behavior and asynchronous error delivery for genuine worker failures.

## R4. Restore the four lifecycle regressions to Cargo’s test inventory

Removing `lifecycle_open.rs` also removed the declaration that compiled `lifecycle_open.test.rs`. That companion was edited but remains unregistered. Its four tests cover ownership reclamation under pinned views, large framed-file admission accounting, multi-collection admission totals, and inference with overlapping collections. [Orphaned tests](https://github.com/Simon-Martens/objekte/blob/0eeffe49605fe08758eea3a8314995b8a288e195/crates/core/src/lifecycle_open.test.rs) · [Former module declaration](https://github.com/Simon-Martens/objekte/blob/407410cbc28ae2a2c6fa89ba0e6af2458157c472/crates/core/src/lifecycle_open.rs#L60)

Move the reclamation test under lifecycle tests and the three construction tests under the appropriate registered construction or initial-task test module. Import fixtures explicitly and delete the orphaned source after the tests move. Preserve assertion strength. Where the old comparison exercises two callers of the same new builder, check explicit expected IDs, offsets, classifications or inference facts independently. Confirm each of the four test names appears once in `cargo test --locked -p objekte-search-core --lib -- --list` and that each actually runs. Compare base and final test inventories by test name to catch other accidental losses. Explain purposeful renames or replacements.

## R5. Make public view metadata describe the pinned revision

New `IndexView::files()` returns `self.repository().files()`. On file addition, the writer appends the file to the new root’s catalogue and installs its segments while sharing the existing repository. A fresh view can therefore find the new file through `file_handle_for_path` or `catalogue_rows` and still omit it from `files()`. The new `collections()` also exposes underlying `CollectionState` as a public convenience for migrated callers. [New accessors](https://github.com/Simon-Martens/objekte/blob/0eeffe49605fe08758eea3a8314995b8a288e195/crates/core/src/lifecycle/view.rs#L50) · [File addition](https://github.com/Simon-Martens/objekte/blob/0eeffe49605fe08758eea3a8314995b8a288e195/crates/core/src/lifecycle/mod.rs#L1547)

Use existing revision-aware catalogue iteration, file lookup, collection definitions and live segment summaries for callers asking about pinned state. For build-only reports that require captured construction statistics or a base-inventory byte estimate, supply narrowly named observations, or use existing ones. Remove the two broad newly added accessors once their callers are migrated. Keep existing external APIs predating this branch. Do not reconstruct a complete inventory or copy physical indexes to answer metadata questions. Hold a view across add and rename or deletion. Verify old and new views each report the expected paths through the supported public enumeration and lookup APIs.

## R6. Finish the construction split before Assignment 3 takes the physical half

| Current location | Required owner and change |
| --- | --- |
| First 663 lines of `mutation_segment.rs` | Move `SegmentAdmission`, `SegmentAggregate`, `SegmentBatch`, `CollectionSegment`, the admission limits alias and their admission/finalization methods to `construction/segment_admission.rs`. That destination currently contains collection admission, while the segment types are still in the mixed file. Keep the physical operations from `project_inference_for_partitions` onward at the agreed Assignment 3 boundary. |
| `construction/initial.rs` | Move `SegmentBuildScheduler`, its destination/message/result types, private group, and worker dispatch/drain mechanics into `construction/segment_jobs.rs` beside their gate. Keep the ordering and publication coordinator in `initial.rs`. |
| Final metadata assembly in `construction/initial.rs` | Move `SegmentMetadata`, `segment_metadata_generation`, and the final repository-assembly body into `construction/repository_state.rs`. Delegate fingerprint consensus to that owner. Keep a thin prior-path re-export only for an existing consumer. Keep lifecycle revision installation outside assembly. |
| `construction::repository_state::initialize_empty` | It unconditionally constructs a single-thread Rayon pool. Both the catalogue and finalization call sites already own the build pool. Borrow that pool at both call sites, or use a directly constructed empty gram index after proving identical fields and accounting. Standalone `empty_like` must continue to work without an active build. Preserve the separately owned query executor. |
| Module imports and mixed tests | Replace the production parent wildcard imports, especially `use super::*` and `use crate::repository::*` in the coordinator. Use explicit internal exports for the real callers. Move the admission tests with their implementation. Keep the physical tests running at their existing owner or agreed Assignment 3 destination. |

Coordinate edits with the Assignment 3 agent if it has already moved the physical tail. Integrate the actual code, preserving both owners. Keep scheduling, callbacks, cancellation and move semantics intact. [Mixed implementation](https://github.com/Simon-Martens/objekte/blob/0eeffe49605fe08758eea3a8314995b8a288e195/crates/core/src/mutation_segment.rs#L19) · [Scheduler](https://github.com/Simon-Martens/objekte/blob/0eeffe49605fe08758eea3a8314995b8a288e195/crates/core/src/construction/initial.rs#L202) · [Metadata](https://github.com/Simon-Martens/objekte/blob/0eeffe49605fe08758eea3a8314995b8a288e195/crates/core/src/construction/initial.rs#L828) · [Empty-state pool](https://github.com/Simon-Martens/objekte/blob/0eeffe49605fe08758eea3a8314995b8a288e195/crates/core/src/construction/repository_state.rs#L25)

## Validation and delivery

1. Repair owner lifetime, validation, metadata access and test registration first. Run targeted compilations and the regression checks above. Keep each repair reviewable, then complete the construction moves and rerun the construction, private-segment cancellation and rebuild tests.
2. Run `cargo fmt --all -- --check`, `cargo check --locked --workspace --all-targets`, `cargo check --locked -p objekte-search-core --all-targets --all-features` and `cargo test --locked --workspace`. Record the implementation SHA, the four restored tests, and the actual example commands, modes and exit statuses. Preserve the existing all-features fix.
3. Retain the existing 32-worker Musenalm evidence. After the pool or measured setup changes, repeat the small full-build comparison on the same host and inputs. Time from immediately before `start_build` through `wait_complete`, with the completed owner still alive when stopping the timer. Compare wall time, peak RSS, source reads and parses, segment membership and index signature. The old constructor raised internal `build_ns` to its enclosing elapsed time, so use caller wall time for cross-version build comparisons. A full query benchmark run is unnecessary.
4. Update the Assignment 2 completion report on the integration branch with fixes, validation and any unresolved limitations. Push that branch for review. The owner merges after the repaired tip is checked.

## Implementation report

- Task identifier: `core-construction-review-repairs`
- Completion date: 2026-11-18
- Comparison base: `407410cbc28ae2a2c6fa89ba0e6af2458157c472`
- Reviewed tip: `0eeffe49605fe08758eea3a8314995b8a288e195`
- Integration branch: `integration/core-index-construction`
- Integration worktree:
  `/home/simon/source/objekte-integration-core-index-construction`
- Implementation commits: `bb124f3adb`, `2bb6a9cb77`

### Implemented repairs

R1 now retains the completed owner for benchmark sessions. `bench_musenalm`
stores the owner and pinned view together in both build modes, configures the
cache through the owner, and queries through the pinned view. The other
benchmarks use the existing `IndexedRepository` ownership model where they need
build-time metadata. The public cache setter was removed from `IndexView`.

The real Q001 smoke runs passed in both modes. The default mode measured
6,359.613 µs cache-off and 69.240 µs warm; `--incremental` measured 6,434.828 µs
cache-off and 62.835 µs warm. `bench_case_modes` and
`bench_corrective_cases` also completed their real cache setup and query
workloads successfully.

R2 removed the unbounded exclusive-ownership loop. The test-only
`into_repository` conversion now performs one immediate `Arc::try_unwrap` at
each layer and returns a typed error when an owner or view retains a layer.
Ordinary fixtures use `build_repository_for_test`; tests that inspect private
repository metadata borrow it from a pinned view. Retained-owner and
retained-view error cases are covered explicitly.

R3 calls `SegmentPolicy::validate` in `Index::start_build` before dispatch.
The 64-bit regression checks record and file limits independently, expects the
validator's existing typed messages, and verifies that the registered reader is
never invoked.

R4 moved the reclamation test to `lifecycle::tests`, and the three construction
tests to `lifecycle_initial::tests`. The orphaned `lifecycle_open.test.rs` was
deleted. All four tests are registered:

- `lifecycle::tests::adopted_segment_payloads_are_reclaimed_after_replacement_and_pinned_views`
- `lifecycle_initial::tests::large_framed_file_admission_has_component_accounting`
- `lifecycle_initial::tests::incremental_admission_totals_match_each_collection`
- `lifecycle_initial::tests::direct_json_build_matches_static_inference_overlap_and_native_publication`

The base normal-feature inventory had 589 tests and the final inventory has
593. The remaining differences are the five module-path changes caused by the
construction tree extraction, the faithful mutation-test module split, the
three repair regressions, and the pinned-metadata regression. No unrelated test
name disappeared.

R5 removed `IndexView::{files,collections,startup}`. Revision-specific callers
use `catalogue_rows`, file lookup/handle APIs, `collection_definitions`, live
segment summaries, and `startup_stats`. Build-only benchmarks use narrow
aggregate observations instead of repository-shaped view access. A regression
holds three views across add and move and verifies independent paths, catalogue
enumeration and file handles.

R6 completed the construction split:

- Segment admission and finalization moved to
  `construction/segment_admission.rs`; physical transformation begins at
  `project_inference_for_partitions` in `mutation_segment.rs`.
- Scheduler, completion types, private groups, dispatch and drain mechanics
  moved to `construction/segment_jobs.rs`; ordering and revision publication
  remain in `construction/initial.rs`.
- Segment metadata, generation metadata and final repository assembly moved to
  `construction/repository_state.rs`; lifecycle revision installation remains
  in `initial.rs`.
- Catalogue and finalization reuse the active build pool. `empty_like` retains
  its standalone fallback pool.
- Production parent wildcard and repository wildcard imports were replaced with
  explicit imports. Admission tests moved beside their implementation and
  physical transformation tests remain under `mutation_segment`.

### Validation

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed. |
| `cargo check --locked --workspace --all-targets` | Passed. |
| `cargo check --locked -p objekte-search-core --all-targets --all-features` | Passed. |
| `cargo test --locked -p objekte-search-core` | Passed: 577 unit tests, 16 ignored, plus all core integration and doc tests. |
| `cargo test --locked --workspace` | Passed on rerun. The first run exposed one isolation-sensitive executor assertion; the exact test passed alone and serially, and the complete workspace rerun passed. |
| Restored all-features regressions | Both profiling-gated tests passed. |
| Construction focused tests | Segment-admission, physical transformation, private-segment cancellation/error, repeated-wait and rebuild tests passed. |

### Refreshed build comparison

The comparison used `bench_build_musenalm`, ASCII, 32 build workers, and the
same Musenalm input for the reviewed repair tip and final tip.

| Metric | Reviewed repair tip | Final repair tip | Delta |
| --- | ---: | ---: | ---: |
| Wall time | 676,449,362 ns | 654,873,036 ns | -3.19% |
| Build time | 662,736,229 ns | 641,357,687 ns | -3.22% |
| Peak resident bytes | 666,431,488 | 658,628,608 | -1.17% |
| Peak admitted bytes | 523,665,408 | 523,665,408 | 0% |
| Peak concurrent segment jobs | 15 | 15 | 0 |

The exact composite index signature
`76536ba48f89bb29fac5fad1baa186cec9e2c7e2c90a793223f84ef8a317632a`
matched the reviewed repair tip under the same current corpus. The schema
digest `38f2dc5112146aa6ac0514291c76eaeb741421fa8a4624e50fa83c4c0a449bf9`,
148,486 records, 148,486 parses, zero failed parses, 31 stores, and all segment
memberships matched. The older committed signature
`4a38808629a1544a354d715e42c85ac426b1cfe93376e862253b3d5cc03f349f` includes
file modification timestamps and belongs to the earlier corpus
materialization; the reviewed tip rerun on the current input reproduced the new
`76536...` signature exactly. Final JSON is stored at
`docs/ai/generated/core-index-construction/benchmark/repair-current.json`.

### Limitations and cleanup

No unresolved correctness, test-registration, ownership, or build-performance
limitation remains in the repair scope. The public view continues to expose
narrow aggregate construction observations for benchmark compatibility while
pinned catalogue and segment APIs own revision-specific state.

The temporary reviewed-tip and base-inventory worktrees used for comparison were
removed. Temporary benchmark result directories were not retained. The
integration branch and integration worktree are retained for the owner.
