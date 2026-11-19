# Objekte query engine deduplication specification

## Objective

Give each of five repeated query decisions one implementation. Remove the old bodies in the same commits that migrate their callers. Preserve query answers, query plans, source-read behavior and the performance of repeated searches. The deliverable is one integration branch ready for the owner to merge.

The reviewed baseline is `master` at `d35ddde56e17114da35d1a76e776615262897409`. At that revision, `crates/core` has 71,813 production Rust lines, including comments and blank lines. Parsing, binding, access selection and execution account for 33,451. The work below targets repeated decisions within that code. Count the actual net reduction, including replacement code. [Reviewed source][root]

## Work to deliver

Paths in this table are relative to `crates/core/src/`. The five rows are acceptance criteria. Each names a piece of code that should cease to have multiple production implementations.

| Commit | Duplicated responsibility | Required result |
|---|---|---|
| 1 | Boolean truth and matched-field collection in `query_execution/relations/evaluation.rs` and `query_predicate/scan.rs` | Base and merged relation evaluation use one Boolean walk over `BoundExpr`. The proof-aware residual evaluator reuses the same Boolean rules while retaining its own leaf and proof handling. |
| 2 | Selected-field full-text matching in `query_execution/relations/evaluation.rs` | One field loop handles local fields, related fields, borrowed text, match evidence and the existing membership-only early exit. Base and merged target lookup stay specialized. |
| 3 | Segment text offer selection in `query_execution/restrictions.rs` and relation candidate pruning in `query_execution/runner/relations.rs` | Estimate and apply use one offer-selection implementation. Serial and parallel relation searches use one implementation for the common candidate-pruning decisions. |
| 4 | Bounded file reads in `query_execution/runner/relations.rs` and target batch bounds in `query_access/relations/semijoin.rs` | One source-file read loop serves base and segment relation scans. One target-batch calculation serves ordinary and segment-qualified target verification. |
| 5 | Page admission in `query_execution/runner/mod.rs` and `query_results/ranking.rs` | Base and segment paths share the bounded-page admission rule. The two repeated exact-segment candidate blocks share the slot-handling work they perform after admission. |

### 1. Share Boolean semantics and keep the leaf operations local

Start with `eval_filter_truth` and `eval_filter_truth_merged`. Both recurse through `BoundExpr`, form branch-local evidence and implement three-valued `AND`, `OR` and `NOT`. Their meaningful difference is how a related leaf resolves its source and target. `eval_filter_truth_membership` repeats the same truth rules with an earlier exit. `Residual::membership`, `with_evidence` and `unavailable` operate on a different tree carrying proofs, prepared term IDs and verification state. [Base walk][bool-base] · [Merged walk][bool-merged] · [Residual evaluator][residual]

Use a private Boolean implementation near `query/semantics/`. For `BoundExpr`, pass a statically dispatched leaf evaluator that closes over the base repository or pinned relation view and mutable work accounting. For `Residual`, keep its representation and borrow its child slices directly. Share the child-fold and evidence rules across the two representations. Reuse `Option<bool>` or the existing `Truth` representation at the relevant boundary. A second boxed tree, per-record trait objects, and prebuilt vectors of child results would add work to the hot path.

| Operator | Truth and visitation | Matched-field behavior |
|---|---|---|
| Leaf | Evaluate the physical leaf once, or consume an applicable proof. Only true matches. | Preserve each leaf’s existing evidence eligibility. Positive ordinary comparisons can contribute the query path, right operand path and relation evidence path. |
| `AND` | False is decisive. Otherwise any unknown makes the result unknown. Empty `AND` is true. | Commit branch fields only after the whole conjunction is true. Preserve fields already in the caller’s sink on failure or unknown. |
| `OR` | True is decisive for membership. With no true branch, any unknown makes the result unknown. Empty `OR` is false. | Collect all contributing true branches in evidence mode. Preserve `Residual::with_evidence`’s existing skip for later branches that cannot contribute after a true result. |
| `NOT` | Invert true and false. Unknown stays unknown. | Suppress the child’s fields. Preserve the caller’s existing visitation policy under negation, including observable errors and work. |

