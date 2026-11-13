# Objekte: Optimize snapshot loading and attribute batch admission costs

## Objective and authorized scope

Implement and measure two bounded snapshot-loader optimizations, then separate incremental construction admission time by component:

1. Construct restored inventory lookup maps once.
2. Avoid redundant bigram domain-subset scans when independently checked record metadata proves that the allowed domain contains every record slot.
3. Attribute batch admission to its actual components, globally and by collection.

Deliver one reviewed integration branch, correctness tests, reproducible before/after measurements, and a report identifying the next admission bottlenecks. This task authorizes these implementations and profiling changes. It does not authorize optimizing admission, changing posting representations, or redesigning finalization based on the new measurements.

Preserve query behavior, index contents, cache compatibility, validation, incremental publication, and lifecycle operations. No new source-content validation or freshness checks on search.

## Integration and prerequisite

Task identifier: `snapshot-loading-admission-profiling`.

- Integration branch: `integration/snapshot-loading-admission-profiling`.
- Integration worktree: `/home/simon/source/objekte-integration-snapshot-loading-admission-profiling`.
- Keep the original checkout on master. Do not commit to, merge into, or push master.
- Read `AGENTS.md` and `docs/ai/README.md`; preserve unrelated changes and user task documents.
- Push immediately after each commit, including integration and report commits. Retain the integration branch/worktree for user review.

The preceding profiling task is a prerequisite, currently at:
`integration/index-creation-loading-profiling`, commit
`965826869bea367fb18971506a7d1ef9ca68f710`.

Master at specification preparation is
`8682dcb201d3cd0b6c1c0bf0b1a5c552f1710b67`; it does not yet contain that profiling branch.

Fetch current refs. Create the new integration branch from current master and incorporate the prerequisite into this integration branch if not already present. Integration of this prerequisite is authorized; merging master is not. Record the resulting exact baseline before any work in this task. Resolve ordinary conflicts within scope and inspect whether intervening changes already implement either optimization; avoid duplicate implementations. Do not rewrite the prerequisite branch.

One supervisor owns integration and benchmarking. Subagents are optional; if used, assign separate sibling worktrees and ownership boundaries, integrate their changes here, and run performance campaigns sequentially.

## Evidence and starting points

Read the preceding report and raw archive:

- `docs/ai/generated/index_creation_loading_profile_results.md`
- `bench/results/index-creation-loading-profiling/`

The preceding warm-cache Musenalm campaign used 148,486 files, 193,297,230 source bytes, ASCII bigrams, inference disabled, configured fields, and 32 workers. Unprofiled medians were 1,015 ms construction and 802 ms restore.

Relevant profiled intervals:

| Interval | Median | Interpretation |
|---|---:|---|
| Inventory reconstruction | 104 ms | Inside checked storage loading |
| Live catalogue reconstruction | 53 ms | Separate later phase |
| Segment bigram validation | 72 ms | Includes more than the potentially redundant scans |
| Coordinator admission | 388 ms | Nested in 477 ms source/index processing |
| Segment bigram finalization | 148 ms | Separate from admission and gram generation |
| Segment sorting finalization | 83 ms | Separate from admitting sort keys |

These are observations, not savings targets. Do not claim the entire interval is removable. Old campaign results are context; collect a matched baseline.

Code references refer to the prerequisite commit; resolve current symbols rather than relying on line numbers:

- `storage.rs::load_snapshot_parts`
- `inventory.rs::FileInventory::{add,rebuild_lookup}`
- `lifecycle_segment_codec.rs::{decode_cache_segment_profiled,validate_segment}`
- `bigram.rs::BigramIndex::{validate,validate_domain}`
- `mutation_segment.rs::CollectionSegment::into_indexed_profiled`
- `repository_incremental.rs::{admit_normal,publish_segment}`, including the large-file admission path
- `mutation_segment.rs::{CollectionSegmentBuilder::admit_batch,SegmentAggregate::admit}`; confirm actual builder type names
- `repository.rs::BuildProfileStats` and existing admission statistics
- `crates/core/examples/bench_index_construction.rs`

## 1. Restore inventory maps once

### Existing repetition

The snapshot loader iterates encoded files and calls `FileInventory::add()`. That method populates the path lookup and ordered relative-path index. The loader subsequently calls `rebuild_lookup()`, which clears and reconstructs those same maps.

Implement a checked bulk restoration path, or an equally narrow alternative, which creates final file entries and both lookup structures exactly once. Prefer reusing existing validation helpers. Do not retain a second permanent map to make restoration faster.

### Required behavior

