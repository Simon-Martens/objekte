# Objekte: implement the archiving repairs on master

## 1. Task

Implement the five repairs below in Objekte. Begin with the current `master` source and treat every requirement as pending. Complete the implementation, validate it, and commit and push the finished changes to `master`.

This is a self-contained implementation task. The document defines all required behavior, tests, and delivery conditions. Read the actual source to adapt the changes to its current structures; do not assume the listed function signatures or file locations are immutable.

| Identifier | Repair | Main benefit |
| --- | --- | --- |
| C01 | Make quiescence completion independent of concurrent resume and subsequent drains. | Prevent hangs and unintended additional drains. |
| C02 | Evaluate metadata predicates without constructing unnecessary owned IDs, paths, and rows per candidate. | Reduce query allocation and execution cost. |
| C03 | Share unchanged data during archive preparation. | Reduce copying, preparation time, and peak memory. |
| C04 | Serialize catalog and segment metadata through borrowed row/path adapters. | Remove transient owned serialization copies. |
| C05 | Convert each distinct native storage owner once, scheduling independent base, catalog, and segment work together. | Remove duplicate conversion and unnecessary serial work. |

Preserve public API behavior, query semantics, native/archived/mixed operation, explicit archive invocation, atomic publication, pinned readers, and the current archive format. Do not introduce another storage backend, implicit archiving, persistence features, or frontend changes unrelated to these repairs.

**Excluded:** report-field corrections, owner-count reporting, newly-serialized-byte reporting, installation/retirement timing corrections, historical coverage-table maintenance, and new reporting infrastructure. These are not acceptance conditions. Verification of actual ownership, correctness, and conversion behavior remains required through focused tests.

**Benchmark scope is deliberately small.** Follow section 9. Do not run a worker/alphabet matrix, a repeated complete query benchmark suite, or a broad allocation-profiling campaign.

## 2. Work directly on master without disturbing other work

The user explicitly authorizes implementing, committing, and pushing these repairs on `master`. This overrides a repository default requiring task delivery on a separate integration branch. Read `AGENTS.md` and `docs/ai/README.md` and follow their other applicable instructions.

1. Identify the correct repository checkout, current branch, master commit, configured remote, and local/staged changes. Record the starting commit.
2. Fetch the configured remote using the normal repository procedure. Preserve any concurrent master commits. Do not reset or force-update master to match a remote reference.
3. Use one coordinated writer for the master checkout. If another worker owns overlapping files, arrange exclusive ownership of those edits; continue independent work while the overlap is resolved. Never switch, stash, reset, clean, or stage another worker's checkout or changes.
4. Implement the repairs in the order in section 8. Stage only task-owned files or hunks. Preserve unrelated changes, including unrelated changes in the same file.
5. Make coherent, buildable commits. Push each commit according to the repository workflow. If master advances remotely, incorporate the new commits safely, resolve conflicts in the affected code, and rerun affected tests. Do not force-push.
6. Deliver the final master commit and confirm that it is present on the configured remote. A separate repair branch alone is not completion of this assignment.

Do not spend time searching other branches for implementations. Work from the code present on master. If the current source already uses a correct equivalent of a required design, retain that design and verify its behavior rather than adding redundant machinery. Every C01–C05 outcome must still be established by this task.

Implementation entry points:

| Repair | Files and symbols to inspect |
| --- | --- |
| C01 | `crates/core/src/lifecycle.rs`: `quiesce`, `closed_outcome`, `resume_after_quiesce`, `perform_quiesce`, close state and transition gates |
| C02 | `executor.rs`: `segment_metadata_matches`, metadata probe/context construction; shared metadata operand evaluation and callers in `run.rs` |
| C03 | `repository.rs`: `archive_candidate`, `CollectionState`; `inventory.rs`: `with_published_paths`; segment candidate construction in `lifecycle_archive.rs` |
| C04 | `segment_archive.rs`, `lifecycle_catalog.rs`, `archived_path.rs`: borrowed adapters, `StoredFileRow`, `StoredPath::from_shared`, serialization and resolution |
| C05 | `lifecycle_archive.rs`: base/catalog conversion, `archive_segment_groups`; backing owners in catalog, posting, and segment storage |

## 3. C01 — bind each quiescence caller to its drain completion

### Defect

`quiesce()` can observe `Quiesced` while holding the close-state lock, release the lock, and then call `closed_outcome()`, which locks again and waits for the global state to become `Quiesced`. Concurrent `resume_after_quiesce()` can change the global state to `Running` in the gap. The caller then waits for a later drain that may never happen.

