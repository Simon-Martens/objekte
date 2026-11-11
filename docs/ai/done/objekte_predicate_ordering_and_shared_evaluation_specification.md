# Objekte: predicate ordering and shared residual evaluation

## 1. Dependency, scope, and execution

This is Task 2. Complete `objekte_unified_query_planner_specification.md` first. Assume its common index catalogue, capability assessment, cost estimates, and `must/may` proofs exist. Inspect the delivered implementation and report, record its final commit as this task's baseline, and reuse any optimization already implemented there.

Both specifications can be assigned now, but implementation is sequential. Continue the direct-on-master workflow: do not have two agents modifying the master checkout concurrently. Preserve unrelated changes, make coherent commits, and push each commit without force-pushing.

Implement exactly these three improvements through the shared planner:

1. Cost- and selectivity-based ordering of AND predicates.
2. OR ordering with exclusion of records already proved to match.
3. Shared source reads, decoding, and transformations for residual predicates.

Do not add parallel OR execution, new workers, another planner, index formats, index-admission rules, or query syntax. Preserve explicit hints, opt-in automatic indexing, and existing index-update/validation policy. This task improves the completed planner; it does not repeat its refactoring.

## 2. Common decision rules

Use the predecessor's bound expressions and capabilities. Do not classify predicates by index origin or use a fixed rule such as “exact indexes first.”

For each applicable access or residual operation, use:

- Work on the current input, distinguishing fixed lookup/setup cost from per-record work.
- Estimated records retained or proved to match.
- Capability coverage and exactness.
- Remaining verification work.
- Source, decoded-value, and transformation work already available or shared with another predicate.

Keep two decisions explicit: index restriction order and residual evaluation order. A candidate index may be an excellent early restriction while its expensive verifier belongs later.

Use a bounded greedy strategy with cached estimates and stable tie-breaking. Reassess remaining choices after an executed restriction changes the actual input size. Do not read sources, materialize extra candidate sets, or scan large dictionaries merely to estimate selectivity. Unknown estimates require a conservative documented fallback, not invented precision. Avoid exhaustive plan enumeration, global learned statistics, or a new cost-model framework.

Reorder only expressions permitted by existing purity, error, and extension contracts. Preserve required evaluation order for opaque operations where those properties are unknown. Preserve all binding errors, established short-circuit behavior, cancellation, and observable evidence requirements.

## 3. AND ordering

For `A && B && C`, choose the next useful access by estimated total remaining work, including the cost of evaluating later predicates on its survivors.

Requirements:

- Flatten nested conjunctions only within a semantically equivalent scope.
- Prefer useful reduction at low cost; exactness reduces residual work but is not an unconditional priority.
- Intersect restrictions with the current active input. Carry per-expression proofs so an already proved term is not verified again.
- Reassess remaining alternatives against the reduced domain using cached statistics. Skip an optional prefilter if its lookup costs more than the verification it is expected to save.
- Stop membership evaluation when the candidate domain is empty.
- Order remaining per-record predicates by expected rejection benefit and incremental cost, accounting for already-read/decoded data.
- Apply inexpensive index/metadata restrictions before starting a source-read group when that saves work.

A required example: an exact boolean posting retaining 90% of records must not automatically outrank a similarly inexpensive bigram restriction retaining 0.1%. The latter remains approximate, and its verification must still occur for surviving candidates.

Keep Boolean nesting intact. Do not distribute expressions into an expanded normal form or reassociate array quantifiers, positional selectors, or relation correlation scopes.

## 4. OR ordering and proven-match exclusion

For `A || B || C`, prioritize inexpensive definite matches and the work they can avoid. Estimated additional matches and branch overlap matter; a branch's unrestricted output count is not necessarily its contribution to the remaining input.

Maintain the predecessor's `must ⊆ actual_matches ⊆ may` bounds. Within the OR node's eligible input D:

- Accumulate definite matches in `proven`.
- For membership evaluation, run later branches against `D - proven`.
- Add candidate-only results to the possible-match set, not to `proven`.
- A branch's exact source verification can add new definite matches as it executes.
- Deduplicate records in the final union and count each match once.
- Stop membership work when the whole input is proved, subject to evidence and other required output work.

A required example: if a cheap exact branch proves 95% of records, a later source predicate should evaluate only the remaining 5% when evidence requirements permit. If the first branch supplies only bigram candidates, those candidates must not be excluded from the second branch as though they were matches.

Preserve complete evidence when requested. If membership is already proved but another branch can contribute required evidence, evaluate only the necessary evidence work; retain current behavior when that cannot be reduced safely. Exact counts, sorting, pagination and output materialization retain their existing completion requirements. A LIMIT alone is not permission to stop an exact-count query early.