Keep `Residual::proven` scoped to `proof.input`. An unresolved leaf increments residual evaluations exactly where it does today. A missing logical value can still allow a metadata-only predicate to be evaluated. Preserve the unavailable-record route and the prepared verifier hooks. In membership-only execution, Boolean composition needs no evidence scratch sets. Where an existing leaf verifier requires a sink, inspect its actual uses before introducing a discard mode. [Residual proof lookup][proof] · [Prepared leaf verifier][verify]

**Acceptance example.** Give two true branches different field names and a third branch a dangling relation. Evidence-enabled `OR` must return the union of the two true branches. Membership-only `OR` may finish at the first true branch. Negating the dangling leaf alone must never return a hit. Assert results and branch visits with hand-written expectations. The reference search already shares some evaluation infrastructure, so comparing the two engines alone cannot catch a common Boolean error.

### 2. Share selected-field full-text evaluation

`eval_fts_with_relations_inner` and `eval_fts_with_relations_merged` both choose borrowed logical text versus reader-produced text, evaluate local fields and run the same terminal matcher for related fields. Extract the field loop and terminal matching. Supply the physical hop visitor from each caller. The base path still calls `visit_hop`. The pinned path still calls `visit_merged_hop` with its segment-qualified address and reference mode. Preserve the direct path for an entirely local field list. [Base FTS][fts-base] · [Merged FTS][fts-merged]

Distinguish “the caller needs every matched field” from “the caller only needs membership.” Preserve today’s choice at each call site, including merged callers that currently visit all fields even when the outgoing hit omits evidence. Changing their visitation can change I/O, errors or budget consumption. Keep target decoding, cache admission, cache-lock lifetime and release before the terminal callback under the existing physical owners. Preserve multi-value occurrence positions, selectors and the distinction between borrowed value text and reader-defined text. A new format must continue to work through its registered reader.

### 3. Share offer selection and relation candidate pruning

**Text offers.** `segment_fts_estimate_with_term` and `segment_fts_bounds_with_term` both construct a `NativeSegmentTextProvider`, assemble its catalog, propose offers and call `choose_offer`. Put that sequence in one private operation. Keep the provider and catalog alive through the selected continuation so its operation ID belongs to the instance that issued it. The estimate returns cost/cardinality without calling `apply` or reading source bytes. Execution applies the selected offer once to the effective input and preserves `possible` and `Proven::{None, All, Subset}`. Preserve the separate no-offer results, request-local term IDs, test-provider injection and native-provider suppression. [Bounds][bounds] · [Estimate][estimate] · [Provider contract][contract]

**Relation candidates.** `try_parallel_relation_page` and `search_segmented_relations` both intersect filter and FTS candidates and lazily derive a relation restriction. Share this computation, including the three states of the lazy prune result and the existing `local_count > 256` decision. Keep the differences visible at the callers. The parallel route can decline an exact plan and fall back. The serial route can attach exact evidence and enumerate sparse candidate files directly. Preserve the distinction between unrestricted input and a proven-empty set. [Parallel candidate selection][parallel] · [Serial candidate selection][serial]

Move owned bitmaps, borrow existing sets or share their `Arc`. In particular, remove the eager `relation_may.clone()` used as a `map_or` default in both copies wherever consuming the owned bitmap suffices. Preserve active membership, the pinned domain and sparse enumeration. A slot number only has meaning together with its collection, publication and physical segment. [Membership representations][slots]

### 4. Share range processing at the point where it is identical

**Source files.** In `search_segmented_relations`, keep the base and segment loops that determine eligible file IDs, ranges, active slots and proof fields. Once a nonempty ordered range list and pinned file have been resolved, use one private function to open the revision-checked reader, report an unavailable source, read batches and deliver owned bytes to the existing record processor. Carry the physical address and exact proof evidence supplied by the caller. Borrow the range slice. [Base source loop][baseio] · [Segment source loop][segio]