A second interleaving affects a caller waiting during drain A. A completes, another caller resumes, and the delayed waiter observes `Running` when it wakes. It may initiate drain B instead of returning A's result. Repair both interleavings.

### Required implementation

Associate each drain with a retained completion object. Prefer a small `Arc`-owned operation containing a completion slot and condition variable, using the existing result/error types. The close coordinator tracks whether it is running, draining a particular operation, or quiesced with that operation's completed result.

The state transitions are:

| State observed by caller | Action |
| --- | --- |
| Running | One caller creates the operation and becomes its drain leader. |
| Draining | The caller captures a strong reference to that operation and waits for its outcome. |
| Quiesced | The caller obtains that operation's completed outcome. |
| Resume after completion | The coordinator returns to Running; existing callers retain the operation they selected. |

Capture the selected operation under the coordinator lock, then release that lock before waiting. Wait on the selected operation's completion predicate, not on the coordinator's future global state. A completed result remains available as long as attached callers need it; avoid an unbounded history of old results.

Publish the outcome and completed coordinator state consistently before resume can observe completion. Establish a single lock order. Waiters must never hold the close-control lock while waiting, or reacquire it while holding a completion lock in the reverse order. Notify every attached waiter on completion, including error completion.

Preserve the current public error/cancellation semantics and the rejection of resume while draining. Preserve the publication-admission rules. Release the transition write gate before cancelling or joining workers; holding that gate while a worker tries to publish can deadlock. Do not hold close-control or publication locks across external callbacks or worker joins.

An existing equivalent operation/generation mechanism is acceptable if it retains the exact completion selected by every caller. Merely returning the result under the initial lock repairs only the already-completed path; it does not by itself repair delayed active-drain waiters.

### Tests

Use barriers or narrow test hooks to force the interleavings. Sleeps are not synchronization; a timeout is only a failure bound.

- A caller selects completed drain A; resume occurs before the caller returns. The caller returns A's result without waiting for another drain.
- Multiple callers join active drain A. Delay one until after completion and resume. All receive A's result, and only one caller executes A's drain work.
- Drain B starts after resume while a delayed A caller still exists. Both callers receive their own selected operation's result.
- The retained outcome remains correct when A completes with an injected error.
- Existing reconciliation-preparation and paused-archive tests continue to demonstrate that quiescence does not deadlock workers or permit publication after close.

## 4. C02 — remove unnecessary metadata-query ownership construction

### Defect

The metadata candidate loop uses borrowed rows but still creates `RecordId(Arc::new(...))` per candidate. Even an unused ID can allocate an empty `Arc<String>`. It also constructs placeholder paths and can materialize catalog entries to evaluate file-path predicates. Borrowing the outer row does not eliminate these internal allocations.

### Required implementation

Evaluate metadata through borrowed operands or a narrow borrowed metadata view. Extend the existing evaluator rather than creating a second evaluator with different comparison rules. Avoid synthesizing full owning `RecordEntry` or `FileEntry` objects solely to satisfy a generic context interface.

- Read scalar metadata directly from the relevant borrowed source.
- Borrow record IDs for ordinary identity comparisons.
- Read file paths from the current published catalog/path directory. Preserve moves and current file routing; a stale segment-local path is not an acceptable substitute.
- Evaluate collection-name and repository-root expressions once when they are constant over the candidate domain, without fetching an arbitrary file row to construct a context.
- Determine which operands are needed once per term/execution. Field-to-field comparisons must consider both operands.
- Materialize owned output only where ownership is required by a result or explicit compatibility API, and only for the selected objects.

Inspect the full operand evaluation path: replacing an explicit `to_owned()` with a helper that internally constructs `Value::String` does not eliminate the copy. Preserve missing/null behavior, transformations, case handling, IDs, record ranges, revisions, times, and file status. Confirm which catalog fields are authoritative when one file belongs to multiple collections or has been moved/replaced.

Ordinary scalar, ID, and path comparisons must not construct unused owned placeholders or copy strings per examined candidate. Operations that inherently transform text may use the existing appropriate scratch mechanism; this task does not require rewriting every arbitrary transformation to allocate nothing.

### Tests

Use a small fixture to compare native and archived results for scalar/range metadata, record IDs, file paths, both operands referencing metadata, zero matches, and nonzero matches. Include moved/replaced files and malformed/fallback records using existing fixtures where possible. Exercise ordering or result materialization if those paths share the changed accessors.

