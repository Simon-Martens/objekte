# Objekte RocksDB bulk construction repair specification — third repair

Date: 15 November 2026  
Task identifier: `rocksdb-bulk-construction-repair`  
Integration branch: `integration/rocksdb-bulk-construction-repair`  
Integration worktree: `/home/simon/source/objekte-integration-rocksdb-bulk-construction-repair`  
Inspected implementation base: `bc8696eb485d00221ad78e53fca70ed273f72b3c`  
Parent branch: `integration/rocksdb-admission-repair`  
Historical first-repair revision: `08591e8452132f392201cfdb745d958f502a556b`  
Pristine native control: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`

## 1. Assignment and controlling decision

Replace the initial-build algorithm that repeatedly performs transactional read/modify/write on partly constructed bitmap blocks. Implement bounded block accumulation with explicit ownership, complete bitmap-image writes, and atomic publication of physical-file groups. Remove per-membership encoded-key allocation, redundant discovery persistence, and finalization reconstruction.

The user wants performance comparable to, and ultimately faster than, the native implementation. The historical deficit is approximately 33–35 times. Improvements of a few percent, or another modest reduction that retains the same repeated-work pattern, are insufficient evidence of feasibility.

This document is the controlling third repair specification. When assigned to an implementation agent, it authorizes the changes below and supersedes conflicting construction requirements in the first and second repair specifications. Do not implement both specifications as independent competing rewrites. Retain their compatible correctness, resource-accounting and validation requirements, restated here.

The important change is algorithmic: a newly allocated block starts empty in its owner's memory and is built there. The constructor must not repeatedly ask the database for the bitmap it is itself constructing. Partial publication is supported, but it is an explicit exception with retained ownership and measured rewrite cost.

Completion has two separate outcomes:

1. Implementation correctness, boundedness, and removal of the specified repeated work.
2. Performance feasibility against matched native controls.

A correct and fully measured implementation may still fail feasibility. Report that result honestly. Do not mark missing implementation requirements as a performance failure to invoke an early exit.

## 2. Current evidence and starting revision

The first repair measured:

| Metric | Native | First repair |
| --- | ---: | ---: |
| Caller construction | 1.079 s | 35.650 s |
| Resident bytes | 395,444,224 | 1,093,632,000 |
| Peak bytes | 404,967,424 | 2,432,851,968 |

Its workload comprised 148,486 files/records, 193,297,230 source bytes, and seven collections. It processed 56,275,908 gram memberships, 5,219,688 admission keys, 2,692,703 bitmap reads, and 854,242,205 encoded admission bytes. Discovery took 1.472 seconds and finalization 1.278 seconds.

The 107.006-second historical “database” interval is summed worker elapsed time, including application processing and possible waiting. It is not RocksDB CPU time. The approximately 112.6 million transient FTS key-buffer allocations were inferred from two allocations per membership in that source, not measured with an allocator profiler.

During specification preparation, the second-repair branch appeared at `bc8696eb...`, two commits ahead of the first repair. Its inspected changes already include batch-wide BlockDelta bitsets instead of per-file offset-tree merging, worker-owned contribution scratch, iterator changes, and a version/digest change. It still constructs encoded Vec keys per membership and retains bitmap database read/modify/write. No completed second-repair performance report was found at that revision. Do not attribute the first repair's timings to this new revision.

At implementation start:

1. Read repository `AGENTS.md`, `docs/ai/README.md`, relevant task notes and existing repair reports.
2. Fetch and inspect the parent branch. If its current committed head is a descendant of the inspected base, use that head and record its exact SHA and diff. Do not discard newer fixes or wait for unrelated work.
3. Pin the actual base once. Do not silently move the implementation base while work proceeds.
4. If another agent is still implementing the second repair, preserve its branch and worktree. This task replaces the remaining construction design; integrate useful subsequently completed fixes explicitly, with reviewed commits.
5. Create the new sibling integration worktree. Leave the original checkout on its existing main branch. Never reset user changes or automatically merge into master.

A new repair is explicitly authorized by the user's assignment. Earlier reports saying “do not start another cycle” do not block this task.

## 3. Fixed choices and exclusions

| Area | Required choice |
| --- | --- |
| Engine and binding | Keep locked rocksdb 0.25.0 / librocksdb-sys 0.19.0+11.8.1 |
| Database owner | Keep TransactionDB and the existing snapshot mechanism |
| Fresh-build write API | Atomic WriteBatchWithTransaction<true> through TransactionDB::write_opt |
| Later mutation path | Keep transaction-based version checks and protected bitmap read/modify/write |
| Physical layout | One column family, existing namespace ordering, 4,096-slot bitmap blocks |
| Slot assignment | Dense monotonically assigned slots per collection; no padding to manufacture independence |
| Source handling | Existing adapters, schemas, exact numeric semantics, case folding and source fingerprints |
| Initial construction | Exclusive construction authority; concurrent external mutation is prohibited |
| Publication | All collections and index families for each physical file in one atomic batch |
| Normal batch objective | Complete the current 4,096-slot range of an anchor collection |
| Initial publication | Publish the first completed physical file promptly |
| Memory target | 64 MiB soft target for all construction work and retained unfinished blocks |
| Durability | Preserve existing memory/file WriteOptions; do not add or remove durability for a favorable comparison |
| Engine options | Preserve cache, compression, memtables, background jobs and allocator configuration |
| Format | Use version 4; if already occupied at the actual base, choose the next unused version and record why |

The pinned Rust binding exposes `TransactionDB::write_opt(WriteBatchWithTransaction<true>, &WriteOptions)`. This removes the application transaction/get/put loop; it does not promise that TransactionDB performs no internal locking. Do not invent a public “skip concurrency control” option or reach into private FFI.

Do not switch to plain DB, close/reopen the engine at readiness, introduce external SST ingestion, Merge operators, multiple databases, a second permanent index representation, sparse slot allocation, or new dependencies. Those are separate experiments. Do not enlarge buffers or disable compression in this campaign.

Do not implement production query planner, CLI, desktop, inference, crash resume, slot reuse, or full lifecycle migration. Existing implemented query primitives, source-free fixture mutations, and retained snapshots must remain correct.

The fresh-build authority and scheduling changes deliberately replace the earlier requirement to lock/read existing bitmap rows for every admission. They do not weaken protection for mutations of an existing index.

## 4. Required construction architecture

Separate these responsibilities with explicit types and interfaces:

| Component | Responsibility |
| --- | --- |
| DiscoveryInventory | Immutable discovered physical files, captured revisions, paths and collection memberships |
| PreparationWorker | Parse/extract once per required adapter configuration; emit compact forward deltas and record-associated reverse facts |
| SlotCoordinator | Assign deterministic dense collection slots and group complete physical files |
| ConstructionCohort | Bounded group of complete physical files and all their collection memberships |
| BlockOwnership | Exclusive ownership and generation of every touched collection/block range |
| ConstructionAccumulator | Current complete image of each unfinished bitmap key, with dirty tracking |
| PreparedWritePlan | Canonical encoded actions, receipt digest, summary and ownership dependencies |
| BatchWriter | Build and submit one atomic native WriteBatch; report completion |
| BuildSummary | Commit-confirmed collection/coverage counts and inventory-derived fingerprints |
| FinalizationWriter | Stream required directories/statistics and publish the complete header |

These names are illustrative interfaces, not permission to build a general framework. Keep the implementation within the prototype modules and small shared extraction helpers. Do not change the native production path except to expose a reusable, behavior-preserving helper.

One physical file is indivisible even if it belongs to several collections or contains many records. A cohort may include several block ranges. Preparation chunks, ownership ranges, write batches, and publication units are distinct concepts; do not use the old 256-file preparation chunk as the automatic publication boundary.

## 5. Remove per-membership allocation and reconstructive planning

### 5.1 FTS extraction and forward accumulation

Preserve the native gram generator, normalization and segment boundaries. Reuse the native 64-record word-accumulation approach where possible.

For ASCII, use reusable numeric arrays indexed by the existing 95×95 gram domain, with touched-index tracking. For Unicode, use typed numeric keys in the existing standard map machinery. Do not allocate byte-encoded database keys while processing memberships.

A preparation worker should produce compact tuples of logical gram identity, local word/block position and membership bits. The coordinator or block owner combines words into the canonical 4,096-slot block. Where local slot ranges do not align with destination words, implement the two-word shift with checked boundaries; do not iterate each set bit merely to remap an entire packed word.

Keep physical-file range descriptors alongside packed words. If a preparation chunk straddles cohort boundaries, mask/split its word ranges without dropping the remaining files or copying all their facts. Keep its ownership charge until every retained portion is released. Different collection memberships requiring different adapter configurations may require separate extraction; count and explain those passes. Repeated passes caused solely by admission grouping are forbidden.

Reverse per-record gram lists are still required for source-free deletion. Collect them during the same gram traversal. For ASCII, reusable generation/touched arrays can suppress duplicates before appending unique identifiers. For Unicode, use reusable numeric deduplication storage. Sort only the unique list needed by the reverse codec, not a vector containing every repeated occurrence in a long text. Handle generation-counter wrap by clearing the marker array.

Do not obtain reverse facts by reparsing the source or by later decompressing/reconstructing the forward index. Do not construct a complete native repository as an intermediate.

### 5.2 Other posting families

Use typed identities for state, length and value postings. Preserve exact ScalarKey and Number ordering/equality; no conversion to float or lossy string keys.

FTS and common state postings can use 64-word bitsets. Do not allocate a 512-byte dense bitmap for every singleton distinct scalar value: use an inline small sorted offset vector for sparse structured postings and promote to a dense block when appropriate. A default inline capacity of four offsets and promotion at 256 memberships matches the existing sparse/dense boundary; preserve canonical encoded output regardless of the accumulator representation.

Group variable-length values once per distinct logical posting key within the owned range. Borrow, intern or move data so the same string/number is not cloned for every occurrence. No new hash dependency is necessary.

Deduplication must preserve set membership semantics while retaining relation occurrence multiplicity where positions are significant.

### 5.3 Linear record association

Prepared facts must identify each record's contiguous range or an equivalent once-built offset table. Contribution encoding visits only that record's values, lengths, states, sorts, relations and grams.

Remove per-record filters across a whole file's fact arrays. Require O(records + facts) association work, apart from necessary distinct-key ordering. Keep schema and binding descriptors shared at runtime/chunk scope.

Reverse grams remain stored once in RecordGrams and excluded from the compressed contribution payload. Move or borrow their lists; do not clone them through intermediate maps.

### 5.4 Encode an executable plan once

After aggregation, encode each dirty distinct physical key once for that publication. Construct an action vector with explicit put/delete kind and arena ranges for keys and values. Sort it once outside database calls.

Remove reconstruction of all keys through another BTreeSet and all searches across unrelated posting maps. The writer consumes actions directly; it does not recover their meaning by looking them up again.

Compute the receipt digest by streaming the encoded plan, file identities, captured versions/fingerprints, slot ranges, coverage and ownership generation under a versioned domain separator. Use explicit tags and lengths. Exclude the receipt's own digest field. No allocating serialization of all prepared fact vectors solely for hashing.

Do not allocate an individual key buffer and then a second namespace-prefixed copy. Encode directly into the reusable plan arena.

## 6. Cohort formation and publication policy

### 6.1 Deterministic normal grouping

Assign slots in deterministic discovery/preparation order, independently of writer completion. All memberships of a physical file are assigned together.

After the first-file publication, form cohorts using the following rules:

1. The anchor is the first collection in the cohort that receives records, using deterministic collection order.
2. The desired endpoint is that collection's next 4,096-slot boundary.
3. Add whole physical files until the anchor reaches or crosses that endpoint.
4. A file crossing the endpoint stays intact in the same cohort. Never split a physical file's publication.
5. Other collections touched by the cohort may end in partial blocks; retain those unfinished block images for later cohorts.
6. Seal at end of input, 4,096 physical files, projected 32 MiB encoded plan, 262,144 operations, publication-age expiry, or memory pressure, whichever occurs first.
7. A single file exceeding an ordinary limit is processed as an explicitly recorded exclusive oversized-file exception. Ordinary large groups must be sealed smaller, not declared exceptions.

The existing 256-file/8-MiB preparation-job limits may remain as extraction scheduling limits. They must not force publication every 256 files. The earlier 2,048-record admission cap is superseded.

The 32 MiB limit is a ceiling, not a reservation that every cohort consumes. Account for the encoded plan and the native WriteBatch copy together before admitting writer work.

### 6.2 Early and age-limited publication

The first preparation job contains exactly one physical file. Once it is prepared and mapped, publish all its memberships without waiting for another file or a full block.

For subsequent cohorts use a default maximum accumulation age of 100 ms, measured from the first accepted prepared file in the open cohort. On expiry, seal at the next complete-file boundary. This bounds deliberate accumulation delay; it does not guarantee total latency under slow source reads or database writes. Do not flush on every worker completion.

Use event-driven deadline handling in the coordinator. Account for time waiting on source preparation, ownership, budget, encoding and database completion separately. Record the seal reason for every cohort.

### 6.3 Complete images and unfinished ranges

For a never-published block, create the accumulator from empty memory. For a published unfinished block whose owner is retained, continue from that authoritative image. Track which logical keys changed since the last successful publication.

Encode and write only dirty keys, but write their complete cumulative bitmap images. Do not write only a delta under the existing complete-block format.

Once a collection's slot assignment passes a block's end and all its publications have committed, the block can never receive another initial-build record. Release its accumulator. Every ordinary complete block key should therefore be written once, except keys included in an explicitly counted earlier partial publication.

The first-file publication can cause a second write of affected keys when that first block fills. That is an intentional, small exception; it does not justify sixteen regular rewrites per common key.

A prepared later cohort must never overwrite a block using an image missing an earlier cohort's committed bits.

## 7. Ownership, concurrency and atomicity

### 7.1 Construction authority

Introduce an internal BuildSession or equivalent exclusive construction capability. It owns the store generation, immutable discovery inventory, slot allocator, cohort identities and publication scheduler.

Only that authority can submit fresh-build batches. Public serial mutation APIs must reject Building/Finalizing state and cannot obtain a raw unrestricted transaction through the build handle. Keep low-level test hooks private.

The store begins empty for this build identity. A fresh-build operation is not a shortcut for opening an arbitrary existing database and assuming its keys are absent. Reject a nonempty/incompatible build destination. Keep configuration, alphabet, root and codec checks.

The build authority replaces per-cohort header/file-version database reads with validated immutable assignments. File versions still come from captured inventory, not record offsets. Post-readiness mutation continues to validate current database versions transactionally.

### 7.2 Block leases and dependent publications

Use a conservative ownership resource `(collection_id, block_number)` across all bitmap families. At most one active writer/plan generation owns that range.

Acquire all a cohort's required ranges as one scheduler operation or grant none. Do not acquire one range and wait while holding it for another: overlapping collections otherwise introduce deadlocks.

Transfer an unfinished range only after its preceding publication has a terminal successful acknowledgement. Transfer the authoritative accumulator and its generation with the lease. A later owner must not independently reconstruct an empty block.

Cohorts touching disjoint ranges may encode/write concurrently. Cohorts sharing a range wait in the scheduler, outside all database operations. Do not add a mutex around every database write.

A coordinator-owned resource map is allowed for short scheduling operations. It must not be held while extracting, encoding, waiting for memory, or calling RocksDB.

### 7.3 Non-bitmap key conflicts

Block independence alone does not prove all keys are disjoint. In particular, repeated logical IDs may target the same LogicalIds key even when records have different slots.

Register cohort resource/key descriptors in deterministic cohort order before deciding which can run. Their identities come from mapped prepared facts; registration must not require the final bitmap image or wait for a predecessor's lease. After registration and dependency resolution, grant leases and let a worker finish accumulation/encoding. Track dependencies for non-unique metadata keys among active/pending cohorts, especially LogicalIds. Keep this registry bounded by the global in-flight budget and release entries after acknowledgement. Do not build another whole-corpus logical-ID index just to schedule writes.

Namespaces whose keys include a unique physical file, reserved record slot, relation occurrence or cohort ID can use those proven ownership identities. Final shared collection/field/statistics rows are not written by concurrent cohorts.

Preserve the native contract for repeated source IDs. Establish it with an independent fixture at the start: records must not disappear merely because IDs repeat, and any representative lookup rule must be deterministic. Do not introduce a new rejection policy or allow completion order to select the winner.

Where an earlier not-yet-registered cohort could contain a conflicting key, a later cohort cannot bypass registration order. Out-of-order encoding and database completion are allowed only after dependencies have been established. A waiting cohort must not prevent the coordinator from registering independent work or receiving the acknowledgement that will release its dependency.

### 7.4 One atomic native batch

Each write plan contains all of the following for its complete physical-file cohort:

- Final file metadata, path mappings and final per-file state.
- Record metadata, identity and file-to-record mappings.
- Domain memberships and all required structured/FTS bitmap images.
- Reverse contributions and reverse grams.
- Sort and relation rows.
- One receipt covering every file, membership and slot range in the cohort.

Submit them with one atomic WriteBatch through the pinned public API. Readers using one native snapshot see either the complete cohort or none of it. A physical file cannot become Indexed before all its collections and index families appear.

The batch must not be split into visible fragments merely to meet a normal size limit. Seal the cohort earlier or use the single-file exception.

Never use highest completed cohort ID as a visibility watermark. Cohorts can finish out of order. Progress derives from committed receipts and visible record/file rows within the reader's snapshot.

### 7.5 Failure, replay and retained snapshots

The native batch is prepared once. Retry only recognized transient failures, at most four attempts with the existing 1/2/4-ms backoff. Keep ownership until the outcome is resolved.

After an ambiguous result, read the receipt: matching identity/digest means the batch is already committed; a mismatch is corruption/state error. If safely resolving absence and retry is impossible, fail the build rather than risk replaying an old complete image over later state.

The scheduler must prevent duplicate concurrent submissions for the same cohort. Successor leases are released only after receipt resolution. Explicit replay of an older completed cohort returns AlreadyAdmitted; it must never rewrite old bitmap images.

Account for every failed attempt and wait. Do not reparse, regroup, recompress or recompute the plan digest on retry. On terminal failure stop dispatch, join/cancel safely, and never return a Ready store. Existing committed cohorts remain atomically consistent; crash resume is still unsupported.

Old snapshots retained across partial and complete publications must continue to expose their old complete state.

## 8. Discovery without a write/read round trip

Use one immutable DiscoveryInventory returned directly by discovery. It contains exactly the physical-file metadata, deterministic IDs, captured revisions and memberships needed by preparation and construction.

Remove the pattern:

`construct inventory → write every pending file to RocksDB → read Files back → reconstruct inventory`.

Do not write Paths and QueryPaths during discovery and again during admission. In the new build path, write final file/path/state rows once, with their cohort. Do not create per-file PendingFiles rows solely to delete them in the same construction campaign. Persist schema and compact collection discovery counts before admission.

This changes the representation of pending work during construction, not the meaning of completed indexed data:

- A construction view may borrow an Arc of immutable DiscoveryInventory.
- Pending status is determined by an inventory entry with no final file-state row in that same database snapshot.
- An admitted/unavailable file has its final state and metadata in that snapshot.
- Any pending-file enumeration combines the immutable inventory with snapshot-visible states; it must not consult a mutable global “completed” set.
- Raw database namespace iteration returns stored/admitted rows, not invented pending rows. Provide the explicit construction-view accessor where pending inventory is needed.
- Label the view incomplete, as in the original prototype. This task does not add full progressive query-planner support.

At Ready, inventory is not a second backing store for indexed data. Release construction ownership; retained old construction views may keep their own Arc until dropped. Include such retained-view memory in snapshot tests, and use no retained external views in the normal readiness benchmark.

Do not add a second stat pass, source reread, or new surveillance for concurrent file changes. Preserve the existing captured-revision semantics. Empty/unreadable files still receive a final atomic state, and every discovered file is accounted for.

## 9. Finalization without re-decoding the constructed repository

### 9.1 Accumulate commit-confirmed summaries

Produce compact per-cohort summaries during preparation/planning and apply them once after acknowledged commit. Maintain:

- Indexed/unavailable file counts and collection membership counts.
- Record/logical/fallback/domain counts and reserved high-water endpoints.
- Exact field-coverage completeness flags using logical AND.
- Declared/named covered-record counts with existing semantics.
- Receipt/file accounting and outstanding work count.

Fingerprint reduction must follow the existing deterministic inventory/file order, not writer completion order. Use captured revisions/fingerprints; do not decode all Files again to hash the same metadata.

Initialize coverage for configured empty collections explicitly. Named binding coverage must not overwrite a declared field descriptor with the same path.

Do not persist transient FieldCoverageDeltas merely to scan/delete them at finalization. Their construction role is replaced by acknowledged summaries. Keep their namespace reserved if compatibility requires it.

### 9.2 Stream exact posting directories and statistics

Do not retain unbounded whole-corpus value/length dictionaries as construction summaries. They have arbitrary cardinality.

After admission, use one ordered streaming pass per relevant bitmap namespace. Existing keys group a logical posting identity before its block number; sum canonical stored cardinalities for that identity, then emit its directory/statistics row once. Keep only the current group and bounded output batch.

Use a checked cardinality-header reader on the normal finalization path. Full decoding of all bitmap payloads belongs in the independent validator. This is safe only for fresh output produced by the canonical checked encoder; do not reuse this shortcut as general corruption validation of an arbitrary imported database.

Keep metadata output batches bounded by the existing 8 MiB / 32,768-operation limits. Do not rescan a namespace once per field. Do not allocate a whole value_directory BTreeMap, a complete file-state map, or decoded reverse contributions to reconstruct counts.

In particular, normal finalization must perform zero RecordContributions decompressions and no full Records/Files scan solely to recount construction.

### 9.3 Readiness boundary

Drain all writers and acknowledge all cohorts before Finalizing. Verify compact accounting, emit all required metadata, and atomically publish the complete header last.

The outer caller timer includes discovery, thread creation, preparation, accumulator work, all database writes, required directory scans, summary output, final header publication, worker joins and release of construction-only buffers.

Do not defer required directory creation, buffer release or reopening to the first query. Do not wait for a forced full compaction to improve memory reporting. Report background engine state at readiness.

Full corruption/reference validation remains a separate benchmark mode. It cannot replace mandatory readiness work, and it must not be silently included in only one backend's timed build.

## 10. Resource budget and liveness

Carry forward the second repair's real reservation/ownership ledger. A counter that only measures the ready map is insufficient.

Charge all live construction ownership:

- Dispatched preparation estimates, replaced with measured owning capacities.
- Out-of-order prepared chunks and partially assembled cohorts.
- Active/retained bitmap accumulators, including published unfinished tails.
- Reverse facts, action/key/value arenas, compressed scratch and summaries.
- Queued/active native batch serialized sizes and any retained retry copy.
- Writer scratch even while idle.

Use Vec/String capacities and nested allocations, not slice lengths. Count shared inventory/schema once separately and include it in RSS. Native engine interior allocations and exact allocator overhead remain outside the exact application ledger; report supported properties and process memory separately.

Defaults:

- Retained-work target: 64 MiB.
- Maximum dispatched-but-unreleased preparation jobs: max(1, min(16, 2P + W)).
- Initial reservation: max(1 MiB, 8 × source bytes), with checked arithmetic.
- Idle scratch per writer: at most min(8 MiB, target / (2 × W)); handle small budgets explicitly.
- Completed workers can always report a result even when actual size exceeds their reservation. Stop dispatch and drain after an overshoot.

Do not reserve all sixteen jobs up front irrespective of the byte target. The ownership token follows work until final acknowledgement/release; splitting a preparation chunk among cohorts must split or reference-count its charge correctly.

### 10.1 Pressure and unfinished-block eviction

First stop dispatch and seal/drain the open cohort when memory is tight. Release sealed committed ranges promptly.

A pathological interleaving of many collections can retain too many partial ranges. Permit eviction only of an acknowledged, published partial range, never an uncommitted authoritative image. Record that range as cold.

On later ownership of a cold range, lazily read an existing bitmap only when its key is actually changed; merge into a new owned image and retain it while active. This is a bounded pressure fallback, not the ordinary constructor. Do not scan an entire collection to reconstruct a tail, and do not use get_for_update: the exclusive lease and predecessor acknowledgement establish construction ownership.

Count cold-range reloads separately from new-block reads. Normal non-evicted construction performs zero bitmap database reads. The standard benchmark must explain every fallback read and its pressure cause.

Do not solve pressure by silently retaining an unlimited decoded cache, ignoring the byte target, padding slots, or enlarging the budget.

### 10.2 No resource deadlocks

The coordinator must service preparation completions, writer acknowledgements, age deadlines and budget releases while deciding new dispatch.

Never block on a writer acknowledgement while preventing that writer from sending it. Never require additional budget before an already-dispatched worker can report completion. Never hold a database transaction or resource-map mutex while waiting for budget.

Grant multi-collection ownership atomically. Waiting cohorts retain charged input but no partial lease set. Tests must cover a slow first job, out-of-order completions, low estimates, blocked writers and oversized files.

## 11. Compatibility and correctness repairs to retain

This task includes unfinished compatible repairs from the second specification:

1. Clamp every StoreView iterator to its namespace for absent, one-sided, prefix and reverse bounds; check iterator errors.
2. Validate actual SortEntries and use actual forward rows in candidate reference exports. Reverse contribution counts alone are not proof of the forward index.
3. Reject missing/extra/misordered sort rows even if reverse contributions are untouched.
4. Reduce completeness flags across cohorts correctly; handle empty configured collections and declared/named descriptor collisions.
5. Preserve exact scalar order, null/missing semantics, sort ties, raw fallback, relations and duplicate source-ID semantics.
6. Preserve source-free mutation fixtures after readiness, using the transactional path and retained snapshots.
7. Enforce format/configuration/alphabet/root checks on reopening; reject old prototype formats. No conversion utility is required.
8. Keep memory mode free of host database payload files. Test durable file-backed reopening separately with the existing explicit durability boundary.

The new header/digest version must describe the changed pending-work representation and construction protocol. Do not merely change a numeric version while retaining readers that assume persisted PendingFiles rows.

## 12. Instrumentation that can explain a large deficit

Collect inexpensive operation counts in normal benchmark runs. Put per-key timers and allocator instrumentation behind diagnostic modes.

Required counts and timings:

| Category | Required evidence |
| --- | --- |
| Source | Files, bytes, reads, parses, memberships; no repeated source pass |
| Extraction | Gram occurrences, unique memberships, emitted packed words, fact visits |
| Aggregation | Distinct logical bitmap keys, word OR operations, sparse promotions |
| Key handling | Physical key encodes per namespace; bytes; arena growth |
| Publication | Cohorts, files/records each, seal reason, first-file latency, partial publications |
| Ownership | Granted ranges, dependency waits, transfer generations, disjoint active writers |
| Bitmap IO | Reads, cold reloads, puts, unique final keys and repeated puts by namespace |
| Planning | Accumulation, static encoding, sorting, digest and native batch assembly separately |
| Database | write_opt duration, attempts, receipt checks, errors and acknowledgements |
| Discovery | Enumeration/stat time, inventory creation, persisted metadata operations |
| Finalization | Summary reduction, per-namespace directory scans, metadata writes, contribution decodes |
| Memory | Estimated and actual ownership, tail state, scratch, native batch bytes, overshoots and RSS/HWM |
| CPU | Process user/system CPU, elapsed time, supported engine counters |

Report bitmap write amplification as total bitmap puts divided by distinct final bitmap keys, with startup, age, pressure, file-boundary and cold-reload causes separated. Obtain final unique counts with bounded validation scans; do not retain a whole-corpus set in the timed build merely to count keys.

For a normal fixture of 16,384 one-record files in one collection with repeated common grams, sufficiently generous fixed budget and no age expiry:

- Only the first range receives the intentional first-file partial publication.
- Later full ranges are emitted once.
- Fresh/non-evicted bitmap reads are zero.
- No encoded-key allocation occurs per membership.
- Ownership-conflict database reads/waits are not needed to reconcile initial construction.

Use a deterministic test clock for the age policy. Test forced age/pressure publication separately.

Record a sampling CPU profile of one representative full build if available. Separate Rust extraction/planning, binding/native batch assembly, TransactionDB locking, memtable insertion, and background maintenance. If unavailable, record why; do not infer CPU consumption from summed worker elapsed times.

Record actual native compiler configuration when available: release profile, optimization flags, target CPU and linked library provenance. Do not report a debug-build diagnosis without evidence. Preserve all inputs in a campaign manifest.

## 13. Required tests before the full campaign

| Test | Required result |
| --- | --- |
| Native/candidate small reference | All implemented facts, actual forward sorts and query primitives agree |
| Packed word remapping | Unaligned bases, 63/64 and 4,095/4,096 boundaries preserve bits |
| Common-key construction | Complete ranges written once apart from the explicit startup partial |
| Partial publication | First file visible promptly; later images retain all earlier bits |
| Multi-collection physical file | All memberships appear atomically in one snapshot |
| Large multi-record file | No partially Indexed file; linear record/fact visits |
| Concurrent owners | Disjoint ranges overlap execution; shared ranges cannot overwrite each other |
| Repeated logical IDs | Completion reordering cannot change native semantics |
| Replay | Exact replay no-op, changed payload rejected, old image never reapplied |
| Error handling | Before-write failure, ambiguous success, retry exhaustion and cancellation terminate safely |
| Retained snapshots | Old and new views each contain a complete consistent state |
| Memory pressure | Smaller target changes dispatch; cold-tail reloads preserve bits |
| Scheduler liveness | Slow first job, completion reordering, blocked writer and oversize file finish or fail explicitly |
| Discovery view | Pending/admitted state comes from one snapshot plus immutable inventory |
| Finalization | No reverse decompression; exact summaries and directory cardinalities |
| Corrupt forward sort | Missing/extra/wrong row detected independently of reverse facts |
| Iterator bounds | No cross-namespace leakage for all boundary forms |
| Empty/malformed input | Empty collections, empty files and fallback records remain accounted for |
| Post-ready mutation | Add/change/delete/move fixtures remain correct with old snapshots |
| Persistence | Memory host empty; durable file reopen exact; old formats/config mismatch rejected |

Use operation/visit counts for complexity tests, not fragile timing assertions. Keep allocator-heavy diagnostics out of primary performance timings.

Run the repository workspace tests and relevant existing benchmarks on the combined integration result, following documented commands. Do not replace them with a new benchmark framework.

## 14. Benchmark campaign and decision rules

### 14.1 Controls and provenance

Use the same Musenalm corpus, schema profile, ASCII alphabet, memory mode, release configuration, CPU affinity and cache conditions as the established controls. Record corpus/configuration digests and executable SHA-256 for every run.

Use:

1. Pristine native at the recorded native control.
2. Native path from the candidate revision, to detect shared-helper regressions.
3. Parent RocksDB implementation at the actual pinned base.
4. New bulk constructor with P=8, W=1.
5. New bulk constructor with P=8, W=4.

If the machine cannot support these worker counts, choose a matched documented feasible configuration before any comparison. Use the same candidate executable for W=1/W=4. No compilers or other benchmark instances may run concurrently.

Use a fresh process and fresh index per build. Record wall time, user/system CPU, readiness RSS and process HWM. Preserve old raw results rather than overwriting their labels.

Use the existing example, built separately in each revision's worktree. The candidate's ordinary source-build mode must select the new constructor by default; do not accidentally benchmark a retained old implementation. Representative commands, subject to the repository's documented build requirements:

```bash
cargo build --locked --release -p objekte-search-core \
  --features rocksdb-prototype --example bench_rocksdb_index_store

