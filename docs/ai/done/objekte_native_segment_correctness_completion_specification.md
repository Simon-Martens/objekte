# Objekte native segment correctness completion specification

## 1. Assignment, branch, and priority

Continue implementation on the existing **`integration/native-segment-construction-repair`** branch. Do not create another integration branch, restart from master, reset the branch, or merge into master. The user will decide when to merge.

Reviewed and remotely verified starting revision: **`35ea1b83bd5fa08fd321e900b563a473ef12715f`** in `Simon-Martens/objekte`. The previous implementation commit is `d6941e17598f922d745c589fc88c0004ea4003d0`. If the branch has advanced, inspect the intervening changes and continue from its actual head; do not discard them.

This is an implementation assignment, not another investigation-only task. Implement the mandatory items below, add regressions that exercise the actual failing paths, run validation, record the results, and push the commits on the repair branch. Do not stop because the older specification labels related work blocked: this document supplies the scope and implementation decisions for the present task.

The user accepts the current performance and explicitly defers memory work. Preserve the existing native BTreeMap/HashMap, Roaring bitmap, bigram, sorting, and Rayon design. Add no library, storage backend, persistence format, async runtime, or allocator framework.

Priority order:

1. Correct relation results for every segment and selected execution strategy.
2. Correct collection-wide inference observations, semantic admission decisions, and coverage.
3. Actual overlap between framed-file extraction and segment finalization, with atomic publication.
4. Tests and committed performance evidence proving the repaired behavior.

### 1.1 Scope decisions that supersede the earlier repair specification

| Work | Decision for this assignment |
| --- | --- |
| Relation FTS local/global slot collision | Mandatory correction |
| Source-restricted structured semijoin collision | Mandatory correction |
| Streaming relation prefetch using unqualified slots | Mandatory audit and correction/removal |
| Inference counts, distinct values, shape observations, semantic thresholds and capability coverage | Mandatory correction |
| Numeric fixture incorrectly presented as a string-threshold regression | Replace with meaningful regressions |
| Large framed-file finalization begins only after EOF | Mandatory pipeline correction |
| Whole-segment cloning during finalization | Deferred; do not undertake clone elimination in this task |
| Temporary sorting copies, retained flat metadata/relation compatibility payloads | Deferred |
| Repository-wide allocation-budget redesign and inference memory-cap accounting across retained segments | Deferred as memory work; preserve current guards and document the limitation |
| RSS reduction, compact summaries, allocator work, memory benchmark gates | Deferred; record RSS for information only |
| Full R07 public low-level API/snapshot migration | Deferred except narrow adapters necessary for correct relations |
| Full R08 independent scheduling of every custom/text/index component | Deferred; preserve current measured scheduling |
| Earlier sorting, metadata, ownership-reclamation, and configured mutation-policy fixes | Preserve and rerun their regressions |

The memory deferral does **not** authorize wrong exact proofs, unchecked arithmetic, lost cancellation, publication of incomplete files, or silently changing configured semantic thresholds. In particular, occurrence/repetition thresholds and distinct-key limits remain part of inference behavior. Existing local resource overflow must continue to produce incomplete coverage or an established safe fallback. Do not claim that the global memory allowance is enforced if it is not.

The original full repair specification remains partially deferred after this narrower assignment. Mark this document complete only when its own requirements pass; do not relabel all original R01–R12 requirements complete.

### 1.2 Repository workflow

Read `AGENTS.md` and `docs/ai/README.md`. Locate the existing repair worktree with `git worktree list`; use it. Keep the original master checkout and unrelated user modifications untouched. If no repair worktree exists, attach the existing repair branch to a sibling worktree under `/home/simon/source`, following the repository convention. Do not use `/tmp` for repository worktrees.

Fetch the remote branch, inspect local/remote divergence, and preserve local work. Never force-push or reset unrelated changes. Record the actual starting SHA. Commit in the phases in section 9 and push after each completed implementation commit according to repository instructions. Keep the final repair branch/worktree available for the user.

One agent can implement this task. Subagents are optional; if used, establish the common address interface before parallel implementation and integrate all work into this same repair branch. Do not let separate agents introduce competing address types or change the query semantics independently.

## 2. Evidence and navigation map

These locations are at the reviewed revision; navigate by function names after edits.

| ID | Evidence | Required change |
| --- | --- | --- |
| C01 | `run.rs`, `try_semijoin_relation_page`, around line 6467: `input.insert(record.slot.0)` across segments | Preserve collection/segment/slot identity from input through witnesses and result selection |
| C02 | `relation_semijoin.rs`, `segment_target_candidates`, around line 194: global restriction tested against local slot | Intersect restrictions in the same address domain as target postings |
| C03 | `run.rs`, segmented streaming source-dependent prefetch around line 9440 | Stop sending merged local slots to flat target-cache helpers |
| C04 | `inference_build.rs`, `aggregate_segment_inference` | Aggregate observations, not reconstructed information from surviving indexes |
| C05 | `inference.test.rs`, `global_threshold_admits_a_field_when_each_segment_is_locally_below_it` | Exercise string dictionary thresholds and assert the particular capability |
| C06 | `repository_incremental.rs`, `private_segments` collection and `publish_private_segment_group` | Submit sealed segments during reading; consume results privately; publish atomically |
| C07 | Completion report and missing `docs/ai/generated/native_segment_construction_repair_results.md` | Commit a real results report and link existing evidence accurately |