- Preserve file IDs and serialized ordering; do not sort or renumber files.
- Preserve relative-path representation, shared ownership, revisions, creation timestamps, collection membership, status, diagnostic contents/references and omitted-diagnostic counts.
- Preserve rejection of invalid identities, invalid/escaping paths, duplicate paths, invalid collection references, invalid diagnostics, and conflicting ordered path keys under existing semantics.
- Preserve all invariants currently established by `add()`, `rebuild_lookup()`, and their callees. Document which new step establishes each invariant; do not simply delete `rebuild_lookup()`.
- Build query path keys once where practical; use known lengths to reserve existing structures where supported. Avoid new path/string duplication.
- Do not access source files, stat the filesystem, canonicalize paths against disk, or perform freshness checks while reconstructing stored metadata.
- Keep public APIs stable where feasible. An internal constructor/helper is sufficient.
- Do not redesign the live catalogue in this task. Its separate 53 ms cost is not automatically eliminated by this change.
- Apply the correct shared implementation to every affected snapshot-loading entry point, not only the profiling example.

Keep snapshot format versions and serialization layout unchanged. Validate compatibility using a fixture written by the baseline.

## 2. Skip provably redundant bigram subset traversal

### Existing repetition

Loading first checks record identities, slot ordering, record classes, and the logical-record bitmap. It also checks bigram coverage and maximum slot bounds. It nevertheless calls `validate_domain(&logical)` for raw and custom text indexes even when the checked logical domain is the entire record-slot interval.

Construction already conditionally avoids these logical-domain subset scans when all records are logical.

### Required proof and implementation

For each segment, establish the following from checked data before taking the shortcut:

1. Record slots are exactly the validated contiguous interval `0..record_count`.
2. Logical membership has been reconstructed or otherwise independently checked against record classes and that interval.
3. Every record is logical, so the allowed set equals the entire interval.
4. The relevant bigram index has passed coverage and slot-bounds validation.

Only then is another bitmap subset traversal redundant. Reuse that fact for raw and custom text indexes sharing the logical domain. Keep the full-domain proof explicit and local; a serialized count or untrusted bitmap cardinality alone is insufficient.

Retain:

- Bigram record-count, maximum-slot, alphabet and binding/coverage checks.
- Full subset validation for mixed logical/raw-fallback segments and any restricted domain.
- Appropriate fallback-text domain checks. Do not apply the logical-domain shortcut to fallback text.
- Existing corrupt-cache rejection, including checksummed but structurally invalid snapshots.
- Empty-segment behavior, lifecycle tombstones and active-record semantics. The logical domain and the active/liveness domain are distinct; do not conflate them.

Use the existing construction predicate/helper if it provides the same proven condition, or factor a small shared helper if this prevents divergence. Do not introduce a broad validation framework, unchecked loading mode, or query-time validation.

Add opt-in counters identifying checked versus skipped domain scans and affected segments/indexes if needed to demonstrate that the intended path executes. Avoid per-posting instrumentation.

## 3. Separate admission by component

### Measurement scope

Instrument the actual incremental construction path used by `Index::start_incremental_build`, covering ordinary worker batches and large-file/chunk admission. Extending only the older direct builder does not satisfy this task.

Measure all admission batches, globally and per collection. Existing extraction sampling remains separate. Do not extrapolate sampled worker timings into admission elapsed time.

Distinguish these intervals:

1. Waiting for completed contributions and waiting for their admission order.
2. Coordinator bookkeeping before/after admitting a contribution.
3. Actual contribution admission/merging.
4. Segment sealing/index finalization.
5. Publication installation and callbacks.

Only intervals 2 and 3 belong to the new exclusive admission-work total. Existing scheduling metrics may retain their names and behavior for compatibility, but clearly document their broader boundaries. Reconcile the old `build_admission_ns` with the new total instead of silently changing its meaning.

### Required component breakdown

Use stable, precise field names and extend existing statistics where their semantics match. Populate the relevant existing component fields that were previously zero placeholders.

| Component | Include |
|---|---|
| Coordinator bookkeeping | Fingerprint bindings, counters and non-publication batch accounting |
| File/record inventory | Segment file entries, range ownership, slot rebasing, record/diagnostic mapping and record-ID lookup insertion |
| Field postings | Merge/shift of declared field posting indexes, including their embedded structural facts |
| Custom postings | Custom operation posting merges |
| Inference | Inference contribution merges |
| Relations | Relation contribution merges |
| Raw-text bigrams | Raw bigram contribution merges |
| Fallback-text bigrams | Fallback bigram contribution merges |
| Custom-text bigrams | Field/custom text contribution merges |
| Segment structural bitmaps | Logical-record and empty-array bitmap appends, separate from structural facts already inside field postings |
| Sorting contributions | Appending/merging sort keys; exclude final sorting |
| Unattributed admission remainder | Measured admission total minus its exclusive components |