Preserve the 128-record and 1 MiB batch boundaries, including the existing handling of one record larger than the batch target, the configured per-record check before opening, `charge_reads` on successful open, distinct-file accounting and the first-row-of-file flag. Open failure skips that file. Read failure records the failing batch’s first range and stops that file. A record-processing error still propagates. Keep the reference route’s independent active-record enumeration and source-backed relation resolution. It may share the mechanics of reading the bytes it selected. [Independent reference entry][oracle]

**Targets.** `verify_target_records` and `verify_qualified_targets` duplicate the rules for forming and charging the next target batch. Share the bounds calculation and validation in `semijoin.rs`. Keep the local-slot and segment-qualified mappings and their distinct source sessions. Reuse the borrowed slice on the base path. Validate the current request’s per-target limit and the fixed hard limit for every admitted record before issuing that batch’s read. Charge once. Keep the source and target budget policies distinct. [Base target verification][target1] · [Qualified target verification][target2]

### 5. Remove repeated winner admission in the main runners

The proven-slot loop and all-exact candidate loop in `search_segmented_impl` repeat the same cap check, comparison to the worst winner, and delayed record and rank-key lookup. The base runner already has `would_enter_page`. Give the admission decision one owner in `query_results/ranking.rs`. Let each physical path supply a borrowed, lazy comparison. An empty page capacity rejects immediately. A non-full heap accepts without comparing. A full heap accepts only a strictly better key under the existing tie breakers. Preserve fallible segment comparisons and the base route’s inexpensive comparison. [Proven slots][wins1] · [Exact candidates][wins2] · [Base page admission][ranking]

In the two segment loops, share the common path for an eligible slot. Keep differences in evidence production, cancellation checkpoints and diagnostics at the caller or as lazy inputs. If the slot misses the page and verified-cache collection is off, avoid fetching its record or constructing its full rank key. With cache collection on, a losing slot can still belong to complete cached membership. Count-only execution must keep its shortcuts. Stored-order traversal, the top-k heap, streaming delivery, cancellation and reference evaluation keep their existing roles.

## Verification and performance

1. **Record the baseline once.** Use the actual integration branch point and final candidate SHA. Capture the Rust toolchain, profile, worker count, corpus fingerprint, query-cache setting and executable revision. Keep baseline and candidate binaries distinct. Use the same test selections on both.
2. **Exercise the branches that changed.** Run the existing `query_execution::relations::evaluation`, `query_access::relations::semijoin`, `query_execution::restrictions`, `query_execution::runner`, and `query_execution::streaming` tests as relevant after each commit. Run the three integration tests `relation_oracle`, `relation_occurrence_oracle` and `non_json_relations`. Reuse existing tests first. Add a focused test only where an observable distinction lacks coverage.
3. **Check work as well as answers.** Compare ordered IDs, counts and completeness, matched fields, locators or requested payloads, source diagnostics and error class. Check selected strategies, provider applications, candidate/proven counts, record parses, files and bytes read, target reads, edge visits and cache hits. Compare like-for-like configurations. Keep the estimate-only `explain` call separate from measured execution. Assert against fixed expected answers for the Boolean edge cases and use the source-backed reference for end-to-end differential checks.
4. **Use a short release benchmark.** Run `bench_musenalm` with `--cases Q001,Q042,Q083,Q136 --profiles ascii-off --repetitions 3 --query-workers 4 --verified-cache-bytes 0` on the same corpus. These cases cover full text, exact string lookup, negated array membership and a deep sorted page. Run the existing deterministic relation fixtures separately. One checks a 40-source target-first result and a selective source-restricted request; another checks a source-dependent comparison and two sort-target reads. Time the actual relation searches in release mode on the two revisions as well. Reuse a built fixture, disable the verified-result cache for this comparison, and exclude fixture creation, reference checks and explain calls from the timed interval. If an individual query is too short for a stable measurement, time a small repeated batch and report time per query. Use the existing warm-cache, ordering and streaming tests for their specialized branches. [Benchmark driver][bench] · [Relation strategy fixture][fixture] · [Residual sort fixture][sortfixture]
5. **Resolve discrepancies before handoff.** Each requested case and repetition must succeed on both revisions. Compare ordered-result digests and work fields. For a slower query, alternate fresh baseline and candidate batches for that case and inspect its plan, reads, allocations where relevant, and route before accepting the consolidation. Preserve the faster specialization if sharing causes a repeatable slowdown. Expand the benchmark only for affected routes or an unexplained result. Finish with `cargo check --locked --workspace --all-targets --all-features` and `cargo test --locked -p objekte-search-core`. Reproduce any claimed inherited failure at the branch point.

