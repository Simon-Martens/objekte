# Objekte index lifecycle repair specification

Task identifier: `index-lifecycle-repair`  
Prepared: 9 November 2026  
Reviewed baseline: `63f354b2b3d0bb7749f8af5187919c600767a8ea` on `master`  
Integration branch: `integration/index-lifecycle-repair`  
Intended task location: `docs/ai/tasks/objekte_index_lifecycle_repair_specification.md`

## 1. Assignment and completion standard

Repair all seven findings from the index lifecycle implementation review. Preserve the existing lifecycle architecture and desktop interaction model while restoring repeated-query performance, incremental result delivery, scalable maintenance, and correct automation behavior.

This is an implementation specification for the agent assigned to execute it. Preparing or receiving this document does not itself start repository work. Once the user assigns this task, the complete scope below is authorized, including the necessary internal changes to query execution, index storage, maintenance scheduling, and desktop adapters. Do not request additional permission for routine implementation choices within that scope.

The task is complete only when all seven findings are resolved and validated on the combined integration branch. A safe temporary fallback can be an intermediate commit; it does not satisfy the final requirement to implement efficient verified maintenance and compatible moves. Do not close an in-scope finding merely by recording another issue, raising a fixed limit, disabling an existing capability, or benchmarking a different execution path.

Preserve the requirements of the completed `objekte_index_lifecycle_implementation_guide.md` and `objekte_desktop_index_lifecycle_integration_specification.md`. This document adds repair requirements and resolves the implementation shortcomings identified in the review. Read their current repository copies before implementation. Reconcile subsequent user instructions and changes already present on the execution baseline.

### Required outcomes

| ID | Finding | Required result |
| --- | --- | --- |
| R1 | Lifecycle searches bypass verified-result caching | Revision-scoped verified membership and ordered-result reuse through the real `IndexView` and caller paths |
| R2 | Lifecycle streaming runs a complete search before delivering results and forces counting | Incremental delivery where ordering permits it; optional counting; bounded export execution |
| R3 | Reconciliation and compaction reject ordinary large datasets | Bounded processing without the fixed 250,000-file and 262,144-slot dataset ceilings; correct overlapping collections |
| R4 | Loading another index loses periodic checking while the UI remains enabled | Transfer the active checking policy to the replacement handle and report the actual state |
| R5 | Shutdown saves before mutation-producing workers have stopped | Quiesce publication, then persist the final accepted revision; preserve retry and cancel-close behavior |
| R6 | Verified maintenance of a base record compacts collection-sized state | Local physical removal/replacement for eligible base and delta records, with bounded copying |
| R7 | Every move rereads/extracts and replaces the file's contributions | A compatible move route that preserves content-derived index contributions and updates routing correctly |

## 2. Evidence and initial inspection

The review was a source inspection, not an independent Rust test run. The repository reports passing workspace tests and 109 desktop tests at the reviewed revision. Treat those reports as baseline evidence, not proof that the following races or performance paths are covered.