Do not count a parent and its children as separate additive costs. For example, field structural facts remain included in field-posting merge time unless a separately measured nested breakdown is clearly labeled and excluded from the additive table.

If aggregate merging can occur during sealing on another path, attribute it to that execution phase; do not relabel deferred merge work as coordinator admission. Avoid double-counting shared instrumented helpers.

### Implementation constraints

- Opt-in through existing profiling configuration; profile-off execution must not read clocks or allocate component profiles inside admission loops.
- Time phase boundaries per batch, not individual records, keys, bitmap entries or grams.
- Accumulate locally with existing profile ownership. Do not add shared atomics, locks, global counters or scheduling changes.
- Export totals and per-collection data through the existing benchmark JSON and summary tooling.
- Report batches, files and records alongside durations. Mark unavailable fields explicitly; do not present unsupported measurements as genuine zero work.
- Define whether phase timers include local temporary destruction; keep boundaries consistent between revisions.
- Ensure exclusive component sums plus residual reconcile with admission-work time per run. Aim for under 5% unexplained admission time; if exceeded, locate the missing phase or state the unresolved attribution explicitly. Never silently clamp negative residuals.
- Keep receive wait, completion-to-receipt intervals and accumulated worker durations outside the additive admission table.
- Demonstrate attribution for `contents`, the dominant collection, as well as all-collection totals.
- Do not change batch sizes, worker defaults, publication thresholds, collection ordering, merge algorithms or posting data structures.

## 4. Correctness verification

Run focused tests during implementation and the workspace suite once against the integrated result. Reuse existing fixtures and corrupt-cache test helpers.

### Inventory restoration

Compare baseline-equivalent observable inventory state and queries after restoration. Cover:

- Empty inventory; ordinary configured repository; files belonging to multiple collections.
- Relative path lookup and ordered path queries; stable IDs and collection membership.
- Diagnostics, invalid-source/raw-fallback metadata and omitted-diagnostic counts.
- Snapshots after add, update, delete and move, including historical segment metadata and retired slots.
- Duplicate paths/identities, invalid path encodings where applicable, escaping/absolute paths, bad collection references and malformed diagnostics.

Preserve existing accepted semantics; do not introduce unrelated stricter validation.

### Bigram domain shortcut

Cover:

- All-logical segments exercising the shortcut for raw and custom indexes.
- Mixed logical/fallback records exercising the retained subset checks.
- A posting referencing a valid slot of the wrong record class.
- Out-of-range slots and mismatched record counts, including all-logical segments.
- Empty segments, empty indexes and Roaring container boundaries around slots 65,535/65,536.
- Fallback postings and deleted/inactive records.
- Corrupted serialized logical membership/cardinality that must not unlock the shortcut.

Structural-corruption tests must reach structural validation: recompute checksums with existing test helpers when needed. A checksum-only rejection does not verify these invariants.

### Integration and instrumentation

- Restore the same compatible baseline snapshot with baseline and optimized binaries; compare query counts and deterministic result digests.
- Exercise FTS, ordered-field equality/ranges, structural/type/Boolean predicates, sorting/pagination and path filters through existing suites/fixtures.
- Verify profile-on/off equality, zero source-content parsing during an unchanged restore, and unchanged background reconciliation behavior.
- Verify component counts and reconciliation properties on ordinary and multi-record/large-file batches. Do not assert exact nanoseconds or require every tiny timer to be nonzero.
- Confirm errors/cancellation still propagate and nothing partially validated is published.
- Build affected CLI/desktop consumers if shared statistics/API types change. No UI redesign or full native interaction campaign is required.

Run repository-documented workspace tests, relevant lint checks and formatting checks. Preserve inherited formatting issues without broad cleanup. Record commands and results.

## 5. Benchmark design

Use the existing release benchmark and scripts; do not build a separate framework.

### Matched variants

Retain reproducible commits for:

- B0: integrated prerequisite baseline before this task.
- B1: inventory restoration optimization only.
- B2: domain-subset shortcut only.
- B3: both optimizations plus completed admission instrumentation.

Small experiment branches/worktrees may supply isolated variants; the final deliverable remains one integration branch. All variants must use the same relevant loader profiling definitions.