Particularly valuable existing checks cover the second target exceeding its read budget, repeated local slot IDs in different segments, unknown targets under negation, multiple true `OR` branches, membership-only FTS, pagination before an exact count completes, and cancellation before cache publication. For a new batch helper, cover an empty range list, a first record over the batch target and a failure after the first successful batch. [Semijoin tests][budgettest] · [Relation evaluator tests][evaltests] · [Runner tests][runtests] · [Streaming tests][streamtests]

## Branch and handoff

Use task ID `query-engine-deduplication`, branch `integration/query-engine-deduplication` and sibling worktree `/home/simon/source/objekte-integration-query-engine-deduplication`. Follow `docs/ai/README.md`. One supervisor integrates the commits and pushes each commit. If delegating, give the Boolean and FTS work to one owner and serialize edits to `runner/relations.rs`. The owner merges the finished branch into `master`. [Repository workflow][workflow]

In the completion report, map each of the five rows to the functions removed, surviving implementation and callers. Show production lines added and removed, including shared helpers and moved files, with test churn reported separately. Use the same counting method on both revisions. Separate file moves and formatting churn from the production reduction. The combined production diff should be smaller and each named duplicate should be gone. Report any retained specialization, the behavior or measurement that justifies it, the tested SHA and the focused results. Append the report to the completed task under `docs/ai/done/` and give the owner the final branch tip.

[root]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/lib.rs#L16
[bool-base]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_execution/relations/evaluation.rs#L124
[bool-merged]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_execution/relations/evaluation.rs#L776
[residual]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_predicate/scan.rs#L614
[proof]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_predicate/scan.rs#L597
[verify]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_predicate/mod.rs#L305
[fts-base]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_execution/relations/evaluation.rs#L269
[fts-merged]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_execution/relations/evaluation.rs#L873
[bounds]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_execution/restrictions.rs#L704
[estimate]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_execution/restrictions.rs#L772
[contract]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_access/contract.rs#L304
[parallel]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_execution/runner/relations.rs#L1560
[serial]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_execution/runner/relations.rs#L2424
[slots]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_access/membership.rs#L162
[baseio]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_execution/runner/relations.rs#L2315
[segio]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_execution/runner/relations.rs#L2543
[oracle]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/reference/relations.rs#L68
[target1]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_access/relations/semijoin.rs#L595
[target2]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_access/relations/semijoin.rs#L857
[wins1]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_execution/runner/mod.rs#L2177
[wins2]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_execution/runner/mod.rs#L2350
[ranking]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_results/ranking.rs#L210
[bench]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/examples/bench_musenalm.rs#L85
[fixture]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_access/relations/semijoin/tests.rs#L1174
[sortfixture]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_execution/relations/evaluation/tests.rs#L3282
[budgettest]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_access/relations/semijoin/tests.rs#L168
[evaltests]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_execution/relations/evaluation/tests.rs#L3663
[runtests]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_execution/runner/tests.rs#L6442
[streamtests]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/crates/core/src/query_execution/streaming/tests.rs#L684
[workflow]: https://github.com/Simon-Martens/objekte/blob/d35ddde56e17114da35d1a76e776615262897409/docs/ai/README.md#L31

