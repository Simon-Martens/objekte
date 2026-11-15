# Objekte RocksDB admission repair specification — second repair

Date: 15 November 2026  
Task identifier: `rocksdb-admission-repair`  
Integration branch: `integration/rocksdb-admission-repair`  
Integration worktree: `/home/simon/source/objekte-integration-rocksdb-admission-repair`  
Required starting implementation: `08591e8452132f392201cfdb745d958f502a556b`  
Parent branch: `integration/rocksdb-construction-repair`  
Native control revision: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`

## 1. Assignment

Complete the missing resource-accounting work from the first repair, replace the allocation-heavy admission path, close the identified correctness gaps, and run a bounded diagnostic and performance campaign.

This is a second, specifically authorized corrective task when the user assigns it to an implementing agent. The prior report's instruction not to start another cycle does not override the user's assignment of this document.

Build on the first repair. Preserve its improvements: one receipt per transaction, compact batch coverage, bounded metadata writes, format checks and the corrected retry count. Do not restart from master or replay the original prototype work.

The result must distinguish three questions:

1. Is the repaired application implementation correct and bounded?
2. Where does the remaining construction cost occur?
3. Does the resulting prototype meet the original feasibility gates?

A negative answer to question 3 does not excuse unfinished requirements under question 1. An honest, fully measured performance failure can complete this task. An implementation that still omits required accounting or validation is incomplete.

Do not implement production query-planner/CLI/desktop integration, inference, slot reuse, crash resume, full lifecycle migration or memory-to-disk export. Keep the selected engine and logical posting representation. External SST ingestion, Merge operators, sharded databases, sparse slot allocation and a different transaction engine require a separate future decision.

## 2. Current evidence: what changed and what remains

The inspected parent report and source are at `08591e8452132f392201cfdb745d958f502a556b`. Its measured executable was built from `0147a0cb73`, with SHA-256 `a7da2b6e636b7b5a1fe2ca361a375060b696af841a7f1c5a6ea9ff886dfec66f`.

| Metric | Original prototype | First repair |
| --- | ---: | ---: |
| Construction readiness | 76.344 s | 35.650 s |
| Resident bytes | 4,766,621,696 | 1,093,632,000 |
| Peak bytes | 5,054,058,496 | 2,432,851,968 |
| Finalization | 17.845 s | 1.278 s |
| Committed admission transactions | 582 | 582 |
| Coverage deltas before cleanup | Approximately 6.4 million, inferred | 25,586, measured |

The first repair's matched native caller time was 1.079 s. Its time/RSS/HWM ratios were 33.03 / 2.77 / 6.01. These historical samples establish substantial improvement and continued failure; they do not establish the performance of this second repair.

First-repair admission measurements:

- Admission wall interval: 32.789 s.
- Summed worker planning: 20.038 s.
- Summed worker “database” interval: 107.006 s.
- Summed worker commit calls: 2.124 s.
- 5,219,688 processed keys, 2,692,703 bitmap reads and 854,242,205 encoded bytes.
- 582 attempts and no retries.

The 107.006-second interval includes application key cloning, BTreeSet construction, sorting, repeated BTreeMap searches, bitmap decoding/updating/encoding and database calls. It is not a measurement of RocksDB-only time or lock waiting. Zero retries does not imply zero waiting: successful lock acquisition can wait without timing out.

Discovery plus finalization already totals approximately 2.75 seconds, exceeding the approximately 1.24-second original time allowance even before admission. Report this residual cost honestly. Admission improvements alone cannot guarantee the original target.

### 2.1 Source-audited implementation status

| Requirement | Current state | Required action |
| --- | --- | --- |
| Transaction-level receipts and compact coverage | Implemented | Preserve and test |
| Bounded discovery/finalization put/delete batches | Implemented | Preserve; retain limit assertions |
| Static planning before transaction | Partial | Move remaining key ordering out; reuse plan across retries |
| Batch-wide block aggregation | Missing | Remove per-file MutationPlan and merging |
| Writer-lifetime scratch | Partial | Scratch now lasts one admission call, not the writer thread |
| Retained byte scheduling | Missing | Implement actual reservation/ownership control |
| Global in-flight job limit | Not established by channel sizing | Count dispatched-to-released job ownership |
| Capacity accounting | Still length-based and incomplete | Measure owning capacities and nested buffers |
| Shared per-file schema descriptors | Missing | Stop cloning fields/bindings/coverage into every file |
| Linear multi-record admission | Missing | Replace per-record scans of whole-file lists |
| Bounded finalization aggregation | Partial | Cleanup streams; full directory maps remain |
| Forward sort-index validation | Missing | Inspect actual SortEntries |
| Namespace defaults on general iterator | Missing | Always clamp to namespace |
| Fine admission/engine attribution | Missing | Add separately measured stages and CPU evidence |

The completed first-repair task document explicitly acknowledges that dynamic byte-reservation handoff was not implemented. A small ready-map observation does not establish bounded total retained work.

## 3. Branch, worktree and reporting discipline

Read `AGENTS.md`, `docs/ai/README.md`, both preceding specifications and their implementation reports.

Create the new task branch and sibling worktree from the pinned first-repair revision. If the user directs use of a newer first-repair head, inspect its diff, retain completed fixes and record the actual base. Do not overwrite an existing worktree or reset user changes.

Keep the original checkout on its existing main branch. Preserve the prototype, first-repair and abandoned-redb branches. Do not automatically merge into master or modify the parent repair branch.

Use one supervisor initially. Optional parallel work may begin after interfaces are fixed: assign forward-index/iterator tests or measurement/report validation to separate subtask branches/worktrees. Only the supervisor edits the integration worktree.

Commit explicit task-owned paths and push after each commit. Follow the repository's authoritative task-document rules. Keep historical measurements unchanged. New raw results belong under `bench/results/rocksdb-admission-repair/`; generated reports belong under `docs/ai/generated/rocksdb-admission-repair/`.

## 4. Fixed design and resource policy

Retain the locked `rocksdb 0.25.0` / `librocksdb-sys 0.19.0+11.8.1` dependency, bundled native build, optional feature and existing engine options. Do not change cache sizes, write buffers, background jobs, compression, durability or allocator policy.

Retain one column family, namespace tags, ordered key encoding, 4,096-slot bitmap blocks, native snapshots, whole-file atomicity and pessimistic read-modify-write protection. Keep batch targets of 256 files, 2,048 records and 8 MiB source bytes; an oversized physical file remains indivisible and reported.

The changes below preserve posting and contribution formats. Because the admission digest algorithm is revised, set the prototype header format version to 3 and use an explicitly versioned digest domain. Build fresh databases; reject old prototype versions on version-3 reopen. No conversion tool is required.

Do not introduce an application mutex around complete transactions. Do not remove version checks or snapshot guarantees to improve a result. Reuse source fingerprints already captured during preparation; add no new whole-corpus reread or source-change surveillance.

## 5. Replace the admission data path

### 5.1 Required separation

Introduce explicit internal operations with these responsibilities:

- `prepare_admission_plan`: consume/borrow prepared facts, aggregate changes, encode static operations, establish deterministic order and compute the receipt digest.
- `execute_admission_attempt`: perform protected current-value reads, bitmap modification, writes and commit using an already prepared plan.
- `WriterScratch`: reusable storage owned by one admission worker across successive batches.

The worker prepares a plan once, then attempts that plan at most four times. A retry must not reparse files, rebuild per-file maps, recompress contributions, sort static keys or recompute the receipt digest.

Keep the existing 1/2/4-ms backoff. Each failed transaction is dropped before retry. Read current bitmap values anew on every attempt; cached values from aborted attempts are invalid.

### 5.2 Aggregate fixed-width keys without allocating encoded keys per membership

Remove `BTreeMap<Vec<u8>, BTreeSet<u16>>` from the per-file hot path and remove `MutationPlan::merge`.

For FTS use a typed key containing collection, binding, gram kind, gram and block number. For structural facts use collection, binding, state kind and block number. Use a batch-wide map from these fixed-width keys to a delta index. A standard HashMap, or the repository's existing equivalent, is sufficient; add no hashing dependency solely for this task.

Store block deltas in a contiguous vector of 64-word bitsets. Each membership sets one bit in its block delta. Duplicate membership is naturally idempotent. Do not allocate a tree node or an encoded Vec key for every gram occurrence.

For value/length keys, group using the existing exact ScalarKey/Number semantics. A batch-local ordered map of borrowed logical keys is acceptable if hashing would require changing equality semantics. Intern or clone a variable-length value only once per distinct group, not once per occurrence. Exact numerics must not become floating point.

After aggregation, encode each distinct physical bitmap key once. Preserve one protected read and one put/delete per changed block per attempt. This is a CPU/allocation repair; it does not change the persisted bitmap layout or avoid necessary shared-block locking.

Diagnostic invariants:

- Bitmap key encodes equal distinct changed bitmap keys, excluding explicitly counted verification work.
- The executor performs at most one bitmap read and one bitmap write for each changed key per attempt.
- Duplicate memberships do not inflate block cardinality.
- No per-file block map survives in the implemented path.

### 5.3 Build one ordered action vector before opening the transaction

Represent actions with an explicit kind, for example:

- Static put.
- Required pending deletion.
- File-state validation followed by replacement.
- Bitmap OR using a delta index.

Store encoded keys/values in reusable arenas and refer to ranges by integer offsets. Avoid unsafe self-references. Extend existing codec helpers with encode-into-buffer forms where necessary; retain their existing byte-level output.

Sort one action vector by physical key outside the transaction. Coalesce only semantically identical duplicates. Unexpected conflicting actions are errors; do not silently choose whichever file was visited last.

The transaction must execute a single match on the action kind. Remove the current loop that checks every key against four different block maps, a delete set and a put map. No key cloning, BTreeSet-to-Vec conversion or second sort is permitted inside the transaction.

The lock order remains shared gate, exclusive receipt, then other action keys in physical byte order. Put the receipt only after validating that it is absent or an exact replay; its lock is already held.

Use pinned get-for-update only where supported by the locked public binding API. Borrow the value for decoding and release the pin promptly. Reuse a bitmap encoding buffer. Do not add unsafe FFI or remove read-modify-write protection.

### 5.4 Make contribution construction linear for multi-record files

Preparation already has slot-associated facts. Establish grouped ranges or a prefix-offset array once for each fact family. Then consume a record's own values, lengths, states, sort keys, relations, relation states and grams directly.

Forbidden pattern: for every record, filter the entire file's sort/relation list or scan the entire gram-set map.

Target complexity is O(records + facts), apart from necessary one-time ordering and distinct-key aggregation. Preserve source record order, duplicate-ID semantics, relation positions and native sort ties.

Borrow or move gram lists directly to their encoder. Remove the intermediate gram map clone and the subsequent clone into each reverse contribution. Reverse grams remain stored once and excluded from the compressed contribution payload.

Keep immutable binding/field descriptors in shared runtime or chunk-owned storage. Resolve local-to-runtime binding maps once per prepared collection group, not separately for every file. File projection metadata should identify slices/ranges, not duplicate the complete schema.

### 5.5 Reuse scratch and hash the actual plan

Create WriterScratch before each writer's receive loop. Reuse arenas, vectors, block deltas, aggregation tables and contribution buffers across admissions. Charge their retained capacities.

Large-file exceptions must not leave an idle worker holding unbounded scratch. An idle worker may retain at most `min(8 MiB, preparation_target / (2 * writer_count))` of aggregate scratch, with checked handling of small nonzero budgets. Drop excess application buffers at the batch boundary and include that release in construction time. Do not call a process-wide allocator trim.

Replace `digest_fact`'s allocating serialization of every fact vector with a streaming digest over the canonical prepared action plan:

- Versioned domain separator, build identity/epoch and admission ID.
- Counted, length-delimited file identities, captured versions/fingerprints and reserved ranges.
- Ordered action kind, encoded key and static value or canonical bitmap delta.
- Required preconditions and coverage operations.
- Exclude the receipt's own digest field to avoid recursion.

Use explicit lengths and tags; concatenated raw strings are ambiguous. Hash encoded data already produced for the write plan instead of creating a second serialized copy of all facts. The digest is computed once before retries. Tests must distinguish batches with identical IDs but different values, grams, paths, fingerprints or preconditions.

## 6. Complete the resource-accounting contract

The existing `peak_reservation_estimate_bytes` is a sum over the first jobs, not a running ownership ledger. Replace it. Merely reporting estimates or reducing channel sizes does not meet this section.

### 6.1 Reservations and dispatch

Implement a shared ledger and an ownership token for each dispatched preparation job.

- Maximum dispatched-but-unreleased jobs: `max(1, min(16, 2P + W))`.
- Initial reservation estimate: `max(1 MiB, 8 * source_bytes)`, checked for overflow.
- Dispatch only when both job capacity and the 64 MiB soft retained-work target permit the reservation.
- Replace estimates with measured owned capacities at preparation completion.
- The token follows data through ordered ready storage, mapping, writer queue, active plan, transaction/retry and release.
- Split ownership correctly when one chunk feeds several admissions; never release the full charge at the first send.
- Add writer-plan/scratch ownership to the ledger; do not double-count buffers borrowed by a plan.
- Count shared schema/inventory separately once, and include their memory in process RSS.

A previously dispatched job may always report completion even if actual capacity exceeds its estimate. Stop new dispatch and drain work after such an overshoot. A known oversized job runs alone. Record actual peaks and overshoot duration.

Ensure liveness when the earliest job is slow and later jobs finish first. Never require a completed worker to acquire more budget before reporting its result. Never wait for memory while holding database locks. The coordinator must service writer completions and release notifications while dispatching/collecting results.

Holding only bounded channels does not bound the ordered ready map: later completions can accumulate there while an early job is outstanding. The global token count must cover that case.

### 6.2 Capacity and completion measurements

Inspect owning Vec/String capacities and nested buffers, including exact numbers, paths, diagnostics, grams, sort/relation data and compressed/encoded arenas. A helper accepting only slices cannot determine outer Vec capacity.

Report live and peak charges by phase, separately from estimates. Retained worker scratch stays charged until released. Native transaction/allocator internals are not exactly observable through this ledger; report that limitation and sample RSS/engine properties separately.

At readiness, all job tokens and construction-only buffers must be released and workers joined. Include release costs in caller time. A claim of “no overshoot” requires the complete ledger, not just ready-map bytes.

The test suite must demonstrate that changing the configured byte target changes dispatch behavior, including while writers are blocked or retrying.

## 7. Close the correctness and finalization gaps

### 7.1 Validate actual forward sort rows

The current validator counts `contribution.sorts`; the candidate reference exporter also derives sort facts from reverse contributions. Neither establishes that SortEntries exist or are correctly ordered.

Add checks that traverse actual SortEntries:

- Decode collection, sort definition, encoded sort tuple, record identity and slot.
- Verify referenced live records and sort definitions.
- Compare expected key/value membership against the actual forward namespace.
- Detect missing, extra, duplicate-equivalent and malformed entries.
- Compare forward/reverse iteration, ties, missing/null ordering and ascending/descending behavior with native results.

For small fixtures, construct expected output independently through native sorting. Do not reuse the candidate key encoder as the only oracle. Candidate comparison must consume actual forward rows.

Corruption tests are mandatory: remove an actual sort row, insert an extra row and alter its encoded order component. Each must fail validation even when reverse contributions are untouched.

### 7.2 Clamp all iterators to their namespace

For `StoreView::iterator(namespace, lower, upper)`, treat absent logical lower bound as the namespace prefix and absent upper bound as the successor of that namespace prefix. Clamp provided bounds to that same namespace range.

Preserve half-open range semantics. Handle empty ranges, reverse seeks, all-FF logical prefixes and absent bounds. If an upper byte successor does not exist, enforce the namespace through the iterator wrapper rather than assuming saturation produces a valid bound. Check iterator errors.

Add mixed-namespace fixtures proving that None/None, one-sided and prefix ranges cannot leak keys.

### 7.3 Correct coverage reduction and empty collections

The first repair combines flags within an admission, but finalization still replaces previous coverage with the last scanned delta. Combine completeness with AND across admissions as well.

Initialize field definitions and zero-record coverage from the runtime schema. A configured empty collection must finalize successfully without requiring a delta from a nonexistent file.

Keep binding-specific custom-operation coverage separate from the declared field descriptor. A named operation on a field must not overwrite that field's declared descriptor merely because it is processed later.

Verify these cases against native on fixtures. Check actual coverage values and flags, not just the number of coverage rows.

### 7.4 Finish bounded aggregation without another format campaign

Keep the repaired metadata writer and streamed cleanup. Replace whole-directory BTreeMaps in finalization with grouped ordered scans and small per-binding counters, as required by the first repair.

Do not rebuild complete value/length directories in memory or rescan all directory keys for each field. Use ordered file/state scans or existing inventory ownership instead of adding whole-corpus duplicate maps.

This is completion of the existing boundedness requirement. Do not change page/block sizes, omit metadata, or create a cache of the entire decoded index. Report finalization CPU and scratch so its remaining cost stays visible.

## 8. Measure enough to identify the remaining bottleneck

### 8.1 Separate timing categories

Retain old measurements with their old labels. New detailed measurements must distinguish:

1. Source extraction and flat fact production.
2. Slot/binding mapping and record-range grouping.
3. Typed block aggregation.
4. Static key/value and contribution encoding.
5. Action ordering and digest computation.
6. Transaction begin and gate/receipt checks.
7. File-state/pending get-for-update calls.
8. Bitmap get-for-update calls.
9. Bitmap decode/OR/encode.
10. Put/delete calls.
11. Commit calls, aborted attempts and retry backoff.
12. Discovery, finalization, worker/buffer release and complete caller wall time.

Record failed-attempt work too. The current retry helper only returns the final successful attempt's detailed stats; that would undercount a contended run.

Move the preparation end timestamp to actual preparation completion; it currently includes finalization. Use actual caller time in the benchmark wrapper, including drops on function return, and keep internal ready time separate.

Get-for-update duration includes lookup and waiting; do not call it pure lock time. A low retry count establishes only that few calls exceeded their contention conditions.

### 8.2 CPU and engine evidence

For each fresh child process, record user CPU, system CPU, wall time and peak RSS. The benchmark driver can obtain these through standard process accounting. Do not divide summed worker wall time by thread count and label the result CPU time.

Run one sampling CPU profile of the representative admitted workload if the environment permits it. Attribute samples to application planning/encoding, RocksDB transaction/locking, memtable writes and maintenance. If profiling is unavailable, record the concrete limitation and retain fine call timers; do not invent a flamegraph diagnosis.

Sample supported engine properties at opening, after discovery, after admission and at readiness: memtables, block-cache usage, reader estimates, SST bytes, pending compaction bytes and running flush/compaction counts. Missing properties are unavailable, not zero.

A separately timed wait=true flush and maximum 30-second maintenance observation may follow readiness. Do not substitute settled RSS for readiness RSS or force full compaction to obtain a favorable sample.

### 8.3 Cheap operation-count checks

Primary runs collect coarse counters; detailed per-key timers are profiling-only. Record:

- Input gram memberships versus unique changed bitmap keys.
- Physical key encodes, action count and bytes.
- Bitmap reads/writes per namespace.
- Plan preparations versus attempts; they must not increase together on retry.
- Per-record range visits and whole-list fallback scans, with the latter zero.
- Worker scratch capacity and allocation growth events.
- Admission overlap and touched collection/block ranges.

Use actual key-overlap evidence or supported engine counters before diagnosing contention. Do not conclude from concurrently live transactions alone that they progressed independently.

## 9. Required tests and implementation checks

| Area | Required test |
| --- | --- |
| Aggregation | Repeated memberships and several files sharing a block produce one delta/key with correct bits |
| Execution | Each planned bitmap key has one protected read/write per attempt; current-state changes survive retry |
| Ordering | All action types follow one physical-key order after gate/receipt acquisition |
| Digest | Same IDs with changed payload/path/version fail exact replay; byte-identical replay is a no-op |
| Scratch | Repeated admissions reuse capacity; exceptional large inputs release excess at the defined boundary |
| Multi-record behavior | 100/1,000/10,000-record fixtures visit fact ranges linearly and match native semantics |
| Sort index | Missing/extra/wrong forward rows fail even when reverse contributions are unchanged |
| Namespace isolation | Unbounded, one-sided, prefix and reverse iteration remain inside the namespace |
| Coverage | Mixed completeness across transactions and empty configured collections match native |
| Budget | Ready, queued, active and retrying ownership remains charged; dispatch responds to byte target |
| Liveness | Slow first job, out-of-order completion, undersized estimates, oversized file and writer error terminate correctly |
| Atomicity | Multi-record/overlapping files and retained snapshots see complete old or new state |
| Metadata | Existing put/delete byte/operation caps and zero remaining coverage deltas remain true |
| Persistence | Memory host remains empty; small version-3 file fixture survives a durable reopen; older versions rejected |

For multi-record complexity, assert counted visits rather than fragile wall-time thresholds. A short release timing sample is additional evidence.

Before full-corpus timing, perform a source checklist:

- No per-file MutationPlan or merge.
- No encoded heap key per FTS membership.
- No BTreeSet-to-Vec-to-sort inside a transaction.
- No per-record scan over all other records' facts.
- No per-file schema descriptor clone.
- No whole-plan rebuilding on retry.
- No byte target that is merely logged.

Do not claim an item complete solely because a symbol was renamed. Use tests and counters to demonstrate the actual path.

## 10. Bounded benchmark campaign and decision

### 10.1 Three comparison subjects

Compare pristine native, the unchanged first-repair implementation, and the new candidate. Also run the candidate's native control to catch feature-hook regressions.

Use basic ASCII Musenalm, P=8, memory mode and the same schema/source manifest. Keep engine configuration fixed. Record commit, clean state, binary hash, Cargo.lock hash, command, toolchain, CPU-affinity policy and raw artifact hashes.

The old 35.650-second first-repair result remains historical. Run one fresh first-repair control on the same machine/session if its clean binary is available or reproducibly buildable. Do not rebuild redb or repeat earlier failed prototype campaigns.

Build once per implementation, then invoke release executables directly. No concurrent corpus builds.

### 10.2 Sequence

1. Complete all mandatory code corrections, focused tests and source checks.
2. Run native and candidate-native controls.
3. Run one corrected W=1 and one corrected W=4 construction using the same binary. Hold P and batch limits fixed. Both results must have equivalent canonical output and include all required metadata.
4. Run one W=4 detailed diagnostic/profile trial. If W=1 is faster, diagnose that result rather than hiding it.
5. Run one ordinary confirmation of the selected W=1 or W=4 configuration.
6. Run full-corpus internal validation in a separate process, now including actual forward-sort and coverage checks.
7. If the candidate meets the original performance gates, obtain three alternating native/candidate pairs in total and complete independent full-corpus native equivalence before a positive feasibility recommendation. Existing compatible runs may count; do not repeat them needlessly.

A 120-second run limit remains a censored-failure boundary. Record elapsed lower bounds for terminated runs. Do not convert failure or absent data into zero.

The experiment ends after this campaign. There is no W=8, cache/compression matrix, pipelining campaign, alternative transaction API or fresh engine experiment in this task. Correct discovered implementation defects and rerun affected evidence; do not repeatedly change the design to chase a target.

### 10.3 Unchanged acceptance and useful attribution

Preserve caller construction <= 1.15 times matched native, readiness RSS <= 1.40 times native and construction HWM <= 1.60 times native. Use raw integers and floor integer allowances.

Also report improvement versus the first repair, CPU consumption, plan-operation reductions and memory ownership. A large relative improvement is useful evidence but does not itself pass the native gate.

If database calls still dominate after application planning is separated and the required defects are fixed, report that evidence. A possible future bulk-construction design can be discussed in the report, but it is not implemented under this task.

If allocation/ordering or missing accounting still dominates because a mandated repair is unfinished, classify the task as implementation incomplete. Do not invoke the performance early exit as a substitute for completing these corrections.

Full-corpus internal consistency and fixture-native equality are different evidence classes. Reference code that derives expectations only from reverse contributions cannot certify every forward namespace. Full native reference work must run outside performance timing and avoid unbounded whole-corpus fact materialization, using bounded streaming/spill sorting if necessary.

## 11. Work sequence, validation and deliverables

| Phase | Work | Suggested commit |
| --- | --- | --- |
| 1 | Typed batch aggregation, grouped record facts, writer scratch and ordered action plan | `Replace per-file RocksDB admission planning` |
| 2 | Plan reuse, streaming digest, minimal transaction executor and retry accounting | `Execute prepared RocksDB admission actions` |
| 3 | Dynamic reservations, capacity accounting and bounded finalization | `Complete RocksDB construction memory bounds` |
| 4 | Forward-sort/coverage validation, namespace bounds and negative tests | `Validate persisted RocksDB query structures` |
| 5 | Fine attribution, corrected campaign, comparison and task report | `Report RocksDB admission repair measurements` |

Run appropriate focused checks after each phase. Complete the repository's workspace checks and at least:

```bash
cargo check --workspace --all-targets
cargo test --workspace
cargo test -p objekte-search-core --features rocksdb-prototype --lib
cargo check -p objekte-search-core --all-targets --features rocksdb-prototype,detailed-profiling
cargo test --workspace --features objekte-search-core/rocksdb-prototype
```

Verify that feature-off builds do not link RocksDB. Follow existing formatting procedures; preserve unrelated files.

Deliver:

- `docs/ai/generated/rocksdb-admission-repair/admission-repair-report.md`
- `docs/ai/generated/rocksdb-admission-repair/comparison.json`
- Raw build, profiling, memory, validation and campaign-manifest files under the task's benchmark-results directory.
- This task document under `docs/ai/done/`, with an appended Implementation report, only after its required corrections and checks are complete.

The final report must include a requirement matrix marked implemented/tested, failed or missing. It must distinguish performance failure from unfinished code. Explain the residual construction floor, which measured categories remain expensive, what memory remains unattributed, and whether another architectural decision would be needed.

Preserve all earlier reports. Keep the new integration branch/worktree for user review; do not merge into master. No implementation or new benchmark was executed while preparing this specification.

## 12. Source references

- [First repair at inspected revision](https://github.com/Simon-Martens/objekte/tree/08591e8452132f392201cfdb745d958f502a556b)
- [First repair report](https://github.com/Simon-Martens/objekte/blob/08591e8452132f392201cfdb745d958f502a556b/docs/ai/generated/rocksdb-construction-repair/repair-report.md)
- [First repair completed specification and limitations](https://github.com/Simon-Martens/objekte/blob/08591e8452132f392201cfdb745d958f502a556b/docs/ai/done/objekte_rocksdb_construction_repair_specification.md)
- [Admission implementation](https://github.com/Simon-Martens/objekte/blob/08591e8452132f392201cfdb745d958f502a556b/crates/core/src/rocksdb_store/write.rs)
- [Scheduling and accounting](https://github.com/Simon-Martens/objekte/blob/08591e8452132f392201cfdb745d958f502a556b/crates/core/src/rocksdb_store/build.rs)
- [Finalization](https://github.com/Simon-Martens/objekte/blob/08591e8452132f392201cfdb745d958f502a556b/crates/core/src/rocksdb_store/finalize.rs)
- [Validation](https://github.com/Simon-Martens/objekte/blob/08591e8452132f392201cfdb745d958f502a556b/crates/core/src/rocksdb_store/validation.rs)
- [Candidate reference construction](https://github.com/Simon-Martens/objekte/blob/08591e8452132f392201cfdb745d958f502a556b/crates/core/src/rocksdb_store/reference.rs)
- [Iterator bounds](https://github.com/Simon-Martens/objekte/blob/08591e8452132f392201cfdb745d958f502a556b/crates/core/src/rocksdb_store/mod.rs)
- [Final first-repair raw result](https://github.com/Simon-Martens/objekte/blob/08591e8452132f392201cfdb745d958f502a556b/bench/results/rocksdb-construction-repair/ascii-w4-final-20261115/build.json)

## Implementation report

**Task identifier and completion:** `rocksdb-admission-repair`, finalized
2026-11-15 in the combined review branch
`integration/rocksdb-bulk-construction-repair`; worktree
`/home/simon/source/objekte-integration-rocksdb-bulk-construction-repair`.
The admission integration branch remains at `00d16a040b` for audit.

**Controlling decision:** the later bulk-construction specification explicitly
superseded this task's transactional fresh-build bitmap read/modify/write
requirements. The combined branch retains the compatible fixes rather than
maintaining two competing constructors: per-file planning replacement,
namespace clamping, coverage AND, shared descriptors, direct discovery
inventory, complete-image accumulation, forward-sort validation, resource
accounting and bounded streaming finalization.

**Implementation commits:** `54d07604ac`, `bc8696eb48` and `00d16a040b` on the
admission branch; `a83ed20b2b`, `8481b32152`, `a28892d022` and `f9e0e7c94c` on
the combined branch. Every commit was pushed.

**Result:** the combined constructor improved caller readiness from
35,650,433,616 ns to 11,584,790,610 ns. It remains a performance failure at
11.69 times native time, 2.75 times native resident RSS and 2.91 times native
peak RSS.

**Validation:** workspace checks and tests passed; feature-enabled core tests
passed 553 with 15 ignored; the feature-enabled workspace suite passed. Full
ASCII validation covered 148,486 records/files, 40 receipts, 2,814,411 values,
921,173 lengths, 14,004,908 states, 56,275,908 gram memberships and 566,768
actual forward sort entries with zero errors.

**Evidence and limitations:** see
`docs/ai/generated/rocksdb-admission-repair/admission-repair-report.md` and
`docs/ai/generated/rocksdb-bulk-construction-repair/repair-report.md`.
Coordinator-serialized live-tail publication means W=4 did not demonstrate
disjoint writer overlap. Sampling profiling was unavailable because the host
denied PMU access. The integration worktree is retained for review.