Keep execution sequential between branches. If several unresolved branches need overlapping source records, use the shared residual evaluation below instead of launching independent scans.

## 5. Share residual work without eager preparation

Prepare one record-scoped evaluation context for pending predicates on the same local source record. Reuse existing borrowed visitors, combined source/value preparation and worker-local buffers.

Within one query evaluation:

- Do not add another source read or full parse solely because a second predicate refers to the same record.
- Read/decode lazily after cheaper restrictions. Combine required field projections before a required parse.
- Reuse an already required `Value` and borrow its strings. When logical values are unnecessary, retain the direct borrowed-source route.
- For streaming decoded text, visit a segment once and apply its pending compatible matchers while the decoded/transformed bytes remain alive.
- Perform a shared transformation once per segment/representation, then use the result for all compatible pending predicates.
- Do not materialize all text segments, retain an entire transformed document, or build a `Value` merely to enable sharing.
- Retain existing file-level batching and locator behavior for files containing multiple records.

A reuse key must distinguish the record revision, field/segment occurrence and transformation/binding identity, including case and normalization settings. A repeated field path can contain several different array values or duplicate source occurrences; field path alone is not sufficient.

Keep raw bytes, decoded scalar text, logical values and normalized representations distinct. Preserve overwritten source occurrences and numeric spellings where the configured searchable corpus includes them. Reuse only representations that satisfy the predicate's semantics.

Different AND predicates may match different field values or array elements under existing semantics. Sharing a segment visitor must not accidentally require every predicate to match the same segment. Conversely, do not combine separate segments to create a phrase match, or discard existing same-element/position constraints.

Use lazy, query-local state rather than a new persistent document cache. Reuse scratch across records with the established retention bounds and cleanup on success, error and cancellation. References into scratch must not survive reuse. Do not introduce locking or allocation per term per record.

Initially guarantee sharing for built-in local predicates and matching built-in transformations. Preserve opaque extension and relation contracts: reuse through existing compatible hooks, otherwise retain their established evaluation path. Do not force a new public extension API or redesign relation-target caching. Record concrete remaining limitations.

## 6. Integration and diagnostics

Implement each policy once in the shared planner/evaluator. Ordinary, count, streaming, pagination, sorted, and local relation-target evaluation must consume it; do not add a separate optimized entry point.

Preserve source-work counters and extend existing diagnostics only where needed to show:

- Chosen predicate order and current input/output cardinalities.
- Estimated work/selectivity and the reason for the choice.
- Definite matches excluded from later OR branches.
- Residual verification still required.
- Shared source preparation and transformation work.

Estimate-only EXPLAIN performs no source I/O and no candidate execution. Normal execution must not collect expensive diagnostic details unless requested. Existing index-only predicates remain source-free. Output materialization is accounted separately from predicate verification.

No search-time file freshness checks, revalidation, content hashing or automatic reconciliation. Existing published revisions and explicit updates remain authoritative.

## 7. Required verification

Use reference-evaluator parity and work counters rather than tests tied to one scoring constant.

| Fixture | Required proof |
| --- | --- |
| Selective indexed term AND an expensive residual, in either textual order | Same result; residual work follows the restricted domain. |
| Broad exact term AND selective candidate index | Selectivity can win over exactness; survivors still receive required verification. |
| Empty AND restriction | No later membership source reads or predicate evaluations. |
| Broad exact OR branch plus expensive residual | Only unproved records receive later membership verification. |
| Candidate-only OR branch plus exact/scan branch | Candidate false positives never become results or suppress necessary evaluation. |
| Overlapping OR branches | Correct deduplicated membership, counts, ordering and evidence. |
| Several local residuals requiring the same source | One shared source preparation, without constructing unnecessary logical values. |
| Several predicates using the same case fold/normalization | One transformation per relevant segment/representation; scratch reused. |
| Cheap rejecting/accepting predicate | Unnecessary source reads and transformations remain lazy. |

Also cover evidence on/off, nested AND/OR/NOT, partial coverage, missing/null/empty values, Unicode/escapes, array selectors and independent-versus-correlated matches, malformed fallback, opaque operations, cancellation and locator/bytes/logical output. Use normal and incremental index generations.

Use test-only or existing counters to verify read, parse and transformation reductions. Do not assert that every query becomes faster or require fragile wall-clock timings in unit tests.

## 8. Benchmark and delivery

Baseline against the completed unified-planner task, not the earlier implementation. If the predecessor already meets a requirement, keep it and document the evidence instead of rewriting it.