Use the last review as evidence of the observed code, not a substitute for reading it. Tests must demonstrate each defect and its repair. A fixture that never executes the affected optimization is not evidence of a fix.

## 3. C01–C03: relation execution with explicit record domains

### 3.1 Representation and reuse decisions

Use the existing `SegmentRecordAddress { collection, segment, slot }` and `SegmentRecordSet`, whose bitmap keys are `(CollectionId, SegmentId)`, for native segmented semijoin membership. Extend that container with only the operations required here: membership, local-set access, intersection, union, iteration, and cardinality. Binary operations combine bitmap contents **only for equal collection/segment keys**. An absent key denotes an empty set, not unknown coverage.

Do not represent a segmented candidate set as one `RoaringBitmap`. Do not flatten it to allocated record-ID strings. Logical IDs are used to resolve relation edges, not as a replacement membership representation for all query records.

Reuse `MergedRelationView` in `relation_query.rs` for the pinned publication's record lookup, `resolve_id`, `forward_edges`, and `backlink_sources`. It already receives repository metadata, base liveness, current segments, and the current file catalog. Extend its narrow helper interface if necessary; do not create a second relation engine or migrate all public collection APIs.

`MergedRecordSlot.segment: Option<usize>` is a position inside a **particular pinned view**. `SegmentRecordAddress.segment` is a stable segment identity. They are not interchangeable. Provide checked conversion helpers owned by the query view:

1. Build a small `(CollectionId, SegmentId) -> segment position` table once per pinned relation view, or reuse an existing equivalent route.
2. Convert a qualified native address to a view address using that table and check the local slot against the segment's records and active domain.
3. Convert a native view address back by reading the segment's actual `SegmentId` from the same pinned view.
4. A `segment == None` address is a genuine legacy/base address. Never manufacture a native segment ID for it or silently place it into a native bitmap.

The existing native semijoin optimization may remain ineligible when a genuine active legacy base is present, using the already existing correct merged evaluator for that case. This is a specific representation boundary, not permission to disable the optimized path for normal native multi-segment indexes or after mutations.

Do not key a cross-query cache by segment position alone. Any cache that stores these addresses must already be query-local to the pinned publication or include the existing index instance/revision namespace and stable segment identity.

### 3.2 Repair native relation FTS end to end

Change the segmented FTS semijoin interface to accept `SegmentRecordSet` and a pinned relation view/context. It must return qualified source witnesses directly. Keep any old flat interface only for callers whose inputs are genuinely collection-wide flat slots, with a separate name/type and no implicit conversion.

Implement this sequence:

1. For each source segment, derive the initial logical active candidate bitmap. If a source filter is eligible for an exact proof, evaluate its bounds in that segment and its current catalog. Keep the existing fallback when the source predicate cannot be proved exactly.
2. Insert that local bitmap under the source collection and segment ID. Do not copy its integers into another segment's domain.
3. Select the existing target-first or source-restricted strategy using summed candidate cardinalities. Preserve the strategy cost model initially; changing the heuristic is not this repair.
4. For source-restricted execution, traverse the selected source records' relation edges through `forward_edges`. Resolve each target logical ID through `resolve_id` in the same pinned view, retain only active logical targets, and deduplicate by qualified target address.
5. For target-first execution, enumerate the target collection's current native segments and derive each segment's active logical domain.
6. Evaluate target bigram/FTS candidates per target segment. Intersect them with that segment's restricted set, if present. Then perform the existing exact target verification with current file metadata and byte ranges. A bigram candidate is not an exact FTS match.
7. Traverse reverse relations for verified targets through the pinned relation view. Keep the returned source addresses qualified. For multiple hops, repeat this operation in the proper intermediate collection at each hop.
8. Intersect the qualified witnesses with the **original qualified source input** and active source domains.
9. Store selected-field evidence against those same qualified source addresses. Build hits directly from them; do not convert to flat slots and then search every segment for that number.

Conceptual membership flow:

```text
source_candidates: Map<(CollectionId, SegmentId), LocalBitmap>
reachable_targets: same qualified representation in the target collections
verified_targets: subset of reachable/domain candidates after exact verification
source_witnesses: reverse edges from verified targets, still qualified
matches = source_witnesses intersect source_candidates
```

For multi-field relation FTS, preserve the current OR/evidence semantics. For mixed local and related text selections, retain the established merged evaluator unless the current optimization already proves equivalence. Do not broaden eligibility while repairing identity.

### 3.3 Repair structured source-restricted semijoins

The current code checks `restricted.contains(record.slot.0)` before translating local target results to collection-wide slots. That is the precise defect. Do not merely translate output slots.

