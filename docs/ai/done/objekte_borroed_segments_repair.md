# Objekte borrowed segment verification repair specification

Date: 11 November 2026  
Task: `borrowed-segment-verification` — reopen and repair  
Branch: `integration/borrowed-segment-verification`

Complete the original borrowed-segment specification and resolve the correctness and performance findings below. Preserve the working allocation reductions, particularly on duplicate-heavy records. This is a repair of the existing task, not a replacement optimization project.

## 1. Execution and baseline

Read current `AGENTS.md`, `docs/ai/README.md`, the original follow-up specification, and the implementation report before editing. Continue in the existing integration worktree, normally `/home/simon/source/objekte-integration-borrowed-segment-verification`. Keep the original checkout on `master`; do not commit, merge, or push to `master` for this task. The previous direct-master authorization concerned the separate query-preparation repairs.

Record these comparison points:

| Revision | Purpose |
| --- | --- |
| `5166876c7228f4fe2f8f064861cdc8a88275ad18` | Completed predecessor; semantic and performance baseline. |
| `47931f0cdfefb860b0a0cac17ae06e068daf8a1e` | Reviewed borrowed-verification implementation. |
| `f872e8d750fd2f7f83994a147394b84dcbf7d21e` | Reviewed integration head, including completion documents. |
| Actual starting and final commits | Record any intervening work and avoid overwriting it. |

The review findings came from code inspection and committed measurements; reproduce correctness failures with tests before changing behavior. Recheck findings already changed by later commits.

Use one supervisor. If delegating, keep parser/decoder changes under one owner and executor integration under another clearly defined owner; independent regression analysis is suitable separate work. Subordinate worktrees remain siblings under `/home/simon/source`. Run performance measurements serially.

## 2. Preserve the task boundaries

Retain public ownership types, query syntax, reader semantics, output modes, ordering, evidence, extension contracts, and index format. Do not add parser libraries, unsafe lifetime conversions, a global scratch mutex, or a new benchmark framework.

Keep the explicit validation policy: indexed records use their recorded state until an explicit index operation changes it. Search must not add freshness probes, metadata/hash comparisons, whole-document/schema revalidation, validator retries, or implicit index updates. Preserve validating behavior for unvalidated inputs and recorded raw-fallback behavior. Keep safe access, decoding errors, cancellation, and execution limits.

Complex queries may keep their existing evaluator, but that does not authorize redundant matching-only text copies when the required logical values already own the text.

## 3. Restore root-scoped FTS semantics

Relevant code: `json_scalar_text.rs::scan_borrowed_value()`, `match_borrowed_source_segment()`, and `reference.rs::eval_bound_fts_field()`.

The established evaluator treats an empty selected field path (`@root`) as covering the reader's text segments. The new string preselection requires exact path equality and skips nested strings. Its numeric branch takes a different route.

- Make source preselection, matching, and duplicate visibility agree with the established root-field semantics.
- Preserve the distinction between unscoped decoded-scalar FTS, which sees all duplicate occurrences, and selected/resolved FTS.
- Do not fix this by changing the reference evaluator or narrowing the public meaning of `@root`.
- Reuse one internal path-selection rule where practical to avoid another mismatch.

Add differential tests for literal `needle` scoped to `@root` over `{"title":"needle"}`, nested objects, root arrays, scalar roots, and mixed string/number scalars. Include duplicate replacements by a nonmatch, null, and empty containers, plus mixed root/local field selections. Exercise Json, JsonLines, and JsonDecodedScalars with evidence on/off and locator, bytes, and logical output.

## 4. Complete borrowing when logical values are required

### Combined logical values and source-derived text

The reviewed executor borrows value text only for Json/JsonLines and disables the direct route for logical output. JsonDecodedScalars requests requiring values retain the owned combined parser and matching-only `TextSegment` strings. Preserving duplicate occurrences and number spellings is necessary; retaining redundant owned text is not the required solution.