## Implementation report

- Task identifier: `query-engine-deduplication`
- Completion date: 2026-11-19
- Base commit: `d35ddde56e17114da35d1a76e776615262897409`
- Integration branch: `integration/query-engine-deduplication`
- Integration worktree: `/home/simon/source/objekte-integration-query-engine-deduplication`
- Production implementation and benchmark-tested tip: `093726e6b4b8168499133bd35086188e50e93b64`

### Delivered changes

| Row | Removed duplicate bodies | Surviving implementation and callers |
| --- | --- | --- |
| 1 | The recursive Boolean branches in `eval_filter_truth`, `eval_filter_truth_merged`, `eval_filter_truth_membership`, `Residual::unavailable`, `Residual::membership` and `Residual::with_evidence`. | `query/semantics/boolean.rs` owns three-valued `AND`, `OR`, `NOT`, evidence branch sinks and decisive membership exits. `BoundExpr` and `Residual` implement the small `BooleanTree` shape adapter. Their leaf evaluators retain base/merged relation resolution, proof lookup, prepared verification, evaluation accounting and evidence eligibility. |
| 2 | The local/related field loops in `eval_fts_with_relations_inner` and `eval_fts_with_relations_merged`, plus their duplicated terminal matcher. | `eval_fts_fields` owns field dispatch, local matching and evidence collection. `eval_related_fts_terminal` owns borrowed-value versus reader-text matching. Base callers still use `visit_hop`; merged callers still use `visit_merged_hop`. Membership-only base execution still stops at its first hit, while merged callers retain their visit-all policy. |
| 3 | Provider construction, catalog proposal, offer choice and no-offer handling in `segment_fts_bounds_with_term` and `segment_fts_estimate_with_term`; candidate intersection and lazy relation pruning in parallel and serial relation runners. | `with_segment_text_offer` keeps the provider and catalog alive through estimate/apply and passes the selected offer to a scoped continuation. `prune_relation_candidates` owns filter/FTS intersection, the `local_count > 256` decision, lazy relation-source pruning and unrestricted versus proven-empty composition. Relation candidate bitmaps are shared through `Arc`; the eager deep clone is gone. |
| 4 | Base and segment source reader setup/batch loops; target batch validation and range formation in `verify_target_records` and `verify_qualified_targets`. | `read_relation_source_file` owns revision-checked open, source diagnostics, successful-open charging, the 128-record/1 MiB batching, read failure handling, distinct-file accounting and first-row delivery with the caller’s physical address and proof fields. `target_batch_bounds` owns per-record limits and batch bounds for both target verification mappings while retaining their separate source sessions. |
| 5 | Cap/full-heap comparison in the proven-slot and all-exact segment loops; repeated admitted-slot lookup, rank-key construction, cache publication and winner delivery. | `would_enter_rank_key_with` in `query_results/ranking.rs` owns the bounded admission rule and invokes a lazy caller comparator only for a full heap. `would_enter_page` delegates to it for the base route. Both segment loops call it with lifecycle-aware, fallible comparison. `offer_admitted_segment_slot` owns the common post-admission slot path. |

All named duplicate production bodies are removed. The remaining specializations are
intentional: base versus pinned target lookup, estimate versus apply, unrestricted
versus proven-empty relation domains, local versus segment-qualified target sessions,
fallible lifecycle comparisons versus the base in-memory comparison, membership versus
all-evidence visitation, and count-only versus cache-completing admission behavior.

### Source-size account

The specification’s reviewed baseline is 71,813 production Rust lines. The same
file-line method was used on the implementation tip and excludes dedicated test
modules and `#[cfg(test)]` items while retaining comments and blank lines. The
uncommitted test timing probes are excluded from production accounting.

