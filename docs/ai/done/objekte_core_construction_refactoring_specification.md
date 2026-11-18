# Assignment 2 — Core index construction

**Task ID** `core-index-construction`  
**Reviewed master** `0a0e0c3507af94164fce2387070cfcb6b5277d23`  
**Integration branch** `integration/core-index-construction`  
**Sibling worktree** `/home/simon/source/objekte-integration-core-index-construction`

## One public build operation

Make progressive construction the normal initial build. Rename the existing four-argument `Index::start_incremental_build_with_sources` in place to **`Index::start_build`**. Starting returns an `InitialBuildTask`. The caller chooses when to wait. The explicit registry accommodates built-in and third-party readers through the same interface.

```rust
pub fn start_build(
    root: impl Into<PathBuf>,
    definitions: Vec<CollectionDefinition>,
    options: RepositoryOptions,
    sources: SourceRegistry,
) -> Result<InitialBuildTask, IndexError>
```

A caller that needs the finished index uses the following two statements. A caller displaying progress can first call `wait_catalogue()` on the same task, then observe publications with `wait_next(cursor)`. Catalogue readiness means metadata is available. Records become searchable as segments are published. Waiting for completion must never restart discovery or extraction.

```rust
let task = Index::start_build(root, definitions, options, SourceRegistry::builtin())?;
let index = task.wait_complete()?;
```