Implement a native segmented restriction helper parallel to the FTS path:

1. Accept qualified source input and derive reachable qualified target sets for the source-restricted strategy.
2. For target segment `s`, obtain `restricted.local(collection, s.id)`; if no set exists, skip this segment.
3. Compute `input = local_restriction & s.active & s.logical_records`.
4. Evaluate the terminal expression's may/must bounds against `s.index` and the pinned catalog.
5. Verify residual target predicates as before; retain exact matches under `s.id`.
6. Reverse the qualified matches through each relation hop and intersect with the original source input.

Retain both strategies. Add a **test-only** means to force target-first and source-restricted execution for the same fixture, using the existing strategy parameter where possible. A heuristic choosing target-first cannot count as coverage of source-restricted correctness.

If the old flat `restrict` path remains for legacy consumers, isolate it. Native callers must enter the qualified implementation. Search every caller of `restrict`, `candidates_with_strategy`, `segment_target_candidates`, `segment_fts_target_candidates`, `verified_targets`, `verify_target_records`, `reachable_targets`, `reverse_sources`, and `fts_witnesses` to verify the domain at each boundary.

### 3.4 Target verification, lifecycle, and errors

Refactor the existing verification loop to accept resolved current records or qualified addresses. Reuse its grouped file reads, projection, exact scanner, cancellation checks, resource charging, and source-unavailable behavior. Do not regress to opening one file for every relation edge or reparsing the same target for every source record.

Group work by current file identity/revision and byte range as existing code does. Target deduplication must distinguish records in the same physical file; `FileId` alone is not a record identity. Charge actual reads/parses and preserve existing missing-target and unavailable-source semantics.

After add, replace, delete, move, or compaction, resolve targets and reverse sources from the pinned publication. Do not use the repository's initial `generation.records` or initial relation payload as the authoritative current record route. Metadata declarations may still be borrowed from `Repository`.

The old pinned view remains tied to its old index publication. This does not promise historical source-file bytes after external overwrite; tests of old-view behavior must use source-free predicates/locators or account for the established source-revision validation behavior.

### 3.5 Streaming prefetch decision

At the reviewed revision, the segmented streaming code combines native local slots with a base bitmap, calls the flat `prepare_stream`, and discards the successful returned value.

For this task, **remove that unused flat prefetch call from segmented execution**. The existing merged relation evaluator already has query-local target reuse. Do not replace the discarded call with another prefetch subsystem. Keep the flat prefetch where a genuine flat consumer actually uses its result.

Prove with a streaming regression that native execution uses the correct target and that it does not issue unrelated source reads or fail because of an unused stale prefetch. Recheck repeated-target parse counts so removal does not accidentally eliminate the actual merged-view cache.

### 3.6 Mandatory relation fixtures

Tests must assert named logical IDs, counts, and evidence, not just agreement between two optimized implementations. Use the reference evaluator with optimized candidate pruning disabled as additional evidence. Never share the new candidate conversion with the reference oracle.

| Test | Fixture and exact assertion |
| --- | --- |
| FTS source collision | Source A in the first segment and source B in a later segment both have local slot 0. A points to text `other`; B points to text `needle`. Related-field FTS for `needle` returns B only. Both targets may be in one segment to isolate the source defect. |
| FTS source-filter collision | An exact source filter selects B only, while A has the same local slot. Verify only B survives and evidence belongs to B. |
| Target restriction collision | A selected source points to target T512 whose collection-wide slot was 512 and local slot is 0 in the second segment. Force source-restricted execution: T512 and its source must be found. |
| Both strategies | Force each eligible strategy on the same fixture; compare exact logical IDs and counts to explicit expectations and the reference evaluator. |
| Multiple matching segments | Place matching and nonmatching records at local slots 0 and 1 in at least three segments. Count all distinct matching sources once. |
| Lifecycle | Add a matching target, replace a target while retaining its logical ID, delete a source/target, move a file, and compact. Verify fresh views, applicable pinned views, and cache invalidation. |
| Multi-hop and many-to-one | Repeated local slots in an intermediate collection; many sources share one target. Preserve hop semantics and target-read reuse. |
| Streaming | Repeat the collision and lifecycle cases through streaming with and without verified result caching; assert final counts/evidence and no speculative unrelated reads. |

For a public end-to-end fixture, the current ordinary-file extraction batch can hold 512 files. `target_records = 1` alone does not force one record per segment. Use 513 ordered files with the meaningful source/target at positions 0 and 512 and nonmatching fillers, as existing tests do. Explicitly assert segment count, actual IDs, and repeated local slots before executing the query. For smaller unit fixtures, use existing internal preparation helpers to make explicit independent contributions; do not construct invalid hand-written indexes.

Exercise `Repository`, a completed progressive `Index`, and an adopted `Index::from_repository` as applicable. Include locator and payload modes; ensure at least one locator case demonstrably enters the repaired semijoin rather than another direct relation optimization. Add a test-only execution marker if plan estimates alone cannot prove which path ran.

## 4. C04–C05: preserve inference observations and admission semantics