Verify the allocation change by reading the complete executed path and using existing lightweight test instrumentation where available. A small isolated allocation check may assert that zero-result scans do not allocate one ID/path object per candidate. Do not build a new profiling framework or run large datasets for this check. Existing whole-row-cache assertions are useful but do not alone prove that the candidate loop avoids allocations.

## 5. C03 — share unchanged payloads during archive preparation

### Defect

Archive candidate construction clones collection state and query caches, including some directly owned maps, bitmaps, or metadata. Segment candidate construction can copy unchanged payloads with its outer index. `with_published_paths()` clones an inventory and then clears its native path maps, paying for copies that the candidate immediately discards.

### Required implementation

Inspect actual field ownership and `Clone` behavior. Cloning an `Arc` is intended; cloning its backing payload is not. Split large immutable payloads from small representation/publication wrappers where necessary.

| Data category | Required treatment |
| --- | --- |
| Immutable payload unchanged by archiving | Share its existing owner. |
| Native component being serialized | Borrow it until a validated replacement exists. |
| Small publication/control wrapper | Copy only the required wrapper state. |
| Populated mutable query cache | Preserve valid cached behavior without copying the entire cache. |
| Native path maps replaced by a published directory | Omit them from the candidate rather than cloning and clearing them. |

Use the existing immutable-publication or copy-on-write model. Updating a representation field must not trigger copying an adjacent unchanged payload. Relevant targets include immutable generations, ID maps, classification bitmaps, and segment/control arrays.

Construct the replacement inventory wrapper explicitly: share retained fields, install the current published path directory, and leave obsolete native lookup maps absent/empty. Keep the old wrapper valid for pinned readers.

For caches, inspect validity keys and retained references. Share the existing synchronized cache owner when archiving preserves its logical validity. If a cache contains representation-dependent references, adapt validity/ownership narrowly so stale references are never reused. Do not simply empty a valid populated cache to make archiving appear cheaper; that transfers the work to later queries.

Prepare converted data in locals. Do not destructively extract from the published source to avoid allocation. On conversion or validation failure, leave the source and pinned readers fully usable. After success, the old source remains alive only for legitimate readers and necessary references; do not introduce permanent retention of obsolete owners.

### Tests

Cover standalone repository and live-index archiving where supported, including populated caches, mixed state after mutation, and a retained pre-archive view. Use pointer identity to show unchanged backing owners are shared. Use weak references or existing lifetime tests to check eventual release of replaced owners after the last old reader is dropped. Check the backing allocation, not merely an outer wrapper.

Reuse focused failure-injection tests to verify transactional behavior. Use small populated-cache fixtures to verify query behavior across archive. Large allocation/peak-memory profiling is not required.

## 6. C04 — serialize borrowed metadata rows and paths

### Defect

The catalog adapter constructs an owned `StoredFileRow` per row. Segment file adapters construct `StoredPath` during serialization and again during resolution. Ordinary paths copy strings; escaped paths can also copy native path bytes. Removing a complete staging vector leaves these per-row copies in place.

### Required implementation

Preserve the current archive representation and use the workspace's existing `rkyv` version. Implement borrowed adapters targeting the same archived types and resolver contracts. Do not upgrade the dependency or change the format to avoid resolving the adapter correctly.

Implement a borrowed path adapter with the following behavior:

- Ordinary UTF-8 paths serialize the existing string slice and resolve it using its associated resolver.
- Escaped Unix paths serialize the existing query representation and exact borrowed native `OsStr` bytes, preserving the variant and field layout.
- Windows paths retain the supported native encoding. Use a sequence/iterator serializer when available. If the API requires a contiguous UTF-16 slice, reuse worker-local scratch capacity instead of allocating a fresh vector per row. Lossy conversion is prohibited.
- Resolver state retains the positions, lengths, and variant information needed to resolve the borrowed values. Resolution must not reconstruct an owned path.

Inspect query-key accessors as well: a seemingly borrowed call may generate a new escaped string. Reuse a suitable existing representation or scratch mechanism while preserving exact query/path semantics.

Complete the catalog row adapter by serializing borrowed variable-length fields directly: paths, diagnostic data, collection memberships, and any other stored sequences. Copy scalar and optional fixed-size values as needed. Preserve fingerprints, configuration identifiers, source revisions, record/file ownership, and diagnostics. Do not build a temporary complete `StoredFileRow` or a vector of such rows.