Prepare one compatible completed snapshot outside measured intervals using B0. Restore the exact same file, recording its hash and size, in every comparison. Do not include save/build time in restore measurements. Confirm `restored_from_cache=true`; never accept fallback construction as a restore sample.

### Primary campaign

- Reuse the configured Musenalm workload, ASCII, inference off, 32 workers, same compiler/release settings and fixed CPU affinity.
- Run seven fresh-process, profile-off restores per variant, interleaving/rotating variant order.
- Run seven profile-off construction trials each for B0 and B3, interleaved, to check that loader work and added instrumentation do not regress normal construction.
- Run seven profiled B3 construction trials for admission attribution and compare overhead against profile-off B3.
- Collect repeated profiled restores for B0 and B3 (at least three each) to show which loader phases changed. Extend to seven only if variability prevents interpretation.
- Run campaigns sequentially, without concurrent compilation or competing task benchmarks.
- Record warm-cache conditions honestly. Fresh process does not imply cold storage. Do not clear host-wide caches.
- Keep automatic reconciliation policy identical. Report load-return readiness and reconciliation completion separately.
- Keep query verification outside primary build/load intervals and use the same checks for all variants.

Three-trial Unicode cross-checks and targeted mixed-fallback fixtures are sufficient for secondary evidence. Do not repeat the entire worker-count matrix or change defaults in this task.

### Required results and interpretation

Report raw samples, median, IQR and paired differences. Separate:

- End-to-end restore latency and each changed loader phase.
- B1/B2 individual effects and B3 combined effect; do not assume savings add.
- Construction time, catalogue readiness and first publication.
- Exclusive admission components, per collection and globally, with per-run residual.
- Accumulated worker measurements and waits, clearly nonadditive.
- Profile-on overhead and normal profile-off performance.
- Snapshot size; post-load/post-build RSS and process peak RSS.
- Snapshot logical reads, physical read counters where available, source-content reads/parses and reconciliation activity.
- Query digest correctness and cache-restore success.

Seek no material profile-off regression or memory growth. Investigate a repeatable regression above 5% in relevant elapsed/RSS measures; for smaller differences distinguish noise from evidence. The profiling overhead target is at most about 5%; reduce instrumentation overhead if consistently higher.

Do not impose an invented minimum speedup. Retain an optimization when repeated measurements and code simplification justify it; if it is neutral or harmful, report that clearly and remove or revise the unsuccessful production change within scope. Preserve instrumentation and the experiment results. Do not label a noise-level difference a performance win.

No snapshot format/size expansion or new permanent lookup duplication is acceptable. Do not attribute the entire previous 562 MB construction versus 329 MB restore RSS difference to this work; record updated figures and distinguish live state from unmeasured allocator retention.

## 6. Boundaries

Explicitly outside this task:

- Changing ordered-posting B-trees, Roaring representations, compression or serialization format.
- Optimizing admission based on the newly measured breakdown.
- Reworking bigram packing or final sorting.
- Fusing checksum/deserialization passes or changing checksum algorithms.
- Streaming snapshot encode/decode or changing encoded-buffer lifetimes.
- Redesigning catalogue ownership, discovery, worker scheduling or allocator behavior.
- New indexing policy, inference defaults, query semantics, validation-on-search, or desktop features.

Document consequential findings and rank follow-up experiments using measured elapsed costs. Do not implement them automatically.

## 7. Commit sequence and completion

Use focused commits so each optimization can be reviewed and benchmarked independently:

1. Establish prerequisite baseline and component admission instrumentation.
2. Implement checked single-construction inventory restoration with tests.
3. Implement the proven-domain bigram shortcut with tests.
4. Integrate, run correctness and performance campaigns, and retain/revise changes according to evidence.
5. Commit the report, reproducible measurements and completed specification.

Commit order may differ to simplify isolated measurements; preserve the B0/B1/B2/B3 definitions and exact revision records.

Deliver:

- `docs/ai/generated/snapshot_loading_admission_profiling_results.md`
- `bench/results/snapshot-loading-admission-profiling/` with README, commands, manifests, raw samples and summary script/output.
- Completed specification at `docs/ai/done/objekte_snapshot_loading_and_admission_profiling_specification.md`, preserving this specification and appending an `Implementation report`.

Keep large snapshot fixtures local and ignored; record their generation command, format, size and hash. The report must explain validation preservation, measured benefit of each optimization, residual admission uncertainty, memory implications and the next measured bottlenecks. Cite current functions/lines, not stale test-only helpers.

Completion requires passing relevant checks, correct queries, a real per-component admission breakdown, honest before/after results and a pushed integration tip. Preserve and report any blocked acceptance item rather than declaring success. Follow repository rules for document lifecycle and task-memory cleanup; retain the integration worktree.