### 4.1 Separate observations, candidate payload, and admission

The current aggregator reconstructs observations from `InferenceState.indexes`. This is invalid: ReportOnly does not build those indexes, rejected/partial indexes omit keys, and published reports omit some shape information.

Introduce or extend an internal **segment inference observation** object produced from `InferenceBuilder` before `finish` discards or moves the relevant state. It is separate from the segment's candidate `PostingIndex` objects and from the collection's admission decision. Reuse existing types where they already express these responsibilities.

Required observations include:

- Logical eligible and unavailable record counts; completeness and omission information.
- Per-path observed kinds, array nesting, integer/fractional flags, and direct-versus-through-array occurrence categories.
- Per-record scalar/array/object/null/present categories and value/nonblank occurrence counts, deduplicated within a record exactly as the old observer does.
- Exact distinct nonblank-string summaries up to the existing policy limit, with an explicit overflow/truncation marker; do not obtain them from posting keys.
- Short-string observations and actual local coverage separately from general dictionary observations.
- Ancestor empty-array information necessary to recognize empty selections for paths first discovered in later segments.
- Existing resource-drop/incomplete flags so a discarded local candidate can never become a complete index merely because global thresholds pass.

**Memory decision:** retaining existing segment-local ancestor bitmaps and bounded distinct-key sets is allowed in this task. Do not invent a lossy cardinality-only summary to save memory. Do not reinstate a second collection-wide posting membership builder. Retain or move only the observation data needed for semantic correctness; optimize its representation later.

Generate observations from the already parsed builder state. Do not reread every source or walk every parsed Value tree a second time. Summaries must exist in both AutoIndex and ReportOnly. Disabled mode must avoid this work.

### 4.2 Exact aggregation algorithm

Aggregate in deterministic source/contribution order, independent of worker completion order. Use a two-pass approach over the immutable per-segment observations:

1. Compute the collection's total eligible/unavailable records and the union of observed paths.
2. For each global path, visit each segment's observation, including segments in which that path was never observed.
3. If the path is present in a segment summary, contribute its recorded counts and category information.
4. If absent, its value/kind/nonblank counts are zero. Its eligible count is still that segment's logical record count. Derive any empty-selection/presence contribution from empty ancestor arrays as described below.
5. Sum per-record counts only across disjoint segment record domains. Within a segment, compute overlapping categories with bitmap union/intersection before taking cardinalities. Do not sum overlapping categories as if they were disjoint.
6. Set each field's eligible count to the full collection logical total. Compute missing count using the existing present/empty-selection semantics, not by initializing a field when it first appears during a one-pass merge.
7. Merge distinct-key sets by set union, with deterministic ordering where a bound changes the result. Once exactness is lost, retain that fact. Never report zero as a substitute for unknown/truncated distinct count.
8. Populate direct and through-array counts from the real observer state. Remove unused placeholder fields only if the replacement decision receives equivalent information; do not leave compatibility checks operating on permanently zero counters.
9. Carry omitted-path and incomplete state accurately. The aggregator's current unconditional `omitted_paths: 0` is not valid after observation truncation. Preserve the established count semantics using the ordered observations; if a public field cannot represent unknown exactly, retain the existing incomplete/truncated convention rather than inventing a precise count.

For a late nested path such as `items.name`, a segment containing only `items: []` still contributes empty-selection information even though it has no explicit `items.name` field observation. For an absent path, take the union of retained empty-ancestor bitmaps for the path's relevant prefixes, using local slots, and intersect with the segment's logical domain before counting. Do not add ancestor counts directly: nested selections can overlap. For a path already finalized locally, do not apply those ancestors a second time. Compare the result to the existing single-domain inference evaluator on the same records.

The same principles apply to short-string distinct counts: `max(local_distinct_counts)` is not the union cardinality. Use retained bounded key observations and distinguish exact from truncated results.

### 4.3 One semantic admission decision function

Extract the semantic decision logic from the existing full `InferenceBuilder::finish` implementation into a reusable internal function. Feed it either complete single-domain observations or aggregated observations. Do not maintain a simplified second decision ladder in `aggregate_segment_inference`.

Preserve declared-field precedence, explicit opt-out, kind/shape compatibility, ReportOnly behavior, string repetition thresholds, semantic distinct-key limits, short-string behavior, exact-empty-string behavior, array-length and structural accelerators. Do not alter String/Id comparison semantics in this task.

Represent the result at **component capability** granularity. Use or extend the current metadata to distinguish general value dictionaries, short-string partial indexes, exact empty-string membership, array length, and structural facts. A field-level `Built` status does not mean all values have complete indexed coverage.

Local candidate construction must not apply a global minimum or repetition decision. In particular, relaxing only `min_nonblank_occurrences`, `min_nonblank_records`, and repeated percent is insufficient if another local repetition/cardinality ratio still discards data needed globally. Preserve bounded provisional candidates until the collection decision. Existing hard local resource limits may discard a candidate only with explicit incomplete coverage.

Apply the final decision coherently at collection completion:

1. Determine which capabilities the collection policy admits.
2. Combine that decision with each segment's actual retained capability and coverage.
3. Permit exact planner proofs only where both permit them; otherwise use the existing may/must/residual mechanism.
4. Preserve permitted structural/length/empty-string capabilities even when general dictionary admission fails.
5. Prevent a locally admitted general dictionary from bypassing global rejection through another `IndexCatalog` lookup or sort eligibility path.
6. Preserve safe partial short-string coverage rather than calling it a complete dictionary.

For initial progressive construction, inferred capabilities that need a final global decision remain unavailable to the planner until that decision is published; declared indexes and normal FTS remain usable. Do not mutate a decision in an old pinned view. Existing segment cloning during installation is accepted for this task; do not broaden it unnecessarily or make clone elimination a prerequisite.

Audit `IndexCatalog` lookup, sortability, EXPLAIN and diagnostics together. They must describe the same admitted capability that execution uses. A globally rejected capability can remain physically retained because memory cleanup is deferred, but it must not be exposed as admitted or exact.

### 4.4 Mutation and failure rules

Only accepted file contributions enter the finalized collection observation set. Stage observations with a private framed-file group and discard them if the whole attempt is invalidated. Recoverable bad-record fallback continues to use the adapter's existing policy.

Use the existing revision/liveness rules after mutations. At minimum, mutation tests must establish exact query parity and that stale inferred coverage is never used as a proof. A final collection decision cannot establish coverage of a newly added segment by itself. If necessary, withhold the affected inferred capability until a current decision is available and use its established residual path; do not disable declared indexes or normal FTS. Do not present stale build observations as a complete current-generation report. Keep any existing construction-generation report explicitly tied to that generation; an API that promises current statistics must compute or mark incomplete current statistics.

This task does not redesign global allocation accounting. Leave the known repository-wide memory-cap limitation in the deferred section of the report. Preserve local drop flags and do not use a locally exceeded memory budget to fabricate a successful exact capability.

### 4.5 Mandatory inference fixtures

Use the old single-domain observation/decision implementation as a test-only oracle where useful. Keep it out of production aggregation. Also assert explicit expected values to prevent shared implementation mistakes from blessing each other.

| Test | Construction and expected proof |
| --- | --- |
| Global minimum reached only together | Use 513 records with one nonblank string value, split 512+1. Set both minima to 513. Choose a value longer than `short_string_max_utf8_bytes` but within `max_string_bytes`, e.g. lower the short-string threshold to 4 and use `category_alpha`. Neither segment meets the minimum; the completed collection admits the general dictionary. |
| Global rejection despite local admission | Two explicit segments, each with a repeated set of distinct strings; sets are disjoint. Example: 40 distinct labels per 512-record segment, global maximum 64. Disable the short-string shortcut as above. Local repetition is sufficient, global distinct count is 80, and the general dictionary must be unavailable while permitted structural capabilities remain. |
| Threshold boundaries | Exactly N and N-1 eligible nonblank records/occurrences; exactly K and K+1 distinct strings; repeated-percent boundary. Verify the general-value capability, not merely field status `Built`. |
| ReportOnly | Same input as AutoIndex, but no candidate posting maps. Report kinds/counts/distinct observations correctly; only admission/build status differs as specified. |
| Late field | First segment lacks `category`, later segment contains it. Field eligible count equals all logical records; missing count includes earlier records. |
| Late nested field | First segment has `items: []`; later segment has `items: [{"name":"alpha"}]`. Verify present/missing/empty-selection counts against explicit expectations from existing semantics and the single-domain oracle. |
| Mixed shape | Direct scalar versus the same path reached through arrays, scalar versus array, mixed kinds, object/empty-array cases. Verify compatible capabilities and residual coverage, including independent array-length behavior. |
| Null overlap | A record containing both null and non-null selected values. Distinguish null occurrence from exact-null and avoid double-counting records. |
| Short strings | Distinct sets overlap between segments; some records have long values. Verify union cardinality and short-string covered/unresolved counts. |
| Local overflow | Force an existing small local observation/candidate limit. Report truncation/incompleteness and verify query parity with residual scanning; no false exact proof. |
| Invalidated file and mutation | Discarded framed-file attempts contribute no finalized observations. Add/replace/delete cannot reuse stale exact coverage. |

For the global-minimum test, query the admitted string value using locator mode with evidence disabled and verify exact membership with zero source reads/parses where the established planner supports it. For global rejection, inspect the general dictionary capability and compare results to reference execution; a source-free structural shortcut is not proof that the rejected dictionary was used.

Retain the numeric fixture only as a numeric-posting regression with an accurate name. It must no longer be cited as string-threshold evidence. Assert actual segment record distribution in every segmentation-sensitive test.

## 5. C06: finalize framed-file segments while reading

### 5.1 Required execution model

Keep one atomic publication for all segments belonging to a successfully validated physical-file group. The change is to when segment work runs, not when unvalidated data becomes visible.

Replace the unbounded `Vec<CollectionSegment>` accumulation with a private group that owns:

- File/group identity and reading/validated/failed/cancelled state.
- The current unfinished draft.
- Submitted job identities and outstanding counts.
- Completed `IndexedCollectionSegment` payloads, ordered by planned segment identity.
- Staged observations and final file diagnostics/fingerprint state.

Completed indexed payloads are retained output and may grow with the final index for the file. Their memory reduction is deferred. Unsubmitted unfinished drafts must not accumulate for the entire file.

Use the existing segment worker pool and completion mechanism. Extend its completion destination to distinguish normal publication from a private file group; do not create an additional worker pool. To simplify ordering, drain preceding ordinary work before beginning a private file group, after sealing its pending ordinary draft. Do not let unrelated draft records enter the private group.

### 5.2 Submission and completion algorithm

Let `P` be the resolved build worker count and `W = max(1, 2 * P)`. Use checked arithmetic and existing supported limits.

```text
begin_file_group:
    seal and finish preceding ordinary work
    create private group

on_extracted_contribution(contribution):
    check cancellation
    apply configured record/file boundary rule
    whenever a draft is sealed:
        while outstanding_segment_jobs == W:
            receive one completion
            consume it into this private group
        assign the next checked planned SegmentId
        submit draft to existing segment workers immediately
    nonblocking-consume available completions

on_successful_file_validation(metadata):
    submit any remaining nonempty draft using the same bound
    consume all outstanding completions
    check cancellation and group validity again
    finalize file metadata and accepted observations
    publish the entire group in one revision

on_file_invalidation_or_cancellation:
    stop submission
    drain/join submitted jobs, discarding this attempt's results
    discard its drafts, observations, routes and diagnostics
    apply the existing fallback outcome only if the adapter requires it
```

A completion releases its scheduler slot **when consumed into the private group**, not when the group is published. Otherwise a file producing more than W segments can deadlock before validation. Completed-but-unconsumed results count against W. The bounded extraction queue remains independently bounded.

The contribution callback/coordinator may wait outside the Rayon worker pool. Do not block a worker on a completion whose producer needs the same saturated pool. Verify the current callback execution context; if necessary move the coordinator work to the existing calling thread. Do not add a thread per segment.

No publication or index lock may be held while waiting. Use the existing worker error channel and cancellation mechanisms. On return from a cancelled/failed build, dispatched work must have been joined/drained; do not detach jobs retaining file data.

Use monotonic checked SegmentId allocation. Discarded private attempts may leave unused IDs; never reuse IDs that could be retained by a view/cache. Resolve skipped planned completion positions explicitly so the normal scheduler does not wait forever for a discarded group. Planned ordering and publication revision are separate concepts.

### 5.3 Boundary and metadata rules

Use the configured `SegmentPolicy` for initial builds and mutation paths. Seal before an incoming indivisible contribution would exceed a nonempty draft; seal after admission when a target is reached. Preserve the existing allowance for one indivisible contribution larger than the target and report it; do not treat an already chunked framed file as indivisible.

Count distinct destination file memberships by FileId. A later source chunk can be the first contribution of that physical file to a new destination segment even when `first_range` is false. Preserve local slot and diagnostic remapping.

Keep the adapter distinction between a recoverable malformed record and whole-file invalidation. Do not turn every bad JSONL line into rollback of all prior records. On a whole-file failure, no private records or inferred observations from the discarded attempt become searchable.

Shared file status/diagnostics must be consistent at publication. Existing metadata-related cloning may remain; do not repair memory ownership here. Mutations retain their existing atomic replacement behavior.

### 5.4 Pipeline tests that prove overlap

1. **Finalize before EOF:** use a deterministic test-only observer/channel around extraction and segment completion. Produce enough contributions to seal an early segment, then pause before final validation. Assert that segment finalization has completed while the group is still unvalidated and that queries see none of its private records. Release the producer and verify one atomic publication containing all records. Use bounded test timeouts for deadlock diagnosis, not sleeps to infer overlap.
2. **More than the job window:** one file must produce at least `W + 3` genuine segments. Run with one worker and multiple workers. Assert completion, stable record IDs, one file-group publication, and observed outstanding jobs never above W.
3. **Late invalidation:** invalidate an attempt after at least one private segment completes. Assert no records, routes, or inference observations from that attempt escape; check the established fallback outcome.
4. **Cancellation:** cancel while work is outstanding and again after validation before publication. Assert no partial publication and zero outstanding jobs after return.
5. **Worker failure:** use the existing test injection mechanism or a test-only finalization failure. Ensure errors propagate and draining does not deadlock.
6. **Boundary continuity:** a single file spans several segments with repeated local slots; verify counts, queries, metadata and configured policy after initial build and replacement.

The existing large-JSONL completion test is retained, but it does not by itself prove streaming finalization. The before-EOF test is mandatory.

## 6. Preserve existing behavior and measured work

Do not change default segment targets, batch sizes, worker resolution, benchmark query definitions, schemas, normalization, collation, source-fallback behavior, or verified-cache defaults merely to improve the result. Do not convert a wrong indexed path into a full collection scan as the final repair.