Each independent conversion job owns its serializer context and may reuse suitable scratch capacity within that job. Do not serialize every job through a global mutex. Preserve checked byte validation and semantic validation before candidate publication.

Custom resolution is a correctness-sensitive area. Incorrect positions, relative pointers, alignment, or enum layout can invalidate the archive. Use established serializer APIs and the repository's working resolver patterns. Do not use unchecked roots, layout casts, transmutation, or disabled validation as substitutes for a correct adapter. Keep any required unsafe block narrow and document its invariants.

### Tests

Use small round-trip fixtures and the existing checked archive validator. Where an owned adapter already provides a useful oracle, compare decoded values with the borrowed adapter. Require semantic equality and valid archives; byte-for-byte equality is not necessary when ordering/layout legitimately varies.

Include ordinary and long Unicode paths, diagnostics, empty/nonempty sequences, optional fields, malformed/fallback files, and zero-record files. On Unix, include non-UTF-8 native paths and native paths with ambiguous display representations. Preserve supported Windows compilation/tests; report honestly if Windows execution is unavailable.

Run relevant existing corruption and failure-injection tests. Review the adapter call path or use existing lightweight allocation instrumentation to establish that full owned rows/paths are no longer constructed per item. A large serialization profile is unnecessary.

## 7. C05 — convert unique owners in one bounded schedule

### Defect

Base collections and catalogs are converted before parallel segment jobs start. Segment deduplication uses the `PublishedSegment` wrapper identity rather than the underlying backing owner. Base/current catalogs are reused wholesale only when all their owners match, allowing partially shared native backing to be converted more than once.

### Unit of conversion

A conversion job represents one distinct native backing that can be independently serialized and validated. The unit may be a posting dictionary, metadata shard, path directory, ownership table, or combined archive root. A logical component or a segment wrapper is not automatically a distinct backing owner.

If multiple logical components share one inseparable archive root, convert that root once. Do not split it into duplicate archives merely to increase concurrency. Use typed owner identity that includes the backing family. An existing stable owner ID is suitable; an `Arc` allocation identity is also suitable while the source owner is retained for the complete operation. Do not persist process addresses or deduplicate unrelated owners by content equality.

### Required implementation

1. Capture a consistent source publication using the lifecycle protocol.
2. Enumerate relevant base, current-catalog, and segment backings. Register each distinct native owner once and retain existing archived owners for reuse.
3. Associate each registered owner with all consumers that need its converted result. Shared native shards in base and current catalogs must have one job and one resulting archived owner.
4. Schedule independent base, catalog, and segment jobs through one bounded execution mechanism. Remove the unconditional base/catalog serial prefix.
5. Have each job produce a validated immutable result. Workers must not mutate the published source or concurrently edit the shared candidate root.
6. Assemble candidate wrappers from the results, preserving file routes, tombstones, active masks, bindings, ordering, and logical revisions.
7. Perform required cross-component validation and publish atomically after checking that the source publication is still the intended one.

Use typed job/result variants or typed registries. Avoid a generic runtime-downcast framework. Keep the operation small enough to understand and reuse existing storage abstractions where possible.

Use the existing worker pool only if its lifecycle and reentrancy rules make that safe; otherwise use one bounded pool for the operation. Do not create nested pools or place all serializers behind one lock. Limit active conversions to the requested worker count and available work. Use a direct sequential path for one job or one worker.

Retain inexpensive batching for small jobs where helpful, provided each owner is still converted once and independent catalog/base work participates in the schedule. Do not introduce a complex cost-estimation framework. A true data dependency may require ordered preparation, but shared immutable inputs alone do not require serial conversion.

Any conversion or validation failure must leave the current publication unchanged. Release unpublished results, preserve old readers, and permit a retry. Retain correct cancellation/quiescence interaction. Keep owner registries operation-local; published result owners provide the subsequent lifetime.

### Tests

Use small fixtures and test-only observation hooks where needed. Establish:

- One collection with several independent owners uses multiple jobs and never exceeds its requested worker limit.
- Independent base/catalog owners can execute through the same bounded schedule as segment-owned backings.
- Two different wrappers sharing one native backing trigger one conversion and share the resulting archived owner.
- A catalog modified before its first archive shares unchanged native shards with the retained base catalog. Shared shards are converted once; changed shards receive their own conversion.
- One-worker execution, more workers than jobs, mixed state, and already archived no-op behavior remain correct.
- Failure in one active worker cannot partially publish other workers' results; retry succeeds.
- Deletes, moves, replacements, pinned views, and rearchiving preserve logical results.

