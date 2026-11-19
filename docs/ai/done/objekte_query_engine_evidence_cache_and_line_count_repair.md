# Objekte query engine repair — matched fields, cache exhaustion and line counting

## Assignment

Start from `master`. The reviewed revision is `f4b3c9f0dac82315e856c7cd66080d01d779db14`. Deliver the three corrections below on `integration/query-engine-evidence-cache-repair`, using the sibling worktree `/home/simon/source/objekte-integration-query-engine-evidence-cache-repair`. Follow `docs/ai/README.md`. The owner merges the branch. [Reviewed merge](https://github.com/Simon-Martens/objekte/commit/f4b3c9f0dac82315e856c7cd66080d01d779db14)

## 1. Return the same matched fields from residual and merged evaluation

**Failure.** For a true `entry.title = local_title`, the merged evaluator reports both paths. The residual scanner reports only `entry.title`. The binder clears the outer term’s `rhs` and stores the local operand in `related.source_operand`. The residual scanner still looks only at the outer query path, `rhs` and the relation evidence path. The latter repeats `entry.title` for this query. The search can therefore return the right record with incomplete highlighting. [Binder](https://github.com/Simon-Martens/objekte/blob/f4b3c9f0dac82315e856c7cd66080d01d779db14/crates/core/src/query/binding/relation.rs#L316) · [Residual scanner](https://github.com/Simon-Martens/objekte/blob/f4b3c9f0dac82315e856c7cd66080d01d779db14/crates/core/src/query_predicate/scan.rs#L685)

**Change.** Move the existing `emit_bound_filter_evidence` rule to an internal location usable by both evaluators. Call it from `Residual::with_evidence`, `eval_filter_truth` and `eval_filter_truth_merged`. Compute leaf truth once. For a qualifying true leaf, collect the outer query path, ordinary right operand, related source operand and related evidence path wherever present. Preserve set semantics. Keep the existing false, unknown, negative-operator and derived-term exclusions. Preserve proof lookup, verification counts, branch visitation and the metadata-only handling of unavailable records. If sharing the eligibility predicate with `Residual::can_emit_evidence`, keep that method’s existing recursion for Boolean branches. [Existing rule](https://github.com/Simon-Martens/objekte/blob/f4b3c9f0dac82315e856c7cd66080d01d779db14/crates/core/src/query_execution/relations/evaluation.rs#L177)

**Acceptance.** Extend the existing relation fixture. On the matching record `m1`, exercise the actual residual evaluator and the merged evaluator with evidence enabled. Assert each independently produces exactly `{"entry.title", "local_title"}`. Exercise the reverse spelling `local_title = entry.title` against the same expected set. The present test compares two callers that already share the helper, so it passes before this repair. Demonstrate that the new residual assertion fails on the branch point. Retain the existing membership, negation and suppression checks. [Existing test](https://github.com/Simon-Martens/objekte/blob/f4b3c9f0dac82315e856c7cd66080d01d779db14/crates/core/src/query_execution/relations/evaluation/tests.rs#L3585)

## 2. Stop cache-only work as soon as complete membership can no longer be retained

**Failure.** In `crates/core/src/query_execution/runner/mod.rs`, `LifecycleCacheBuilder::push` clears `ranked` when the budget or allocation is exhausted. The outer builder remains present. For every subsequent slot that loses page admission, `offer_admitted_segment_slot` still calls the evidence producer, resolves the record and builds a full rank key. The builder then discards the member. [Builder](https://github.com/Simon-Martens/objekte/blob/f4b3c9f0dac82315e856c7cd66080d01d779db14/crates/core/src/query_execution/runner/mod.rs#L668) · [Exact-slot handler](https://github.com/Simon-Martens/objekte/blob/f4b3c9f0dac82315e856c7cd66080d01d779db14/crates/core/src/query_execution/runner/mod.rs#L1797)

**Change.** Use `ranked.is_some()` as the authoritative signal that the builder is accepting members. At the shared exact-slot handler, treat an exhausted builder as absent for both the early rejection and evidence ownership. Both the proven-slot and the all-exact loop must inherit the fix. The slot that first discovers exhaustion may already have incurred the admission cost. Apply the following rules to subsequent slots.

| Slot and builder state | Required action |
|---|---|
| Page loser, builder absent or exhausted | Return after the required page comparison. Skip evidence production, post-admission record lookup and full rank-key construction. |
| Page winner, builder absent or exhausted | Construct the hit and move its owned field vector directly into it. |
| Builder accepting members | Preserve complete membership, including page losers. If collection later fails, discard the entire cache entry. |

Preserve totals, sort tie-breakers, comparison errors, cancellation and winner memory accounting. The query must continue serving its page after cache exhaustion.

**Acceptance.** Extend `exact_posting_count_preserves_page_evidence_with_and_without_membership_cache`. Reuse its 100 exact slots, five winners, heap route and evidence-call counter. Construct a fresh verified cache whose total budget is the actual lifecycle key length **plus 128 bytes plus one byte**. At the reviewed revision, the runner deducts the key length and 128 bytes before creating the builder. This leaves one positive byte, so collection starts and fails on the first member. A one-byte *total* budget would bypass the failing path. [Budget calculation](https://github.com/Simon-Martens/objekte/blob/f4b3c9f0dac82315e856c7cd66080d01d779db14/crates/core/src/query_execution/runner/mod.rs#L1881) · [Page fixture](https://github.com/Simon-Martens/objekte/blob/f4b3c9f0dac82315e856c7cd66080d01d779db14/crates/core/src/query_execution/runner/tests.rs#L3969)

| Scenario on that fixture | Expected result |
|---|---|
| Tiny positive builder budget, first page | Count 100, IDs `000` through `004`, five evidence calls. The branch point makes 100 calls. |
| Same cache, second page | IDs `005` through `009`, count 100, cache miss. A partial entry must never be published. |
| Existing controls | Disabled cache still makes five calls on the first page. Sufficient cache still makes 100 calls and serves the second page with zero new calls. |

## 3. Make the line counter reproduce both historical comparisons

**Failure.** `docs/ai/generated/query-engine-deduplication-repair/count_core_rust_lines.py` opens the new path at both revisions. Comparing `d35ddde` with `d064445` aborts because the new Boolean module did not exist at the old revision. Its check of the combined physical total also permits an incorrect production/test split to pass. [Counter](https://github.com/Simon-Martens/objekte/blob/f4b3c9f0dac82315e856c7cd66080d01d779db14/docs/ai/generated/query-engine-deduplication-repair/count_core_rust_lines.py#L112)

**Change.** Classify additions using the new path and tree, and deletions using the old path and tree. Handle added and deleted files as empty on their missing side. Use Git’s path/status information to distinguish an absent side from a failed Git read. Preserve change attribution for paths on each side of a rename. Count physical Rust lines under `crates/core/src`, including comments and blank lines. Follow module declarations from `lib.rs` to classify test-only modules and delimit inline `#[cfg(test)]` items accurately. Production code after an inline test module or in a file containing “test” in its name must remain production. Use the recorded counts as expected values and compute the actual classifications from source.

Run the corrected command on both revision pairs. The expected output is given below. The old and new trees for the second pair are the input to the **previous** repair's measurement.

| Range | Production added / removed | Test added / removed | Ending production / test totals |
|---|---:|---:|---:|
| `d35ddde` → `d064445` | 872 / 942 | 239 / 16 | 71,743 / 54,001 |
| `d064445` → `f4b3c9f` | 98 / 109 | 524 / 23 | 71,732 / 54,502 |

Also run the first range in reverse. Its additions and removals must swap. This exercises the deleted-module case using the same real history. Preserve the existing `--from` and `--to` command interface. Record the new repair's own delta separately.

## Verification and delivery

Make one reviewable commit per correction. Run the two targeted Rust regression tests, then `cargo test --locked -p objekte-search-core` and `cargo check --locked --workspace --all-targets --all-features`. The field-set assertion and evidence-call counter directly measure the two runtime failures. The existing release benchmarks can stand for this follow-up.

Append a short completion report to the task in `docs/ai/done/`. Include the branch point and tested tip, the field sets from both routes, the three cache-budget outcomes, and the line-counter output for both historical ranges and the reverse check. Deliver the integration branch for owner review.

## Implementation report

- Task identifier: `query-engine-evidence-cache-repair`
- Completion date: 2026-11-19
- Branch point: `f4b3c9f0dac82315e856c7cd66080d01d779db14`
- Tested source tip: `9529c479a3defdb65fb1a589d739456906a66558`
- Final tip before this report: `fb2875b4828138510d448ddea55ab9d29b9580de`
- Integration branch: `integration/query-engine-evidence-cache-repair`
- Integration worktree: `/home/simon/source/objekte-integration-query-engine-evidence-cache-repair`

### Matching fields

The shared rule now emits the outer query path, scalar right operand, relation
source operand and relation evidence path. Actual evaluation produced:

| Spelling | Residual fields | Merged fields |
| --- | --- | --- |
| `entry.title = local_title` | `{entry.title, local_title}` | `{entry.title, local_title}` |
| `local_title = entry.title` | `{entry.title, local_title}` | `{entry.title, local_title}` |

With the new residual assertion applied at `f4b3c9f`, the first spelling failed
as expected with residual fields `{entry.title}` versus expected
`{entry.title, local_title}`.

### Cache exhaustion

| Cache state | First page | Second page |
| --- | --- | --- |
| Disabled | count 100, IDs `000..004`, 5 evidence calls | not cached |
| Sufficient membership cache | count 100, IDs `000..004`, 100 evidence calls | IDs `005..009`, cache hit, 0 new calls |
| Key + 128 + 1 byte | count 100, IDs `000..004`, 5 evidence calls | IDs `005..009`, count 100, cache miss |

An exhausted builder is now treated as absent after the first failed member.
The subsequent page winners still receive their owned evidence, subsequent page
losers skip evidence/key work, and no partial entry is published.

### Line-counter outputs

```text
d35ddde -> d064445
production: +872 / -942
test:       +239 / -16
ending:     71743 production, 54001 test

d064445 -> f4b3c9f
production: +98 / -109
test:       +524 / -23
ending:     71732 production, 54502 test

d064445 -> d35ddde (reverse)
production: +942 / -872
test:       +16 / -239
ending:     71813 production, 53778 test
```

The script resolves paths independently per revision, treats missing sides as
empty, follows `lib.rs`, respects added/deleted files and reproduces the
recorded historical ranges exactly.

This follow-up repair's own Rust delta from `f4b3c9f` to `fb2875b4` is
`+59/-53` production lines (net `+6`) and `+156/-5` test-only lines (net
`+151`).

### Validation

- `cargo test --locked -p objekte-search-core query_execution::relations::evaluation::tests::bound_filter_leaf_evidence_has_fixed_expected_fields`: passed.
- `cargo test --locked -p objekte-search-core query_execution::runner::tests::exact_posting_count_preserves_page_evidence_with_and_without_membership_cache`: passed.
- Both historical line-count ranges and the reverse check: passed with the recorded outputs.
- `cargo test --locked -p objekte-search-core`: 620 passed, 16 ignored; all integration tests and the doc-test passed.
- `cargo check --locked --workspace --all-targets --all-features`: passed.
- `cargo test --locked --workspace`: all query/core tests passed; one inherited desktop lifecycle test failed and reproduced at the branch point.
- Existing release benchmarks stand per the assignment; the focused tests directly measure the changed paths.

### Handoff

- No query-engine behavior deviation or unresolved query issue.
- The inherited desktop failure is recorded in
  `docs/ai/issues/bug/desktop-incremental-open-construction-task.md`.
- The temporary baseline reproduction worktree was removed.
- The integration worktree and branch remain for the owner to merge.
