# Objekte Hybrid Index Allocation and Query Performance Specification

Date: 2026-11-13  
Target branch: `integration/hybrid-index-storage`  
Reviewed source: `e397f8f3046926710e3e75b144d26279e8d4eb83`  
Status: implementation and measurement required

## 1. Objective and current priorities

Improve the remaining construction and query overhead through local changes to allocation, ownership, synchronization, and repeated work. The user considers a further improvement of a few percentage points in construction useful; smooth repeated queries and representative query tails are the higher priority. Do not expand this into another storage architecture project.

Treat memory as an explained resource cost, not an isolated score. A modest increase caused by bounded reusable capacity, allocator retention, or page overhead is acceptable when performance benefits justify it. Do not use an earlier percentage RSS threshold as the sole rejection criterion. Conversely, do not attribute growth to alignment without evidence, retain duplicate representations without purpose, or permit unbounded caches or pinned-generation retention.

Continue the existing hybrid storage task. This document adds the latest source findings and updates the performance priorities in:

- `objekte_hybrid_index_storage_repair_specification.md`
- `objekte_hybrid_index_storage_remaining_inefficiencies_specification.md`

Their unfinished functional, correctness, recovery, and durability obligations remain applicable. The user's latest priorities govern conflicts concerning performance tuning and RSS. Preserve original benchmark results and report prior target ratios accurately; do not label an unmet target as passed.

Before implementation, read `AGENTS.md`, `docs/ai/README.md`, the authoritative task and current notes. Record the actual starting commit and uncommitted changes. Verify each finding against that checkout; the source revision above is a review reference, not an instruction to reset newer work.

### Preserve the current contract

- Memory-only construction and use is the default, without managed index writes.
- Temporary-directory, repository `.objekte`, and custom-directory persistence support on-demand and fully resident use.
- Keep Roaring postings, immutable ordered dictionaries, exact typed comparisons, string/collation semantics, sorting, relations, inference, custom bindings, and raw fallback behavior.
- Preserve progressive availability, updates, active masks, pinned readers, cancellation, atomic generation publication, and observable synchronization errors.
- Preserve validation of external files and the authorized deprecated-file cleanup policy. Do not introduce a public unchecked reader or new legacy loader.
- Keep required construction inside readiness timing. Disk-only serialization may remain deferred to persistence, but its cost must be included in flush and build-through-durable-readiness measurements.
- Do not use verified-result caching, larger undisclosed caches, reduced validation of external input, or additional source freshness checks to influence comparisons.

## 2. Starting evidence and findings already addressed

The latest archived campaign measures implementation `e15b3f5c4b0efe9b89e8e2789db4fe22a56f72f3` against original baseline `da8fde68472b8646a63a05b3b3654c58de877301`. The subsequent implementation change concerns managed-store error classification. These are archived results, not measurements of any future candidate.

| Measure | Baseline | Hybrid checkpoint |
| --- | ---: | ---: |
| Complete memory construction | 999.99 ms | 1,130.43 ms |
| First searchable batch | 338.26 ms | 353.77 ms |
| Warm stored opening, on-demand | 637.35 ms | 386.26 ms |
| Warm stored readiness, fully resident | 637.35 ms | 443.28 ms |
| First complete save or flush | 352.01 ms | 236.97 ms |
| Retained RSS after build | 532.69 MiB | 562.74 MiB |
| Peak RSS during build | 542.22 MiB | 580.82 MiB |

Across 220 cases in ASCII and Unicode, the geometric means of per-case median query ratios are 1.075 memory-only, 1.091 resident, and 1.092 warm on-demand. Q175 ASCII memory p95 is 5.37 ms versus 3.71 ms baseline. All 440 case/profile result digests matched in the archived campaign.

Source: [measured checkpoint and provenance](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/bench/results/hybrid-index-storage/repair-20261113/final-e15b/REPORT.md). The save/flush implementations have different durability steps; preserve that qualification.

Already addressed at the reviewed source revision:

- Constructed record, sort, and relation sections are deferred instead of eagerly retained beside their native structures.
- Relation contributions are distributed once across compaction partitions.
- Sealed posting sections can be persisted directly.
- Initial incremental publication has a consuming sealing path.
- Bigram `u64` comparison and a compact gram-presence filter exist.
- Ordinary build input buffers are reused within a batch.