Observe actual conversion calls or backing identities in these tests. Production owner-count reporting is outside scope and is not a reliable substitute for these assertions.

## 8. Implementation sequence and commits

Complete the work in this order:

| Step | Change | Verification before proceeding |
| --- | --- | --- |
| 1 | Establish the master checkout and initial revision; read relevant code and repository instructions. | Task-owned edits can be made without disturbing unrelated changes. |
| 2 | Implement C01 drain completion ownership. | Deterministic close/resume tests and affected worker-drain tests. |
| 3 | Implement C02 borrowed metadata evaluation. | Native/archived predicate equivalence and candidate allocation-path inspection. |
| 4 | Implement C03 shared archive input ownership. | Pointer/lifetime, populated-cache, and transactional-failure checks. |
| 5 | Implement C04 borrowed row/path serializers. | Checked round trips, platform path cases, and affected failure tests. |
| 6 | Implement C05 unique-owner scheduling. | Deduplication, bounded concurrency, mixed state, and failure/retry tests. |
| 7 | Validate the combined code and run the reduced performance check. | Section 9 completed. |
| 8 | Commit/push all completed changes to master and prepare the concise completion note. | Final remote master commit verified. |

Commit coherent changes with their relevant tests. Do not push a knowingly broken intermediate state. Fix regressions introduced by this task before proceeding to delivery. Routine decisions within this specification are authorized; do not stop after writing an implementation plan or request permission to perform the listed fixes.

Use a single writer for master. Parallel analysis or isolated test execution may be useful, but do not introduce additional branch-management work merely to satisfy an agent-count target. If work is delegated under repository instructions, the responsible implementer still owns integration and delivery on master.

## 9. Verification with a limited benchmark budget

### 9.1 Essential correctness checks

Run focused tests when changing the associated code. At completion, run the core library suite once and compile affected workspace consumers. Include relevant lifecycle/compaction integration tests not covered by that suite. Use the repository's current package names and required features.

Prefer existing tests and fixtures. Do not duplicate broad suites when focused additions cover the new risks. Apply formatting to changed files and run applicable required checks. A passing check on unchanged code does not need to be repeated after a documentation-only change. If a code change or merge invalidates a check, rerun the affected portion; broaden only for a concrete unresolved risk or an applicable repository requirement.

Never weaken archive validation, skip a reproducible failure, or increase test timeouts merely to hide a race. Keep correctness checks separate from benchmark repetition.

### 9.2 Reduced performance check

Use the existing benchmark executable and one representative dataset, normally Musenalm. Use the normal alphabet and worker configuration; keep the normal 32-worker setting if that is the established configuration. Do not sweep 1/8/32 workers or both alphabets.

The required performance workload is limited to:

1. **One native build timing and one explicit archive timing.** Capture them separately in the ordinary build-then-archive lifecycle. Do not include compilation in these times.
2. **Q001 and Q119, plus one metadata ID/path query exercising the changed evaluator.** Compare native and archived results and ordering. If those query identifiers changed, use equivalent existing queries. Run each once per representation; do not run the full benchmark query suite.
3. **First-execution archive-query timing.** Use a fresh archived query state when needed to avoid measuring a warmed compatibility/result cache. Setup is outside the query timing. Do not expand this into repeated cold/warm series.

Reuse valid results produced during this task for the same code revision and configuration. Use an existing compatible baseline only when dataset, configuration, build profile, and machine match. Do not rebuild historical revisions solely to create a benchmark baseline. If none is available, report absolute times and make no measured speedup claim.

Repeat only the affected operation when there is a suspected material regression, using at most three samples to distinguish noise from a repeatable problem. Small differences from a single sample are inconclusive. Correct actual regressions introduced by the repairs; do not launch a general optimization campaign.

No complete alphabet/worker matrix, repeated full query suite, large allocation profiles, cross-platform performance campaign, or new benchmark harness is required. Focused structural tests from C02–C05 establish that the specified unnecessary allocations, copies, duplicate conversions, and serial prefix were removed.

If the standard benchmark dataset is unavailable, use an available representative fixture for the same native/archived checks and identify that limitation. Do not spend the task downloading or reconstructing a large dataset. Correctness and structural acceptance remain mandatory.

## 10. Completion and delivery