Use this pinned [baseline revision](https://github.com/Simon-Martens/objekte/tree/63f354b2b3d0bb7749f8af5187919c600767a8ea) to understand the findings. Implementation must start from the current authorized `master` revision, recording its commit. If a finding has already been fixed, verify it with the required test and retain the correction instead of implementing it again.

| Finding | Baseline inspection locations | Evidence to reproduce |
| --- | --- | --- |
| R1 | `crates/core/src/run.rs`: `search_active_inner`, `search`, active/segmented streaming; `crates/core/src/lifecycle.rs`: `IndexView` query methods | Active-view execution bypasses the existing verified cache. Static warm-cache benchmarks do not establish lifecycle cache behavior. |
| R2 | `run.rs`: `search_stream_segmented_with_context`, `search_segmented_impl`; `crates/desktop/src/backend/query.rs` | Streaming collects `SearchResult` before delivery. Base execution sets `count = true`. All-results execution retains matching locators before checking the stream buffer limit. |
| R3 | `lifecycle_scan.rs`, `discovery.rs`, `lifecycle_compaction.rs` | Scan allowance is reduced by previously observed paths before discovering an overlapping collection. Compaction limits allocated slots, including retired records. |
| R4 | `crates/desktop/src/controller/runtime.rs`: `WorkerEvent::IndexLoaded`; `backend/index_lifecycle.rs` | Old automation is stopped after replacement, but periodic checking is not enabled on the replacement handle. |
| R5 | `backend/index_lifecycle.rs`: `shutdown_with_final_save`; core `Index::shutdown` | Final flush precedes cancellation/join of running reconciliation and maintenance. |
| R6 | `lifecycle_mutation.rs`: `compact_verified_base`; `lifecycle_verified.rs`; `lifecycle_compaction.rs` | Eligible base maintenance invokes staged compaction instead of modifying bounded affected structures. |
| R7 | `lifecycle_mutation.rs`: `move_file`; `lifecycle.rs`: `prepare_replace_file`, `prepare_file_at`; catalog and source routing | Move uses ordinary replacement preparation, including extraction. Segment routing also assumes its stored path equals the current catalog path. |

Read `docs/ai/generated/index-lifecycle-performance-validation.md` and `docs/ai/issues/perf/verified-base-maintenance-collection-rewrite.md`. The recorded 148,486-file benchmark measured approximately 1,279 ms for a verified base update versus 0.329 ms for appended replacement, with a substantial temporary memory increase. These are historical measurements on the documented machine, not universal timing promises.

Before editing, trace the actual desktop, CLI, and TUI query entry points and identify their cache settings, stream modes, count requests, and continuation ownership. Trace a fresh build, a cache restore, a normal mutation, compaction, and rebuild. The repaired behavior must apply to each resulting representation.

## 3. Repository workflow and agent ownership

Follow `AGENTS.md` and `docs/ai/README.md`. Keep the original checkout on `master`, preserve its staged and unstaged changes, and perform all implementation and integration in separate worktrees. Do not commit to, merge into, or push `master`.

For the normal checkout at `/home/simon/source/objekte`, use:

| Role | Branch | Worktree |
| --- | --- | --- |
| Supervisor/integrator | `integration/index-lifecycle-repair` | `/home/simon/source/objekte-integration-index-lifecycle-repair` |
| Query execution agent | `task/index-lifecycle-repair/query-execution` | `/home/simon/source/objekte-task-index-lifecycle-repair-query-execution` |
| Index storage agent | `task/index-lifecycle-repair/index-storage` | `/home/simon/source/objekte-task-index-lifecycle-repair-index-storage` |
| Reconciliation agent | `task/index-lifecycle-repair/reconciliation` | `/home/simon/source/objekte-task-index-lifecycle-repair-reconciliation` |
| Desktop lifecycle agent | `task/index-lifecycle-repair/desktop-lifecycle` | `/home/simon/source/objekte-task-index-lifecycle-repair-desktop-lifecycle` |

Use sibling worktrees under `/home/simon/source`, never `/tmp`. If the actual original checkout differs, reconcile its location with the user's worktree preference before creating directories. Reuse an existing task branch only after inspecting its history and local state; do not overwrite it.

Delegate the independent workstreams above after the supervisor publishes the shared contracts. A smaller team may execute the same ownership plan if resources require it. Keep caching and streaming under one query owner because they share the execution engine. Keep compaction, verified mutation, and moves under one storage owner because they share catalog and partition invariants.

The supervisor owns shared public API decisions, publication/shutdown contracts, cross-component integration, and final validation. In particular, assign explicit ownership for changes to `lifecycle.rs` and shared codec/type definitions. Other agents submit narrow interface requests or patches rather than independently rewriting the same sections. Query changes to `run.rs` belong to the query owner; storage changes requiring query routing adjustments must be coordinated with that owner.

Push immediately after every commit on the corresponding branch, including integration merge commits. Review explicit staged paths; do not blanket-stage unrelated files. Do not rewrite published history or force-push. Each subagent reports its pushed commit, test results, measurements, and unresolved issues. Only the supervisor writes to the integration worktree and merges subordinate branches.

Use ignored notes in the original checkout at `docs/ai/memory/index-lifecycle-repair/`. Preserve the initial Git state of the task document and all user comments. Untracked or user-modified task documents remain uncommitted until completion according to the repository workflow. At completion, reconcile the authoritative document into `docs/ai/done/`, append the implementation report, commit it on the integration branch, and push it. Keep the integration worktree for the user's merge. Remove subordinate worktrees only after their work and necessary files are safely preserved.

## 4. Shared correctness and resource contracts

These invariants apply to every workstream:

1. One query, count, stream, or export execution pins one publication. Its workers and continuations do not independently switch revisions.
2. A current query excludes retired records before verification and materialization. Older pinned views retain their own logical index state, subject to source-revision checks when bytes are read.
3. File operations remain index operations. Do not add source-file editing, renaming, or deletion to the library or desktop. Tests and benchmarks may modify disposable fixtures.
4. A file mutation spanning collections publishes all its memberships, records, postings, identities, relations, and metadata atomically.
5. A stale file incarnation/version is a conflict. An unsuitable old-content hint is a strategy fallback only when the target remains current. Never use fallback to act on an unintended newer incarnation.
6. Publication locks protect short root operations. They do not cover extraction, hashing, filesystem traversal, serialization, large copying, or destruction of retired roots.
7. No-op checking and saving do not advance the data revision or invalidate query caches.
8. Memory budgets limit working sets and retention. They must not become unexplained total-dataset limits. Accounting happens before admission/allocation, using checked arithmetic.
9. Unavailable or changed sources preserve the existing typed completeness contract. An incomplete result is never labeled an exact count or a successful complete export.
10. Explicit schema/cache selection remains explicit. Automatic persistence remains restricted to the configured repository policy and explicit opt-in. Do not introduce implicit disk caches or temporary paths in the core.
11. Normal append updates remain sparse and fast. Fixing the optional verified strategy must not move collection-sized work into ordinary mutation or query paths.
12. Preserve broken-file raw fallback, extension behavior, supported index types, exact ordering, and deterministic tie-breaking.

Before parallel implementation, document a small shared contract covering cache ownership, record addresses across partitions, source routing for moves, maintenance cancellation, scheduler transfer, and quiescence/final-save semantics. Use existing types where possible. Do not introduce a general database or task framework.

## 5. R1 — Restore verified-result caching through lifecycle queries

### 5.1 Cache ownership and validity

Reuse and adapt the existing verified membership and ordered-result cache. Provide its configuration through the lifecycle API and caller configuration; the cache must not be usable only through the legacy static repository API.

A cache entry must be owned by, or explicitly keyed to, the index instance and publication revision. Include the compatible configuration/source semantics where revision ownership does not already imply them. Query identity must include the normalized semantic terms, relevant collection scope, resolved clock/parameters, and ordering semantics for ordered entries. Page offset and page size must not fragment reusable membership/ordering entries. Audit existing keys before changing them.

Use publication identity for invalidation, with no filesystem traversal or per-candidate metadata checks just to validate a cache hit. Actual source materialization still follows source-revision checks. Cached membership does not preserve file bodies and must not be advertised as a source-content cache.

Conservative whole-revision invalidation is acceptable. Cross-revision cache reuse is optional and must not delay this repair. Old views may use entries for their own revision, but a newer view must never consume those addresses as current records.

Bound retention across all revisions and pinned views. Do not allocate a fresh independent 64 MiB cache for every publication or tab. Use a shared bounded store or equivalent accounting, and remove obsolete entries when they cannot be reused. Do not hold its mutex while executing a query, reading source files, constructing a large entry, or dropping a large displaced entry.

### 5.2 Integration requirements

Cover synchronous search, context-aware search, streaming page delivery, exact counting where useful, and desktop pagination. The CLI and TUI must receive the same behavior when they use lifecycle execution. Match the previous supported query semantics, including sorting and frozen-clock behavior.

Publish only complete, successfully verified entries. Cancellation, resource exhaustion, or unavailable sources must not leave a partial entry that can later masquerade as a complete result. A query that intentionally stops after finding a small page must not populate a complete-membership cache from that prefix.

Do not force a complete scan solely to warm the cache for a request that can legitimately stop early. Populate complete entries when the execution already computes complete membership, when explicitly requested, or under an existing documented cache-fill policy. Repeated counted and fully evaluated queries must reuse their results. Ordered pages must reuse established ordering rather than repeatedly sorting or verifying the same full set.

Respect execution limits on cache hits. Either account for the work and retained state correctly or decline reuse when cached execution cannot honor the request's constraints. Expose cache hit/miss and verification work through existing diagnostics or narrow counters usable by tests and benchmarks.

### 5.3 Acceptance tests

Warm a residual/FTS query through `IndexView`, repeat it, and prove a cache hit with reduced verification work and identical ordered results. Fetch later pages and verify reuse of the same membership/order. Include ASCII and Unicode text indexes and a supported sorted query.

Run the same query after add, update, delete, compatible move, compaction, rebuild, and replacement-cache load. Check current-view results and old-view isolation. Distinguish genuinely unchanged scans from data publications. Test clock-dependent queries, cancellation during cache construction, incomplete sources, eviction, and aggregate retention across many revisions.

At least one integration test and benchmark must use the desktop backend query entry point. A warm static `Repository` benchmark alone cannot satisfy R1.

## 6. R2 — Restore incremental execution and bounded exports

### 6.1 Delivery and counting

Connect the lifecycle execution path to an incremental collector/cursor/sink instead of obtaining a complete `SearchResult` and replaying it as a stream. Preserve the public meaning of ordered batches, page replacement events, cancellation, and progress.

Remove unconditional exact counting from merged base/delta execution. Propagate the caller's count request. When ordered indexes or a deterministic traversal permit a final page to be known early, stop or deliver it without verifying the remaining domain solely to compute an unused count. When counting is requested, allow page delivery before the separate remaining count work completes where ordering permits it.

Do not promise early final rows for arbitrary computed sorts: establishing their order can require inspecting the whole candidate set. In that case, report progress, obey cancellation, and use bounded ordering state. Page replacements must follow their established provisional-result contract; never mark a provisional ordering as final.

Merge base and partition/delta candidates under one deterministic comparator and one pinned view. Handle overlapping logical identities, active masks, offset/limit, null ordering, collation, computed sorts, and relation-dependent queries consistently. Avoid one separate algorithm per caller; adapt existing planning, collectors, verification, and materialization.

### 6.2 Memory and export behavior

An all-results export must not retain every expanded `SearchHit`, path string, or body before emitting its first batch. Traverse existing ordered indexes/partition cursors where available. If additional sorting is necessary, use compact addresses and a bounded sort strategy. Spill/merge is an option only with explicit scratch configuration and reliable cleanup; it is not an authorization to invent a hidden core path or add a new dependency unnecessarily.

A compact complete membership list is acceptable only within an explicit accounted budget and when execution semantics require it. It must not be the universal prerequisite for ordered streaming. Collections exceeding the in-memory result budget must remain exportable through an appropriate bounded route for supported indexed ordering. For an operation that genuinely cannot fit the configured resources, fail with a typed error before exceeding them and retain the existing partial-output contract.

Account for heaps, address arrays, pending materialization, per-partition cursors, cached lists, batches, and source buffers. Check byte arithmetic before growth. Include unusually long paths and large record bodies. Do not merely move the current after-the-fact check to a different completed-result vector.

Backpressure and cancellation must stop future verification/materialization and release buffers promptly. A sink failure must not continue scanning the repository or install a partial verified cache. An exact total is published only after complete successful execution.

### 6.3 Acceptance tests

Use a controllable source that stalls late candidates. For a query whose final first page is already knowable, observe the first event before releasing the stalled portion. Repeat with counting enabled to prove page delivery is independent of final count completion. Separately test a computed sort that legitimately needs all candidates.

For `count = false`, assert verification stops when the plan can safely satisfy the page; do not rely only on wall-clock timing. Compare all page concatenations with a trusted full sorted result across base and delta records, mutations, ties, offsets, and empty pages.

Export a result set whose expanded locator representation would exceed the existing 64 MiB default. Verify bounded peak retained memory, complete output, and no preliminary full hit vector. Test small budgets, a single oversized record, sink failure, cancellation, unavailable sources, and generation-safe old-view export during publication.

## 7. R3 — Scale reconciliation and compaction

### 7.1 Reconciliation

Replace the fixed reconciliation file ceiling with bounded observation and preparation batches. Do not simply increase or remove the constant while retaining an unbounded queue of pending extraction work.

Deduplicate physical-file metadata observations across overlapping collections, preserving each logical collection membership and existing alias/symlink policy. The second collection's admission allowance must not be reduced as if already known files were all new files. Equal modification time/size is not a physical-file identity.

Use the existing catalog plus compact scan-generation/seen state, a suitable ordered traversal, or another accounted representation. An O(number of indexed files) compact seen structure can be legitimate; a second unbounded content inventory or unbounded pending-task queue is not. Bound directory-frontier state and preparation concurrency, too. If temporary disk state is needed, its location must be explicitly configured.

Separate observation from proof of absence. Additions and validated replacements may be published in bounded transactions while the scan continues. Retire a missing membership/file only when the relevant source scope has been completely and successfully enumerated. A failed directory, cancellation, or incomplete overlapping collection must not be interpreted as deletion. Preserve the existing conservative policy where a failed membership scope prevents a safe complete file transaction.

Revalidate file versions and observed source revisions when applying changes. A scan prepared before a direct API mutation must not overwrite that mutation. Coalesce repeated automatic requests and bound pending work. Preserve observable progress, partial outcomes, cancellation, and retries. Unchanged files require no content read; reuse metadata observations within a pass unless a retry or race requires a fresh check.

### 7.2 Compaction

Replace the allocated-slot ceiling with partitioned/batched compaction. Its per-job working set may be bounded, but a collection with more than 262,144 records must remain compactable, including when retired slots take an otherwise smaller collection beyond that number.

Compact active contributions without reading source content. This must work when source files are missing or unavailable. Process base and delta data consistently and remap all required postings, sorting, identities, relations, diagnostics, coverage, and ownership metadata. Keep logical file identity/version rules and public locator invalidation explicit.

Share unaffected state. Retain only the old pinned view, the required new representation, and bounded additional working state; measure unavoidable old-plus-new coexistence separately from temporary copying. Do not make another complete deep copy of the base merely to iterate it.

Publish the completed coherent replacement atomically, or use a documented partition-publication protocol preserving the same query semantics and concurrent writes. Retain bounded retry/rebase behavior under mutation conflicts. A permanently busy repository may produce an observable retry/cancellation outcome, but never an incorrect successful publication.

Cancellation or failure leaves the current view searchable. Report progress in useful work units and distinguish obsolete records removed from allocator capacity and memory still held by old readers.

### 7.3 Acceptance tests

Use configurable small batch budgets in routine tests to exercise multiple batches without hundreds of thousands of filesystem entries. Then run a real scale check above 250,000 files; target at least 300,000. Include two overlapping collections selecting the same 150,000 files and verify updates to shared files, metadata-read deduplication, and complete outcomes.

Test failed directories late in enumeration, cancellation between batches, partial overlapping collections, file reappearance, direct concurrent mutations, and repeated scheduling. Confirm failed scans never remove unobserved files merely because their directory could not be read.

Compact more than 262,144 allocated slots, including a collection pushed over the old limit by tombstones. Use a generated multi-record fixture for large record counts where appropriate. Remove source access before compaction, compare semantic results, retain an older reader, and test a concurrent commit and cancellation. Run a million-record scale case if the execution machine has adequate resources; record any resource-based omission explicitly.

## 8. R4 — Preserve automatic checking across cache replacement

Capture the active checking policy independently of transient progress: enabled state and interval, scoped to the current repository/source configuration. Successful same-repository index replacement must install that policy on the new handle. Reuse the replacement's automatic initial reconciliation and avoid starting redundant scans.

Only retire the old scheduling ownership after a valid replacement is ready. A failed/cancelled load preserves the old usable session and its policy. Repeated replacement cannot leave detached periodic workers. Report a scheduling failure as a real disabled/failed state; do not leave a checked control claiming automation is running.

Tag asynchronous policy work with session, index instance, and operation generation. Ignoring a late UI reply is insufficient if its underlying worker can still mutate scheduler state: serialize or validate policy application so superseded commands cannot enable an obsolete handle. A user toggle during loading must resolve to the latest accepted policy, not whichever thread finishes last.

Preserve existing same-repository tabs, query definitions, frozen query clock, outdated-result signaling, and refresh rules. Loading a cache must not become a generic repository reopen. Do not silently enable automatic saving for a new destination; retain its separate destination-scoped opt-in and current load restrictions unless a necessary change is explicitly documented.

Add controller/backend tests for enabled and disabled checking, a changed interval, fast initial scan completion, failed load, cancelled load, rapid successive loads, a concurrent policy toggle, stale worker replies, and cleanup of the previous scheduler. Use controlled scheduling or injected ticks instead of long sleeps. Verify a change made after the initial replacement scan is detected by a later periodic check while the UI still shows enabled.

## 9. R5 — Quiesce mutations before the final save

### 9.1 Closing protocol

Provide a narrow core lifecycle operation or coordinator state that establishes a publication barrier for closing. Adapt existing shutdown APIs where possible; a broad executor rewrite is unnecessary.

The required event order is:

1. Enter a closing/quiescing state and invalidate pending scheduling/policy requests for that session.
2. Stop periodic initiation and stop or drain the automatic cache writer without holding a lock it needs to finish.
3. Cancel or finish and join in-flight reconciliation, rebuild, compaction, and other mutation-producing tasks under a documented policy.
4. Ensure any prepared commit accepted before the barrier is either fully published or safely cancelled; reject/defer new publication while quiesced. Keep searches on existing views available.
5. Capture the final accepted revision and flush it atomically to the selected destination.
6. Confirm the saved revision/target covers that final state before signaling successful close.

Use a barrier that also covers query-triggered reconciliation requests and direct mutation entry points, rather than assuming stopping the timer stops all producers. Existing cache writer serialization and destination guards still apply. Do not hold a writer/publication lock while waiting for workers or writing the cache.

The API must define final-save failure and resumability. Retry uses a consistent quiesced state and a correctly selected destination. Cancel close restores a usable session and its prior explicit automation policy without duplicate workers; already cancelled long tasks need not restart silently. Close without saving releases workers and closes without claiming persistence. Manual-mode close must not gain an implicit cache write.

Keep waiting, joining, and filesystem I/O off the UI thread. Preserve the existing closing dialog and ordinary Retry, Cancel close, and Close without saving interactions. Repository switching must not leave an old producer able to target the new repository's destination.

### 9.2 Deterministic race tests

Use synchronization barriers around commit and snapshot capture. Reproduce a reconciliation or rebuild ready to publish while close begins. Assert that it is either included in the saved final revision or cancelled before publication; no successful mutation may appear after the supposedly final snapshot.

Test pending automatic writes, a slow manual save, concurrent explicit mutation, query-triggered reconciliation, a policy toggle completing during close, flush failure, retry, cancel close, and close without saving. Confirm no lock-order deadlock and no UI-thread join. Reopen the saved cache and compare its active logical records to the quiesced view; distinguish the saved snapshot from subsequent automatic reconciliation of the filesystem.

## 10. R6 — Make verified base maintenance local

### 10.1 Representation and cost

Remove collection-wide staged compaction from an eligible single-file verified update/delete. Reconstructing the supplied old file and processing its new data is legitimate; copying or scanning unrelated collection-sized structures for every such operation is not.

Use independently replaceable partitions/chunks, bounded posting containers, or equivalent existing ownership structures so work scales with the affected file and a bounded amount of surrounding index data. A practical design may partition base records into manageable groups and use the same address/routing model for later additions. The exact layout is an implementation decision for the storage owner, measured before expansion across all index families.

Do not create an unbounded segment per record or trade the update cost for queries scanning thousands of tiny fragments. Bound partition size and merge fan-out by accounted records/bytes, preserve ordered traversal, and measure query overhead. A large individual multi-record file may necessarily touch many partitions; distinguish that legitimate file-sized work from unrelated dataset work.

All normal fresh builds and supported cache restores must receive the efficient behavior. Do not fix only recently appended one-record segments. Prefer building the appropriate representation directly and serializing it consistently. If a cache format changes, version it explicitly and implement the repository's compatibility/rebuild policy with tests; never silently reinterpret old addresses or claim a cache was restored when it was rebuilt.

The necessary internal storage/codec changes are within this task's scope. Use existing dependencies and index families. Stop for a user decision only if the required solution entails a materially different public behavior or a broader system replacement outside these requirements, after presenting measured options.

### 10.2 Verification and mutation semantics

Retain SHA-256 verification of the exact indexed old bytes, configuration compatibility, layout/capability checks, and commit-time file-incarnation/version checks. Do not accept an old-data hint merely because some reconstructed terms are present in the index.

For a compatible one-record file with unchanged logical identity and removable built-in contributions, implement true slot-preserving replacement for base and delta storage, using private copy-on-write structures so an old view remains unchanged. Moving a container in memory does not require changing the record's logical slot. For other supported physical-removal cases where record layout or identity changes, preserve the documented identity/version rules and explicitly invalidate generation-qualified locators when relocation occurs; do not label that operation slot-preserving. Multi-record slot preservation requires a reliable record mapping and updated offsets.

Physically remove the old contribution from the new published structures. Keeping the old contribution behind a tombstone and calling it verified removal does not satisfy this requirement. Old immutable readers may retain prior containers; immediate allocator release is not required.

Cover text bigrams, typed postings, presence/null/array facts, coverage, sorting, IDs, relations/backlinks, raw fallback, custom supported contributions, and multi-collection ownership. Unsupported nondeterministic or non-removable extension behavior retains an explicit safe fallback. Report the selected strategy and reason accurately.

Keep appended replacement as the ordinary default. Old-data mismatch falls back atomically; stale target version conflicts. Do not make every base update fall back as the final answer to R6. Eligible built-in base operations must demonstrate actual local verified maintenance.

### 10.3 Acceptance tests and measurements

Test fresh-base and restored-base verified update/delete, plus delta operations, with old readers retained. Use unchanged and changed logical IDs, multi-record files, changed ranges, multiple collections, sorting, relations, raw fallback, unsupported extensions, stale hints, stale handles, and concurrent publication.

Instrument copied/scanned records, touched partitions, allocation volume or equivalent counters, and strategy/fallback outcomes. Hold the changed file fixed while increasing unrelated collection size. The operation must not walk or copy every record as the collection grows.

Repeat the documented 148,486-file benchmark using an original base file and verified old bytes. The current approximately 1.28-second behavior must disappear. Aim for single-digit millisecond median latency for a small one-record file on comparable hardware; this is an engineering target, not a universal hardware guarantee. The mandatory acceptance is bounded affected-data work, actual verified removal/preservation, and a substantial measured improvement without material query/build regressions. If the target is missed, explain the remaining measured cost before declaring the task complete.

## 11. R7 — Add compatible moves without re-extraction

### 11.1 Eligibility and content proof

The application has already moved the source file. The library changes index state only. Preserve the file's logical incarnation identity where appropriate, advance its version, update path ownership atomically, and reject destination collisions and unsafe paths.

A cheap move is eligible only when content and indexing semantics are compatible. Check collection memberships, reader/extractor registration and version, schema/configuration, record framing, path-dependent identity, and extension capabilities. A different filename extension can select a different reader. A move that also changes content uses ordinary replacement.

Do not use equal modification time and size as proof of identical bytes. The conservative route may hash the destination and compare the stored raw digest; that costs file-size I/O but avoids extraction and index reconstruction. If the API offers an explicit trusted rename notification/content-unchanged assertion, document the caller's responsibility and make it opt-in. A handle or a previously stored digest alone cannot prove the current destination is unchanged. Do not silently weaken the default contract.

Distinguish metadata-only routing work, content hashing, extraction, and posting reconstruction in diagnostics and benchmarks. State whether a public fast path avoids parsing or also avoids content I/O. Do not advertise constant-time movement when a digest read is required.

### 11.2 Routing and derived values

Preserve content-derived postings and record ranges for an eligible move. Route source access through the file catalog or equivalent revision-owned indirection. Update embedded path metadata only where required, within bounded affected structures. Do not merely weaken the segment/catalog consistency check and leave reads using the old path.

Audit result locators, direct record lookup, materialization, source views, sorting/filtering on path or filename, diagnostics, relations, and cache serialization. Every reader must resolve a path from the same pinned view as its record address. New views use the destination; old views may report the old source unavailable and must not silently read unrelated replacement bytes.

For path-dependent logical IDs or extraction, implement the necessary bounded metadata updates where straightforward, or use an observable ordinary reindex fallback. Full compatibility is required across affected collections; partial reuse must not expose mixed metadata. At least one normal built-in configuration with path-independent content IDs must exercise the cheap route, including a multi-record file.

Round-trip moved state through flush/load and compaction. Background reconciliation need not infer renames: its conservative delete/add behavior remains valid when no explicit move notification exists.

### 11.3 Acceptance tests

Test within-collection compatible moves, cross-collection moves, changed reader/extension, filename-derived IDs, path sorting/filtering, destination collisions, changed destination content with equal mtime/size, missing destinations, stale handles, and concurrent moves/updates. Preserve custom-source safety.

Use extraction counters to prove zero re-extraction and unchanged content postings for the eligible route. With a large multi-record file, prove no record-by-record posting reconstruction. Measure optional hashing separately. Exercise returned file paths, record tabs, direct lookup, subsequent update/delete, old-view behavior, cache restore, and compaction.

## 12. Implementation phases and commit boundaries

Each phase has reviewable commits. Adapt the number of commits to the changes, while keeping unrelated repairs separate and pushing each commit immediately. Do not create empty commits just to match this list.

| Phase | Work and suggested commit subjects | Exit criterion |
| --- | --- | --- |
| 0 | `test: reproduce lifecycle query and maintenance regressions`; record baseline and shared contracts | Findings revalidated, meaningful reproductions/counters present, ownership assigned |
| 1 | `refactor: define revision-scoped cache and lifecycle operation contracts`; prototype storage locality | Query/storage addresses and shutdown/policy contracts agreed; bounded mutation approach measured |
| 2A | `fix: restore verified query caching for index views`; `fix: stream lifecycle query results incrementally`; `fix: bound ordered export working memory` | R1/R2 targeted tests pass through lifecycle and desktop adapters |
| 2B | `refactor: partition index contributions for local maintenance`; `fix: compact indexes in bounded batches`; `fix: update verified base contributions locally`; `fix: preserve compatible index contributions on move` | R3 compaction, R6, and R7 pass representation and source-free tests |
| 2C | `fix: reconcile overlapping collections in bounded batches`; implement core quiescence under supervisor ownership | R3 scanning and core shutdown barriers pass deterministic tests |
| 2D | `fix: transfer periodic checking on index replacement`; `fix: save final index revision after quiescence` | R4/R5 controller/backend tests pass, including retry/cancel-close |
| 3 | Integration merges and narrowly scoped regression corrections | All combined correctness tests pass against the final representation |
| 4 | `test: validate lifecycle query and maintenance performance`; evidence/report updates | Required benchmark matrix and native desktop checks complete |
| 5 | `docs: complete index lifecycle repair report` | All seven requirements accounted for; completed document committed and pushed |

Phases 2A–2D may run concurrently after shared contracts are available. The query owner must integrate and revalidate against the storage representation before performance conclusions. Desktop shutdown work depends on the core quiescence contract; UI policy-transfer work can proceed independently. The supervisor must not merge a final query implementation that only supports the former monolithic base layout.

Prioritize restoring query reuse/delivery and eliminating the two UI bugs early. Continue through storage locality, large-scale maintenance, and move behavior; those are required scope, not optional follow-up work.

## 13. Validation matrix

Use the repository's established test/benchmark commands and extend existing harnesses. Avoid a separate benchmark framework. Keep test-only synchronization deterministic and remove production timing dependencies introduced only for testing.

| Area | Mandatory coverage |
| --- | --- |
| Query semantics | Exact result membership/order/count before and after each mutation; active filtering; sorted pagination; relation and raw fallback routes |
| Query caching | Same-revision hits, ordered reuse, revision/instance isolation, frozen clock, incomplete/cancelled execution, aggregate eviction budget |
| Streaming | Early first event where order permits, optional count, provisional vs final semantics, cancellation/backpressure, bounded all-results export |
| Reconciliation | More than 250,000 files, overlapping collections, deduplicated metadata, small batches, failures, cancellation, concurrent mutations |
| Compaction | More than 262,144 allocated records, obsolete-slot overflow, absent sources, old readers, concurrent commit, cancellation, persistence |
| Verified mutation | Base/delta/restored base; genuine physical removal; slot/identity behavior; stale data vs stale target; locality counters |
| Move | Proven compatible content, metadata routing, multi-record file, path-derived behavior, safe fallback, persistence and source access |
| Desktop policy | Same-repository load, interval retention, periodic work after initial check, stale replies, failed load and scheduler cleanup |
| Close | Publication barrier, no late commit after final snapshot, save failure/retry/cancel, manual no-write close, no deadlock/UI-thread join |
| Persistence | Coherent pinned revision, target-specific dirty state, destination protection, atomic write, compatible cache format/load policy |

Run targeted tests during implementation. At final integration run the workspace test suite, relevant binary checks, changed-file formatting checks, and `git diff --check`. Use the documented toolchain. Distinguish preexisting repository-wide formatting failures from changed-code failures; do not reformat unrelated files.

Suggested baseline commands, subject to the repository's current documentation:

```text
cargo test --locked -p objekte-search-core
cargo test --locked -p objekte-desktop --lib
cargo test --locked --workspace --all-targets
git diff --check
```

Verify actual package/target names and required features before using these commands. Report exact commands and results. An unavailable toolchain or GUI environment is a validation blocker, not a pass.

### Native desktop verification

Follow `docs/development/gui_environment.md` and use the existing native app, not a browser substitute. On a representative collection:

1. Load a cache and search while initial reconciliation remains active.
2. Repeat a query and paginate; confirm lifecycle cache counters and responsive delivery.
3. Enable periodic checking, load another compatible cache, wait for its initial check, then externally change a fixture and observe a subsequent periodic update.
4. Keep record tabs open through replacement, compaction, and a compatible move; verify stale labels, current refresh, correct source routing, and preserved query context.
5. Start reconciliation/rebuild and close with automatic saving enabled; verify the final saved revision and a responsive closing interface.
6. Exercise save failure, Retry, Cancel close, and Close without saving. Confirm periodic checking resumes correctly after cancel and does not continue on discarded handles.

Use screenshots or short recordings as internal validation evidence where useful. Do not claim visual validation based only on controller unit tests.

## 14. Performance validation and acceptance

Record the actual implementation baseline, final commit, build profile, compiler, machine, CPU workers, corpus/schema/workload hashes, cache configuration, and warm/cold conditions. Use disposable fixture copies for mutations. Preserve comparable runs and raw numeric results in the task's generated report directory.

Extend `bench_lifecycle_musenalm` and `bench_lifecycle_mutations` or their current equivalents. Also measure the desktop backend query path. Run the established complete query suite at its current size; the prior report's 194 cases must not be treated as a fixed limit if the suite has grown.

| Measurement | Required comparison |
| --- | --- |
| Repeated queries | Lifecycle cache disabled, first cached execution, same-revision warm execution; ASCII/Unicode; existing static route only as an additional reference |
| Pagination | First and later pages, indexed and computed sort, cache cold/warm, exact count on/off, base-only and mutated views |
| Delivery | Time to first useful event, final page time, final count time, verification counts, cancellation latency |
| Export | Output larger than the old expanded-hit buffer budget; total time, retained working memory, first-batch time, exact output comparison |
| Scan | Small batch budget, at least 300,000 files, two overlapping 150,000-file collections; physical metadata operations and content-read counts |
| Compaction | Above 262,144 allocated records, high tombstone ratio, source-free execution; working memory, throughput, reader concurrency |
| Verified updates | Original base, restored base, delta; fixed changed file as unrelated collection grows; copied/scanned records and allocation |
| Move | Compatible large multi-record file versus reindex fallback; hash bytes/time, extraction calls, postings rewritten, memory |
| Existing fast paths | Fresh build, ordinary appended update/delete, flush/load, no-change scan, representative query latency during maintenance |

Use at least five repetitions for ordinary query comparisons and at least three independent mutation/maintenance runs, following the established harness where it already specifies more. Report medians and relevant tails, not only a single best result. Distinguish peak process RSS, live retained index/cache memory, and per-operation allocation/copy counts.

The first two findings require direct evidence: cache hits on the lifecycle route and first-result delivery before avoidable remaining work. Large-dataset fixes require actual scale runs as well as reduced-limit unit tests. Verified locality requires counters or profiling beyond observing one faster timing.

Treat a repeatable material slowdown of unaffected query, normal-update, or fresh-build paths as unresolved until investigated. Use both relative and absolute changes so microsecond noise does not block completion; flag roughly 10–20% sustained regressions outside observed variance and all new collection-sized costs. Do not hide them by comparing different query options, cache settings, datasets, profiles, or unoptimized builds. Explain necessary tradeoffs with measurements and obtain a user decision only when the accepted requirements genuinely cannot be met together.

## 15. Required delivery and completion report

Deliver one pushed integration branch for the user to merge. Do not merge it into `master`.

Store the detailed repair validation report under `docs/ai/generated/index-lifecycle-repair/`, including concise commands, environment, test outcomes, benchmark tables, interpretation, and references to preserved raw outputs. Update the known verified-base performance issue with its resolution and evidence when R6 is actually satisfied; preserve its historical observations.

Append `Implementation report` to the completed task document. Include:

- Task identifier, completion date, actual baseline and final implementation commits, integration branch and worktree.
- A row for every R1–R7 requirement identifying the implementation, focused regression test, and measured evidence.
- Contributing subtask branches and commits, public API/cache-format changes, and compatibility policy.
- Full integration checks, native desktop validation, benchmark commands/results, and any unrun checks with concrete reasons.
- Remaining limitations, distinguishing genuine resource limits or unsupported extension capabilities from unresolved required behavior.
- Task-document Git-state reconciliation, push status, and subordinate worktree cleanup status.

Do not report complete if lifecycle cache reuse is still bypassed, streaming is still a complete-result replay, routine scale cases hit the former ceilings, periodic checking remains only visually enabled, final-save races persist, eligible base verification still rewrites a collection, or compatible moves still re-extract every record.

Keep the integration worktree and branch. Remove only completed task-specific temporary notes and safely merged subordinate worktrees according to the repository workflow. The final response to the user must identify the mergeable branch, evidence, and any material remaining limitation plainly.

## Implementation report

**Task:** `index-lifecycle-repair`  
**Completed:** 2026-11-09  
**Base:** `63f354b2b3d0bb7749f8af5187919c600767a8ea`  
**Final implementation commit:** `66a0260e751d955a08b274229fb9b5839b4e397a`  
**Integration branch:** `integration/index-lifecycle-repair`  
**Integration worktree:** `/home/simon/source/objekte-integration-index-lifecycle-repair`  
**Detailed evidence:** [Index lifecycle repair validation](../generated/index-lifecycle-repair/validation.md), including raw benchmark outputs and native screenshots.

| Requirement | Implementation | Focused test and measured evidence |
| --- | --- | --- |
| R1 | Bounded, instance/revision-scoped verified cache shared through `IndexView` and the desktop, CLI, and TUI query paths. | `desktop_page_reuses_verified_order_and_keeps_pinned_revision`; desktop backend 2,000-record test: first query 48.632 ms/2,000 scans, next six 0.364–0.431 ms/cache hits/zero scans. Final Musenalm selected warm queries: 50/50 cache hits with zero scans, 0.011–0.024 ms. |
| R2 | Incremental stored-order page cursor, optional exact count, bounded all-results batches with cancellation. | `ordered_rows_arrive_before_count_finishes_and_reconstruct_page`; 24,000-row export delivered its first batch in 0.244 ms, completed in 70.133 ms with 57,260 bytes peak buffered data versus 80.592 MB expanded locators. |
| R3 | Batched physical reconciliation with overlapping collection membership; source-free, bounded-shard compaction without a total-slot ceiling. | `reconciles_300000_files_and_updates_a_shared_file`: 300,000 physical/450,000 logical records; unchanged scan indexed zero bytes, one shared update indexed 16 changed bytes once. Source-free compaction passed above 262,144 slots and at 1,000,001 active records with pinned readers and concurrent retry. |
| R4 | Desktop replacement transfers the active periodic-check policy through the policy gate and operation generation; UI reflects the replacement scheduler. | `loaded_index_receives_check_policy_before_old_scheduler_retires`; native cache replacement, subsequent external edit, outdated notice, Refresh, and Cancel close followed by another periodic edit were observed. |
| R5 | Core quiescence rejects late publication, drains mutation producers and the automatic writer, then saves the final accepted revision; retry/cancel restore the session. | `concurrent_publication_is_either_saved_or_rejected_by_quiescence`, `pending_automatic_save_enable_cannot_write_after_manual_close`; native permission-failure dialog exercised Retry, Cancel close, and Close without saving. |
| R6 | Bounded 2,048-record built-in JSON base shards and slot-preserving local verified replacement; fallback for unsupported readers/oversized files. | `restored_shared_partition_verified_update_preserves_its_record_slot`; three independent Musenalm runs: original-base verified update 45.585 ms median versus historical 1,278.918 ms, all `VerifiedSlotPreserving`. Fresh build 1,321.780 ms versus 1,317.728 ms. |
| R7 | Compatible digest-proven move retains postings and slots, updates catalog routing, and preserves pinned source identity; unsuitable moves use replacement. | `compatible_move_keeps_index_contributions_and_advances_file_version`; 50,000-record JSONL move median 1.132 ms with zero extraction/parsing/posting rewrite versus fallback 94.982 ms, 50,000 parsed records, and 100,000 posting keys rewritten. |

**Contributing branches and commits.** `task/index-lifecycle-repair/index-storage` culminated at `0f4ee5e649` (partitioned storage, verified updates, compatible moves and pinned routing); `task/index-lifecycle-repair/reconciliation` at `4322e59a80` (bounded overlapping scan); `task/index-lifecycle-repair/query-execution` at `66a0260e75` (cache, stream, query correctness and execution). The supervisor integrated these with the R4/R5 desktop and core work and pushed every resulting implementation commit. The verified-base performance issue is resolved with evidence in `docs/ai/issues/perf/verified-base-maintenance-collection-rewrite.md` while preserving its history.

**Public API and persistence.** `Index::configure_verified_cache`, `Index::quiesce`, and `resume_after_quiesce` expose cache and closing control; `IndexView` provides shared query/count/stream use. `move_file` retains its existing index-only contract after the caller moves the source. Static snapshot format remains 17; partitioned lifecycle snapshots use 18 with explicit validation. Incompatible older lifecycle snapshots fail explicit load and follow configured rebuild policy on convenience open. Verified query cache is memory-only and never serialized.

**Integration validation.** `cargo test --locked --workspace --all-targets` passed at the final implementation commit: 465 core tests (11 ignored), 116 desktop (1 ignored), 77 CLI, 32 CLI process (1 ignored), 49 TUI, and all other targets. Relevant ignored scale/measurement tests ran separately with commands and outputs in the linked report. `cargo check --locked --workspace --all-targets`, the native debug build and launch, and `git diff --check 63f354b2b3..HEAD` passed. `cargo fmt --all -- --check` still fails on preexisting formatting: 101 failing paths here versus 107 on `master`, with no newly failing paths. The unused `UiState::index_mode` compiler warning remains.

**Performance comparison.** Three independent Musenalm storage runs used disposable mutation copies. The full final query matrix used the original 148,486-file source and 194 cases × two text profiles × five repetitions; all 1,940 runs matched static `master` totals, page counts and completeness flags. Cache-disabled median per-case latency ratios were 1.381× ASCII and 1.388× Unicode, with 44/194 and 53/194 cases over 2×. The first cache-enabled execution is slower because it builds complete membership serially; repeated same-revision queries hit the shared cache and scan zero records. See the linked report for exact commands, environment, hashes, medians, tails and raw outputs. Native verification used a fixed 1920×1080, 100% scale, 60 Hz Sway/WayVNC session. A representative configured collection was opened and used for pagination, search, cache reload, periodic updates, tabs, compaction, and all save-failure choices; final binary launch was repeated after the last implementation commit.

**Measured limits.** The local verified-base update is about 28× faster than the historical collection rewrite yet still 40–46 ms, above the single-digit-ms engineering target. Source-free compaction is 45% slower and rebuild 15% slower on the recorded corpus; the linked report provides counters and explains the affected-shard costs. Several cold source-reading queries and source-free relation queries remain slower than static search; the latter spend 24.5–27.3 ms unioning partition postings. Warm cache reuse addresses repeat queries but does not erase those cold costs. A long in-flight rebuild during native close was covered by deterministic core/controller tests but not visually exercised. The desktop has no direct compatible-move menu; move routing and pinned views were exercised through core tests. These native visual limits are reported separately from passed automated validation.

**Document and worktree state.** The authoritative task document was untracked in the original `master` checkout. Its complete SHA-256 `20dbb0beb4fc2178e01da6a44521a6cf6ec8df54f2dde13023ab56696fd5a322` was copied byte-for-byte before this report was appended; the original index and other user files were left untouched. The obsolete untracked source will be removed only after the completion commit is pushed and latest-content equality is rechecked. The three subordinate worktrees are retained until that push and then subject to normal safe removal. The integration branch and worktree remain for the user's merge; `master` is unchanged.