Delete the synchronous initial constructors `Repository::{open, open_with_options, open_with_sources}` and `Index::{build, build_with_sources}`, their obsolete private bridge `build_with_sources_inner`, and the three-argument `Index::start_incremental_build`. The old four-argument name disappears in the rename. Remove these APIs in this assignment without deprecation aliases, a replacement blocking constructor or an incremental mode flag. Both old paths already used the segmented engine. Waiting on its task removes the round trip through an owned `Repository` and gives every caller the same publication and error semantics. [Existing synchronous path](https://github.com/Simon-Martens/objekte/blob/0a0e0c3507af94164fce2387070cfcb6b5277d23/crates/core/src/repository.rs#L1693) · [Task entry to rename](https://github.com/Simon-Martens/objekte/blob/0a0e0c3507af94164fce2387070cfcb6b5277d23/crates/core/src/lifecycle_initial.rs#L294)

Within that one initial build, two calls to `build_fresh(..., false)` initialize empty state. One supplies the early catalogue. The other receives final metadata derived from finished segments. Keep these distinct publication stages and the lifetime of pinned views. Neither requires a second scan of the corpus. The maintenance operation `Repository::empty_like` also needs empty-state initialization. [Initial catalogue](https://github.com/Simon-Martens/objekte/blob/0a0e0c3507af94164fce2387070cfcb6b5277d23/crates/core/src/repository_incremental.rs#L1517) · [Final metadata](https://github.com/Simon-Martens/objekte/blob/0a0e0c3507af94164fce2387070cfcb6b5277d23/crates/core/src/repository_incremental.rs#L1930)

## Branch and scope

Start from current `origin/master` in a fresh sibling worktree. Record the base SHA. Assignment 2 can start while Assignment 1 is in progress. Use the existing lifecycle names for initial publication and completion. Leave the locations of `IndexView`, `PublishedIndex`, and the implementation of revision installation to Assignment 1. A short compatibility export can protect its existing imports. Once Assignment 1 lands, merge the updated master into this integration branch and run the final checks on the combined tree. If Assignment 2 is reviewed first, identify the exact tested base and repeat the core checks when the two changes meet. Follow `AGENTS.md` and `docs/ai/README.md`. Push each task commit and deliver to the owner for merge.

Construction belongs in `crates/core`. Removing the old initial-build API is an intentional breaking change. Migrate its callers in the same branch, including CLI, desktop, workspace examples, benchmarks, tests and current API documentation. Preserve unrelated public APIs, serialized forms and build options. The module split must preserve finished-index contents, partial publications and worker concurrency.

### Caller migration

| Caller on the reviewed base | Required change |
| --- | --- |
| Synchronous core callers and fixtures | Use `Index::start_build(..., sources)?.wait_complete()?`. Supply `SourceRegistry::builtin()` for built-ins. Query the resulting `Index` or a pinned view. A local test fixture may encapsulate repeated setup. |
| `lifecycle_initial.rs` and `repository.rs` | Preserve source configuration preparation, relation/inference validation and segment-policy validation on the canonical path. Keep immediate setup failures distinguishable from terminal worker errors. Remove the old ownership round trip and delete `Index::into_repository` if unused. Keep genuine adoption of an already-created index or repository where independently needed. Do not clone published state or require exclusive ownership to retrieve a completed index. |
| `lifecycle.rs::rebuild_with_control` or its Assignment 1 destination | Build the replacement on a private task. Propagate cancellation and drain outstanding work. Install the finished replacement through the existing revision check and retry. Observers of the live index must see the old revision until the single successful installation. |
| `crates/cli/src/execution.rs` and `crates/desktop/src/backend/open.rs` | Start once with the explicit registry. Preserve catalogue-first opening, progress, cancellation, available-record searches and complete-only waits at their current user-visible points. |
| `crates/core/examples/bench_build_musenalm.rs` and `bench_musenalm.rs` | Start the full-build timer before `start_build` and stop it after `wait_complete`. Obtain physical counters and the signature from completed index state. A benchmark switch may select whether to measure partial availability. Every measurement uses the same construction entry. |
| Other consumers, examples and documentation | Search the workspace for calls, function references, imports and doctests using each removed name. Preserve registered readers and options. Migrate queries to the completed index or a pinned view. Historical reports describe their measured revision and require no edits. |

Keep actual catalogue, first-searchable, completion and inference milestones. Preserve task status, partial-view pinning, cancellation and typed terminal errors. An immediate waiter receives the final published index and its actual revision. Keep time spent waiting separate from individual milestone durations.

## Target module structure

Introduce private `mod construction;` in `lib.rs`. Use the following destinations under `crates/core/src/construction/`. Prefer explicit imports and module-private state. Use a small crate-private façade for entry points needed by lifecycle or maintenance.

| Destination | Move from the reviewed tree | Responsibility |
| --- | --- | --- |
| `mod.rs` | The existing segmented coordinator and necessary test hooks delegate here. | Expose one private construction worker for the task and a narrow maintenance preparation interface. Reuse the concrete callback and cancellation contracts. |
| `initial.rs` | Coordination from `repository_incremental.rs::build_incremental_with_sources_inner`, `IncrementalCollection`, `admit_normal` and the existing submission/publication sequence. | Drive catalogue creation, ordered admission, segment finalization, final metadata and callbacks. Delegate each stage. Leave lock acquisition and revision installation in lifecycle. |
| `discovery.rs` | `PreparedDiscoveredFile`, `prepare_discovered_file`, `prepare_discovered_window` and ordered inventory assembly from the repository builders. | Use the existing `crate::discovery` policy for file selection. Turn its output into one ordered physical inventory with collection memberships. Preserve bounded metadata windows. |
| `batch.rs` | The batch records, `BatchContribution`, `BatchBuilder`, file batching and ordinary batch processing from `build.rs`. | Produce owned local-slot contributions. Transfer them by move into ordered admission. |
| `file_preparation.rs` | `FilePreparationBinding`, `PreparedCollectionBuild`, `PreparedFileBuild`, supplied-byte/range validation, framing and large-file preparation in `build.rs`. | Reuse the established per-record extraction path for ordinary builds and file mutations. Keep source errors, fallback classes, diagnostics and hashing intact. |
| `scheduling.rs` | Worker resolution, byte allowances, `RepositoryFileJob`, `process_repository_file_batches_profiled` and its ordered admission machinery. | Own source-job dispatch, reservations and draining. Retain separate build and query worker policies. |
| `segment_jobs.rs` | `SegmentBuildScheduler`, `PrivateSegmentGroup`, finalization messages and `SegmentJobGate` from `repository_incremental.rs`. | Bound concurrent segment finalization and preserve per-collection publication order. Keep test gates on the real worker path. |
| `segment_admission.rs` | `SegmentAdmission`, `CollectionSegment`, `SegmentBatch`, `SegmentAggregate` and `CollectionSegment::into_indexed_profiled` from the first part of `mutation_segment.rs`. | Convert owned batches into an `IndexedCollectionSegment` with the existing native builder. Retain both multi-file worker batches and prepared single-file admission. |
| `inference.rs` | The construction implementation currently included as `inference.rs`’s private `inference_build.rs` child. | Own observation, shifted merges, candidate construction and global admission decisions. Keep the public inference configuration/report API and its types at `crate::inference`; retain narrow internal re-exports for existing consumers. |
| `repository_state.rs` | Empty repository creation, `SegmentMetadata`, `segment_metadata_generation`, digest consensus and final repository assembly. | Initialize correctly configured empty state and derive completed metadata from the published segments. Preserve ID allocation, relation resolution, diagnostic merging, sorting metadata and final counters. |

**Three boundaries require care.**

- `mutation_segment.rs` mixes construction with transforming existing indexes. The first physical projection implementation starts at `IndexedCollectionSegment::project_inference_for_partitions`. Keep that and the following partitioning, consolidation and compaction machinery together at its current physical owner for Assignment 3. Retain explicit compatibility exports such as `crate::build::IndexedCollectionSegment` and `PartitionRouting` where callers need them. A thin private `build.rs` compatibility module is acceptable. Move operational construction bodies to their assigned modules. Split or rewire the mixed tests so both the admission and physical transformation tests continue to execute. [Seam](https://github.com/Simon-Martens/objekte/blob/0a0e0c3507af94164fce2387070cfcb6b5277d23/crates/core/src/mutation_segment.rs#L664)
- `record_preparation.rs` serves construction, pre-index scanning **and** query verification/materialization. Expose the build-side finalization used by `BatchBuilder` through the construction preparation boundary. Keep the existing shared implementation of schema coercion/finite-value checking available to `source_scan.rs`. Leave query projection, bounded query decoding, borrowed FTS extraction and payload materialization with their present callers. This avoids making search depend on the build scheduler or duplicating validation. [Shared record preparation](https://github.com/Simon-Martens/objekte/blob/0a0e0c3507af94164fce2387070cfcb6b5277d23/crates/core/src/record_preparation.rs#L11)
- `Repository` owns runtime state and its public configuration/profile types. Remove the source-building `open*` constructors. Preserve `empty_like`, startup/report accessors, query-worker ownership and cache behavior. Replace the `use super::*` dependency of the old incremental child with explicit imports. Preserve public profile fields and serialized field names, including the nested admission/work profiles. A private re-export can retain a used path without retaining two implementations.

## Eliminate the ambiguous empty-build entry

1. Inventory all `build_fresh` call sites on the integration base, including tests and feature-gated paths. On the reviewed revision all three pass `false`. Replace this boolean entry with an explicitly named `initialize_empty_repository` and route the catalogue, completion and maintenance callers to it. Migrate any intervening initial-build caller to the canonical task. Preserve separately used maintenance primitives.
2. Retain the work that empty initialization actually performs: configuration hash, worker-option validation, reader resolution, duplicate collection-name checks, schema/custom indexes, empty inference and sort structures, relation validation, initial active state, catalog, query runtime and caches. Preserve `empty_like`’s restoration of build statistics and incomplete inference summaries. The initializer must perform zero corpus enumeration, source reads, parses and segment publications. Remove the obsolete discovery boolean and the unreachable full-corpus scheduling branch after proving its call sites. Retain any separately used test or production kernels.
3. For empty-state construction inside an active build, pass the existing construction pool to any empty-index finalizer that needs it. Avoid creating another full-size build pool for zero source jobs. Keep the standalone maintenance caller usable without an active pool, and keep the query pool's ownership/lifetime distinct. Preserve both separately observable publications and the lifetime of already-pinned catalogue views. Avoid cloning finished postings or segments to populate the final repository. Report any inherited metadata copying separately from changes introduced by this task.

## Invariants

- **Input ownership and identity.** Keep deterministic discovery and collection order, file IDs, local slots, record IDs, locators, file status and diagnostics. A shared physical file can have multiple eligible collection bindings. Preserve each binding's content fingerprint and the all-binding consensus rule. Source-read equivalence is measured against the current implementation for the same configuration.
- **Ordinary work.** Preserve the existing ordered admission of completed jobs, count/byte batch limits, explicit and automatic worker validation, and bounded in-flight/ready work. Move contributions through the existing sender/receiver and segment handoffs. A module boundary never requires cloning a contribution, bitmap, payload or segment.
- **Large work and failure.** Preserve bounded streaming of large JSONL files and private segment finalization before EOF. Publish the validated private group atomically at the current boundary. On late invalidation, worker failure, cancellation or revision conflict, drain/join outstanding work as the current engine does. Preserve the public error, callback order and the last valid published revision. Keep the full behavior of malformed input, including raw-fallback records where currently supported.
- **Metadata and revisions.** Keep local slot offsets, source ranges, logical/fallback domains, per-field gram indexes, sort ties, relation targets and global inference admission consistent from partial publication through completion. Preserve partial-search eligibility and `wait_catalogue` / `wait_complete` semantics. A metadata pass over finished segments must not reopen or parse files.
- **Operational cost.** Preserve queue limits, pool parallelism and profiling boundaries. Keep any existing single-worker fast path. Retain 32-worker operation on a host whose available CPU count supports it. A namespace move should introduce no new per-record dynamic dispatch, lock, allocation or metadata lookup.

## Acceptance and evidence

Run existing tests first. The following tests identify the regression each extraction is most likely to introduce.

| Evidence | Existing test or required assertion |
| --- | --- |
| Deterministic metadata and shared physical files | `parallel_metadata_preparation_keeps_order_revisions_and_overlapping_memberships`; `fresh_fingerprints_require_every_shared_binding_to_agree`. |
| Admission, bounds and native postings | `file_workers_overlap_while_admission_keeps_file_order`; `worker_allowance_is_bounded_and_keeps_one_oversized_job_live`; `admits_one_worker_batch_spanning_files_without_reprojecting_postings`. |
| Private groups, concurrency and cancellation | `large_file_finalizes_private_segments_before_eof`; `private_segment_window_is_bounded_for_one_and_multiple_workers`; `multi_worker_segment_finalization_actually_overlaps`; `late_private_invalidation_and_worker_failure_publish_nothing`; `private_final_drain_consumes_worker_error_before_returning`; `private_work_cancels_while_outstanding_and_after_validation`. |
| Malformed records, duplicate IDs and partial views | `oversized_jsonl_preserves_malformed_siblings_and_fallback_grams`; `duplicate_intrinsic_ids_across_batches_fail_before_publication`; `discovery_and_segments_publish_without_losing_records`. |
| One build, custom readers and complete results | Use one `start_build` task with a registered counting reader and controlled publication. Observe partial progress and wait for completion on that same task. Verify reader counts for one build, ordered IDs, numeric filter counts, field-scoped FTS, logical/fallback classification and offsets against explicit fixture expectations or the recorded base. Running the new method twice is insufficient to establish parity. |
| Error and rebuild behavior | Exercise invalid segment policy and setup validation, a typed asynchronous failure, cancellation with work outstanding and a successful live replacement. Keep a view pinned across successful replacement. Verify a failed or cancelled rebuild leaves the current live revision intact. |
| Empty initialization and pinned state | Instrument the actual reader or reuse existing counting fixtures. Assert zero enumerations/read/parse operations attributable to the empty-state helper, including `empty_like` on a populated source tree. Check empty collection indexes and configuration. Exercise a retained partial catalogue view across completion. Its revision stays stable and the new view is complete. |

Use gated public-path tests to observe a partial publication deterministically; an ungated build can finish before a test thread receives its first snapshot. Keep existing source and maintenance tests that exercise shared helpers. Add tests only for an acceptance condition missing from existing coverage.

### Short performance comparison

Measure construction through completion with the `bench_build_musenalm` release example at the recorded base and final SHA. Use the same read-only Musenalm corpus, schema, alphabet, segment policy, feature flags and machine. Use **32 build workers** if available to the process, otherwise the same supported count on both revisions. Run one warm-up per binary, then three measured builds per revision in alternating order. The example emits wall time, phase timings, parse counts, segment counts, the index signature and Linux high-water RSS.

```sh
cargo build --release --locked -p objekte-search-core --example bench_build_musenalm
./target/release/examples/bench_build_musenalm \
  --root "$MUSENALM_ROOT" --alphabet ascii --build-workers "$BUILD_WORKERS"
```

Save one JSON per measured run. Compare `index_signature_sha256`, schema digest, record/file counts, failed parses, segment membership and registrations independently of timing. Report median `wall_ns`, peak resident bytes, admitted bytes and concurrent segment jobs. The signature covers only selected metadata; keep the public query parity checks. If peak RSS is unavailable on the host, measure process maximum resident memory with the host's equivalent. For a repeatable slowdown or RSS increase, identify the phase responsible before delivery. Extra pools, loss of concurrency and copies of finished indexes are actionable even on a noisy host. The full query benchmark suite is unnecessary for this assignment. [Benchmark fields and invocation](https://github.com/Simon-Martens/objekte/blob/0a0e0c3507af94164fce2387070cfcb6b5277d23/crates/core/examples/bench_build_musenalm.rs#L119)

## Implementation order

1. **Characterize the current build.** Record the base, callback behavior, existing coverage, empty-state callers and reference benchmark. Capture independent expected results and source-work counts before deleting either API.
2. **Make the task the only initial-build API.** Rename the four-argument method, remove the synchronous route and aliases, migrate all workspace consumers and add the single-build and rebuild regressions in one compiling commit.
3. **Extract batch, file preparation and source-job scheduling.** Move implementations and their tests in compiling commits. Preserve exports consumed by mutation, lifecycle and test fixtures.
4. **Extract segment admission, finalization and inference construction.** Move the gate and private-group machinery with their tests. Keep physical consolidation/projection outside the construction package.
5. **Extract initial coordination and metadata finalization.** Move the private worker used by the canonical task. Specialize the empty initializer in a separate commit so its behavioral and performance effects can be reviewed.
6. **Validate the integrated branch.** Run the commands below, the focused tests and benchmark. Publish base and tested SHAs, the source-to-destination symbol map, before/after measurements, index parity and any limitations under `docs/ai/generated/core-index-construction/`. Follow the normal task-document closeout. The owner merges the branch.

```sh
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo check --locked -p objekte-search-core --all-targets --all-features
cargo test --locked -p objekte-search-core
cargo test --locked --workspace
```

Review completion requires a workspace-wide call-site inventory, compilation of the examples and dependent applications, and the focused publication, cancellation and rebuild assertions above. Run desktop-dependent checks on the project build host if the local machine lacks its native dependencies.

## Implementation report

- Task identifier: `core-index-construction`
- Completion date: 2026-11-18
- Assignment base: `407410cbc28ae2a2c6fa89ba0e6af2458157c472`
- Tested implementation: `1c44c86bbf`
- Benchmark evidence commit: `bc045102f6`
- Integration branch: `integration/core-index-construction`

### Implemented changes

`Index::start_build(root, definitions, options, sources)` is now the sole
initial construction entry point. The synchronous `Repository::open*` methods,
`Index::build`, `Index::build_with_sources`, the three-argument incremental
entry point, `build_with_sources_inner`, and `lifecycle_open.rs` were removed.
The four-argument method was renamed in place and returns the same
`InitialBuildTask`; callers wait on that task for the final `Index`.

All workspace callers were migrated, including core tests and examples, CLI,
desktop, benchmarks, fixtures and current documentation. Cache-sensitive tests
retain the completed `Index` owner and query through a pinned `IndexView`.
Internal tests that need repository-private metadata explicitly adopt the
completed index only where that inspection is the test subject.

The construction code now physically lives under the assigned locations:

| Destination | Moved responsibility |
| --- | --- |
| `construction/mod.rs` | Thin subsystem façade, module wiring and compatibility surface. |
| `construction/initial.rs` | Discovered inventory, ordered admission, callbacks, segment scheduling, final metadata publication and build completion. |
| `construction/discovery.rs` | `PreparedDiscoveredFile`, bounded metadata preparation and ordered inventory assistance. |
| `construction/batch.rs` | Batch files, contributions, `BatchBuilder`, batch processing and profiles. |
| `construction/file_preparation.rs` | File preparation bindings, supplied bytes/ranges, framing, bounded JSONL and large-file preparation. |
| `construction/scheduling.rs` | Worker resolution, repository jobs, allowances, ordered dispatch and draining. |
| `construction/segment_jobs.rs` | `SegmentJobGate`, private-drain state and bounded finalization gates. |
| `construction/segment_admission.rs` | `CollectionAdmission` and owned batch-to-segment admission. |
| `construction/inference.rs` | Existing inference construction implementation, retaining the public API at `crate::inference`. |
| `construction/repository_state.rs` | The specialized zero-I/O empty repository initializer. |

`build.rs` remains a narrow crate-private compatibility façade. The mixed
physical partitioning/consolidation implementation remains with
`mutation_segment.rs` for Assignment 3, while construction admission exports
are retained for existing callers.

`build_fresh` and its unreachable discovery boolean were removed.
`initialize_empty_repository` is now the explicitly named configuration-only
initializer used by initial catalogue state, final metadata state and
`Repository::empty_like`. It performs no corpus enumeration, source reads,
parses or segment publication and uses only a one-thread finalizer when empty
index payloads require it. `empty_like` still restores build statistics,
startup observations and incomplete inference summaries.

Live rebuild uses the canonical build task, polls it for external cancellation,
cancels and drains outstanding work, then installs the completed replacement
through the existing revision-conflict/retry boundary. Pinned live observers
continue to see the old revision until installation succeeds.

The existing `detailed-profiling` cache instrumentation referenced the removed
`crate::profiling` path. Both references now use
`crate::detailed_timer::DetailedInstant`, allowing the required all-feature
validation to pass. This resolves
`docs/ai/issues/bug/detailed_profiling_missing_module.md`.

### Behavior coverage

Added
`lifecycle_initial::tests::start_build_is_the_single_scan_and_repeated_waits_do_not_rebuild`.
It uses one counting `SourceReader`, calls `wait_complete()` twice on the same
task, verifies exactly one parse per source, checks a numeric indexed count, and
then verifies `empty_like` performs no additional parses while producing empty
records and postings.

Existing deterministic metadata, overlapping membership, admission bounds,
private-segment finalization, cancellation, malformed-record, duplicate-ID,
partial-view, rebuild and pinned-publication tests remain active and passing.

### Commits

- `e14c97acb0` — Unify and modularize core index construction
- `06f4c3cdaf` — Validate modular core construction and record benchmarks
- `1c44c86bbf` — Move construction responsibilities into focused modules
- `bc045102f6` — Refresh core construction benchmark evidence

No subtask branches were created. The task was implemented and supervised on
its integration branch.

### Validation

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed. |
| `cargo check --locked --workspace --all-targets` | Passed. |
| `cargo check --locked -p objekte-search-core --all-targets --all-features` | Passed. |
| `cargo test --locked -p objekte-search-core` | Passed, including 572 unit tests with 16 ignored plus all core integration tests. |
| `cargo test --locked --workspace` | Passed, including CLI and desktop tests. |

The workspace-wide inventory found and migrated all active references to the
removed APIs. Historical benchmark/result reports retain their measured
revision and were not rewritten.

### Benchmark

The required `bench_build_musenalm` comparison used
`/home/simon/source/objekte/examples/musenalm`, ASCII, 32 build workers, one
warm-up per binary and three alternating measured builds per revision.

| Metric | Base median | Current median | Delta |
| --- | ---: | ---: | ---: |
| Wall time | 641,217,095 ns | 653,992,201 ns | +1.99% |
| Build time | 641,197,306 ns | 639,801,447 ns | -0.22% |
| Discovery time | 188,338,306 ns | 191,813,178 ns | +1.85% |
| Peak resident bytes | 655,540,224 | 648,462,336 | -1.08% |
| Peak admitted bytes | 523,665,408 | 523,665,408 | 0% |
| Peak concurrent segment jobs | 15 | 15 | 0 |

Index signature, schema digest, record count, parse count, failed parses,
segment membership, registrations and worker count matched exactly:

- Index signature:
  `4a38808629a1544a354d715e42c85ac426b1cfe93376e862253b3d5cc03f349f`
- Schema digest:
  `38f2dc5112146aa6ac0514291c76eaeb741421fa8a4624e50fa83c4c0a449bf9`
- Records and parses: 148,486
- Failed parses: 0
- Resolved build workers: 32

The timing spread is within observed run noise and does not show a repeatable
construction regression. Raw JSON and the summary are stored in
`docs/ai/generated/core-index-construction/benchmark/`.

### Deviations, limitations and cleanup

- Pinned `IndexView` gained narrow forwarding reads for collection/file
  metadata, diagnostics, segment summaries/signatures and parameterized or
  analyzed query variants. These delegate to the existing immutable
  publication and introduce no corpus or index copying.
- `Index::into_repository` remains crate-private for tests that specifically
  inspect repository-private state and genuine adoption. It waits for worker
  task clones to drain rather than cloning published postings or segments.
- Mixed construction/physical transformation code remains at
  `mutation_segment.rs`, as directed for Assignment 3.
- Construction-focused tests remain reusable through their existing test-file
  paths and execute under the new construction module hierarchy.
- No unresolved performance, correctness or validation limitation remains.
- No subordinate worktree was created, so there was no subordinate cleanup.
  The integration worktree is retained for owner review at
  `/home/simon/source/objekte-integration-core-index-construction`.

## Review repair addendum

The subsequent Assignment 2 review repairs are complete on the same
integration branch at commits `bb124f3adb` and `2bb6a9cb77`. The following
statements supersede the corresponding limitations above.

- Benchmarks retain the completed owner and pinned view across the full cache
  and query session. The cache is configured through the owner, not the view.
- `IndexView` no longer exposes repository-wide `files`, `collections`, cache
  mutation or the duplicate `startup` accessor. Revision-specific metadata uses
  catalogue, collection-definition and segment APIs; build-only aggregates use
  narrowly named observations.
- The test-only repository conversion no longer waits for owners to disappear.
  Retained owners and views fail promptly with typed errors.
- Segment admission and generation finalization, job scheduling/draining, and
  final repository assembly now have their assigned construction-module owners.
  The coordinator keeps publication ordering and lifecycle revision
  installation.
- Catalogue and final repository initialization borrow the active build pool;
  standalone `empty_like` still creates its isolated fallback pool.
- The four orphaned lifecycle regressions are registered again, and admission
  tests moved beside their implementation while physical tests retained their
  owner.

The refreshed 32-worker build comparison against `bb124f3adb` measured
654,873,036 ns wall time (down 3.19%), 641,357,687 ns build time (down 3.22%),
and 658,628,608 peak resident bytes (down 1.17%). The current-input exact index
signature
`76536ba48f89bb29fac5fad1baa186cec9e2c7e2c90a793223f84ef8a317632a`,
schema digest, 148,486 records/parses, zero failed parses, segment membership
and worker count matched the reviewed repair tip. Full evidence and command
results are in `docs/ai/done/objekte_core_construction_review_repairs.md` and
`docs/ai/generated/core-index-construction/benchmark/repair-current.json`.