All five repairs must satisfy their implementation requirements and essential tests. Deliver the code on `master`, including any conflict resolution needed to preserve concurrent master changes. Confirm the final master commit was pushed successfully to the configured remote.

The completion note should be short and contain:

- Starting and final master commits.
- One sentence per C01–C05 describing the implemented change and its validating test.
- Correctness/compilation checks actually run and their outcome.
- The reduced benchmark timings and result-equivalence outcome, with configuration and any limitation.
- Any remaining real blocker or defect, clearly distinguished from completed work.

Use the repository's normal task-document completion procedure, adapted to this assignment's explicitly authorized master workflow. Do not create extensive benchmark reports or update unrelated reporting fields.

The successful outcome is working, tested repairs available on master. If a genuine external blocker prevents that outcome, preserve the work and state the exact blocker; do not describe an incomplete task as finished.

## Implementation report

- Task: `master-archiving-repair`; completed 2026-11-17.
- Starting master: `15f45465ed0b7455577fce5431de50dfeea1ee16`.
- Integration branch/worktree: `master` in `/home/simon/source/objekte`, as explicitly authorized by the task.
- Implementation commits: `1a1961386f` (C01), `ae328044a2` (C02), `1085fcb9cf` (C03), `3366557f19` (C04), `cfea87527c` (C05), and `f14e5687b1` (consumer/test follow-up).

### Implemented repairs

- C01: Quiescence now binds every caller to one `Arc`-owned drain completion selected under the coordinator lock; deterministic tests delay drain-A waiters across resume and drain B, inject an A error, and verify every waiter receives its selected result.
- C02: Ordinary metadata predicates now use borrowed IDs, paths, scalar metadata, and constant collection/root operands in the existing evaluator; native/archive metadata tests and a zero-result archived scan verify equivalent answers without catalog/file/record compatibility caches or native compatibility copies.
- C03: Archive preparation shares immutable generations, ID maps, classification bitmaps, text/sort owners, file-table allocation, and populated caches; pointer-identity, pinned-view release, transactional failure, and populated-cache tests verify sharing and safe retirement.
- C04: Catalog rows and catalog/segment paths serialize through borrowed adapters into the existing rkyv layouts; checked round trips cover ordinary, long Unicode, escaped, non-UTF-8, malformed/fallback, diagnostic, and zero-record cases against the owned-adapter oracle.
- C05: Collection, catalog-shard, and underlying segment-owner conversions are registered once in one operation-local, bounded schedule; tests verify distinct wrappers sharing one segment index create one job, base/catalog jobs overlap, partially shared catalogs reuse archived shards, failure remains atomic, and 1/2/64 worker limits are respected.

### Correctness and compilation

- `cargo test -p objekte-search-core`: passed 610 library tests, 30 integration tests, and 1 doc test; 17 tests ignored; 0 failures.
- `cargo check --workspace --all-targets`: passed for core, CLI, and desktop.
- Focused archive subset (`cargo test -p objekte-search-core archive`): 25 passed, including mixed lifecycle, pinned views, no-op rearchive, fault injection/retry, shared catalog shards, and shared segment owners.

### Reduced Musenalm benchmark

Configuration: Linux x86_64, Intel i9-13900K, ASCII, 32 query workers, 32 archive workers, verified cache disabled, one sample per query and representation. The standard fixture was `/home/simon/source/objekte/examples/musenalm`; compilation was outside the reported timings.

- Native build measured by the native run: 887.306 ms. The build-then-archive run's explicit archive took 98.370 ms (preparation 30.154 ms, peak 32 conversion jobs, 149,111,376 archive bytes).
- Native/archived first-query times (Q001, Q034, Q119, Q149): 6.647/39.688 ms, 0.043/0.045 ms, 5.384/6.184 ms, and 0.482/0.559 ms.
- Native and archived result digests matched exactly: `71f7ff66bd31e3a6b4b7222fa2732c4d5a9ba5ad491d431d929f59fe3b346951`.
- A targeted two-sample archived Q001 retime distinguished cold initialization from a repeatable slowdown: 37.645 ms first execution and 7.025 ms second execution. This is a cold archived-query characteristic, not a correctness failure; no measured speedup claim is made from these single-sample comparisons.

### Remaining limitations

No blocker or unresolved in-scope defect remains. The first archived Q001 execution remains materially slower than subsequent execution and should be treated as a residual cold-start performance observation rather than a verified repair regression, because no compatible pre-change baseline was rebuilt.
