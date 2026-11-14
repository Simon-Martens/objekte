# Objekte hybrid index storage repair specification

Date: 13 November 2026

Task: `hybrid-index-storage`

Integration branch: `integration/hybrid-index-storage`

Status: implementation and acceptance work remains incomplete. Keep all changes out of `master`.

Allocation/query follow-up: [completed specification and implementation report](https://github.com/Simon-Martens/objekte/blob/integration/hybrid-index-storage/docs/ai/done/objekte_hybrid_index_allocation_and_query_performance_specification.md). The broader repair task remains open.

## 1. Objective and governing requirements

Repair the construction, query, memory, persistence, and maintenance costs of the hybrid index implementation while preserving its storage policies and correctness. This document is a supporting repair specification for the existing hybrid index storage task. Continue that task and its integration branch; do not restart the project or create a competing integration branch.

Read the authoritative task document, `AGENTS.md`, `docs/ai/README.md`, and current task notes before implementation. Preserve requirements not repeated here. In particular:

- Memory-only construction and use remains the default, with no managed index writes.
- Disk synchronization remains available in the system temporary directory, repository `.objekte` directory, and a custom directory.
- Every disk location supports on-demand and fully resident operation through the same core policy and query semantics.
- Keep Roaring postings and immutable ordered dictionaries. Preserve typed comparisons, string ordering and collation, exact numeric behavior, range scans, sorting, relations, structural predicates, and inference.
- Preserve progressive availability, explicit updates, liveness masks, pinned readers, background synchronization, and atomic generation publication.
- Deprecated index loading remains disabled in production. Delete only positively identified deprecated index artifacts under the existing ownership and path checks. Preserve source data, configuration, and `.objekte/collections`.
- Do not move construction work into the first query to improve the build timer. Do not use verified-result caching to conceal query regressions.

The desired outcome is the existing feature set with approximately baseline in-memory query and construction performance, faster stored opening, and bounded resource consumption. The current regressions are not an accepted change to that contract.

## 2. Reviewed revisions and evidence

| Role | Revision |
| --- | --- |
| Original performance baseline | `da8fde68472b8646a63a05b3b3654c58de877301` |
| Measured implementation and previous code review | `389375b06d06e96229df60eb064b21dc81ac3fba` |
| Newly reviewed branch head and evidence archive | `a7cdb101cb9340cd61fcb9e498194e947e8d1f23` |

The new head archives measurements and a performance issue. Its `crates` tree is identical to the previously reviewed implementation: tree `a9c07434cf337e116c9601bffe9d12d2e8b099d0`. Therefore the four earlier implementation findings remain applicable. The archive provides additional evidence and exposes additional acceptance gaps.

Primary evidence:

- [Integration diagnostic report](https://github.com/Simon-Martens/objekte/blob/a7cdb101cb9340cd61fcb9e498194e947e8d1f23/bench/results/hybrid-index-storage/diagnostic-20261113/REPORT.md).
- [Paired construction trials](https://github.com/Simon-Martens/objekte/blob/a7cdb101cb9340cd61fcb9e498194e947e8d1f23/bench/results/hybrid-index-storage/diagnostic-20261113/paired-builds/trials.json).
- [Sealing diagnostic](https://github.com/Simon-Martens/objekte/blob/a7cdb101cb9340cd61fcb9e498194e947e8d1f23/bench/results/hybrid-index-storage/diagnostic-20261113/seal-profile.log).
- [Recorded performance issue](https://github.com/Simon-Martens/objekte/blob/a7cdb101cb9340cd61fcb9e498194e947e8d1f23/docs/ai/issues/perf/hybrid-index-storage-construction-and-query-regressions.md).

This review inspected source and archived measurements and recalculated the full-campaign geometric means from the CSV files. It did not rerun the application or benchmarks. The executing agent must verify these findings against its current working tree, including any changes newer than the reviewed head.

### 2.1 Confirmed measurements

The paired construction/open/flush measurements use seven fresh-process trials on the configured Musenalm fixture with 148,486 records, ASCII indexing, and 32 workers. Opening was measured with warm operating-system caches.

| Measurement | Baseline | Candidate | Assessment |
| --- | ---: | ---: | --- |
| Complete memory construction | 1,002.38 ms | 1,710.94 ms | +70.7%; fails construction gate |
| First construction batch, median from trial data | 337.16 ms | 380.50 ms | +12.9%; track progressive availability |
| On-demand stored opening | 652.35 ms | 386.53 ms | 40.7% faster, opening only |
| Fully resident stored readiness | 652.35 ms | 487.63 ms | 25.2% faster, readiness only |
| First complete flush/save | 388.07 ms | 754.30 ms | +94.4%; substantial persistence overhead |
| Retained RSS after construction | 537.8 MiB | 650.4 MiB | +20.9%; fails memory gate |
| Peak RSS during construction | 547.0 MiB | 706.4 MiB | +29.1%; fails memory gate |
| Retained RSS after fully resident open | 312.4 MiB | 317.0 MiB | Within 10% budget |
| Retained RSS after on-demand open | 312.4 MiB | 264.8 MiB | Lower |
| Stored size | 132.71 MB | 128.31 MB | 3.3% lower |

The temporary sealing diagnostic records about 469.8 ms writing owned segments and 173.7 ms decoding/validating them: 643.6 ms across eight shards. The 131,900-record shard accounts for 270.9 ms writing and 154.2 ms decoding. This was a separate instrumented run; do not subtract its timings mechanically from the acceptance median or promise a particular recovered time. It establishes that sealing deserves first priority and that removing decoding alone will not recover the entire regression.

The 220-query campaign, with ASCII and Unicode profiles and five repetitions, gives these verified geometric means of candidate/baseline per-case medians:

| Candidate mode | Ratio | Assessment |
| --- | ---: | --- |
| Memory only | 1.0819 | Fails 1.05 limit |
| Fully resident stored | 1.1427 | Fails 1.05 limit |
| On-demand stored, warm workload | 1.1775 | Fails recorded 1.05 limit |

The archive reports matching result digests in all modes, zero failed samples, and zero verified-result-cache hits. Retain that correctness evidence and reproduce it after changes. No individual code hotspot has yet been assigned an exclusive measured share of the aggregate query regression.

## 3. Repair 1 — Construct immutable readers without reopening freshly encoded segments

Priority: highest. This addresses the first original finding.

Relevant code: `hybrid_segment_codec.rs::seal_segment_owned`, `write_segment_inner`, `decode_segment_impl`; `segment_metadata.rs::encode_records`, `validate_records`, `encode_sort_data`, `decode_sort_data`; `lifecycle.rs::seal_new_owned_segments` and `publish_prepared`.

[Owned sealing implementation](https://github.com/Simon-Martens/objekte/blob/a7cdb101cb9340cd61fcb9e498194e947e8d1f23/crates/core/src/hybrid_segment_codec.rs#L462).

### Observed problem

Memory construction builds the existing index structures, encodes a complete segment into owned bytes, opens a `SegmentReader`, then calls `decode_segment_sealed`. That reconstructs record/file metadata, lookup maps, sort data, and relation/identity data that already existed in the builder.

`encode_records` invokes `validate_records` and discards its returned lookup maps and domain bitmaps. That validator also allocates path sets and per-file slot vectors. Decoding later reconstructs these structures again. Sort data receives validation on both sides of the round trip. Section buffers coexist with a final combined buffer and the original segment during parts of sealing.

### Required implementation

1. Introduce a consuming finalization path for completed builder output. Transfer existing record tables, file mappings, identity mappings, sort columns, relation metadata, and domain information into the published segment wherever their invariants already hold.
2. Encode posting payloads and ordered blocks once. Produce section offsets, posting descriptors, sparse directories, and validation state as part of that encoding.
3. Construct immutable readers from the produced buffers and descriptors without invoking the external-input decoder over the same segment. Preserve the common query interfaces and format semantics across memory and disk modes.
4. Use an internal validated-construction type or equivalent explicit ownership contract. Only the builder/finalizer may create it. Do not make a public unchecked-reader API or accept a caller-supplied trust flag for arbitrary bytes.
5. Preserve external disk validation. Checks of untrusted offsets, lengths, ordering, bindings, and record domains must remain at the relevant input boundary.
6. Avoid deep cloning solely to obtain ownership. An older pinned view may legitimately retain its own generation; use shared immutable ownership or copy-on-write only where necessary. Published readers must not borrow mutable builder storage.
7. Reduce repeated buffer growth and copies where profiles demonstrate cost. Pre-size buffers or transfer section allocations where practical. Do not add a second complete resident representation as a shortcut.
8. Audit both synchronous and incremental fresh disk-build paths. The incremental completion path currently flushes and installs committed sealed segments after construction; the synchronous path builds unsealed, writes, and reopens. Avoid rebuilding equivalent metadata at each transition while honoring the selected residency policy.

### Verification

- Compare results before sealing, after owned sealing, after flush/reopen, and after an update.
- Include logical records, raw fallback records, multiple records per file, empty collections, relations, custom bindings, and sort columns.
- Instrument calls/bytes to show that fresh owned publication does not traverse the external decode path and that metadata is not recreated solely for publication.
- Verify cancellation, publication conflicts, and pinned older readers do not expose partially constructed segments.
- Keep all required finalization inside complete-construction timing and before the corresponding publication becomes ready.

## 4. Repair 2 — Validate each invariant at its proper boundary

Priority: highest, coordinated with Repair 1. This addresses the second original finding.

Relevant code: `mutation_segment.rs::into_indexed_profiled`, `bigram_dictionary.rs::append_bigram_index`, `posting_dictionary.rs::append_visited`, `bigram.rs::validate_domain`, and segment/dictionary opening.

### Observed problem

The builder skips expensive subset checks when the logical domain covers all slots. Sealing does not preserve that proof. `append_bigram_index` runs `validate_domain`, then `append_visited` runs `is_subset(allowed)` again for each posting. Scalar posting packing also performs domain checks after builder validation. The fresh segment is then subjected to decoder validation.

### Required implementation

- Establish a validation matrix for builder output, owned sealing, external disk open, first external payload access, flush, and compaction.
- Separate slot bounds, allowed-record-domain membership, dictionary ordering, descriptor bounds, checksum integrity, and configuration compatibility. Reusing one proof must not accidentally suppress a different invariant.
- When the allowed domain is every slot and slot bounds have already been proved, reuse that fact rather than scanning every bitmap for membership.
- For mixed logical/fallback domains, perform the necessary membership check once for each produced payload/domain relationship. Bind reusable proofs to the immutable segment, descriptor, and relevant domain.
- Reuse writer-established ordering and descriptor bounds for freshly constructed dictionaries. Preserve checks needed to trust negative answers from external dictionaries: an absent lookup must not silently succeed because an unread key block is malformed or incorrectly ordered.
- Avoid rebuilding complete metadata maps merely to validate them. Return and retain necessary structures, or validate incrementally while constructing their authoritative representation.
- Perform checksum work once per required integrity boundary. Do not remove checksums from the file format merely to improve construction time.

### Verification

Exercise malformed/truncated payloads, out-of-bounds descriptors, invalid cardinalities, unsorted and duplicate keys, mixed-domain violations, corrupt routing keys, and absent-key/range queries over corrupt dictionaries. Results must remain errors or follow the specified rebuild policy; corruption must never become a false negative. Include a fixture that proves the full-domain shortcut is used without weakening mixed-domain checks.

## 5. Repair 3 — Persist sealed sections without decoding and re-encoding their postings

Priority: high. This addresses the third original finding.

Relevant code: `lifecycle_storage.rs::flush`, `hybrid_segment_codec.rs::write_segment_inner`, sealed `for_each_value` and bigram iteration, `generation_catalog.rs`, `managed_store.rs`.

[Current new-segment flush path](https://github.com/Simon-Martens/objekte/blob/a7cdb101cb9340cd61fcb9e498194e947e8d1f23/crates/core/src/lifecycle_storage.rs#L176).

### Observed problem

A first flush of a memory-built index takes the sealed segment through the general writer. Iteration materializes Roaring postings, validates them, and serializes them again. These reads also enter the decoded-posting cache. Subsequent flushes already reuse unchanged durable files; preserve that optimization.

### Required implementation

1. Add a path that writes existing immutable dictionary and posting sections directly. Reuse unchanged sort/relation sections where their encoding and references permit it.
2. Resolve the actual reference difference: memory sealing uses local file references, while the generation catalog supplies catalog references. Introduce stable indirection or rewrite only the affected metadata. Preserve distinct historical file revisions and current catalog references.
3. Account for section-relative versus file-relative offsets and checksum coverage. Rebase descriptors only where the format requires it. Do not blindly concatenate sections with invalid offsets.
4. Make persistence iteration independent of the query cache. Writing a new generation must not clone every decoded bitmap, populate the posting cache, or evict the user's useful query entries.
5. Preserve the existing crash-safe commit protocol, synchronization guarantees, locks, pinned readers, manifest publication, and cleanup ownership checks.
6. Distinguish no-op flushes, liveness-only updates, one new segment, catalog-history rebases, and full first persistence. Do not rewrite unaffected segment payloads in routine cases.

### Verification

Use counters to establish zero posting deserializations on direct persistence of already sealed, unchanged payloads. Compare reopened results, metadata identities, active masks, and checksums. Exercise failures around immutable file creation, synchronization, manifest replacement, and cleanup; the old committed generation must remain valid until the new one is published. Retain history-rebase coverage and test a pinned reader across that rebase.

Measure flush elapsed time, bytes read/written, decoded postings, allocations, and durability operations. The reported baseline snapshot save and managed flush must have their durability semantics stated; do not claim exact equivalence without checking them.

## 6. Repair 4 — Remove avoidable query decoding, allocation, and synchronization

Priority: high. This addresses the fourth original finding and the newly available query evidence.

Relevant code: `ordered_block.rs`, `posting_dictionary.rs`, `segment_format.rs`, `posting.rs`, `posting_values.rs`, `run.rs`, and relation/sort execution used by segmented indexes.

### 6.1 Dictionary lookup

Within-block offsets and binary search already exist. Keep them. The remaining issue is that binary search decodes owned keys using the generic codec at each comparison, and exact lookup can decode the selected entry again.

- Compare supported keys directly against validated encoded views, or use reusable scratch storage where borrowing is impractical.
- Avoid a heap allocation for every string-key comparison and a temporary serialized vector for every primitive bigram key during construction.
- Reuse the final search position and descriptor instead of repeating decoding.
- Preserve the existing exact typed comparator. Raw byte ordering is not a valid substitute for numeric ordering or ICU collation unless the encoding explicitly guarantees equivalence.
- Cover inclusive/exclusive bounds, reverse traversal, missing keys, strings, exact numbers, and sorted result tie-breaking.

### 6.2 Posting validation and operations

`with_validation` currently performs a mutex-protected hash lookup and an `Arc` clone on repeated accesses. First external access materializes a complete bitmap to validate it. Materializing a cached posting also clones the cached bitmap.

- Let builder-produced validated postings bypass redundant external-payload validation through the internal construction contract.
- Make repeated validation-state access inexpensive, for example with descriptor-indexed state. Choose the design after accounting for state memory across all descriptors; replacing a hash map with a large eager table is not automatically an improvement.
- Prefer borrowed or operation-oriented posting access for intersection, union, cardinality, and membership. Allocate a bitmap when an operation needs ownership, not because the reader interface always returns an owned clone.
- For external postings, validate safely once and retain proof for that immutable payload/domain. Keep malformed input handling and concurrent-reader behavior explicit.
- Do not restore complete mutable B-trees and bitmap maps alongside sealed data in fully resident mode merely to regain speed.

### 6.3 Decoded-posting cache

`retain_posting` selects an eviction victim with `entries.iter().min_by_key(...)` while holding its mutex. Each eviction scans the whole cache. Persistence can exercise this path while visiting every posting.

- Use an established bounded eviction structure, such as a clock or an O(1) LRU implementation, if the cache remains useful after the other repairs.
- Remove avoidable bitmap cloning during insertion and cache hits. Preserve reference lifetimes when readers borrow cached data.
- Exclude flush and bulk maintenance traversal from cache admission unless a measured policy justifies admission.
- Account for decoded containers, cache metadata, validation state, and multiple pinned generations. Current capacity is based on total encoded segment bytes divided by 20, capped at 8 MiB per segment; the report's description as a percentage of posting payload alone is imprecise.
- Correct manifests that state `decoded_cache_bytes: 0` while an internal decoded-posting cache is active. Distinguish cache categories and record effective limits and observed retained bytes.
- Do not begin by increasing the cache. The recorded experiment caching every tiny posting made performance worse and was reverted.

### 6.4 Broad relation queries and sorting

Prioritize Q167, Q175, and Q177 in addition to point lookups and bigram queries:

| Case | Operation |
| --- | --- |
| Q167 | `entry.year >= 1800 && year >= 1800` |
| Q175 | `entry.year >= 1800 SORT BY numbering ASC` |
| Q177 | `entry.year >= $year`, with `year = 1803` |

In the full campaign, memory Q175 ASCII takes about 6.478 ms versus 3.068 ms baseline; memory Q177 Unicode takes about 7.106 ms versus 2.885 ms. These are substantial absolute regressions despite their dilution in the aggregate mean. They cannot be dismissed as tiny-query measurement noise.

Profile target range evaluation, reverse relation posting access, liveness filtering, slot translation, bitmap combination, count calculation, and sorted page selection separately. Preserve index-proven matches and avoid falling back to source reads. Determine which layers changed before changing the planner or introducing another cache.

The archived diagnostic counters need care: the segmented relation path increments `relation_reverse_postings_visited` by `slots.len()`, and the compared direct path reports a different unit. Candidate traces can display an estimated scanner path while execution counters show entirely index-proven matches. Define and expose comparable posting-list and posting-member counters and clearly separate estimated plans from executed work. Do not infer a 25-fold work increase from 95,153 versus 3,821 until those units and backends match.

## 7. Repair 5 — Bring construction memory within budget

Priority: high; implement with Repairs 1–3 and verify independently.

The construction memory increase is an additional failed gate, not just a side effect to ignore once elapsed time improves. Lower RSS after stored opening does not establish acceptable memory behavior for memory-only construction.

- Measure ownership at extraction completion, packing, publication, builder release, first query, and after pinned readers are released.
- Separate live allocated bytes, allocator-retained memory, process RSS, mapped pages, and peak RSS. Elevated RSS alone does not prove a leak.
- Find overlapping lifetimes of builder maps, temporary section buffers, final segment buffers, decoded metadata, catalog copies, and previous publications.
- Transfer ownership and release temporary representations at segment boundaries. Avoid keeping both complete metadata/catalog representations where immutable sharing suffices.
- Do not retain all decoded postings as an undocumented fully resident optimization. The byte-resident representation, bounded decoded cache, and query temporaries must be budgeted together.
- Check both the large shard and many small shards. Preserve first-batch responsiveness; the current first-batch median has also increased.

Acceptance remains no more than 10% growth in retained and peak memory on matched fixtures, subject to the original specification's exact accounting rules. Record allocator effects rather than using a process-wide allocator purge as the production solution.

## 8. Repair 6 — Make maintenance cost bounded by work, not just output size

Priority: required before task completion. Avoid combining this larger change with the first construction repair commit.

Relevant code: `lifecycle.rs::make_room_for_segment`, `lifecycle_compaction.rs`, `lifecycle_partition.rs::compact_source_parts_with_limits`, `mutation_segment.rs::compact_active_mapped`, and catalog-history maintenance.

### Confirmed issues

The implementation has segment-count admission consolidation and source-free compaction. The archive acknowledges missing independent byte-size and retired-ratio triggers and missing bounded maintenance evidence.

There is also concrete repeated work: `compact_source_parts_with_limits` creates selections of at most 2,048 records, but invokes `compact_active_with_postings` for each selection with `source.records.len()` as the allowed scan budget. `compact_active_mapped` then builds a source-sized slot map and loops over the source record and file tables for every output part. Metadata work therefore grows with both source size and number of output parts. Shared posting projection does not remove these repeated metadata passes.

### Required implementation

1. Build the source-to-output mapping once, and route records, files, diagnostics, and index contributions to their destination parts without rescanning the complete source for each part.
2. Bound maintenance by records examined, encoded/decoded bytes, allocations, and elapsed work per job or resumable batch. A 2,048-record output limit alone is insufficient.
3. Select affected segments/collections and preserve unaffected immutable segments. Avoid rebuilding every collection after a small number of deletions.
4. Implement the original specification's independent triggers for segment accumulation, segment bytes/size, and retired-record ratio. Use documented thresholds and hysteresis; prevent maintenance on every small update.
5. Keep active-only compaction distinct from coalescing small live segments. Append-only workloads also need controlled segment growth.
6. Preserve cancellation and compare-and-publish conflict handling. Do not restart an unbounded full rebuild repeatedly under sustained updates. Bound wasted work and leave newer publications intact.
7. Preserve source-free execution, file-revision identity, relations, diagnostics, sorting, and historical catalog correctness. Existing history-rebase behavior is useful and must remain covered.

### Verification

Test large single shards, many small shards, append-only growth, repeated edits/deletions, moves, multi-record files, pinned old readers, concurrent queries, cancellation, and publication conflicts. Record examined records and copied bytes, not just output sizes. Scaling from one output part to many must not introduce a complete metadata scan per part. Run sustained updates through several maintenance/history-rebase cycles and report foreground query p95 and synchronization lag.

## 9. Repair 7 — Preserve store error categories and recovery behavior

Priority: correctness and operability.

`ManagedStore::read_generation` returns string errors for both I/O failures and invalid content. The managed open paths map all of those failures to `IndexError::InvalidCache`. A permission failure or failed lock can consequently be reported as corruption. The current code returns the error; this review does not claim it currently deletes the store after such a failure.

- Introduce an internal typed error boundary distinguishing absent store/generation, incompatible format/configuration, corrupt content, I/O/access failure, and lock/publication conflict. Preserve useful path and underlying error information.
- Map those categories consistently through synchronous opening, incremental opening, explicit flush, background synchronization, and CLI/TUI/GUI status.
- Rebuild only for the conditions authorized by the original recovery policy. Never treat permission denial, disk full, or lock failure as permission to delete or replace a store.
- Keep ordinary cache misses eligible for create/build and keep deprecated-format cleanup narrowly scoped. Unknown files and unsafe paths remain protected.
- Exercise interrupted writes, corrupt supported files, unsupported versions, permission/read-only failures, failed locks, missing files, symlinks, and synchronization failures.
- Verify that a failed background flush remains observable and does not report the current in-memory revision as durably synchronized.

## 10. Repair 8 — Correct benchmark comparability and complete acceptance evidence

Priority: establish the measurement contract first; run the final campaign after integration.

### 10.1 Newly identified tail-comparison mismatch

The archived `tail-30/baseline/run-manifest.json` command omits `--incremental`. The memory/resident/on-demand candidate commands include it. Baseline samples report 29 prepared-plan hits across 30 repetitions; candidate samples report zero. These measurements compare the direct repository backend against the incremental backend, not a matched incremental baseline/candidate pair.

The full 220-case baseline campaign does use `--incremental`, so this mismatch does not invalidate its independently confirmed aggregate regressions.

Required action:

- Rerun the 30-trial tail campaign using the same baseline and candidate backend, cache configuration, query mode, schema, and fixture.
- Keep a direct-versus-incremental comparison as a separately labeled diagnostic if useful.
- Report warm prepared-plan behavior separately from verified-result caching. Do not disable an optimization only on the baseline to make the candidate pass.
- Select tail cases before examining the new results. Include Q167/Q175/Q177 plus representative text, exact, range, sort, and no-match cases.

### 10.2 Revision and statistics provenance

The tail baseline manifest names `389375b06d...` as `code_revision` even though the retained baseline binary is identified as built from `da8fde6847`. This is consistent with recording the execution checkout rather than the binary's build revision. Record both explicitly, together with the executable SHA-256 hash, build flags, compiler, and dependency lockfile identity. Never infer build provenance solely from a binary filename.

Use one defined median and percentile calculation across CSV, reports, and gate evaluation. With even sample counts, the CSV median and report median currently differ slightly; document and eliminate that ambiguity. Retain raw samples and warm-up policy. Do not mix medians from different runs as one acceptance result.

### 10.3 Phase accounting

Sealing currently occurs from `publish_prepared` and is charged to `publication_install_ns`. Split preparation/sealing from actual publication installation so reports identify the expensive work accurately.

Record at least: posting and dictionary encoding; metadata encoding; validation/domain scans; checksum processing; buffer allocation/copying; external decode where applicable; publication; and flush/reopen stages. Distinguish exclusive elapsed time from nested timers and summed parallel worker time. Instrumentation must be optional, and acceptance measurements must use the normal release configuration.

Do not instrument the source parser unnecessarily: the new evidence identifies added sealing work. Use focused profiling first, then broaden only if unexplained residual cost remains.

### 10.4 Required final matrix

| Area | Required evidence |
| --- | --- |
| Construction | Seven paired fresh-process trials; first batch and complete readiness; same backend; sealing included |
| Queries | All 220 cases, ASCII and Unicode; matching counts, identities, ordering, and digests; result cache disabled |
| Query tails | At least the established 30 repetitions; matched backends and cache settings; explicit percentile method |
| Stored opening | Opening-only and opening through first complete representative query, measured separately and together |
| Access temperature | Warm and controlled cold-index trials; state whether source-file pages are warm or cold independently |
| Residency | Memory-only, fully resident disk-backed, on-demand; constrained-memory workload and page-fault/I/O evidence |
| Memory and size | Retained and peak RSS, estimated live allocations where available, cache retention, stored bytes |
| Inference | Independent inference-enabled run, progressive availability, and final query correctness |
| Dataset scale | Established large configured fixture, ordinary/default configuration, and small fixtures |
| Updates and flush | Add/update/delete/move; no-op and incremental flush; first full persistence; disk reopen |
| Maintenance | Sustained updates, compaction, catalog-history rebase, pinned readers, and foreground latency |
| User interfaces | CLI, TUI, and desktop storage policy parity; errors, synchronization state, and resident option |

Prefer file-scoped cold-index testing over disturbing host-wide caches. State exactly what was evicted and verified; a fresh process alone does not establish cold disk access. Coordinate benchmarks so compilation and other campaigns do not compete for CPU or I/O.

The fully resident option means the required index bytes are owned in memory; it does not imply an operating-system guarantee against swapping. Its readiness and first-query measurements must include the work promised by the interface. Opening-only gains do not close the opening-plus-first-query gate.

### 10.5 Completion gates

Apply the authoritative original gates without relaxing them silently. The current recorded limits include:

- Complete construction no slower than 1.05 times baseline.
- Aggregate query ratio no greater than 1.05 in the modes covered by the task, including the recorded warm on-demand campaign.
- Representative p95 no greater than 1.10 times matched baseline, subject to the original treatment of very short/noisy queries. Report absolute deltas as well as ratios.
- On-demand opening improvement of at least 20%, and fully resident readiness improvement of at least 10%, with the required first-query evidence also completed.
- Retained and peak memory, and stored size, within the original 10% growth budget.
- Exact semantic parity, preserved update/durability behavior, bounded maintenance, and successful required tests.

First-flush parity was not assigned a new numerical hard gate in this repair document. Remove the demonstrated redundant transcoding, measure the remaining cost under stated durability semantics, and report any material residual regression. Do not advertise the current near-doubling as an accepted trade-off.

If a gate remains unmet, keep the task incomplete and explain the remaining measured cost and design limitation. Do not move the task to `done`, hide work beyond a timer, cherry-pick passing query subsets, or compensate for one failed gate with gains in another.

## 11. Execution sequence, ownership, and commits

Continue in `/home/simon/source/objekte-integration-hybrid-index-storage`, or the existing integration worktree recorded in task notes. Keep the original checkout on `master` and preserve all user changes. Subtask worktrees belong beside the original checkout under `/home/simon/source`, never under `/tmp`.

The supervisor owns interfaces, integration, and acceptance. Delegation is optional; the following boundaries permit independent work after the construction/validation contract is agreed:

| Workstream | Ownership | Dependency |
| --- | --- | --- |
| Construction and validation | Repairs 1, 2, and construction memory ownership | Establish first |
| Persistence | Repair 3 and related metadata reference handling | Agreed sealed representation |
| Query execution | Repair 4 and query profiling | Agreed reader/validation interfaces |
| Maintenance and recovery | Repairs 6 and 7 | Construction/persistence interfaces; serialize edits to shared lifecycle files |
| Verification | Repair 8, benchmark scripts, evidence aggregation | Can prepare immediately; final runs follow integration |

For delegated work use `task/hybrid-index-storage/<descriptive-subtask>` branches from a recorded integration commit and separate sibling worktrees. Only the supervisor writes to the integration worktree. Avoid concurrent ownership of `hybrid_segment_codec.rs`, `lifecycle.rs`, or the same benchmark artifacts. Each agent supplies its pushed commit, tests, measurements, and remaining limitations. Push immediately after every commit and integration merge, following repository workflow.

Suggested commit order:

1. Correct benchmark provenance/backend matching and add narrowly scoped phase accounting. Preserve the original archive unchanged; write new results under a new directory.
2. Add the internal validated-construction contract and consuming owned finalization; remove metadata reconstruction. Run focused correctness and construction/memory measurements.
3. Remove repeated domain/ordering validation where proofs are reusable. Recheck corrupt-input coverage and construction timing.
4. Persist sealed sections directly with correct catalog-reference handling. Validate crash behavior and measure full/incremental flushes.
5. Remove lookup allocations and unnecessary posting clones; repair cache eviction and validation-state access. Measure focused cases, then the complete query suite.
6. Correct source-sized repeated compaction scans and complete maintenance triggers/budgets. Validate sustained updates and pinned generations.
7. Preserve typed store errors and consistent status/recovery behavior. Run failure scenarios and interface smoke tests.
8. Run the final combined workspace tests and acceptance matrix. Publish the repair report and exact reproduction commands on the integration branch.

Use coherent commits rather than forcing these steps into exactly eight commits. Keep causally separate performance changes measurable. Run short diagnostic benchmarks after relevant steps; reserve repeated full campaigns for meaningful integrated candidates. Never run competing performance campaigns in parallel.

## 12. Required final report

The supervisor must provide:

1. Reviewed and final implementation revisions, integration branch/worktree, and contributing commits.
2. A disposition for each Repair 1–8: implemented, verified, or still incomplete, with code references.
3. Before/after construction, phase timing, first-batch availability, memory, opening, first-query, query distribution, persistence, and maintenance results.
4. Exact benchmark binary provenance, commands, environment, fixtures, cache settings, raw samples, and statistics definitions.
5. Correctness and failure-test results, including data ordering, mixed record domains, corruption, interrupted writes, concurrent readers, and update/compaction cycles.
6. Any unresolved performance limit, its measured cause, and why the task remains incomplete if a required gate fails.

Store generated reports in `docs/ai/generated/` or the established benchmark result tree. This supporting document belongs to the existing task and should be linked from its authoritative specification. Preserve uncommitted user task documents according to `docs/ai/README.md`. Move the authoritative task to `done` and append the implementation report only when all required work is integrated, validated, committed, and pushed. Leave the final integration branch and worktree available for the user's merge; do not merge into `master`.


## Implementation report

- **Task identifier and closure date:** `hybrid-index-storage` repair, 2026-11-14.
- **Base and integration:** Reviewed branch head `a7cdb101cb9340cd61fcb9e498194e947e8d1f23`; original performance baseline `da8fde68472b8646a63a05b3b3654c58de877301`. Work was integrated on `integration/hybrid-index-storage` in `/home/simon/source/objekte-integration-hybrid-index-storage`, never on `master`.
- **Implemented:** Consuming sealing for memory and incremental construction, checked constructed/external reader boundaries, direct sealed-section persistence, exact dictionary lookup and bounded posting cache, source-free managed opening, typed top-level store errors, maintenance triggers and partitioned-root batching, and full result-digest campaigns. Repair commits include `70a055c002`, measured checkpoint `e15b3f5c4b`, error-classification fix `a9ab5ec284`, allocation/maintenance follow-up through `ca183a1ed8`, generation series `8ca2851ab8`, and query series through `293a10498c`. The completed allocation follow-up has its own [implementation report](objekte_hybrid_index_allocation_and_query_performance_specification.md).
- **Validation:** `cargo test --workspace --locked --quiet` and `cargo clippy --workspace --all-targets --locked --quiet` passed again on combined integration revision `74cab22411`; Clippy emitted warnings. Focused corruption, update, compaction, reopened-store, and pinned-reader tests and their boundaries are recorded in the linked reports. Global formatting checks have existing drift.
- **Benchmarks:** Seven alternating construction/open/flush pairs and 220 queries × ASCII/Unicode × 30 repetitions were recorded at the repair checkpoint with exact command lines, binary hashes, raw samples and digests in [the repair report](../../../bench/results/hybrid-index-storage/repair-20261113/final-e15b/REPORT.md). Later `bench_musenalm` campaigns, construction pairs, maintenance stress, and focused ABBA query tails are in [the allocation report](../generated/hybrid-index-allocation-query-performance-20261114.md), [generation report](../../../bench/results/hybrid-index-storage/maintenance-20261114/REPORT.md), and [query report](../../../bench/results/hybrid-index-storage/query-optimization-20261115/REPORT.md). The final query campaign matches all 440 digests; it is not claimed to satisfy every original numeric gate.
- **User-approved deviation and remaining work:** The user directed that remaining performance evidence and targets not block documentation closure and that unfinished work become issues. The base-containing compaction route still lacks bounded batches and charged-work reporting; see [issue](../issues/feat/hybrid-index-storage-base-compaction-budget.md). The complete failure-injection/typed-error audit remains [an issue](../issues/feat/hybrid-store-failure-validation-coverage.md). The fresh synchronous disk build's older sealing route remains [a performance issue](../issues/perf/hybrid-index-storage-fresh-disk-sealing-path.md). These are transferred obligations, not verified repairs. The integration worktree remains for review; the final query subtask worktree was clean, merged, pushed, and removed.