Preserve these improvements. Remaining repeated domain construction, binding serialization, other sealing callers, and inference projection are separate findings.

## 3. Work packages

### Package 1 — Reuse the candidate bitmap during posting operations

Priority: high; repeated queries.

The native bigram path clones the first posting and then intersects in place. The sealed path calls `intersect_descriptor()` for subsequent grams and replaces its result each time. A decoded-cache hit computes `input & bitmap`, producing a new result rather than updating the existing candidate.

Range and identifier unions also materialize owned posting operands. Cache hits clone the bitmap before the caller unions it into the destination.

Required changes:

1. Add operation-oriented posting access that can intersect or union into an existing destination. Use it in sealed bigram evaluation, bounded scalar unions, and identifier/relation unions where applicable.
2. For a cached operand, hold a stable shared handle and perform the bitmap operation by borrowing the operand. Keep the cache lock out of the computation.
3. Allocate the initial candidate result when ownership is necessary; avoid recreating it for every subsequent cached intersection.
4. Preserve the useful serialized intersection path. Where the available serialized API necessarily returns a new bitmap, keep it until a measured safe alternative is available. Do not invent an unsafe decoder to force in-place behavior.
5. Transfer decoded bitmap ownership into the cache when admission is useful. Avoid cloning solely to populate the cache.
6. Preserve fallible operation semantics, domain validation, early-empty exit, and active-record filtering. A failed operation must not expose a partially valid query result.

Verify exact equivalence against existing owned-bitmap operations with empty, sparse, dense, overlapping, and disjoint postings. Cover cache hits, misses, first validation, eviction, concurrent readers, and corrupt input.

Measure full bitmap clones, temporary results, allocation counts/bytes, and query medians/p95. Warm cached intersections and unions must not create a complete operand copy solely because the access API returns ownership.