Keep the earlier regressions for inferred ordering after multi-segment mutation, metadata RHS evaluation, adopted-payload reclamation, and configured mutation segmentation. The new changes must not reintroduce them.

Count physical reads, bytes, parses and scans at the operations performing them. Do not synthesize counters from result counts or force baseline counter equality. A fixed wrong-result query may legitimately perform more work than the broken baseline. Explain that difference and compare it to the correct reference and expected access path.

EXPLAIN without ANALYZE must not open related files, build an order cache, or execute a speculative target prefetch. Execution markers used to prove test coverage must be test-only or fit existing detailed profiling.

## 7. Performance acceptance and evidence

The accepted reviewed eight-worker build medians are approximately 800.8 ms ASCII and 923.9 ms Unicode on the existing Musenalm setup. They are reference observations, not absolute targets on another machine. RSS was about 520.1 MB and 545.8 MB and is informational for this task.

Before implementation, preserve/build the reviewed repair executable in a separate target directory or existing benchmark baseline location. Do not switch or reset the active repair worktree to obtain a baseline. Reuse a matching verified executable if available; otherwise use a separate read-only/sibling baseline checkout according to repository practice. Record source SHA, binary hash, toolchain, lockfile, features and commands.

Compare the candidate to the immediate repair baseline under the same dataset and configuration. Run the existing 150-case suite in both alphabets through Repository and progressive Index. Use identical cache settings and worker counts. Compare logical result digests, IDs, order, counts and evidence; success status alone is insufficient.

Required measurements:

- At least three warmed full construction repetitions with one and eight workers, both alphabets.
- The unchanged full query matrix, retaining raw per-query samples and result digests.
- Targeted multi-segment relation FTS and forced source-restricted fixtures, including shared targets.
- A representative AutoIndex and ReportOnly workload; the declared-schema Musenalm benchmark alone does not validate inference behavior/cost.
- A framed-file workload producing more than W segments, with observed finalization-before-EOF and correct atomic publication.

Use the existing performance thresholds as investigation gates: investigate a repeatable build median regression above 10%, and a query regression above 10% in comparable measurements. Investigate individual queries that worsen by more than 20% and at least 1 ms. For noisy cases use at least seven isolated repetitions with the same warm-up protocol and retain those samples. With three samples, nearest-rank p95 is the maximum; do not present it as a stable tail estimate.

Correctness remains mandatory. If a necessary correctness fix causes a repeatable slowdown, identify the exact changed operation and attempt a focused optimization; do not revert the fix, weaken semantics, or conceal the result. Report any remaining regression explicitly. Memory/RSS growth is recorded but is not a pass/fail gate in this assignment.

The reviewed query manifests identify `a587fa661f7faad64402b20f58709f7536090359`, before the final scheduling commit. New evidence must identify the actual final tested implementation SHA. A later report-only commit may differ; explain that distinction. The prior report linked a missing results file. Create the report in section 10 and verify all its relative links resolve to committed files.

## 8. Validation requirements

Run narrow regressions while implementing. Once the changes stabilize, run the repository's required checks, including these commands where still applicable:

```bash
cargo test -p objekte-search-core --lib --locked
cargo test -p objekte-search-core --tests --locked
cargo test --workspace --locked --no-fail-fast
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked
cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked
git diff --check
```

Record pre-existing ignored tests and warnings accurately. Do not disable tests or replace precise ID/plan/coverage assertions with merely nonempty output. A new test must fail against the relevant broken behavior: demonstrate that with the reviewed baseline or a temporary local reversion of the specific fix, then restore the implementation. Do not require every new regression to fail on the baseline if it covers a behavior that already worked; identify which tests are true defect reproducers and which guard preservation.

Use deterministic fixtures and explicit expected IDs/counts. The corpus benchmark complements regressions; it cannot prove an unexercised strategy correct.

## 9. Implementation phases and commits

| Phase | Implementation | Commit boundary and acceptance |
| --- | --- | --- |
| 1 | Record actual branch/base; add targeted relation defect reproducers and trace executed paths | Commit coherent test infrastructure with the relation correction; do not leave the shared branch broken merely to publish failing tests |
| 2 | Qualified native membership, pinned-view adapters, FTS and structured source-restricted semijoins, removal of unused native flat prefetch | Both forced strategies and relation lifecycle/streaming matrix pass; existing normal-path performance remains available |
| 3 | Preserve segment inference observations; exact aggregation including late paths and ReportOnly | Explicit counts, distinct union and shape tests pass |
| 4 | Shared semantic admission decisions and capability/coverage application; replace vacuous threshold test | Global admit/reject/boundary tests pass without shortcut masking; queries agree with reference |
| 5 | Submit private framed-file segments while reading; bounded completion consumption and cancellation | Before-EOF, W+3, late invalidation and cancellation tests pass |
| 6 | Full checks and benchmark matrix; focused regression corrections if needed | All mandatory correctness cases pass; measurements and limitations recorded |
| 7 | Completion report, task disposition and final push | Exact tested SHA and committed evidence linked; original deferred memory/architecture work remains explicitly deferred |