target/release/examples/bench_rocksdb_index_store \
  --mode source-build --root /home/simon/source/objekte/examples/musenalm \
  --schema-profile basic --alphabet ascii --storage memory \
  --prepare-workers 8 --write-workers 1 \
  --result-dir bench/results/rocksdb-bulk-construction-repair/candidate-w1-01
```

Use a distinct result directory for every run. Use W=4 for its paired candidate, and the existing native-build mode for controls. Do not reuse an executable across worktrees without recording and checking its identity. Capture external process accounting around the command. Diagnostic runs must identify their profiling feature/flags separately.

### 14.2 Bounded execution

After correctness and mechanism tests pass, run one screening sample of each configuration. Use a 120-second per-process timeout and the repository's established memory safety limit; preserve timeout/failure artifacts.

Compare W=1 and W=4 by total caller time and operation counts. A single writer may win after aggregation; that is a valid result. Do not insist on four writers to satisfy a concurrency narrative.

Repeat the better candidate and matched native three times in alternating fresh processes. Report every sample and the median; do not select a favorable isolated run. Repeat both writer counts only if the initial difference is within approximately 5% or the measurements are unstable.

Run full internal validation and independent native fact comparison in separate modes. Use bounded canonical streams or the existing bounded comparison mechanism; do not retain every corpus fact in a giant reference Vec. Validate forward sort rows directly.

Run a representative Unicode fixture and a small durable file fixture even if ASCII feasibility fails. Full Unicode corpus/query campaigns remain conditional on the ASCII screen.

Allow one focused correction cycle for an observed remaining violation of this specification. Do not run an open-ended cache/compression/worker-count search.

### 14.3 Acceptance language

Keep the original feasibility limits:

- Caller construction time <= 1.15 × matched native.
- Readiness RSS <= 1.40 × matched native.
- Construction HWM <= 1.60 × matched native.

The user's stronger objective is to beat native. Passing the 1.15 feasibility limit is not “faster than native.” Claim a speed win only when the repeated matched measurements support a ratio below 1.00, with variability reported.

Report the improvement factor versus both the actual parent and the historical first repair. A hypothetical 35× improvement is not an acceptance assertion and must not be promised.

A performance early exit may stop the expanded production/query campaign only after:

- Direct typed accumulation and per-distinct-key encoding are implemented.
- Block ownership, complete-image writes and atomic file publication are implemented.
- Normal-path repeated bitmap reads are removed.
- Discovery write/read reconstruction is removed.
- Finalization reverse decoding and whole-directory maps are removed.
- Resource accounting, correctness repairs and mandatory tests are complete.
- Current raw measurements identify the residual cost.

If these are incomplete, report implementation incomplete. If they are complete and performance still fails, report feasibility failure and stop further design experiments. Do not proceed to production integration or switch engines automatically.

## 15. Implementation phases and commits

Use one supervisor. Optional independent subagents may handle test/reference validation or benchmark instrumentation after interfaces are fixed. Give each its own subtask branch and sibling worktree; only the supervisor writes to the integration worktree.

| Phase | Scope and exit condition | Suggested commit subject |
| --- | --- | --- |
| 1 | Pin parent, document baseline and required native duplicate-ID behavior; define construction types and format | Define exclusive RocksDB construction protocol |
| 2 | Shared inventory, snapshot-correct pending view, final metadata written once | Remove discovery persistence round trip |
| 3 | Packed typed forward accumulation, linear reverse facts, reusable encoding arenas | Accumulate construction postings without membership key allocation |
| 4 | Cohort grouping, first-file/age policy, block generations, tail retention and global budget | Assign bounded ownership of construction blocks |
| 5 | Atomic native batches, metadata-key dependencies, receipt replay and failure handling | Publish complete construction images in atomic batches |
| 6 | Commit-confirmed summaries, streaming directories, no contribution reconstruction | Derive final index metadata from construction summaries |
| 7 | Remaining forward-sort, coverage, iterator, snapshot and persistence repairs | Complete construction correctness and resource validation |
| 8 | Workspace checks, operation evidence, matched campaign and report | Record RocksDB bulk construction feasibility results |

The supervisor may combine adjacent commits when necessary to keep each buildable, but must preserve explicit evidence for each phase. Push immediately after every commit and verify success. Stage only task-owned changes. Never force-push or delete predecessor branches.

Do not repeatedly benchmark the old algorithm between small refactors. Use small fixtures until the mechanism is actually replaced.

## 16. Deliverables and completion checklist

Raw results: `bench/results/rocksdb-bulk-construction-repair/`  
Generated report: `docs/ai/generated/rocksdb-bulk-construction-repair/repair-report.md`

The report must answer:

1. Which second-repair commits were reused, and what was the actual base?
2. How many membership key encodes and bitmap database reads were eliminated?
3. What were unique bitmap keys, total puts and write amplification?
4. Which partial publications or cold reloads remained, and why?
5. Did disjoint writers execute concurrently, and was W=1 or W=4 faster?
6. How were physical-file atomicity, repeated IDs and old snapshots verified?
7. How much discovery/finalization work remains?
8. What is charged by the memory ledger, what remains native/unobservable, and what were actual peaks?
9. Do matched time/RSS/HWM gates pass? Is it actually faster than native?
10. If not, what measured application or engine work dominates, and what remains unsupported?

Include exact commands, corpus/configuration manifests, commit identities, compiler/library provenance, executable hashes, raw result paths, test results and limitations. Do not present inferred allocation counts or summed worker durations as measured CPU facts.

Follow the repository task-document lifecycle. At completion preserve the full specification and user additions, append an Implementation report, commit/push the completed document on the integration branch, and retain the integration worktree for the user's review. Do not alter the original checkout's index. Do not merge into master.

This document specifies implementation work. Preparing it did not execute a new corpus benchmark or establish a 35× speedup.

## 17. Source references

- [Inspected second-repair head](https://github.com/Simon-Martens/objekte/tree/bc8696eb485d00221ad78e53fca70ed273f72b3c)
- [First-repair report](https://github.com/Simon-Martens/objekte/blob/08591e8452132f392201cfdb745d958f502a556b/docs/ai/generated/rocksdb-construction-repair/repair-report.md)
- [First-repair raw build](https://github.com/Simon-Martens/objekte/blob/08591e8452132f392201cfdb745d958f502a556b/bench/results/rocksdb-construction-repair/ascii-w4-final-20261115/build.json)
- [Admission implementation](https://github.com/Simon-Martens/objekte/blob/bc8696eb485d00221ad78e53fca70ed273f72b3c/crates/core/src/rocksdb_store/write.rs)
- [Native gram construction](https://github.com/Simon-Martens/objekte/blob/08591e8452132f392201cfdb745d958f502a556b/crates/core/src/bigram.rs)
- [Pinned TransactionDB batch API](https://github.com/rust-rocksdb/rust-rocksdb/blob/v0.25.0/src/transactions/transaction_db.rs)
- [Pinned WriteBatch API](https://github.com/rust-rocksdb/rust-rocksdb/blob/v0.25.0/src/write_batch.rs)

## Implementation report

**Task identifier and completion:** `rocksdb-bulk-construction-repair`,
completed 2026-11-15. Actual parent
`00d16a040bc5f3bf83daba8ef6b0daa9657b9f0c`; integration branch
`integration/rocksdb-bulk-construction-repair`; worktree
`/home/simon/source/objekte-integration-rocksdb-bulk-construction-repair`.

**Implementation commits:** `a83ed20b2b` removed discovery persistence and
shared descriptors; `8481b32152` introduced complete-image preparation,
cohort accumulation, atomic `write_opt` publication, version-4 receipts,
streaming finalization and actual forward-sort validation; `a28892d022`
completed forward/namespace tests, target-sensitive dispatch accounting,
process CPU and engine sampling; `f9e0e7c94c` added Unicode file-mode reopen
and bounded common-key tests. Every commit was pushed.

**Outcome:** performance failure under the unchanged gates. W=1 readiness is
11,584,790,610 ns, resident RSS 1,097,539,584 B and peak RSS 1,191,686,144 B
against native 990,894,820 ns, 399,290,368 B and 410,054,656 B. Ratios are
11.69 / 2.75 / 2.91. This is 3.08 times faster than the first repair's final
35.650 s run. W=1 was selected; W=4 was 0.7% slower.

**Removed repeated work:** normal construction performed zero bitmap database
reads and zero cold-tail reloads. It processed 74,016,400 bitmap memberships
into 415,015 dirty cohort bitmap keys, encoded 415,015 physical keys and
published 415,015 complete bitmap images over 40 cohorts. The first file
committed in 80.2 ms. Discovery wrote seven metadata rows instead of a
per-file pending inventory.

**Correctness:** the full ASCII validator passed 148,486 files/records, 40
receipts, 2,814,411 values, 921,173 lengths, 14,004,908 states, 56,275,908
grams and 566,768 actual sort rows with zero errors. Tests cover common-gram
blocks across 16,384 files, target-sensitive scheduling, forward-sort
corruption, namespace all-FF bounds, overlapping collections, empty/malformed
files, Unicode file reopen and memory host isolation.

**Validation commands:** `cargo check --workspace --all-targets`,
`cargo test --workspace`,
`cargo test -p objekte-search-core --features rocksdb-prototype --lib`,
`cargo check -p objekte-search-core --all-targets --features
rocksdb-prototype,detailed-profiling`, and
`cargo test --workspace --features objekte-search-core/rocksdb-prototype`
all passed. The feature-enabled core run passed 553 tests with 15 ignored.

**Limitations:** W=4 did not provide parallel publication because one
coordinator owns live tails; writer overlap is therefore not evidenced. The
pinned `SingleThreaded` binding exposes no supported engine-detail properties.
Sampling profiling was blocked by `perf_event_paranoid=2` and SELinux.
Application-owned peaks are measured, but native batch/engine/allocator
interiors remain only visible through process RSS/HWM.

**Evidence:** raw results are under
`bench/results/rocksdb-bulk-construction-repair/`. The report is
`docs/ai/generated/rocksdb-bulk-construction-repair/repair-report.md`;
comparison and hashes are in the adjacent `comparison.json`.