Extend the existing combined extraction machinery with synchronous consumption or source spans:

- Build only the logical projection/output already required by the query.
- Deliver the source-derived scalar corpus to matching during the same structural traversal, or through retained source spans without another structural parse.
- Borrow unescaped source text and numeric/boolean/null lexemes. Decode escaped text into reusable storage.
- Decode each relevant string token once and share its contents synchronously between matching and required Value construction. Do not decode a saved span again after already constructing that string's Value.
- When a logical string must be owned, allocate its required value storage once. Matching must not require an additional owned segment string.
- Preserve overwritten scalar occurrences for unscoped decoded FTS and final-member visibility for selected fields. Retain compact provisional decisions or ranges, not discarded source text.
- Keep schema conversions and output annotations correct while preserving source-corpus semantics.
- Do not reconstruct lexical scalar text by serializing the resolved Value.
- Do not add an unconditional second parse or source reread for candidates. Keep existing output scheduling; logical-output candidates needing a complete value use the combined route.

Cover decoded-scalar FTS with numeric/complex residual FILTER evaluation and high-hit logical output. Regex should reuse existing matchers wherever the same consumer suffices; preserve documented unsupported fallbacks.

### Existing-value relation fallbacks

Audit generic relation evaluation as well as optimized direct relation pages. In particular, trace `relation_query.rs` paths that set `collect_text: true` and then prepare both a Value and owned segments.

For Json/JsonLines targets whose converted logical values supply the searched text, suppress redundant segment production and consume existing strings by reference. Retain relation traversal, selection, quantification, evidence, and target caching behavior. This is an ownership repair, not a relation-planner redesign.

List every retained owned query path and its specific semantic/API requirement. A blanket “relations”, “logical output”, or “complex query” explanation is insufficient.

## 5. Remove avoidable executor and traversal overhead

### Prepare projections once

`run.rs::scan_file()` now clones `ScanProjection` unconditionally per file, even when borrowed-value matching is disabled.

Borrow the unchanged projection or prepare immutable route-specific projections once per collection plan. Do not clone selected-field sets or field-name strings per file/candidate merely to change `collect_text`. Apply consistent route selection across ordinary, paginated, and streaming execution.

### Skip irrelevant subtrees

The direct scanner descends through unrelated containers instead of using the predecessor's cheaper trusted skip operation.

Use prepared path requirements to skip a subtree when it cannot contribute to FTS, FILTER, required values, or evidence. Reuse the existing skipper and structural machinery. Account for root/unscoped selection, arrays, duplicate replacement, and selected ancestors before skipping. A later duplicate empty container must still invalidate the earlier selected contribution.

Add a focused FILTER-only case with a small selected string and a large irrelevant nested subtree. Confirm reduced traversal work without changing membership, source reads, or validation policy. Avoid repeatedly scanning prior events or the document to determine relevance.

## 6. Optimize escaped decoding without abandoning reuse

`with_next_quoted_text()` performs another `memchr2` search after every escape and UTF-8 conversion for each literal run. The predecessor rejected repeated delimiter searches on escape-heavy text. Treat this as a measured hypothesis to investigate, not proof of a single cause.

Benchmark the actual reusable decoder against the predecessor's decoder on identical tokens. Cover long plain ASCII, unescaped Unicode, sparse/dense simple escapes, Unicode escapes, surrogate pairs, and alternating short/long strings.

Compare a bounded set of implementations, including the predecessor's hybrid approach: find the first quote/backslash, then handle dense escapes with a suitable loop and copy contiguous literal runs. Retain the fastest semantics-preserving approach supported by representative measurements. Share token handling where practical; do not add another independent JSON grammar.

Requirements remain:

- Plain identity text borrows its source slice.
- Escaped text reuses decoding capacity; transformed output reuses separate capacity.
- No source mutation, temporary owned text per segment within warmed capacity, or allocating fallback for all escape-heavy records.
- No extra full-string pass solely to discover that a transform leaves text unchanged.
- Preserve exact Unicode, escape, and transformation order.

