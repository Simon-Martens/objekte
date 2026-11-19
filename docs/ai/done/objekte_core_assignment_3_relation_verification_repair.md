# Objekte · Assignment 3 relation verification repair

**Review decision.** Hold the final merge for a focused relation comparison and a small module-ownership correction. The main benchmark, test inventories and construction signatures reconcile. I found no query-result regression in the reviewed implementation.

| Reference | Revision |
|---|---|
| Series baseline B | `0f1d5e5ffa566a911f926ffe564e49c0c69ded77` |
| Assignment 2 integration entry, including the trace repair | `9ae617b5df3f0a082aa923195a28019d5c36815d` |
| Tested Assignment 3 source C | `acac94f486ee0f9d35eb5ba834d26cdc01eb1c5e` |
| Reviewed delivery | `bf4c1a32debe228d124d73192a2f3b01bb4895fe` |

Continue on `integration/core-refactoring-completion` in its existing sibling worktree. Keep the original measurements. Publish the repair for the owner’s merge.

## 1. Measure the three relation paths on both revisions

Q088, Q102 and Q122 all selected ordinary postings, with zero relation target reads and zero edge visits in all nine samples. The 450 standard Musenalm samples also recorded zero relation work. The existing focused tests *do* exercise traversal, including source-backed reference checks. Their logs record pass/fail, and several assertions allow a range of read counts. Both runs can pass with different amounts of work. [Recorded probes](https://github.com/Simon-Martens/objekte/blob/bf4c1a32debe228d124d73192a2f3b01bb4895fe/docs/ai/generated/core-refactoring-completion/assignment-3-relations-final-integration/relation-probe-summary.json)

Use the deterministic fixtures already in the semijoin and evaluation test suites. Run the same requests on the repaired A2 entry and the final repair source with the same worker count, cache settings and publication state. These three cases cover the missing comparison.

| Path | Existing request | Observation that establishes coverage |
|---|---|---|
| Indexed relation FILTER | `entry.title ?~ 'Hymn'` in `both_physical_strategies_keep_independent_witnesses_and_reuse_targets` | Selected `RelationTargetFirst`, ordered `S00` through `S39`, one reverse posting and 40 reverse members |
| Selected-field relation FTS | The same fixture’s `selected_fts(Some("title = 'rare'"), "H", false)` | Capture the selected plan for this FTS request itself, ordered `S00`, one relation edge visit and its matched-field evidence |
| Source-dependent residual evaluation | `entry.title = local_title SORT BY entry.alt_title ASC` in `related_sort_does_not_reuse_an_incomplete_filter_target_projection` | Cross-record field comparison, ordered `m8, m1`, two sort-target reads and agreement with source-backed reference search |

For each case and revision, retain the actual selected strategy or residual route, candidate and proven membership where exposed, ordered IDs and requested evidence, and the source/target read, byte, edge and reuse counters. Emit the observations from the running fixtures. Compare them by case and explain any work difference. Keep estimate-only explain outside measured execution. A small test-only probe can use each revision's module paths. [Semijoin fixture](https://github.com/Simon-Martens/objekte/blob/bf4c1a32debe228d124d73192a2f3b01bb4895fe/crates/core/src/query_access/relations/semijoin/tests.rs#L1130-L1202) · [Residual fixture](https://github.com/Simon-Martens/objekte/blob/bf4c1a32debe228d124d73192a2f3b01bb4895fe/crates/core/src/query_execution/relations/evaluation/tests.rs#L3231-L3297)

## 2. Reject incomplete or failed evidence

I replayed the committed `summarize_relation_probe.py` and reproduced its saved output. In separate copies it also accepted paired empty inputs, a missing case on both sides, a missing repetition on both sides, and identical error statuses. It reported zero mismatches in each case. [Verifier](https://github.com/Simon-Martens/objekte/blob/bf4c1a32debe228d124d73192a2f3b01bb4895fe/docs/ai/generated/core-refactoring-completion/assignment-3-relations-final-integration/summarize_relation_probe.py#L27-L75)

Declare each probe’s expected case IDs and repetition keys independently of the input. Require each key exactly once per side, a successful status and an empty error. Require one plan and one ordered-result record per expected case. Validate the intended revisions from the run manifests. For the new traversal probes, validate the route and fixture-specific observation in section 1 before comparing the two sides. For the archived posting probes, keep their accurate posting classification. Distinguish matching schema bytes from a successful schema compatibility check in the report.

Run a valid control, then independently remove a case, remove a repetition, duplicate a plan, replace both sides with empty evidence and inject the same failed query on both sides. Each invalid input must exit unsuccessfully for the intended reason. Compare all recorded work fields on the successful input.

## 3. Complete the evaluation/view split

The original assignment assigns the independent logical-occurrence walk and source/target FTS text selection to `evaluation.rs`. The final `view.rs` defines `logical_references`, `relation_source_uses_value_text` and `relation_target_uses_value_text`. Move those responsibilities to evaluation and let the pinned view call the logical-occurrence helper at the narrowest required visibility. Keep the ordered logical-value walk and its result representation intact. Use `view.repository_root()` in evaluation wherever only the root path is needed. Correct the view module’s description to describe pinned lookup. Preserve the existing lock scope across cache miss, decode and admission, including release before the terminal callback. The native provider keeps the same Rust module path after the physical move to `query_access/relations/mod.rs`; document that path in the handoff. [View](https://github.com/Simon-Martens/objekte/blob/bf4c1a32debe228d124d73192a2f3b01bb4895fe/crates/core/src/query_execution/relations/view.rs) · [Evaluation](https://github.com/Simon-Martens/objekte/blob/bf4c1a32debe228d124d73192a2f3b01bb4895fe/crates/core/src/query_execution/relations/evaluation.rs#L439-L522)

## 4. Resolve the largest timing signal

Q136, the deep-offset sorted `numbering >= 0` query, took longer on C in each paired run. B measured **3.387, 3.018 and 2.766 ms** and C measured **3.887, 3.430 and 3.284 ms**. The median increased **0.411 ms (+13.6%)** while the results and work counters matched. Run only Q136 in fresh, alternating B/candidate batches on the same corpus and settings. Report the samples. If the increase persists, narrow it to a series commit and operation before attributing it to the relation refactor. [Paired timings](https://github.com/Simon-Martens/objekte/blob/bf4c1a32debe228d124d73192a2f3b01bb4895fe/docs/ai/generated/core-refactoring-completion/assignment-3-relations-final-integration/musenalm/comparison/performance-analysis.json)

**Handoff.** Run the affected relation suites and workspace check on the final source. Record the tested SHA, the three paired relation observations, verifier negative cases and Q136 result. Preserve the passing 150-case comparison and build artifacts as evidence for C. The owner can judge the final source from the focused reruns and the exact repair diff.

## Implementation report

- Task identifier: `core-refactoring-completion` / Assignment 3 relation verification repair
- Completion date: 2026-11-19
- Reviewed source C: `acac94f486ee0f9d35eb5ba834d26cdc01eb1c5e`
- Reviewed delivery: `bf4c1a32debe228d124d73192a2f3b01bb4895fe`
- Final repair source: `fd25d8678af5161cb92647d7dbddefa7f1aff391`
- Integration branch: `integration/core-refactoring-completion`
- Integration worktree: `/home/simon/source/objekte-integration-core-refactoring-completion`

The repair source commits are:

1. `ec12e0359c00bdeefe603ffaa0eb702d5432a3be` — `Restore relation evaluation ownership and expose verification probes`
2. `fd25d8678af5161cb92647d7dbddefa7f1aff391` — `Record explicit relation probe success status`

### Ownership correction

`logical_references`, `LogicalReferences`, `relation_source_uses_value_text`
and `relation_target_uses_value_text` now live in
`query_execution::relations::evaluation`. `view` calls the logical-occurrence
helper at sibling visibility and otherwise describes its actual owner:
pinned lookup over one immutable base-plus-delta publication.

The ordered logical-value walk and its result representation are unchanged.
Evaluation uses `view.repository_root()` where only the source root is needed.
The target-cache lock remains around lookup, miss, read, decode and admission,
and remains released before terminal callback invocation.

`NativeRelationProvider` remains at
`query_access::relations::NativeRelationProvider` after the physical module move.

### Three relation paths

The existing deterministic fixtures emit observations only when
`OBJEKTE_RELATION_PROBE` is set. A2 used `9ae617b5df`, and the final side used
`fd25d8678a`; both used ASCII, 32 default query workers, zero verified-cache
bytes and fresh fixture publications.

| Path | Route and ordered result | Evidence and work | A2 = final |
|---|---|---|---|
| Indexed relation FILTER | `relation_target_first`; `S00` through `S39` | One edge, one reverse posting, 40 reverse members, one target candidate/match, zero target reads and zero scans | Yes |
| Selected-field relation FTS | `relation source-restricted`; `S00` | Fields `entry.title,title`; one edge, target read/open, 56 relation bytes, one reverse posting and 40 members; zero scans | Yes |
| Source-dependent residual sort | `relation_target_first`; bounded top-k; `m8,m1` | Fields `entry.title`; two sort-target reads, two target reads/opens, 93 relation bytes, one reused target and nine residual evaluations | Yes, and equal to reference hits |

The selected-field filter candidate/proof membership is `40/40`. The selected
FTS source-restricted posting has `2/1` raw-bigram candidate/proven records.
The residual fixture exposes `9` candidates with no terminal proof and remains
an independently evaluated result.

All result counters, ordered hits, matched-field evidence and normalized plans
match between A2 and the repair source. Every recorded work field is included
in the comparison, not only source and target reads.

### Evidence verifier

[`verify_relation_repair.py`](../generated/core-refactoring-completion/assignment-3-relation-verification-repair/verify_relation_repair.py)
declares the three traversal cases and five expected side/case keys
independently of its inputs. Each side is checked against its explicit run
manifest for revision, settings and publication state. Every key must occur
exactly once with successful status, no error and one plan.

The verifier also validates the archived Q102/Q122/Q088 probes: all are
ordinary posting plans with zero relation target reads and zero edge visits,
all nine B/A2-side samples have equal work, and ordered results match.

The extended schema artifacts are byte-identical, but this is not a successful
repository schema compatibility check. The verifier reports
`not_accepted_extended_adapter_in_either_run` because the schema baseline tool
does not accept the benchmark adapter field. The standard 150-case comparison
retains its empty compatibility-failure list.

The valid control passed. Independent invalid copies were each required to
exit unsuccessfully for the intended reason:

| Negative control | Rejection |
|---|---|
| Remove a case on both sides | `missing expected case/repetition` |
| Remove a repetition on both sides | `missing expected case/repetition` |
| Duplicate an archived plan | `expected one plan` |
| Replace both sides with empty evidence | `missing expected case/repetition` |
| Inject the same failed query on both sides | `failed status` |

### Q136 deep-offset timing

Q136 uses `numbering >= 0`, sorted by `numbering`, with 10,000 offset, 100
rows, exact count and locator evidence. It executes `sort_column` through
`bounded_top_k`, returns 100 of 133,948 matches, and records zero relation
target reads, edge visits, bytes or reuse.

Three fresh alternating B/final batches used four query workers and zero
verified-cache bytes:

| Batch | B median | Final median |
|---|---:|---:|
| 1 | 3.835981 ms | 3.855142 ms |
| 2 | 3.864366 ms | 3.936682 ms |
| 3 | 3.688415 ms | 3.972614 ms |
| All nine samples per side | 3.835981 ms | 3.936682 ms |

The original 0.411 ms / 13.6% increase did not reproduce at that magnitude.
The focused difference is +0.101 ms / +2.63%. Every batch had one/one matching
ordered result, all nine work-counter pairs matched, and there were no
compatibility failures.

A second repeated sweep ran three batches from each exact series revision in
three alternating stage orders. Nine-sample medians were:

| Revision | Median |
|---|---:|
| B | 3.925396 ms |
| A1 | 3.858380 ms |
| A2 | 3.930238 ms |
| A3 commit 1 | 3.918007 ms |
| A3 commit 2 | 4.409480 ms |
| Reviewed C | 4.442216 ms |
| Repair | 3.952558 ms |

The largest step is +0.491473 ms / +12.54% from A3 commit 1 `9c62fede` to
commit 2 `0b36bc25`, the split into `view`, `work` and `evaluation`. The repair
removes 0.489658 ms / 11.02% from reviewed C. Q136 has no relation work, so
this is a deep-offset sorted-posting code-layout/code-generation effect at the
module-split boundary rather than a changed relation traversal operation or
result. The repaired median is 0.027162 ms / 0.69% above the repeated B median.

### Validation

| Command/control | Outcome |
|---|---|
| `cargo check --locked --workspace --all-targets --all-features` | Passed |
| `cargo test --locked -p objekte-search-core --lib query_execution::relations::` | 39 passed |
| `cargo test --locked -p objekte-search-core --lib query_access::relations::semijoin::tests::` | 11 passed |
| Relation oracle, occurrence and non-JSON integrations | 3, 1 and 1 passed |
| `cargo test --locked -p objekte-search-core --lib` | 611 passed, 16 ignored |
| `cargo fmt --all -- --check` | Only the inherited `model/json.rs` conflict remains |
| `python3 scripts/format-rust-spacing.py --check` | Inherited 201-file spacing baseline remains |

The full workspace attempt reached 611 passing core library tests, then found
two unrelated/pre-existing issues. The desktop test
`incremental_desktop_open_exposes_catalogue_and_finishes_without_implicit_cache`
fails identically in isolation at both B and the repair source with
`fresh open must retain construction task`. The parallel discovery integration
suite hit a timestamp-based temp-directory collision in
`exact_directory_exclusion_preserves_non_utf8_path_identity`; that test passes
in isolation at B and the repair source.

### Evidence and cleanup

The evidence and reproduction tools are indexed at
[`assignment-3-relation-verification-repair`](../generated/core-refactoring-completion/assignment-3-relation-verification-repair/README.md).
They include the two traversal logs and manifests, hardened verifier and
self-test, three focused Q136 batches, the per-commit narrowing sweep, raw
plans/results/counters and final test logs.

The original passing 150-case query comparison and build artifacts at reviewed
C remain under
[`assignment-3-relations-final-integration`](../generated/core-refactoring-completion/assignment-3-relations-final-integration/README.md).
The integration branch and worktree remain for owner review. Temporary B, A2
and series-narrowing worktrees are removed after this report and evidence are
pushed.