Sources: [sealed bigram evaluation](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/bigram_dictionary.rs#L232), [posting operations](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/posting_dictionary.rs#L433).

### Package 2 — Remove repeated validation lookup and reduce cache synchronization

Priority: high; externally opened and concurrent queries.

External dictionary accesses enter a mutex-protected validation map even after validation succeeded. Decoded-cache access acquires another mutex and updates a `BTreeMap` recency index on hits. Constructed dictionaries already bypass the external validation map; preserve that distinction.

Required changes:

1. Give repeated posting access inexpensive stable validation state, for example through descriptor identity and lazily allocated per-block state. Choose the representation after measuring its footprint.
2. Bind proof to the immutable payload and its applicable record domain. The same bytes accessed through different eligibility domains must not inherit an invalid proof.
3. Keep first validation once-only under concurrent access, preserve failures, and retain necessary external structural validation before trusting absent-key results.
4. Measure decoded-cache mutex wait time and acquisition count separately from deserialization and bitmap computation. Do not call synchronization overhead measured contention unless threads actually wait.
5. If cache synchronization is material, compare sharded bounded storage or an established cheaper recency policy. A concurrent hash map alone does not remove shared recency updates, bitmap copies, or excessive access frequency.
6. Prefer immutable read state and worker-local scratch where mutation need not be shared. Do not replace publication/status mutexes merely because they are mutexes.
7. Keep aggregate cache limits, eviction behavior, and retained handle memory explicit. Perform expensive bitmap work outside locks. Persistence and bulk maintenance must not pollute the query cache.

Test simultaneous first access, warm access, eviction with live readers, pinned generations, domain mismatch, and malformed payloads. Run single-query latency and concurrent independent queries; intra-query worker count alone does not establish contention on the dictionary path.

Sources: [validation state](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/posting_dictionary.rs#L477), [cache access](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/segment_format.rs#L363).

### Package 3 — Compare typed keys without constructing owned entries

Priority: high; exact and range queries.

Generic `get()` and `lower_bound()` decode an owned key and posting descriptor at each binary-search probe. Strings allocate. Range upper-bound checks also materialize keys that the caller may never need.

Required changes:

1. Separate borrowed encoded-key access, typed comparison, and descriptor decoding.
2. Extend the existing specialized comparison approach to the actual scalar key types used in production.
3. Decode descriptors only for selected or visited entries; reuse the final search position.
4. Preserve the existing comparator for type tags, exact numeric values, strings/identifiers, lengths, and collation representations. Serialized bytes are not generally order-preserving.
5. Keep a checked generic fallback for unsupported types and identify its callers. Do not assume a helper is a significant hot path without tracing execution.
6. Preserve stable reader lifetimes and malformed-key errors.

Differentially test exact, missing, inclusive/exclusive range, cross-block, and reverse iteration against the reference typed ordering. Include numeric boundaries, non-ASCII strings, collation cases, and corrupt dictionaries.

Record owned-key decodes, allocations per lookup, and descriptor decodes. Compare both narrow and broad ranges.

Source: [ordered search](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/ordered_block.rs#L409).

### Package 4 — Eliminate the separate allocation for every encoded key

Priority: high; construction.

`OrderedBlockWriter::push()` calls `encode_key()`, creating one `Vec<u8>` per key, including eight-byte bigram keys. The pending vector is reused, but individual key allocations are discarded after each leaf. Boundary keys are copied for the sparse directory.

Required changes:

1. Use a reusable contiguous leaf buffer with key offsets or an equivalent bounded representation.
2. Encode fixed-width primitive keys without a per-key heap allocation. Serialize variable keys into reusable storage while preserving the existing encoding.
3. Preserve leaf limits, offset checks, ordering proof, and sparse-directory boundaries. Boundary storage may legitimately outlive a reused leaf buffer; account for that ownership explicitly.
4. Transfer or reuse completed buffers where appropriate. Do not retain a vector of every key to avoid leaf-local allocations.
5. Prefer preserving the existing disk format. No new dependency is needed merely to obtain a reusable byte arena.

Test one-entry dictionaries, exact leaf boundaries, large keys, multiple blocks, both bigram alphabets, and disk round trips. Count allocations relative to entries and leaf blocks; demonstrate that fixed-width key encoding no longer allocates per entry.

Source: [ordered writer](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/ordered_block.rs#L130).

### Package 5 — Serialize memory postings directly into their final buffer

Priority: high; construction.

The constructed path supplies a `Vec` destination to `StreamingPostingPack`. Each posting is serialized into reusable scratch, checksummed, and copied into that final vector. `PostingPack` already demonstrates direct append into a growing vector.

Required changes:

1. Use direct append for an owned memory destination, calculating the descriptor/checksum from the newly appended slice.
2. Retain reusable scratch where streaming to a file or other destination needs it.
3. Transfer the completed vector into the constructed reader without another whole-buffer copy.
4. Preserve offsets, cardinality, checksum coverage, error handling, and size limits. Do not perform a complete preliminary serialization merely to reserve capacity.
5. Keep the existing direct persistence path and portable format.

Verify descriptors and reopened query results across empty and large postings, allocation failure/error paths where supported, and multiple dictionaries sharing one payload section.

Record serialized bytes and intermediate copied bytes. The target is removal of the scratch-to-final payload copy during memory sealing, not a claim that serialization itself becomes free.

Sources: [posting encoders](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/posting_dictionary.rs#L37), [constructed destination](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/hybrid_segment_codec.rs#L727).

### Package 6 — Fuse remaining sealing passes and preserve construction results

Priority: high; construction and opening.

Remaining work includes:

- Domain reconstruction in `check_deferred_metadata_limits()`.
- Separate fallback-domain construction during encoding and consuming sealing.
- Encoding field/text binding tables and immediately decoding them to attach readers.
- Recreating and sorting `posting_inputs()` and `text_inputs()` just to obtain counts.
- Walking completed dictionaries again to construct gram-presence filters.

Required changes:

1. Return the binding descriptors needed for reader attachment directly from encoding. Preserve encoded binding bytes needed by persistence without decoding them again.
2. Retain counts from the first traversal, or obtain them without constructing sorted input lists.
3. Construct necessary record domains once and share them where ownership and lifetimes justify it. Avoid scanning records to calculate a domain that a constructed writer does not use.
4. Preserve proof of identity, relation, and domain invariants through private construction types. Do not remove a check until its invariant is established elsewhere.
5. Populate gram-presence filters while writing keys or during an existing external validation traversal. Preserve no-false-negative behavior and complete-coverage requirements.
6. Reconcile the earlier estimator finding: the sealed estimator still has the generic path, but establish production callers before assigning it performance priority. Any touched estimator must propagate errors explicitly; reducing `Result` values with minimum ordering is not error propagation.

Test mixed logical/fallback records, empty bindings, inferred/custom fields, relations, both alphabets, corruption, and flush/reopen. Measure domain scans, key traversals, binding encodes/decodes, and input-list construction.

Sources: [section encoding](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/hybrid_segment_codec.rs#L469), [constructed validation](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/segment_metadata.rs#L169), [gram filter](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/bigram_dictionary.rs#L74).

### Package 7 — Reuse source-query buffers and avoid unnecessary payload rereads

Priority: profile after the sealed-index changes; inherited from the baseline.

Candidate grouping allocates a `BTreeMap<FileId, Vec<...>>`, including a separate vector for single-record files. `RecordRangeReader` allocates fresh bytes on each read. Multi-range reads also allocate an ordered range copy and copy individual records from a shared window. Residual verification retains locators, so non-locator output can read winning records again.

These operations were present in the baseline. They are improvement opportunities, not demonstrated causes of the hybrid regression.

Required investigation and bounded changes:

1. Measure grouping, range preparation, reading, parsing, and final materialization separately on source-heavy queries.
2. Use compact single-range storage or reusable grouping buffers where this reduces allocations without changing deterministic processing or error order.
3. Consider per-worker scratch and a callback/borrowed-slice read interface for immediate verification. Reuse buffers across files and batches, with a policy for exceptional large reads.
4. Avoid sorting already ordered ranges only where the caller guarantees ordering. Do not assume record slot order always equals file byte order.
5. Consider retaining bytes or complete logical payloads only for bounded current winners when non-locator output would otherwise reread them. Release discarded winners promptly and charge retained payloads.
6. Never expose a projected logical value as the complete requested payload. Retention must respect the existing source/revision semantics and cancellation limits.
7. Keep locator-only queries free of payload retention. Do not cache all source files or add freshness metadata checks.
8. If retaining payloads costs more than the small number of final rereads, keep the current bounded behavior and report the measured reason.

Test one and many records per file, sparse ranges, large records, all return modes, source errors, cancellation, and source-heavy queries. Record actual file opens, physical read bytes, parsing counts, buffer allocations, and selected-payload rereads.

Sources: [grouping](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/run.rs#L4211), [range reader](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/source.rs#L2021), [final payload loading](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/run.rs#L4479).

### Package 8 — Reuse validated bigram contributions

Priority: measured follow-up; inherited from the baseline.

Packed bigram blocks are validated after packing, during admission, and before conversion. Those traversals existed before hybrid storage.

Establish a private validated contribution type or equivalent ownership invariant. Validate freshly created block content once; admission must still check base offsets, coverage, ordering between contributions, and overflow. Preserve external validation and malformed internal-contribution test coverage. Do not simply turn checks off.

Measure examined blocks and phase time. Retain this change only if it removes meaningful work or produces a clearly simpler invariant without increasing risk. Ordinary build input already reuses its buffer within each batch; do not claim that optimization as new.

Source: [bigram admission and conversion](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/bigram.rs#L811).

### Package 9 — Complete remaining publication and maintenance repairs

Priority: retain in scope; separate from initial-build attribution.

1. Inventory remaining `seal_segment_owned()` callers. Newly constructed segments should use consuming sealing before publication, without cloning an entire segment to obtain ownership. Keep actual external decoding for external data.
2. Preserve the completed posting/bigram/relation scatter. Distribute logical/empty-array domains and inferred contributions without a full source traversal per output partition.
3. `project_sorting()` sorts ascending, descending, and composite orders again. For partitioning one source, distribute its existing sorted sequences in one traversal and remap slots. Do not scan the complete source order separately for every output.
4. Where combining multiple sources needs merging, preserve comparator semantics, null placement, collation, and identifier tie-breaking. Use existing order construction where reuse is invalid.
5. Preserve source-free compaction, active masks, catalog identity, cancellation, pinned readers, and independent work budgets. A small output partition alone does not bound examined source work.

Test static/incremental construction, add/update/delete/move, compaction, inference, multi-record files, historical rebasing, and pinned readers. Count external decoding during fresh publication, sort comparisons, and source contributions examined as output partition count increases.

Sources: [older sealing route](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/lifecycle.rs#L1433), [sort projection](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/mutation_segment.rs#L1616).

## 4. Execution and supervision

Keep the original checkout on `master`, preserve all user changes, and continue the existing integration branch/worktree. Do not create a competing integration branch or merge into `master`.

Use the recorded integration worktree, normally `/home/simon/source/objekte-integration-hybrid-index-storage`. Any delegated work uses separate sibling worktrees under `/home/simon/source`, never `/tmp`, and descriptive branches under `task/hybrid-index-storage/`.

A supervisor owns interface decisions, integration, and acceptance. Suggested delegation:

| Workstream | Ownership | Coordination |
| --- | --- | --- |
| Posting execution | Packages 1–2 | Posting handles, validation state, cache ownership |
| Ordered dictionaries | Packages 3–4 | Key comparison and reusable encoding |
| Construction | Packages 5–6 and fresh sealing in 9 | Codec ownership and construction proofs |
| Source execution | Package 7 | Begin after focused profile establishes value |
| Maintenance | Package 8 and remaining 9 | Coordinate shared builder/lifecycle interfaces |
| Verification | Counters, benchmark provenance, final report | Performance campaigns run serially |

Only the supervisor edits the integration worktree. Agree interfaces before overlapping edits. Each subagent supplies commits, tests, measurements, and limitations. Push each commit and integration merge according to repository instructions.

Suggested sequence:

1. Record starting revision, relevant call paths, and focused diagnostic counters.
2. Implement direct memory serialization and per-key allocation removal as separate measurable commits.
3. Implement borrowed typed-key comparisons.
4. Implement in-place posting operations and ownership transfer.
5. Optimize validation/cache synchronization based on measured access costs.
6. Fuse sealing passes and migrate remaining fresh publication callers.
7. Evaluate inherited source/bigram opportunities and complete scoped maintenance changes.
8. Integrate, run final validation, and publish the evidence report.

Use focused tests and short measurements after coherent changes. Do not run the entire campaign after every small commit. If an experiment worsens latency or adds unjustified complexity, revert it and retain its result in the report. Profile-gated inherited opportunities may be deferred with evidence; do not mark outstanding functional repairs complete merely by classifying them as performance work.

## 5. Measurement protocol

### Comparisons and attribution

Keep two references:

- The actual integration revision immediately before this work.
- The authoritative original baseline `da8fde68472b8646a63a05b3b3654c58de877301`, after confirming its provenance.

Record exact candidate commits, executable hashes, compiler/build flags, lockfile identity, fixture/schema, backend, worker counts, residency, cache settings, warm-up policy, and raw samples. Distinguish binary build revision from execution checkout. Final evidence must use the final committed implementation.

Use optional counters and allocation/lock profiles for diagnosis; ordinary release builds for acceptance timings. Separate exclusive elapsed time from nested timers and accumulated parallel-worker time.

Do not infer that a mutex is contended, a clone copies underlying data, or a repeated pass is expensive merely from its syntax. Distinguish `Arc::clone` from deep cloning and distinguish required output ownership from avoidable intermediates.

### Required final evidence

- Seven paired fresh-process construction trials with alternating order; include sealing, first-batch availability, and complete readiness.
- Full established 220-query suite in ASCII and Unicode for memory-only, resident, and warm on-demand modes. Preserve the existing 30-repetition final protocol.
- Exact counts, identities, ordering, and result digests; zero verified-result-cache hits.
- Per-case medians and p95, absolute differences, and aggregate ratios. Include Q167/Q175/Q177 and source-heavy Q006/Q007 where their definitions remain unchanged.
- Single-query and concurrent-query measurements for synchronization changes. Keep concurrency conditions explicit.
- First flush, unchanged flush, incremental flush, reopen, and opening through a matched first query. State cache temperature and durability differences.
- Focused correctness and fault tests for changed paths, followed by required workspace checks.
- Update/maintenance scaling, foreground latency, and pinned-reader evidence for Package 9.

Match prepared-plan behavior and all cache limits between comparable runs. Do not discard warm-up samples retrospectively or select only passing queries. Use the established statistical method consistently; the archived summary uses middle-pair mean medians and nearest-rank p95.

### Memory interpretation

Report RSS and peak RSS, but also allocation count/bytes, retained capacities, decoded-cache/state bytes, mapped versus owned sections, live temporaries, and pinned generations where available.

Bounded buffer reuse may increase retained capacity while improving speed. Accept that trade-off when documented. Explain page/allocator effects with evidence; the current memory representation does not imply a separately reserved page for every posting. Leaks, unbounded growth, resource-limit failures, and memory-induced query stalls still require repair.

## 6. Acceptance and final report

Prioritize measurable construction improvement and smooth query execution over minor RSS reductions. Do not promise that any individual finding recovers a specified number of milliseconds.

Retained changes must preserve semantics and avoid an unexplained material regression against the starting integration revision. Report progress toward the original baseline, including remaining slower queries. Aggregate gains must not conceal worse representative tails. Small unresolved timing differences should be described accurately rather than provoking a broad redesign or an unsupported claim of parity.

For each Package 1–9, report: implemented and verified, already addressed with evidence, measured and rejected, profile-gated and deferred, or incomplete. Distinguish new hybrid costs from inherited inefficiencies.

The final report must include code locations and commits, tests, removed work counters, before/after latency distributions, construction phases, memory explanation, shifted costs, rejected experiments, and remaining risks. Store it in `docs/ai/generated/` with raw data in the established benchmark tree.

Link this specification from the existing task. Preserve uncommitted task documents according to repository workflow. Keep the overall task open if functional or correctness obligations remain. Leave the integration branch committed and pushed for the user's merge; do not modify `master`.


## Implementation report

Task identifier: `hybrid-index-storage` allocation/query follow-up. Completed: 2026-11-14. Base: `e397f8f3046926710e3e75b144d26279e8d4eb83`; integration branch: `integration/hybrid-index-storage`; worktree: `/home/simon/source/objekte-integration-hybrid-index-storage`.

The follow-up's local allocation, ownership, construction, query, and normal-path maintenance improvements are implemented and measured. Package 2's two synchronization alternatives were measured and rejected; Packages 7 and 8 retain the documented profile-gated opportunities. The original hybrid repair task remains open for its broader obligations, including the unbatched legacy base-containing compaction path and strict allocator/lock-wait accounting. The user's 2026-11-13 instruction relaxed rigid millisecond targets: improve, document baseline ratios, and move on. No merge into `master` was made.

Implementation commits on this integration branch: `09154558b7` (key arena), `003297081a` (direct posting serialization), `7279796a35` (borrowed key comparison), `fa8394f6a7` (borrowed posting operations), `376dd75b61` (binding/domain reuse), `980a8831dc` and `5ca0598b0d` (partition/inference scatter), `23c4b1a588` (consuming fresh seal), `2ba53f3a20` and `ceba166b72` (gram-filter fusion), `9835f0c4e7` (single-range source grouping), `2188fbd178` (maintained mutation benchmark), `82cc05c21c` and `3bd8b74d79` (affected-only and bounded normal-path compaction), `471b79c35b` (unchanged-flush measurement), and `155bcf8659` (raw evidence and full report). No subtask branches were created for this follow-up.

Validation: `cargo test --workspace --locked --quiet --jobs 2` passed (962 tests, 18 ignored, 0 failures); focused ordered dictionary, bigram, codec, posting, lifecycle, and compaction tests also passed. Relevant commands, fixture, binary hashes, cache limits, raw samples, 30-repetition 220-case suite, seven alternating construction pairs, focused tail pairs, flush/open/cold/constrained/inference runs, and maintenance evidence are in the [full report](../generated/hybrid-index-allocation-query-performance-20261114.md) and [raw archive](../../../bench/results/hybrid-index-storage/followup-20261114/). The final paired candidate/start construction ratio is 1.0003; query median geometric-mean ratios are 0.98995 memory, 0.99012 resident, and 0.98546 warm on-demand. Noncontemporaneous original-baseline ratios and remaining slower tails are stated in the full report. The constructed-reader and external-validation boundary remains intact.

The original checkout remains on `master`. The integration branch/worktree is retained for review. No subordinate worktree was created for this follow-up; earlier hybrid project worktrees are outside this follow-up's cleanup. Its temporary benchmark stores can be removed after this report and raw archive are verified; no user-authored file is included in that cleanup.