Inventory actual built-in transformations and provide output-buffer forms where needed. Opaque extension transformations may retain their documented owned fallback; do not invent new normalization behavior.

## 7. Finish scratch ownership and allocation verification

The folding helper currently releases oversized capacity after each segment; the report describes release after a record. Move exceptional-capacity release to the record/scan boundary while retaining capacity between segments of that record. Cover errors and cancellation without retaining oversized buffers indefinitely.

Retain the existing 256 KiB-per-buffer policy unless measurements justify a documented adjustment. It limits retained capacity, not accepted record size. Account for decoding, transformation, provisional-event, path, and evidence storage separately. Keep query objects immutable and worker scratch isolated.

Tests must exercise production execution paths, not only a helper supplied with a preallocated path vector. After warm-up, verify:

- Existing-value identity matching borrows original string/key storage and produces no matching-only segments.
- Plain direct matching borrows source storage.
- Combined value/source matching adds no redundant segment-text copies.
- Decoding and built-in transforms allocate no temporary text per segment within warmed capacity.
- Multiple oversized transformed segments reuse storage within one record, then release exceptional capacity at the boundary.
- Alternating inputs cannot expose stale scratch contents.

Use allocator/copy instrumentation at the actual changed boundary. Separate required Value/output allocation, evidence, duplicate bookkeeping, and source buffers from redundant text allocation. Pointer identity and counters for one collector alone do not prove the absence of all temporary allocations.

## 8. Investigate regressions with the changed code under measurement

Extend the existing harness minimally so in-memory measurements call the actual borrowed visitor, reusable decoder, combined consumer, and matching path. The current public owned-API control does not measure those changes.

Compare the repaired branch against both the predecessor and reviewed borrowed implementation. Keep candidates, results, output/evidence modes, worker count, compiler/profile, and cache settings equivalent. Disable verified-result reuse and warm filesystem caches. Run allocation instrumentation separately from timing.

Required focused cases are:

- Required values: identity and folded matching, including generic relation targets.
- Direct source: plain, Unicode, escaped, folded, FILTER-only, combined, and scoped/root FTS.
- Required values plus decoded-scalar source text.
- Irrelevant nested subtrees, many scalars, duplicate replacement, and high-hit logical output.

Revisit the recorded regressions:

| Recorded case | Observation requiring investigation |
| --- | --- |
| Long escaped combined FTS/FILTER | Final/base ratios 1.135, 1.049, 1.060 across three comparisons. |
| Long escaped scoped FTS | Ratios 1.101, 1.056, 1.067. |
| Plain Json required-value identity | 1.242 → 1.547 ms in the focused run. |
| Duplicate-heavy FILTER-only | 0.849 → 1.103 ms. |
| Full uncached suite | 31 case/profile rows above 1 ms exceeded 5% slowdown in the reviewed CSVs. |
| Q191 Unicode / Q192 ASCII | 323.34 → 346.48 ms / 220.24 → 238.52 ms. |
| Q177 ASCII | 6.55 → 10.50 ms with zero source reads; decoding cannot explain it. |

Derive the complete affected-case list from the saved CSVs rather than testing only these examples. Repeat flagged cases in at least two alternating baseline/final rounds with seven or more timed repetitions after warm-up. Inspect dispersion and absolute differences. Classify each as resolved, nonrepeatable with evidence, or unresolved. Do not attribute unrelated zero-read controls to text decoding.

Preserve the substantial duplicate-heavy gains. Investigate repeatable regressions above roughly 5% and substantial absolute slowdowns; do not hide them behind an aggregate median or route all required cases back to owned extraction. If a required path remains materially slower, report the isolated cause and concrete tradeoff as an unresolved acceptance blocker.

## 9. Validation and delivery

Use reviewable commits for: root correctness; projection/traversal overhead; combined and relation ownership; decoder/scratch repairs; verification/reporting. Adjust boundaries to avoid broken intermediate states. Push each commit immediately according to repository rules.

