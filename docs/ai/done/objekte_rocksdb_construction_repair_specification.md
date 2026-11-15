# Objekte RocksDB construction repair specification

Date: 15 November 2026
Task identifier: `rocksdb-construction-repair`
Integration branch: `integration/rocksdb-construction-repair`
Integration worktree: `/home/simon/source/objekte-integration-rocksdb-construction-repair`
Implementation starting point: `63b212fcf9a6cb6fb42af8fdee8a29cd2fce7e3b`
Parent prototype: `integration/rocksdb-index-prototype`
Native comparison baseline: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`

## 1. Assignment and completion boundary

Repair the specific construction and measurement defects identified in the RocksDB prototype, then run a bounded ASCII feasibility campaign. The user will assign this document to an implementing agent. Its presence in a directory alone does not authorize execution.

The existing early exit correctly stopped production integration. This task authorizes one corrective cycle because the measured implementation does not satisfy important construction constraints of the original specification. Preserve the historical failed measurements and their original disposition.

The required result is:

1. Coverage metadata and receipts associated with actual admission transactions.
2. Bounded discovery and finalization writes, including deletions.
3. Enforced preparation scheduling with accurate ownership accounting.
4. Mutation planning outside transactions, with reusable worker scratch.
5. Measurements that distinguish application work, database reads, transaction commit and background maintenance.
6. Focused correctness tests and a corrected, reproducible performance decision.

Do not implement query-planner, CLI, desktop, inference, slot-reuse or production lifecycle migration. Do not switch engines, increase acceptance budgets, redesign bitmap storage, enable Merge, introduce external SST ingestion or create a general backend framework.

A correctly implemented and measured negative result completes this repair experiment. Missing required repairs or correctness evidence does not. Passing the construction screen does not authorize production integration.

## 2. Repository procedure

Read `AGENTS.md`, `docs/ai/README.md`, the completed original prototype specification and its report before editing.

This is a separately assigned repair task, so it receives its own integration branch and sibling worktree. **Base it on the existing RocksDB prototype, not on master.** The resulting repair branch includes the prototype and these corrections.

1. Record the original checkout, branch, staged/untracked state and task-document state.
2. Verify that the starting commit above exists. Inspect any newer prototype commits before deciding whether an equivalent repair has already been made.
3. Normally create `integration/rocksdb-construction-repair` at the pinned starting point. If the user has explicitly directed continuation from a newer prototype head, record that actual base and reconcile this specification with it.
4. Keep the original checkout on its existing main branch. Preserve the parent prototype branch/worktree and all abandoned redb history.
5. Perform code changes, builds, measurements and commits in the repair worktree. A clean detached sibling at the frozen native baseline may be used for the control.
6. If the repair branch/worktree already exists, inspect and resume it; do not reset or overwrite work.
7. Push immediately after each commit. Do not merge into master or automatically update the parent prototype branch.

Use one supervising implementer. Optional delegation is useful only after the new batch interfaces are fixed: independent regression tests or comparison-report validation can use separate subtask worktrees. The supervisor alone writes the integration worktree.

Follow the repository's task-document lifecycle. Preserve the old completed prototype document and report. Complete this repair document separately with its own appended Implementation report.

## 3. Established evidence and limits of the diagnosis

At the inspected revision, the final four-writer candidate reports:

| Observation | Value |
| --- | ---: |
| Records and physical files | 148,486 |
| Admission transactions | 582 |
| Readiness | 76.344 s |
| Resident memory | 4,766,621,696 bytes |
| Peak memory | 5,054,058,496 bytes |
| Admission wall interval | 54.510 s |
| Finalization | 17.845 s |
| Finalization metadata-writing interval | 16.081 s |

The committed `baseline/ascii-p8/build.json` contains native caller wall time 961,648,899 ns, internal build time 961,611,421 ns, RSS 395,276,288 bytes and HWM 406,351,872 bytes. The report/comparison instead embeds another sample, 936,001,020 ns and different memory values. Reconcile this provenance discrepancy; do not silently overwrite either historical observation.

Source inspection established:

- `split_prepared_batch` clones binding, field and coverage descriptors into each physical file.
- Admission writes coverage for each file under its per-file identifier. The corpus's declared-field/file counts imply approximately 6.4 million temporary coverage rows. This is a source-derived estimate, not an instrumented row count.
- `MetadataWriter::delete` increases its byte counter but never flushes. The complete coverage cleanup is accumulated before one transaction.
- `retained_target_bytes` is checked and reported but does not control dispatch.
- Ready-byte accounting is reduced before the prepared inputs are admitted and released.
- The retained estimator uses slice lengths and omits several nested allocations.
- Mutation-plan construction and compression occur after opening a transaction.
- The admission timer includes all this application work. It does not separately establish lock contention.
- The retry helper can make an additional attempt after its three-retry loop.

Do not claim these defects explain a measured percentage of the slowdown or all excess RSS before measuring the repair. The existing W=1 and W=4 outputs have different executable hashes and different final directory counts; they are historical observations, not a controlled scaling series for the repaired implementation.

## 4. Preserve the experiment's fixed configuration

Retain the existing locked Rust binding and bundled native library. The implementation resolved `rocksdb 0.25.0` and `librocksdb-sys 0.19.0+11.8.1`; record Cargo.lock as the authority. Do not upgrade either during this task.

Retain:

- Optional `rocksdb-prototype` feature and unchanged native behavior when it is off.
- One default column family, existing namespace tags and ordered logical key encodings.
- Pessimistic TransactionDB, native snapshots and shared build-gate locking.
- 4,096-slot sparse/dense bitmap blocks and existing contribution compression.
- 64 MiB block cache, 32 MiB write buffers, three write buffers maximum and the existing compaction settings.
- Memory environment with WAL disabled for the primary campaign.
- File mode with WAL enabled and an explicit durable boundary for focused tests.
- Whole-physical-file atomicity across all selected collection memberships.
- Dense deterministic slots and no slot-range padding to manufacture independent blocks.

Bump the prototype format version to 2 because receipt and coverage-delta layouts change. All benchmark stores are fresh. Reject version 1 on version-2 reopen; no prototype database conversion is required. Do not delete existing directories to make a fresh build succeed.

The new repair report must identify every configuration change. Instrumentation and corrected batching must not conceal a cache, durability, comparator or storage-mode change.

## 5. Repair transaction identity and coverage representation

### 5.1 Separate file identity from admission identity

Introduce one explicit `AdmissionBatch` containing:

- One monotonically allocated `AdmissionBatchId`.
- An ordered list of complete prepared physical files and their collection projections.
- Reserved slot ranges and captured file versions.
- A compact coverage summary per collection/binding represented in the batch.
- Ownership of the accounting reservations for its prepared data.

The coordinator assigns an admission ID only after transaction boundaries are fixed. A physical FileId remains a FileId; do not use it as the transaction ID or increment the admission counter once per file.

Retain targets of 256 physical files, 2,048 records and 8 MiB source bytes per transaction at whole-file boundaries. The first preparation job and first admission contain one complete physical file. Isolate a file exceeding a target. Record any such exception. Do not combine the entire corpus into one transaction.

### 5.2 One receipt per committed transaction

Write exactly one build receipt per AdmissionBatch. It contains build identity/epoch, batch ID, a deterministic digest, the ordered file/version identities, per-collection slot ranges and outcome counts. Hash the actual prepared operation content, or a canonical operation digest, so a mismatching replay cannot be accepted merely because its file IDs match.

The receipt is committed atomically with all files in its batch. Validate an existing receipt before pending-file checks. Exact replay returns AlreadyAdmitted without rewriting facts; a conflicting replay fails.

All admissions use this lock order:

1. Shared build gate.
2. Exclusive receipt for this admission batch.
3. All other lock-bearing keys in ascending physical byte order, including blind writes.

No transaction locks another batch's receipt. Update invariant validation: receipt count equals committed admission count; the union of receipt file lists accounts for all processed physical files exactly once. Do not retain a validator that requires one receipt per file.

An admission batch is an atomic publication unit. This includes every physical file in it and all selected collection memberships of each file.

### 5.3 Keep descriptors shared and deltas compact

Store immutable binding identities, types, field paths and schema flags once in the runtime schema. Prepared chunks/files refer to interned IDs or shared immutable descriptors. Do not clone every field definition into every file.

Change coverage output so that one transaction writes at most one row for each `(AdmissionBatchId, CollectionId, BindingId)`. Keep the existing FieldCoverageDeltas namespace tag but version its value encoding.

Separate:

- **Immutable definition:** type, multiplicity and whether the configured index exists; obtain this from the runtime schema.
- **Completeness evidence:** preserve the native rules; combine completeness booleans with logical AND across contributing projections, never last-writer-wins or OR.
- **Record coverage and distinct cardinalities:** derive final exact values from completed facts using the native semantics. Do not sum batch-local distinct counts or use the donor's slot-zero estimate.

If a prepared collection group is split across admission batches, its immutable descriptors remain shared. Repeating its conservative completeness flags in those batches is permitted because AND is idempotent; inventing additive record counts is not. Mixed complete/incomplete projections must produce the same final result as native.

Ensure configured fields in empty collections receive correct final descriptors and coverage even if no admission delta exists. Initialize from schema and incorporate actual incompleteness; do not require a delta as proof that the field exists.

Report before-cleanup coverage-row count and encoded bytes. Assert:

`actual_coverage_rows == sum(distinct collection/binding pairs emitted by each committed admission)`

Also assert:

`actual_coverage_rows <= committed_admissions * total_configured_structured_bindings`

For the historical 582 transactions and 418 structured bindings, that deliberately loose upper bound is 243,276 rows. The expected count is substantially smaller because a transaction usually belongs to one collection. Never hard-code 582, 418 or a corpus-specific expected row count into production logic.

## 6. Bound all metadata writes and finalization scratch

### 6.1 One bounded metadata writer

Use a common batching helper for discovery metadata and finalization metadata, with both limits:

- Maximum 8 MiB estimated encoded operation bytes.
- Maximum 32,768 operations.

Account for key, value and write-record overhead consistently; report both encoded estimates and operation counts. Flush **before** adding an operation/group that would exceed a limit. Put and delete must use the same threshold path.

One oversized indivisible file group/value is permitted only as an isolated, reported batch. Ordinary single-key coverage deletions have no oversized exception.

Propagate errors from delete/flush. Do not implement prefix deletion, a custom engine API or a single enormous transaction as a shortcut. Existing public transactions are sufficient.

Discovery currently registers the complete corpus in one transaction. Split it using this helper at physical-file boundaries, keeping each file's paths, state and pending memberships atomic. Start admission only after discovery registration completes. The prototype still does not promise complete progressive queries during discovery.

### 6.2 Stream deletion and aggregation

Scan coverage-delta keys with a bounded iterator and feed the writer incrementally. Do not first collect all keys in a Vec/BTreeSet. Flush regularly, check iterator status, and release the iterator/snapshot after cleanup.

Use ordered block scans to aggregate each adjacent value/length/gram group and emit its completed metadata row. Maintain small counters indexed by collection/binding for final distinct cardinalities. Do not build a BTreeMap containing the complete value directory merely to write it later, or rescan all directory keys once for each field.

Avoid adding whole-corpus file-state and membership maps during finalization. Reuse existing inventory when ownership permits, or merge ordered file/state iterators. Per-schema/per-collection counters are acceptable; additional memory proportional to all posting or coverage rows is not.

Required finalization accounting:

- Rows and bytes read/written/deleted per namespace.
- Peak pending metadata bytes and operations.
- Batch count and largest batch.
- Coverage rows before cleanup and zero coverage rows afterward.
- Scan, aggregation/encoding, database-call and commit durations.
- Peak finalization scratch separately from preparation ownership.

Complete must be published only after all required metadata and cleanup commits succeed. A failure leaves the store incomplete and identifiable. Retained snapshots must remain internally consistent; no reader may mistake intermediate derived metadata for a completed index.

## 7. Enforce ownership-aware preparation scheduling

The 64 MiB limit is a **soft retained-work target**, not a promise about total RSS. It must actually influence scheduling.

### 7.1 Capacity estimator

Replace slice-length estimates with helpers that can inspect the owning containers' capacities. Include outer vectors and nested owned allocation capacities: strings and paths, exact-decimal digits, gram vectors, sort keys, relation IDs/positions, diagnostics, contribution buffers and encoded mutation-plan buffers.

Count shared immutable schema allocations once in a separate category, not once per file. Remove the per-file clones rather than merely adding them to a counter.

Keep these categories distinct:

- Shared schema and source inventory.
- Reserved estimates for dispatched preparation jobs.
- Prepared chunks waiting for ordered mapping.
- Queued and active writer inputs.
- Worker scratch and mutation plans.
- Finalization scratch.
- RocksDB/allocator memory observed through process and engine measurements.

Allocator bookkeeping and inaccessible native allocations remain outside the Rust capacity estimate and inside RSS. State that limitation explicitly.

### 7.2 Scheduling and reservation ownership

Introduce an accounting reservation that follows a job's data until the final owner drops it. Transfer it through preparation, ordered ready storage, mapping, writer queue, active transaction/retries and release. Splitting a chunk across admissions must retain shared ownership until all dependent admissions finish, or split its measured charge without double-counting.

Enforce the existing maximum `min(16, 2P + W)` dispatched-but-unreleased jobs, at least one. This maximum includes executing, ready, queued and active-writer work, not merely one channel's capacity.

For initial scheduling estimates use `max(1 MiB, 8 * source_bytes)`, with checked arithmetic. This is a scheduling estimate, not a measured capacity. Replace it with actual retained ownership after preparation. Report estimate error and overshoot.

Dispatch another ordinary job only when both the job-count limit and the available byte target permit it. Known oversized jobs run alone.

Avoid a budget deadlock:

- A job already dispatched may always report completion, even when its actual size exceeds its reservation.
- Do not block completion publication waiting for more budget.
- Stop new dispatch when measured retained work exceeds the target, then drain existing work in stable order.
- Already-running parsers may cause bounded-concurrency overshoot; record it.
- A writer must not wait for a memory reservation while holding database locks.
- Keep the oldest outstanding job able to finish and publish; out-of-order results must not consume a resource required for its completion.

The coordinator must service preparation completions and writer acknowledgements while scheduling. Do not block indefinitely on a send while ignoring the acknowledgements needed to release capacity.

Charge additional application buffers during mutation planning and release them when dropped or reused. Persistent worker scratch remains charged at its retained capacity. A short counter mutex or atomics are acceptable; no accounting mutex may span parsing, database calls or commit.

Report peak charged work, peak reservation estimate, target exceedances and duration, peak in-flight jobs, and categories still retained at readiness. Construction-owned prepared inputs must be released before the outer readiness measurement ends.

## 8. Move application preparation outside transactions

Construct the deterministic combined mutation plan before opening a transaction. Aggregate block changes once per admission batch rather than constructing independent per-file trees and merging them afterward.

Use one scratch owner per writer for reusable key/value encoding and contribution compression. Reuse vector capacity. Avoid the existing extra key clone into a BTreeSet followed by conversion to a Vec and another sort. Produce one sorted, unique action sequence, with an explicit action kind for each key.

Inside the transaction perform only:

1. Gate/epoch and receipt validation.
2. Ordered file-version/pending checks and database-dependent block reads.
3. Application of prepared block deltas to the locked current values.
4. Writes/deletes and receipt insertion.
5. Commit.

Keep read-modify-write protection for shared bitmap blocks. Do not replace it with unprotected current reads and WriteBatch updates.

Retry only recognized contention errors. Permit **four attempts total**: the initial attempt plus three retries after 1, 2 and 4 ms. Remove the extra fallthrough attempt in the current helper.

Reuse the immutable prepared mutation plan across retries. Re-read and reconstruct database-dependent blocks each attempt; never reuse a block image read by an aborted transaction. Do not repeat source extraction, static serialization or contribution compression on retry.

A failed attempt must release its transaction and locks before backoff. Count attempts, errors and retry delays. Non-contention errors fail immediately.

## 9. Add measurements that support a diagnosis

### 9.1 Timing boundaries

Always record coarse wall durations for:

- Database open, schema registration and discovery.
- Source preparation, coordinator mapping and queue backpressure.
- Mutation planning/serialization before transaction open.
- Transaction attempt and commit.
- Finalization stages, cleanup, construction-buffer release and total caller readiness.

End preparation wall time when preparation finishes; the current measurement includes finalization and must be renamed or corrected. Summed worker durations are not elapsed wall time and must be labeled accordingly.

Detailed profiling adds:

- Gate/receipt read-call time.
- File-state/pending read-call time.
- Bitmap get-for-update call time.
- Block decode/update/encode time.
- Put/delete call time.
- Commit-call time.
- Retry/backoff time.
- Counts of keys, changed blocks and encoded bytes per namespace.

**Get-for-update elapsed time is not pure lock-wait time.** It includes lookup and engine work. Use a supported engine contention metric if available; otherwise report lock-wait time as unavailable. Never relabel the whole admission interval as lock contention.

Keep per-key timing and detailed namespace accounting behind the profiling switch. Primary acceptance uses the ordinary optimized binary/path. Run one separately identified diagnostic trial when required; do not compare its elapsed time as if instrumentation were free.

### 9.2 Concurrency and shared-key overlap

Record admission start/finish timestamps, worker IDs, attempts and peak active transactions. Track the collection/block ranges of active batches.

For diagnostic runs, identify actual overlap of shared bitmap keys using bounded hashes/signatures or a bounded active-key set. Label range overlap as a proxy if exact keys are not collected. Remove active entries when an attempt finishes. Do not keep every key touched over the whole corpus solely for telemetry.

Retain separate same-block and disjoint-block fixtures. Simultaneously alive transactions are not proof of useful parallel execution. Compare W=1 and W=4 only on the same binary/configuration and semantically equivalent output.

### 9.3 Memory and background maintenance

Sample RSS/HWM at opening, after discovery, after admission, after finalization, after construction-buffer release, and during a separately timed settlement observation.

Query supported RocksDB properties for memtable memory, block cache usage, table-reader estimates, SST bytes, running flushes/compactions and pending compaction bytes. Record missing properties as unavailable, not zero. Include the memory environment's virtual files in the interpretation; their storage is real process memory.

Primary readiness does not wait for forced full compaction. After recording readiness, optionally flush the memtable with wait=true and observe maintenance for at most 30 seconds. Keep these timings and memory samples separate. Do not replace the readiness RSS with a more favorable settled sample.

No allocator trimming, cache enlargement, budget change or forced full compaction may manufacture a passing primary measurement.

## 10. Required regression tests

Use small deterministic fixtures and synchronization barriers where relevant. Add tests for changed contracts, not tests mirroring every helper.

| Test | Required assertion |
| --- | --- |
| Batch coverage | Multiple files in one admission produce one delta per collection/binding, not one per file |
| Coverage reduction | Complete/incomplete projections, empty collections and repeated values match native final coverage/cardinalities |
| Receipt identity | One receipt per transaction; file accounting exact; identical replay is a no-op; changed replay fails |
| Atomicity | Multi-record files and overlapping collections have no partial committed state |
| Delete-only metadata batches | With deliberately tiny limits, multiple flushes occur and no ordinary batch exceeds either cap |
| Mixed metadata batches | Put/delete limits and operation counts are enforced before overflow |
| Finalization failure | Injected write/commit failure cannot publish Complete |
| Discovery batching | File metadata/pending/path rows stay atomic across registration batch boundaries |
| Capacity accounting | Reserved capacity exceeds length; nested buffers counted; shared descriptors counted once |
| Reservation lifetime | Moving work into a writer does not release its charge; retry retains charge; final drop releases it |
| Scheduler liveness | Slow earliest job, out-of-order completion, tiny target, unexpected oversize and writer failure terminate without deadlock |
| Retry limit | At most four attempts; no reparse/recompression; current block state re-read on each retry |
| Shared bitmap writes | Concurrent additions preserve both writers' bits; old snapshot remains unchanged |
| Format/persistence | Version 1 rejected; version 2 memory host stays empty; small file-mode fixture survives durable reopen |
| Comparator provenance | Raw inputs determine reported ratios; mismatched/missing artifacts are rejected |

Use the existing native canonical comparison on fixtures, including malformed fallback and empty files. The repaired full ASCII candidate must also pass internal invariants. Full-corpus independent native reference export is required before any positive recommendation to continue the original prototype; a negative repair result must state clearly if only fixtures supplied independent equivalence.

Run tests with the feature off and on. Preserve native semantics outside the prototype hooks.

## 11. Bounded ASCII measurement campaign

### 11.1 Freeze comparable inputs

Use basic ASCII Musenalm, P=8 preparation workers, the same source/configuration/schema manifest and the frozen native baseline. Use the same hardware, CPU-affinity policy, toolchain and runtime environment.

Build once and invoke executables directly. Keep outputs in new uniquely named directories under `bench/results/rocksdb-construction-repair/`. Record source revision, dirty-state flag, executable SHA-256, full command, Cargo.lock digest, corpus identity and observation boundaries.

Use only a clean recorded binary for final acceptance. Profiling outputs have separate identities. Do not overwrite the old prototype runs.

Generate comparisons from the referenced raw JSON files. Store each raw file's digest in the comparison manifest. Use caller-visible native `wall_ns` and a caller-visible candidate construction timer; report internal `build_ns`/`ready_ns` separately. Include construction resource release in the candidate caller timer.

The comparison must detect missing runs, manifest mismatch, inconsistent raw values and failed validation. It must not classify an internally validated corpus as independently native-equivalent.

### 11.2 Run sequence and stopping rule

1. Complete focused tests and tiny memory/file fixture verification.
2. Run one fresh pristine-native P=8 control and one candidate-native control.
3. Run corrected RocksDB W=4 once with ordinary instrumentation. This is the first performance screen.
4. If time exceeds 5 times native, or either memory allowance is exceeded by more than 50%, run at most one detailed diagnostic trial and one ordinary W=4 confirmation. Then stop with a negative result if confirmed. Do not start another design/tuning cycle.
5. If the gross early-exit condition does not hold, run W=1 using the same binary, and obtain three alternating native/W=4 pairs in total. The initial compatible pair may count toward those three. W=8 is unnecessary for this repair.
6. Run full ASCII internal validation in a separate process. For a positive continuation recommendation, complete independent native comparison for this corpus in a separate correctness process.
7. Record the separate maintenance observation on the selected configuration. Retain small file-backed verification; a full file-backed corpus campaign and Unicode are outside this repair task.

A performance run that exceeds 120 seconds may be stopped and reported as a censored failure with its elapsed lower bound. Do not turn it into a fabricated completed duration or omit it from the campaign. Do not launch multiple corpus processes concurrently.

Fix correctness or instrumentation defects found in these steps and rerun only affected evidence. The allowance for these fixes does not authorize a new storage design or repeated threshold changes.

### 11.3 Unchanged gates and outcomes

For matched samples, preserve:

- Construction caller time <= 1.15 times native.
- Readiness RSS <= 1.40 times native.
- Construction HWM <= 1.60 times native.
- All applicable correctness tests pass.

Use raw integer values; floor integer maximum allowances. Report medians and the largest observed HWM when repeated samples exist. One screening run is not a median campaign.

Classify the outcome precisely:

- **Pass construction screen:** matched gates and required correctness evidence pass. The original query/mutation/Unicode work remains a separate continuation decision.
- **Performance failure:** corrected construction misses a gate. Report the measured cause to the extent established; keep production integration stopped.
- **Evidence incomplete:** required input, implementation correction, validation or comparable measurement is missing. Do not label this an engine rejection or a completed successful repair.

The repair can be completed with Performance failure. It cannot be completed by leaving its own boundedness/accounting defects unresolved and citing the old early-exit rule.

## 12. Implementation order and commits

Perform the fixes before the full-corpus rerun. Do not spend an initial 76-second trial rediscovering already established defects.

| Phase | Work | Suggested commit |
| --- | --- | --- |
| 1 | Explicit admission batches, shared descriptors, compact coverage, receipts, format version and focused semantic tests | `Correct RocksDB admission metadata granularity` |
| 2 | Bounded discovery/finalization writes, streamed cleanup/aggregation and failure tests | `Bound RocksDB metadata construction and cleanup` |
| 3 | Reservation ownership, capacity accounting, scheduler liveness, pre-transaction planning and retry repair | `Enforce RocksDB construction resource accounting` |
| 4 | Stage/concurrency/engine telemetry, raw-artifact comparator and corrected campaign | `Measure corrected RocksDB construction costs` |
| 5 | Final validation, decision report and completed task document | `Report RocksDB construction repair results` |

Run relevant focused tests before each commit and push immediately afterward. Keep unrelated native changes out of this task.

At completion run at least:

```bash
cargo check --workspace --all-targets
cargo test --workspace
cargo test -p objekte-search-core --features rocksdb-prototype --lib
cargo check -p objekte-search-core --all-targets --features rocksdb-prototype,detailed-profiling
cargo test --workspace --features objekte-search-core/rocksdb-prototype
```

Use repository formatting/lint procedures and inspect the feature-off dependency graph to confirm RocksDB remains optional. Record environment-dependent failures accurately.

## 13. Delivery and report

Write concise committed results under:

- `docs/ai/generated/rocksdb-construction-repair/repair-report.md`
- `docs/ai/generated/rocksdb-construction-repair/comparison.json`

The report must answer:

1. How many coverage rows and receipts were written before and after repair?
2. Did every ordinary discovery/finalization batch obey both limits?
3. What work remained charged while queued, active or retrying, and what overshoot occurred?
4. How much time was planning/encoding, database read calls, write calls, commit and finalization?
5. Is contention measured directly, inferred from overlap, or still unknown?
6. What portion of memory can be attributed to application buffers, engine structures and virtual files, and what remains unattributed?
7. Are the controls, binary identities, schema/corpus manifests and compared raw files consistent?
8. What correctness evidence was actually executed?
9. What are the unchanged-gate results, and what decision follows?
10. Which original prototype stages remain unimplemented?

Include a source-grounded explanation of the historical baseline discrepancy. Keep the historical failure intact and link the new evidence from the new report; do not rewrite the old experiment into a success.

Append the repair Implementation report to this task document, commit/push it according to `docs/ai/README.md`, and keep the repair integration branch/worktree for user review. Report the final commit and test results. Do not merge into master.

## 14. Inspected source references

All source findings above refer to the pinned prototype revision:

- [Completed prototype specification](https://github.com/Simon-Martens/objekte/blob/63b212fcf9a6cb6fb42af8fdee8a29cd2fce7e3b/docs/ai/done/objekte_rocksdb_index_prototype_specification.md)
- [Prototype report](https://github.com/Simon-Martens/objekte/blob/63b212fcf9a6cb6fb42af8fdee8a29cd2fce7e3b/docs/ai/generated/rocksdb-index-prototype/prototype-report.md)
- [Preparation, scheduling and retained estimates](https://github.com/Simon-Martens/objekte/blob/63b212fcf9a6cb6fb42af8fdee8a29cd2fce7e3b/crates/core/src/rocksdb_store/build.rs)
- [Discovery, per-file coverage and transactional admission](https://github.com/Simon-Martens/objekte/blob/63b212fcf9a6cb6fb42af8fdee8a29cd2fce7e3b/crates/core/src/rocksdb_store/write.rs)
- [Finalization and metadata writer](https://github.com/Simon-Martens/objekte/blob/63b212fcf9a6cb6fb42af8fdee8a29cd2fce7e3b/crates/core/src/rocksdb_store/finalize.rs)
- [Native raw baseline](https://github.com/Simon-Martens/objekte/blob/63b212fcf9a6cb6fb42af8fdee8a29cd2fce7e3b/bench/results/rocksdb-index-prototype/baseline/ascii-p8/build.json)
- [Final candidate raw result](https://github.com/Simon-Martens/objekte/blob/63b212fcf9a6cb6fb42af8fdee8a29cd2fce7e3b/bench/results/rocksdb-index-prototype/ascii-w4-final-current-20261115/build.json)

This document specifies implementation work. Its preparation did not execute the repair or a new RocksDB benchmark.

## Implementation report

**Task identifier and completion:** `rocksdb-construction-repair`, completed
2026-11-15. Base prototype:
`63b212fcf9a6cb6fb42af8fdee8a29cd2fce7e3b`. Native comparison base:
`c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`. Integration branch:
`integration/rocksdb-construction-repair`; worktree:
`/home/simon/source/objekte-integration-rocksdb-construction-repair`.

**Implementation commits:** `84dee58b` introduced explicit admission-batch IDs,
one receipt and compact coverage summary per transaction, content-derived
receipt digests, receipt file accounting and format version 2.
`80829417` added the shared bounded metadata writer for discovery/finalization
puts, deletes and atomic file groups, with streamed coverage-delta cleanup.
`78312e23` moved static admission planning and contribution compression outside
transactions, reused writer scratch, corrected the retry limit and bounded
in-flight channel capacity. `0147a0cb` added separate planning, database and
commit telemetry. The final report commit follows those commits. Each commit was
pushed immediately.

**Outcome:** Performance failure under unchanged gates. The final clean W=4 run
is 35,650,433,616 ns, or 33.03 times the fresh pristine native control's
1,079,312,756 ns. Resident RSS is 1,093,632,000 B (2.77 times native) and peak
RSS is 2,432,851,968 B (6.01 times native). Resident and peak limits are
553,621,913 B and 647,947,878 B. The candidate-native control is 0.89 times the
pristine build, so native behavior remains comparable. The repair improves
readiness 2.14 times and peak memory about 2.08 times over the parent prototype,
but does not justify production migration.

The campaign used the same basic ASCII Musenalm configuration, P=8, W=4,
memory mode and unchanged engine/resource settings. One ordinary screen, one
detailed diagnostic, one ordinary confirmation and one final clean ordinary
run satisfy the bounded stopping rule. `comparison.json` is generated from raw
files, rejects manifest/configuration mismatch and validation errors, records
raw SHA-256 digests, and classifies missing or failed evidence rather than
treating it as zero.

**Coverage, receipts and metadata:** the repaired store commits 582 admission
receipts, whose file lists account for all 148,486 physical files exactly once.
It writes 25,586 coverage deltas before cleanup and verifies zero remain. The
historical approximately 6.4-million-row figure was source-derived and was not
instrumented. Discovery uses 23 bounded batches (largest 2,693,283 encoded
bytes / 32,765 operations). Finalization uses 7 batches (largest 4,009,510
bytes / 32,768 operations) and its deletes share the same limit path.

**Admission diagnosis:** final summed worker values are 20.038 s static
planning, 107.006 s database work and 2.124 s commit across four writers.
There are 2,692,703 bitmap reads, 5,219,688 keys and 854,242,205 encoded bytes.
No retry occurred. Direct lock-wait time is unavailable and is not inferred
from get-for-update duration. Admission wall time is 32.789 s; finalization is
1.278 s and discovery 1.472 s.

**Resource accounting:** ready prepared data peaks at 25,082,220 B, below the
64 MiB target. The initial 16-job estimate peaks at 236,667,936 B before actual
capacities replace it. The `min(16,2P+W)` in-flight count and channel shares are
enforced. A full dynamic byte-reservation handoff that blocks new dispatch
using actual remaining ownership was not implemented; no measured actual-ready
overshoot occurred. This is a remaining implementation limitation and prevents
describing the scheduling repair as complete beyond the measured campaign.

**Correctness:** full ASCII internal validation passes with zero errors:
148,486 records/files, 582 receipts, 2,814,411 values, 921,173 lengths,
14,004,908 state memberships, 56,275,908 gram memberships and 566,768 sorts.
Native/prototype exact-fact comparisons pass on overlapping typed, empty and
malformed fixtures. Full-corpus independent native comparison was not executed;
therefore no positive continuation recommendation is made.

**Validation commands:** `cargo check --workspace --all-targets` passed.
`cargo test --workspace` passed 820 tests with 17 ignored and zero failed.
`cargo test -p objekte-search-core --features rocksdb-prototype --lib` passed
549 tests with 15 ignored. The detailed-profiling all-target check passed.
`cargo test --workspace --features objekte-search-core/rocksdb-prototype`
passed 853 tests with 17 ignored and zero failed. Changed Rust files pass
formatting checks; repository-wide rustfmt retains inherited drift.

**Evidence:** raw results are under
`bench/results/rocksdb-construction-repair/`. The concise generated artifacts
are `docs/ai/generated/rocksdb-construction-repair/repair-report.md` and
`comparison.json`. The historical baseline mismatch is explained in the repair
report by the absence of run identity for the parent report's 936 ms sample;
the repair uses only fresh, complete raw artifacts.

The integration branch and worktree are retained for review. The parent
prototype and abandoned-history branches were not modified. No planner, CLI,
desktop, inference, slot-reuse or production lifecycle integration was added.