**Do not merge into master. The user will decide after reviewing the implementation and measurements.**



## Implementation report

- **Task ID:** `snapshot-loading-admission-profiling`
- **Completion date:** 2026-11-13
- **Base master:** `8682dcb201d3cd0b6c1c0bf0b1a5c552f1710b67`
- **Integration branch/worktree:** `integration/snapshot-loading-admission-profiling` at `/home/simon/source/objekte-integration-snapshot-loading-admission-profiling` (retained for review). No merge into master.
- **Prerequisite:** integrated `integration/index-creation-loading-profiling` at `965826869bea367fb18971506a7d1ef9ca68f710` with merge `0a3989f95fc72f2d3d213f4a2cefe625b9e68b47`; did not rewrite the prerequisite branch.

### Changes and revisions

1. `6c808ebb70203d2442a5e8f2f595199ae5f8d171` established the B0 matched benchmark baseline after the prerequisite merge and local archive ignore rule.
2. `7ac10f36cd762708aa943a70fa4700ffb9592d5c` (B1) restores file entries in serialized order, then uses the existing checked `FileInventory::rebuild_lookup()` once to validate paths, identities, ordered path keys and diagnostics and build final maps. The loader preserves collection membership semantics and checks collection references. Both snapshot-loading entry points share this path.
3. `55a38262b28af74cb5858c4f4478385ff4b15804` (B2, isolated `task/snapshot-loading-admission-profiling/domain-only`) skips raw/custom bigram logical-domain subset scans only after checked contiguous record slots, reconstructed class-based logical membership, serialized-bitmap agreement and bigram coverage/bounds validation. Mixed and fallback domains retain checks. Merged with `2bb1bd10f62278a6c428ab6a8d6720e89a76250b`.
4. `93a98cc4dc578dde768c3a83e41b97d36f33e9b2` (B3) adds opt-in, per-batch exclusive component timing to ordinary and large-file incremental admission, exported globally and by collection. It keeps legacy broad scheduling timing, separate seal/publication boundaries and profile-off behavior.
5. `794b00cd2e` adds final structural corruption, inactive and empty-segment validation cases. B3 release binaries predate this tests-only commit; no production code changed after B3.

No subagents or subordinate worktrees were used. All implementation commits and merges were pushed to the task integration branch immediately.

### Validation and measurements

`cargo test --locked --workspace --all-targets --quiet` passed on the combined integration result, including the final tests-only commit. Focused inventory, storage-corruption, segment-codec, large-file and multi-collection profile tests passed. `cargo check --locked -p objekte-cli -p objekte-desktop --quiet` passed for shared API consumers. `cargo clippy --locked --workspace --all-targets --quiet` exited 0 with warnings. `git diff --check` passed. `cargo fmt --all -- --check` failed on extensive pre-existing formatting differences across the workspace; unrelated formatting was preserved.

The relevant release benchmark is `cargo build --release --locked -p objekte-search-core --example bench_index_construction`, followed by the sequential, fixed-affinity scripts in [the raw archive](../../../bench/results/snapshot-loading-admission-profiling/README.md). All variants restored the same baseline ASCII snapshot. Seven profile-off restore medians were B0 **817.05 ms**, B1 **716.56 ms**, B2 **722.77 ms**, and B3 **683.92 ms**; the B3 reduction is **133.13 ms (16.3%)**. Seven profile-off construction medians were B0 **985.63 ms** and B3 **979.25 ms**, within variation. Seven B3 profiled construction runs measured **395.21 ms** median exclusive admission; **277.09 ms** was field-posting merging and **81.73 ms** record inventory. Every run reconciled its component sum with residual at most **0.21%**. All 68 archived trial sets agreed on the same five query digests; every restore used the cache, and unchanged restore read/parsed no source content. Detailed setup, raw values, Unicode cross-check, RSS, I/O, reconciliation and limits are in [the generated report](../generated/snapshot_loading_admission_profiling_results.md).

### Scope and cleanup

The work keeps lifecycle snapshot format 28, loader validation and query semantics. The proposed field-posting optimization was deliberately not implemented; its measured cost is recorded in [the performance issue](../issues/perf/incremental_field_posting_admission_cost.md). No unresolved in-scope correctness or performance blocker remains. The original untracked task specification was preserved byte-for-byte above this appended report. Large local snapshot fixtures and release binaries remain ignored and are not committed; their hashes and generation commands are archived. The integration worktree remains for user review. Task memory is removed after the completion commit and push; unrelated original-checkout files and index state are preserved.