Run matched before/after measurements with the verified-result cache disabled for optimization evidence. Include selective AND, broad exact OR, overlapping residual OR, multiple shared transformations, and an already-cheap index-only control. Use at least seven timed repetitions after warm-up; report timing dispersion, planning time, candidate/residual counts, source opens/bytes/parses, transformation counts and allocations where available. Keep worker counts and OS-cache conditions identical.

Run the complete 194-query suite under ASCII/Unicode and cache off/warm with the established three-repetition protocol. Preserve query options, counts, evidence and ordering. Verify the structural predicates Q115–118/Q120 retain the predecessor's zero-source-work behavior.

Recheck repeatable regressions above both 5% and 0.5 ms; separately inspect repeatable planning-time or retained-memory growth above 5%. Correct systematic regressions before completion and report unresolved findings explicitly. Run formatting and required tests for affected core and integration surfaces.

Suggested commits:

1. AND ordering and its counter-based regression coverage.
2. OR ordering/proven-match exclusion and evidence coverage.
3. Shared residual preparation/transformations, followed by measured results.

Write `docs/ai/generated/predicate_ordering_and_shared_evaluation_results.md` with the predecessor and final commits, implemented policies, representative before/after plans and work counters, complete-suite results, and remaining limitations. Leave completed commits pushed on master.

Completion requires all three improvements through the common planner, preserved semantics, demonstrated reductions in unnecessary work, and no new branch-parallel execution.

## Implementation report

- **Task identifier and completion:** `predicate-ordering-and-shared-evaluation`, 12 November 2026. Predecessor/base commit `a68cb0c67b68987251d35f528ccbbe3445509ea1`. The user explicitly authorized direct-on-`master` execution of this specification as the third overnight task; the integration branch and worktree are `master` and `/home/simon/source/objekte`.
- **Implementation commits:** `5b86f4994e0c` implements bounded AND/OR ordering and reference/counter tests. `1decacde5fce` adds shared local folding, paired borrowed FTS+FILTER evaluation, tests and the focused fixture/harness. `49e82e881c5d` fixes unnecessary fold-cache cleanup on nonsharing records. `ba418374641a` adds complete ordered-result digests to the Musenalm benchmark. Each task commit was pushed to `origin/master` after creation. There were no subordinate branches, subtasks delegated to agents, or branch-parallel execution.
- **Validation:** `cargo test --workspace --locked` passed after the core fix (506 core and 168 desktop unit tests, plus all other workspace unit, integration and doc tests). `cargo clippy --workspace --all-targets --locked` passed with existing warnings. `git diff --check` and targeted rustfmt checks passed. Repository-wide `cargo fmt --all -- --check` reports pre-existing formatting differences in untouched files, including CLI and example support code.
- **Benchmark:** Exact predecessor and final release binaries ran the same extended 194-query Musenalm corpus with three repetitions, ASCII/Unicode, four workers, and verified-result cache off/64 MiB warm. All 776 query/profile rows passed, with identical counts and nine result/source-work fields. A separate baseline audit with the digest-enabled harness matched all 194 complete ordered-result digests, including evidence and payload. Focused deterministic 1,000-record cases ran 21 repetitions in alternating baseline/final order with reference parity, and 7-/21-sample rechecks plus isolated Q012 runs investigated timing flags. Commands, fixture fingerprint, timings, source counters, memory/build metrics, and raw evidence are in [the results report](../generated/predicate_ordering_and_shared_evaluation_results.md) and `bench/results/predicate-ordering-shared-evaluation/overnight-2026-11-11/`.
- **Regression, limitations, and issues:** The preliminary implementation cleared shared-fold scratch on every record, even for nonsharing predicates; `49e82e88` removed that unnecessary work. The corrected acceptance suite's cache-off timing flags narrowed to one marginal Q012 row and did not reproduce in isolated reversed-order runs. No repeatable material implementation-caused regression remains. The full corpus lacks a matched incremental/partial-coverage timing fixture, and per-query allocations are not exposed; existing tests cover those correctness paths. Direct borrowed pairing supports compatible built-in FTS plus one local FILTER, while other residuals use the shared logical preparation and opaque/relation hooks retain their established path. No unresolved in-scope issue or approved deviation remains.
- **Worktree and cleanup:** Direct implementation used `/home/simon/source/objekte`. The detached baseline-only benchmark worktree at `/home/simon/source/objekte-bench-predicate-base` is retained until the evidence and this document are committed and pushed, then removed normally. The original untracked task source is removed only after this completed document is confirmed to preserve its exact original bytes.