Run focused semantic/allocation tests, the workspace test suite, relevant formatting/checks, and the complete existing query benchmark suite. Compare builds if shared build code changes. Preserve ordinary fallback, regex, streaming, cancellation, output, and explicit-index-update coverage.

Update the original task's completion status honestly while repairs are incomplete. Preserve its specification and earlier measurements. On completion, append a repair report and save measurements under the existing task's results directory with distinct revision labels.

The final report must provide:

- Exact commits and an A/B/C coverage table with production call sites.
- Reproductions and tests for corrected behavior.
- Remaining owned paths and concrete reasons.
- Allocation/copy results and actual optimized in-memory timings.
- End-to-end timings, dispersion, source-work parity, scratch retention, and disposition of every flagged regression.
- Commands, environment, and any unresolved limitation.

Deliver the same integration branch for the user's merge. Do not mark the task complete while the root mismatch, required ownership coverage, required checks, or material regression blockers remain unresolved.


## Implementation report

Task identifier: `borrowed-segment-verification` (reopened repair). Completed 11 November 2026 on `integration/borrowed-segment-verification` in `/home/simon/source/objekte-integration-borrowed-segment-verification`. This resumption started from `d8e3b22f850811eb58f26716c2db819016612e96`, which already contained the earlier repair commits through `2d92d50820f1`. The new implementation commit is `a1bb0990e86a8ed7a8c35c3e4a1610b5077c3bc3`. One supervisor performed this resumption; there were no subagent branches. The detached baseline and reviewed benchmark worktrees were used only for comparisons and are to be removed after their measurements are preserved. The integration worktree remains.

The previous repair fixed root-scoped FTS, combined value/source and relation ownership, traversal overhead, and decoder/scratch behavior. This final commit removes a repeatable FILTER-only slowdown: the direct scanner skips irrelevant scalar siblings with the existing trusted skipper and tracks one prior selected event range for duplicate replacement. A focused test covers 1,000 irrelevant scalar siblings, a nested subtree, duplicate replacement, escaped values, null, and empty contains. The A/B/C production call sites, required owned fallbacks, allocation/copy boundaries, scratch policy, and comparisons are in the [final repair report](../generated/borrowed_segment_verification_repair_results.md).

Validation: `cargo test --locked -p objekte-search-core --lib` passed (501 passed, 15 ignored), `cargo test --workspace --locked` passed, `cargo clippy --workspace --all-targets --locked` passed with existing warnings, `rustfmt --edition 2024 --check` passed for both changed Rust files, and `git diff --check` passed. Release-only production-consumer and decoder probes ran separately from end-to-end timing and allocation measurements. The complete existing 194-query benchmark suite passed in ASCII/Unicode with cache off/warm (776 profile rows). All result and source-work counters matched the exact master baseline. The Q069 FILTER-only regression improved from 16.785/17.563 ms to 13.527/14.340 ms in the first paired ASCII/Unicode round; the reverse round confirmed the direction. The union of 72 flagged full-suite cases was rechecked in alternating seven-sample rounds, and four short-run outliers were investigated in 100-sample reverse-order isolates. Their medians converged, with unchanged plans and work counts; no repeatable material regression remained. One build per alphabet was within 5% of the baseline. Commands, raw manifests, dispersion, source reads, and all regression dispositions are in the linked report and [overnight benchmark results](../../../bench/results/borrowed-segment-verification/overnight-2026-11-11/).

Remaining limitation: the reusable decoder retains a small microbenchmark timing cost for dense Unicode/escape-heavy tokens versus the predecessor's allocating decoder, but the focused end-to-end rechecks found no repeatable material slowdown. Explicit array selectors, complex FILTER expressions, regex, opaque extensions, index construction, and public owned extraction retain their semantically required owned or fallback routes. The user explicitly authorized merging each completed task into `master`, superseding this specification's earlier integration-branch-only delivery instruction.


