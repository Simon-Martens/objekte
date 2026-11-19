# Objekte query engine deduplication repair specification

## Assignment

Finish the ownership and evidence cleanup identified after the query-engine deduplication merge. The reviewed `master` revision is `d06444547660693cf168bce45a9db8ee360c31cd`. Its tree is identical to the delivered integration tip `ba82f07`. The review reproduced the recorded result digests, work counters and normalized plans for the four Musenalm cases. Use the merged implementation as the behavior and performance baseline for this repair.

Deliver one branch, `integration/query-engine-deduplication-repair`, from current `master`. Use a sibling worktree at `/home/simon/source/objekte-integration-query-engine-deduplication-repair` and follow `docs/ai/README.md`. The owner merges the branch. Paths in the four code changes below are relative to `crates/core/src/`. [Reviewed merge](https://github.com/Simon-Martens/objekte/commit/d06444547660693cf168bce45a9db8ee360c31cd)

## 1. Preserve bitmap ownership during relation pruning

**Location.** In `query_execution/runner/relations.rs`, `prune_relation_candidates` receives the filter bound as an `Arc<RoaringBitmap>`. Its filter-only arm calls `filter.as_ref().clone()`. When no relation restriction follows, the function puts that complete copy back in an `Arc`. A broad filter pays for copying every bitmap container.

Carry `Option<Arc<RoaringBitmap>>` through local candidate selection. Move a filter-only handle directly into the result. Wrap an owned FTS-only result once. Where two actual restrictions exist, intersect them using borrowed operands or an already-owned bitmap. Return the existing handle when a later stage adds no restriction. The two callers use `relation_candidates` only to ask whether a usable relation restriction was returned. A Boolean can carry that fact and allow the relation bitmap to be consumed directly, including when it is the sole restriction.

Preserve these distinctions in both the serial and parallel callers.

| Inputs or condition | Required result |
|---|---|
| No applicable restriction | `None` denotes unrestricted input. |
| Exactly one available restriction | Return that bitmap, including a present empty bitmap. Preserve an existing shared allocation. |
| Local and relation restrictions | Return their intersection. A usable, empty relation restriction still establishes that the restriction was available. |
| Lazy relation planning | Preserve the three states of `Option<Option<RelationTargetPrune>>`, the strict `local_count > 256` threshold and reuse across segments. |
| Exact relation plan | Preserve parallel fallback when a required posting is unavailable, serial proof eligibility, active membership, and the collection and segment of every slot. |

**Acceptance.** Exercise the filter-only case and demonstrate reuse with `Arc::ptr_eq` at the composition boundary, or show the direct move in the patch if the producer handle is private. Check unrestricted, empty and intersecting results against fixed expected slots. Reuse the repeated-local-slot relation fixture for segment identity. [Candidate selection](https://github.com/Simon-Martens/objekte/blob/d06444547660693cf168bce45a9db8ee360c31cd/crates/core/src/query_execution/runner/relations.rs#L367)

## 2. Give exact segment slots one page-admission path

**Location.** In `query_execution/runner/mod.rs`, the proven-slot and all-exact loops in `search_segmented_impl` repeat the same worst-winner comparison and early rejection. The proven loop also turns `Vec::new()` into an `Arc<[String]>` for the shared handler, which calls `to_vec()` to construct the hit. This happens even with the verified cache disabled.

Move the repeated admission call and its fallible `segment_compare_to_rank` closure inside the common slot handler. Keep `would_enter_rank_key_with` in `query_results/ranking.rs` as the owner of the capacity and strict comparison rule. Supply evidence lazily, for example with `FnOnce() -> Vec<String>`. The proven loop can pass `Vec::new` and the all-exact loop can pass `|| exact_fields(slot)`. Keep each loop's cancellation checks, total and exact-match counters in their present execution order.

| Slot outcome | Required ownership and work |
|---|---|
| Loses page, cache collection off | Return after admission. Skip evidence construction, post-admission record resolution and full rank-key construction. |
| Enters page, cache collection off | Move the field vector into the hit. An evidence-disabled hit receives an empty vector directly. |
| Loses page, cache collecting | Retain the member and its evidence for the complete cached result. Construct no page hit. |
| Enters page, cache collecting | Obtain evidence once. Supply the hit and cached member with their existing owned representations. Limit any necessary field copy to this dual-consumer case. |

Preserve zero-capacity and count-only shortcuts, equality tie-breakers, comparison errors, winner memory accounting, and all-or-nothing cache publication. Keep the stored-order traversal and the existing public result types. Keep the shared call smaller than the pair of blocks it replaces.

**Acceptance.** Use a published segment and a query that actually reaches the heap loop. Exercise evidence on and off, a full heap with a losing slot, and a cache-enabled second page that includes an earlier loser. Check ordered IDs, count and matched fields. With cache collection off, a losing slot must never invoke the evidence producer. Use an existing fixture or a test-local counter for that assertion. [Slot handler](https://github.com/Simon-Martens/objekte/blob/d06444547660693cf168bce45a9db8ee360c31cd/crates/core/src/query_execution/runner/mod.rs#L1762)

## 3. Share the matched-field rule for bound filter leaves

**Location.** In `query_execution/relations/evaluation.rs`, `eval_filter_truth` and `eval_filter_truth_merged` each implement the same eligibility test and insert the same paths.

Extract one private function for the completed leaf result. Give it the borrowed bound term, its `Option<bool>` result and the optional evidence sink. Both callers evaluate their leaf once through their existing base or pinned-view lookup and then invoke the function. Return immediately when the sink is absent, truth is false or unknown, the term is derived, or the operator is `Ne` or `NotContains`. Otherwise insert the left query path, the right query path when present, and the relation evidence path when present. Preserve set semantics. The Boolean walker continues to own branch-local evidence, negation and visitation. The proof-aware residual evaluator retains its existing verifier and proof handling.

**Acceptance.** Use hand-written expected field sets for a true field-to-field comparison and a true relation comparison, plus suppression for a negative operator, a derived term and an unknown target under negation. Exercise base and published-segment evaluation for equivalent records. Equality between those routes alone cannot detect a mistake in their new common helper. [Base leaf](https://github.com/Simon-Martens/objekte/blob/d06444547660693cf168bce45a9db8ee360c31cd/crates/core/src/query_execution/relations/evaluation.rs#L137) · [Merged leaf](https://github.com/Simon-Martens/objekte/blob/d06444547660693cf168bce45a9db8ee360c31cd/crates/core/src/query_execution/relations/evaluation.rs#L759)

## 4. Test a legal first target larger than the batch byte target

Extend the existing `target_batch_bounds_handle_empty_oversized_and_later_failures` test in `query_access/relations/semijoin/tests.rs`. Its existing 1,025-byte example correctly rejects a record under a 1,024-byte **per-record limit**. The missing case is a record that exceeds the **1 MiB batch target** while satisfying both per-record limits.

Set `large = TARGET_BATCH_BYTES + 1` and give the request a per-record limit of at least `large`. Use two contiguous synthetic byte ranges with lengths `large` and one byte. From start index 0, assert success with `(1, large)`. From index 1, assert success with `(2, 1)`. This proves that the first record makes forward progress alone and the second remains available. The test requires two range descriptors and no source-file allocation. Retain the existing rejection and second-target budget tests. The merged implementation already admits this legal record. [Batch bounds](https://github.com/Simon-Martens/objekte/blob/d06444547660693cf168bce45a9db8ee360c31cd/crates/core/src/query_access/relations/semijoin.rs#L44) · [Existing tests](https://github.com/Simon-Martens/objekte/blob/d06444547660693cf168bce45a9db8ee360c31cd/crates/core/src/query_access/relations/semijoin/tests.rs#L168)

## 5. Correct the completed task's source-size account

Update the source-size section of `docs/ai/done/objekte_query_engine_deduplication_specification.md`. The original report assigns every deleted line to production. Some of the deleted lines were test-only. Use these independently verified endpoint counts for the **original merge**.

| Scope in `crates/core/src` | Before (`d35ddde`) | After (`d064445`) | Added | Removed | Net |
|---|---:|---:|---:|---:|---:|
| Production Rust lines | 71,813 | 71,743 | 872 | 942 | −70 |
| Test-only Rust lines | 53,778 | 54,001 | 239 | 16 | +223 |

Count physical lines including comments and blank lines, follow Rust modules from `lib.rs`, and classify test-only modules and inline `#[cfg(test)]` items on each revision. Classify removed diff lines against the old tree and added lines against the new tree. Replace the inaccurate per-commit table with the verified endpoint comparison or recompute its rows using the same classifier. Report this repair's own delta separately against its actual branch point. Record the counting command or script used, so the next reviewer can reproduce it. [Original size account](https://github.com/Simon-Martens/objekte/blob/d06444547660693cf168bce45a9db8ee360c31cd/docs/ai/done/objekte_query_engine_deduplication_specification.md#L138)

## Verification and handoff

Run focused tests while making each change. Existing fixtures include `relation_semijoin_distinguishes_repeated_local_slots_across_segments`, `exact_posting_count_preserves_page_evidence_with_and_without_membership_cache`, and `membership_only_matches_complete_evidence_for_local_relation_and_fts_routes`. Add focused cases where those fixtures bypass a repaired path. Finish with the following commands.

```sh
cargo check --locked --workspace --all-targets --all-features
cargo test --locked -p objekte-search-core
```

For performance, run two short release comparisons against the repair branch point. Exercise a published segment with a broad filter and no effective relation restriction, and an exact segmented page with evidence and verified caching disabled that reaches the repaired heap loop. Verify route selection separately and time repeated searches on an already-built fixture. Record ordered-result equality, relevant candidates, exact matches, parses and reads, plus median time per query. If a slowdown repeats in alternating baseline and candidate runs, investigate that path before handoff. These two cases cover the ownership changes directly.

Append the implementation report to this task under `docs/ai/done/`. Give the owner the branch point, tested revision, final branch tip, focused results, and the production and test line deltas.

## Implementation report

- Task identifier: `query-engine-deduplication-repair`
- Completion date: 2026-11-19
- Branch point and behavior/performance baseline: `d06444547660693cf168bce45a9db8ee360c31cd`
- Integration branch: `integration/query-engine-deduplication-repair`
- Integration worktree: `/home/simon/source/objekte-integration-query-engine-deduplication-repair`
- Tested source revision: `da7a1e57faf29f0889592e5b3df56b2a9c4ffcf2`
- Final implementation/evidence tip before this report: `696736ad2d`

### Delivered changes

| Repair | Result |
| --- | --- |
| Bitmap ownership | Local candidate selection now carries `Option<Arc<RoaringBitmap>>`. Filter-only handles move directly, FTS-only handles are wrapped once, relation-only handles move directly and dual restrictions create one intersection. `relation_available` carries the Boolean fact separately, so relation bitmaps are no longer retained solely for a caller check. |
| Exact segment admission | `offer_admitted_segment_slot` now owns the full-heap comparison and early rejection. Evidence is an `FnOnce() -> Vec<String>` evaluated only after a slot is admitted or cache collection is required. A cache-disabled loser does no evidence, record or rank-key work. The dual cache/page path is the only path that copies the field vector. |
| Filter-leaf evidence | `emit_bound_filter_evidence` owns truth/derived/negative suppression and inserts the left path, scalar right operand or relation source operand, and relation evidence path. Base and merged evaluators call it after their existing physical leaf lookup. |
| Target batch test | The existing bounds test now covers a legal `TARGET_BATCH_BYTES + 1` first record, proving `(1, large)` forward progress followed by `(2, 1)` for the second record. |
| Source-size account | The completed deduplication document now records the verified 71,813→71,743 production and 53,778→54,001 test-only endpoints. The inaccurate per-commit table was replaced. A reproducible classifier and JSON account are committed under `docs/ai/generated/query-engine-deduplication-repair/`. |

### Source-size delta

Run:

```sh
python3 docs/ai/generated/query-engine-deduplication-repair/count_core_rust_lines.py \
  --from d06444547660693cf168bce45a9db8ee360c31cd \
  --to da7a1e57faf29f0889592e5b3df56b2a9c4ffcf2
```

The script verifies both original endpoint physical totals, then classifies changed lines against old and new trees.

| Scope | Added | Removed | Net |
| --- | ---: | ---: | ---: |
| Production Rust | 98 | 109 | -11 |
| Test-only Rust | 524 | 23 | +501 |

There are no file moves. The repair remains production-line negative.

### Validation

- `cargo test --locked -p objekte-search-core query_execution::runner`: 54 passed, 1 ignored.
- `cargo test --locked -p objekte-search-core query_execution::relations::evaluation`: 33 passed.
- `cargo test --locked -p objekte-search-core query_access::relations::semijoin`: 12 passed.
- `cargo test --locked -p objekte-search-core`: 620 passed, 16 ignored; all integration tests and the doc-test passed.
- `cargo check --locked --workspace --all-targets --all-features`: passed.
- `cargo test --locked --workspace`: passed.
- `relation_semijoin_distinguishes_repeated_local_slots_across_segments`: passed after bitmap ownership repair.
- The exact-page counter test proves five evidence calls for five winners versus 100 when complete cache membership is collected, and zero calls on the cached second page.
- The filter-leaf test checks fixed sets for field-to-field and relation comparisons, negative/derived/unknown suppression, and equal base versus published-segment evidence.
- The oversized target test checks empty, rejected, later-failing and legal-first-over-batch cases.

### Performance

Release runs used warm, already-built fixtures and 21 timed searches per batch. Raw samples, counters and route evidence are in `docs/ai/generated/query-engine-deduplication-repair/`.

- Exact segmented page, verified cache off: alternating batch medians were baseline 15.488/14.196/28.961 us and candidate 11.314/11.308/11.830 us. Median-of-batch-medians improved 26.95%. Count, ordered IDs, 100 candidates, 100 exact matches and zero reads/parses were identical.
- Broad filter with no effective relation pruning: both returned count 2 with 8 candidates, 10 file opens, 527 bytes, 9 parses, 2 target reads and 3 edge visits. The OR selected neither relation access route. Alternating batch changes were +5.1%, +10.2% and -2.3%; sub-100-us samples overlap and include scheduler outliers. No route or work-counter change repeated.

### Handoff and cleanup

- No behavior deviation or unresolved issue.
- The temporary baseline benchmark worktree is removed after preserving the timing harness and results in this branch.
- The integration worktree and branch remain for the owner to merge.