Choose professional commit messages describing the implemented behavior. Each implementation commit should compile and pass the relevant focused tests. Keep unrelated refactoring out. If a shared interface change requires a larger coherent commit, combine adjacent phases rather than creating an intermediate broken public branch.

## 10. Deliverables and definition of completion

Implement on **`integration/native-segment-construction-repair`** and deliver that same branch. Do not ask the user to choose a new architecture or another integration branch for these repairs.

Place this task under the repository task workflow and create:

`docs/ai/generated/native_segment_correctness_completion_results.md`

The report must contain:

1. Starting SHA, final tested implementation SHA, final branch head, commands and binary hashes.
2. A C01–C07 completion table linking implementation locations and exact test names.
3. Which reproducers failed before the fix and why; explicit execution-path evidence for both semijoin strategies and related FTS.
4. Inference observation/capability decisions, meaningful threshold fixtures, and the remaining global memory-accounting limitation.
5. Framed-file overlap, maximum outstanding work, atomic publication, invalidation and cancellation evidence.
6. Full validation outcomes and raw performance links, including targeted reruns of any outliers.
7. A short deferred-work list: whole-segment/temporary sorting copies, RSS reduction, repository-wide inference memory accounting, full R07 migration, remaining R08 component scheduling. Do not describe these as accidental blockers or silently claim them complete.

Completion requires correct relation results across repeated local slots and lifecycle revisions; correct inference reports and semantic capability decisions; actual segment finalization before framed-file EOF; preservation of the accepted query/build behavior; and reviewable committed evidence. A green existing test suite, a field status of `Built`, or a report saying a requirement is implemented is not sufficient without the specific assertions above.

Do not merge into master. Leave the repair worktree and branch ready for the user's review.

## Implementation report

- Task identifier: `native-segment-correctness-completion`
- Completion date: 2026-11-16
- Base revision: `35ea1b83bd5fa08fd321e900b563a473ef12715f`
- Final tested implementation revision: `93210b976fedd45d9f1b1ea4e8eeb293a3b1c21d`
- Integration branch: `integration/native-segment-construction-repair`
- Integration worktree: `/home/simon/source/objekte-integration-native-segment-construction-repair`

The implementation introduces qualified relation record sets and checked pinned-view address conversion, repairs related FTS and both structured semijoin strategies, removes the unused segmented flat relation prefetch, and exercises repeated local slots, forced strategies, streaming, lifecycle mutations, multi-hop traversal, and compaction.

Inference now retains segment observations independently from candidate postings. Collection aggregation is a deterministic two-pass operation over all segment domains, including late paths and empty ancestors. A single semantic admission function feeds both single-domain and collection-wide decisions. Component diagnostics distinguish value dictionaries, short-string coverage, exact empty-string membership, array length, and structural facts. Truncation is explicit, locally incomplete observations retain residual verification, and construction-generation report scope is documented.

Large framed files now submit sealed private segments through the existing segment scheduler while extraction is still active. The completion window is bounded by `2 * workers`; completions are consumed into an ordered private group and published atomically only after file validation. Late invalidation, cancellation at both required stages, and private worker failure drain outstanding work and publish no partial segments or observations.

Implementation commits:

- `68c072894b` - qualified native relation execution
- `17395194a5` - collection-wide inference observations and admission
- `93210b976f` - framed-file finalization during extraction

Validation at the tested implementation revision:

- `cargo test -p objekte-search-core --lib --locked`: 538 passed, 16 ignored, 0 failed
- `cargo test -p objekte-search-core --tests --locked`: passed
- `cargo test --workspace --locked --no-fail-fast`: passed
- `cargo check --workspace --all-targets --locked`: passed
- `cargo clippy --workspace --all-targets --locked`: passed with pre-existing repository warnings
- `cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked`: passed
- `git diff --check`: passed

The relation FTS and late-inference public reproducers were also replayed in a detached worktree at the reviewed base revision. Both failed there (`[]` instead of `S1`; late-field eligible count `1` instead of `513`) and pass on the completed branch.

Three warmed construction repetitions were run at one and eight workers in both alphabets. Final eight-worker medians were `814.378 ms` ASCII and `899.332 ms` Unicode versus immediate repair baseline `800.812 ms` and `923.949 ms`; changes were `+1.7%` and `-2.7%`. The unchanged 150-case Repository and progressive matrices retained identical result digests. Sum-of-medians ratios were `1.0066` for Repository and `0.9972` for progressive execution, with no query crossing the specified `>20%` and `>=1 ms` outlier gate.

The complete evidence report is [native_segment_correctness_completion_results.md](../generated/native_segment_correctness_completion_results.md). It links exact-commit raw construction, Repository, progressive, inference, relation, and framed-file artifacts and records the approved deferred memory/API/scheduling work. No dependency, persistence backend, async runtime, allocator framework, or second worker pool was added. No master merge or force-push occurred. The integration branch is pushed and retained for user review; no subordinate worktree was created for this task.