Production additions and deletions, including shared helpers and the new Boolean
module, are `+874/-958`, a net reduction of 84 lines. Applying that exact production
delta to the reviewed baseline gives 71,729 production lines. File moves: none.
Formatting-only churn: none. Test churn is reported separately below.

Test-only additions are +95 lines in the Boolean walker unit tests, +48 in the
target-batch tests, +38 in ranking tests, and +56 in fixture timing probes, for
+237 test lines. There are no test deletions.

| Commit | Production additions | Production deletions | Production net | Test additions |
| --- | ---: | ---: | ---: | ---: |
| `69347160d6` | 333 | 385 | -52 | 95 |
| `5c62d84f2f` | 87 | 91 | -4 | 0 |
| `5a68799971` | 163 | 168 | -5 | 0 |
| `66c26ec48c` | 135 | 199 | -64 | 48 |
| `4fb6ffdc98` | 156 | 115 | +41 | 38 |
| `093726e6b4` | 0 | 0 | 0 | 56 |

The combined production diff is 84 lines smaller.

### Validation

- `cargo test --locked -p objekte-search-core query_execution::relations::evaluation`:
  passed after commits 1 and 2.
- `cargo test --locked -p objekte-search-core query_execution::restrictions`:
  passed after commit 3, including injected-provider estimate/apply behavior.
- `cargo test --locked -p objekte-search-core query_execution::runner`: passed after
  commits 3, 4 and 5.
- `cargo test --locked -p objekte-search-core query_access::relations::semijoin`:
  passed after commit 4, including empty, oversized-first and later-batch target
  failures.
- `cargo test --locked -p objekte-search-core query_execution::streaming`: passed.
- `cargo test --locked -p objekte-search-core --test relation_oracle --test relation_occurrence_oracle --test non_json_relations`: passed.
- `cargo test --locked -p objekte-search-core`: passed, 617 unit tests and all
  integration/doc tests; 16 manual or large tests ignored as designed.
- `cargo check --locked --workspace --all-targets --all-features`: passed.
- `cargo test --locked --workspace`: passed.
- The focused Boolean tests cover empty identities, decisive membership exits,
  multi-branch evidence union, preserved unknown truth and unknown under negation.
- The target-batch test covers an empty range list, an oversized first record and
  failure after the first successful 128-record batch.
- The page-admission test proves that zero capacity and a non-full heap do not
  invoke the comparator.

### Performance

The required release run used the same physical Musenalm corpus, four query
workers, ASCII bigrams and zero verified-page cache bytes. Ordered-result
digests and every non-timing work column match for Q001, Q042, Q083 and Q136.

Final candidate medians versus baseline:

| Case | Baseline | Final candidate | Change |
| --- | ---: | ---: | ---: |
| Q001 | 18.643 ms | 13.334 ms | -28.5% |
| Q042 | 95.207 us | 102.898 us | +8.1% |
| Q083 | 6.865 ms | 6.734 ms | -1.9% |
| Q136 | 3.982 ms | 3.829 ms | -3.9% |

Q042's initial difference was not repeatable: three alternating fresh
baseline/candidate batches measured -5.9%, -3.0% and +1.1%, with identical
result digests, work counters and normalized plans.

The deterministic fixture medians were 18.846 to 16.797 us for
`indexed_relation_filter`, 15.025 to 15.057 us for `selected_relation_fts`,
and 35.423 to 33.857 us for `source_dependent_residual_sort`. These retain the
40-source target-first route, selective source-restricted route and two
sort-target reads. Raw samples, manifests, plans and comparison commands are in
`docs/ai/generated/query-engine-deduplication/benchmark/`.

### Deviations, limitations, and cleanup

- No approved behavior deviation. No issue report was required.
- The benchmark's schema fingerprint and actual `queries.json` match. Its
  workload fingerprint differs because that fingerprint includes the checkout
  path of the workload file; both files have the same Git blob hash.
- The temporary baseline worktree used only to run identical test timing probes
  was removed after validation.
- The integration worktree and branch remain for the owner to merge.
