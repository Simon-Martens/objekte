# Objekte borrowed segment verification follow-up specification

Date: 11 November 2026  
Task identifier: `borrowed-segment-verification`  
Predecessor: `objekte_candidate_verification_optimization_plan.md`  
Deliverable: one tested and measured integration branch for the user's merge.

## 1 Task contract

Inspect the preceding agent's completed implementation and implement the missing requirements below. Reuse working code; do not repeat the preceding optimization project.

This task has three required outcomes, not alternative experiments:

- **A — Existing values:** when execution already requires a `Value`, match its existing string contents through borrowed references. Do not clone them into `TextSegment` solely for matching.
- **B — Source bytes:** when logical values are unnecessary, match directly from source slices or reusable decoding storage. Do not build a `Value` tree solely to obtain searchable text.
- **C — Scratch reuse:** when decoding, normalization, case folding, or another supported built-in transformation requires owned writable storage, reuse worker-local buffers across segments and records. Do not allocate a fresh text string for every segment.

Both A and B must be implemented where missing; implementing only one does not complete this task. C applies to both. If the predecessor already satisfies a requirement, demonstrate that with code references and tests rather than replacing it.

Keep query results, text representations, segment boundaries, evidence, output modes, and extension behavior unchanged. Preserve the explicit index-validation policy in section 8. Changes are confined to candidate extraction, text consumption, transformation storage, their internal executor connections, and relevant tests/benchmarks.

## 2 Inspect first and choose the baseline

Read current `AGENTS.md`, `docs/ai/README.md`, the predecessor specification and report, and its completed implementation. Record the actual predecessor commit and this task's base. The earlier review at `bebff5d2f82ac8c3230f0adff9365bb1bfc0d41b` is only a navigation reference.

Trace the current equivalents of:

- `collect_strings()`, `text_segments()`, and their query-time callers.
- Source parsing, scalar extraction, and `ExtractedRecord` construction.
- Literal/regex matching, representation transforms, and folding.
- Worker lifetime, source-buffer lifetime, evidence, and logical output.

For each route, record the text owner, copies/allocations before matching, buffer lifetime, and whether the matcher already accepts `&str`. Inspect producers as well as consumers: borrowing text after the producer already allocated redundant segments does not remove those allocations.

Use this decision table:

| Finding | Required action |
| --- | --- |
| A query already owns a required `Value`, but copies its strings or field paths for matching | Implement A and remove the redundant query-time segment production. |
| No logical value is required, but the reader builds values or owned segment strings anyway | Implement B using the predecessor's extraction machinery. |
| A transform allocates a new output string per segment | Implement C with an internal output-buffer adapter. |
| Scratch exists but is recreated per record | Move its ownership to the existing worker/scan context. |
| A route already meets its requirements | Retain it; supply evidence and add only missing coverage. |
| A prerequisite is unfinished | Make the smallest local adaptation necessary, or report the precise dependency and continue independent work. Do not reimplement the entire predecessor task. |

Do not infer completion from a report, a `Cow<str>` type, or a function name. Check actual allocation and ownership behavior.

## 3 Exact route selection

Select the route from the existing bound query, required outputs, reader semantics, and available owners. Prepare query eligibility once per collection plan. Per-segment selection should use existing escape/representation information.

| Available data and required text | Route |
| --- | --- |
| A required `Value` already contains exactly the strings that this reader/query searches | A: borrow from that value. |
| No `Value` is required and source bytes supply the required corpus | B: borrow from source or decode into scratch. |
| A `Value` is required, but searchable text must retain original scalar occurrences or number spellings | Preserve that source-derived corpus through B or existing source spans while building only the required values. Do not substitute text reconstructed from the `Value`. |
| An extension or unsupported expression requires the existing owned representation | Preserve its existing path; record the specific reason. |

The third row is essential: a resolved `Value` can lose overwritten duplicate members and original numeric spellings. Borrowing from it must not change unscoped decoded-scalar FTS semantics.

Keep public `Value`, `TextSegment`, `ExtractedRecord`, and extension interfaces compatible. An internal consumer route is sufficient. Owned segments may remain for index construction and callers that genuinely require ownership.

## 4 A — Borrow text and paths from existing values

Target the query-time equivalent of:

```rust
Value::String(s) | Value::Id(s) => output.push(TextSegment {
    path: path.clone(),
    text: s.clone(),
})
```

For existing-value string search, replace the matching-only production/consumption chain with synchronous borrowed visitation:

1. Pass `s.as_str()` to the prepared matcher. Do not clone, serialize, or decode it again.
2. Traverse object keys by reference. Use a reusable stack of borrowed key references, such as `Vec<&str>`, or the existing equivalent. Avoid `name.clone()` merely to descend into an object.
3. Pass the current path as a temporary borrowed view. Avoid `path.clone()` for every scalar.
4. Retain an owned path or field identifier only when a result/evidence consumer needs it after visitation. Deduplicate evidence according to current behavior.
5. Match each array element separately, with the current field-path semantics.

The immutable `Value` owner must remain alive during visitation. References into the changing path stack must not escape the callback. Required schema/value conversions happen before borrowing from the resulting values; do not mutate the owner while references are in use.

For identity matching, use the existing string directly. For normalization or folding, use C. JSON escapes are already decoded at this boundary; do not send value strings through the JSON decoder.

Remove the redundant segment allocation at its producer. Do not run `collect_strings()`, discard its output, and then visit the same `Value` by reference. Keep an owned collector for build/public callers if they still need it; only reroute the matching-only query calls.

Owning a logical payload does not justify a second copy of its strings for matching. This requirement also applies when complex FILTER evaluation or logical output makes the `Value` itself necessary, provided the searched representation is supplied by that value.

## 5 B — Borrow from source without constructing unnecessary values

Reuse the predecessor's structural traversal, projection, string decoder, and duplicate-resolution machinery. Add or reuse a synchronous segment consumer; do not create a separate general-purpose JSON parser.

Required behavior:

- For an unescaped string under an identity representation, pass a source slice excluding its quotes. Unescaped non-ASCII text is eligible too.
- For escaped strings, decode into C's reusable storage.
- For a required normalization or folding operation, write its output into C's representation buffer.
- Preserve source bytes without in-place modification; original-byte output and other operations may still need them.
- Do not create `Value::String`, `serde_json::Value`, or owned `TextSegment` text solely for matching.
- Borrow scalar number/boolean/null lexemes when that is the reader's existing text representation.

Required direct-source coverage is bounded to these built-in cases:

| Query | Required direct-source coverage |
| --- | --- |
| Literal FTS alone | Unscoped and ordinary local field-scoped search; identity, current case-insensitive behavior, and existing built-in representations used by these paths. |
| One local scalar-string FILTER contains predicate | Existing literal and wildcard behavior; preserve the current missing/null/type semantics. |
| An AND of the above FTS and one qualifying FILTER predicate | Reuse prepared matching without forcing complete logical materialization. |

Support evidence on/off and locator/original-byte output for those cases. Ordinary local field paths exclude relations and explicit array selectors; arrays encountered by FTS must retain their existing separate-segment and field-resolution semantics. For a dynamic FILTER field, apply the existing runtime type rules rather than treating non-strings as strings.

Arbitrary Boolean FILTER trees, field-to-field comparison, complex quantifiers, relation evaluation, computed sorting, custom opaque transforms, and other readers may retain their current evaluator/materialization path. Such fallback does not exempt applicable existing-value matching from A.

Reuse existing regex matchers wherever the same consumer is sufficient; preserve other regex execution through its established path. Do not build a new regex or streaming matching engine.

For logical output or other residuals requiring values, preserve the predecessor's projection and output policy. Apply A to value-derived text, and keep source-derived text where required by section 3. Use a combined traversal where already available. Do not add an unconditional second full parse or source reread of every candidate, and do not undertake a general output-materialization redesign.

## 6 C — Reuse decoding and transformation buffers

Buffer reuse is mandatory for both routes. The worker/scan context owns the buffers for the lifetime of its candidate loop, across many segments and records. Query objects remain immutable and shareable; do not add a global mutex or shared mutable buffer.

Use two reusable text buffers for the ordinary routes:

- `decoded_text`: source string unescaping when needed.
- `transformed_text`: normalization, Unicode case folding, or another required built-in representation.

An existing composition such as normalization followed by folding may require another output while earlier text is still in use. Allow one additional reusable buffer only for that concrete need, document its lifetime, and account for its retained memory. Do not allocate a temporary string per segment to stay within an arbitrary buffer count.

Reuse existing equivalent buffers if already implemented. This table defines the required routing:

| Input and representation | Text supplied to matcher |
| --- | --- |
| Existing `Value` string, identity | Borrow the existing string. |
| Existing `Value` string, transformed | Transform directly from its borrowed string into `transformed_text`. |
| Unescaped source string, identity | Borrow the source slice. |
| Escaped source string, identity | Decode into `decoded_text`; borrow its contents. |
| Unescaped source string, transformed | Transform from the source slice into `transformed_text`. |
| Escaped source string, transformed | Decode into `decoded_text`, transform into `transformed_text`, then match. |

Fusing decoding and transformation is allowed only when straightforward and semantics-preserving; it is not required. Two reused buffers satisfy the final row for a transformation that writes directly to its final output; composed transformations follow the bounded exception above. A transform known cheaply to leave its input unchanged may borrow it; do not add an extra full-string pass solely to discover that optimization.

For built-in routines that currently return a fresh `String`, provide an internal form that writes into caller-owned storage. Keep the owned wrapper where other callers need it. Calling the old allocating routine and then copying its result into scratch does not satisfy this requirement. Nor does moving the scratch string out with `mem::take()` after each segment and losing reusable capacity.

Use `clear()` or equivalent between segments while retaining capacity. Growth when capacity is insufficient is allowed. Avoid per-record scratch construction, per-segment shrinking, and unnecessary clearing of the entire capacity. Finish all consumers of a buffer before clearing, growing, or overwriting it.

Preserve exact normalization/folding order, Unicode expansions, wildcard behavior, and representation-specific matcher semantics. Prepare query literals and matchers once. For multiple predicates, reuse an already produced representation within the current segment where simple; do not add a general transformation cache.

Use the existing retention/resource policy if available. Otherwise retain ordinary capacities and release exceptionally large capacities after the record according to one documented internal bound. That retention bound must not reject segments permitted by execution limits. Report retained memory per worker; one large record must not permanently inflate every worker's buffers.

Opaque extension transforms may retain their owned fallback. Do not redesign extension interfaces to eliminate their allocations; list those fallbacks explicitly.

## 7 Preserve segmentation, duplicates, and evidence

A decoded-scalar JSON segment is a complete scalar occurrence, not a fixed-size chunk. Spaces, line breaks, Unicode characters, JSON escapes, and surrogate pairs do not split a string. Different field values and different array elements are separate segments.

Examples required in tests:

- `{"text":"before\u2139after"}` supplies one segment, `beforeℹafter`; a match may cross the decoded escape.
- `{"a":"New","b":"York"}` cannot match `New York` across fields.
- `{"a":["local history","archives"]}` cannot match `history archives` across elements.

Do not concatenate segments. Do not introduce chunk-overlap handling or a streamed-character matcher in this task.

Preserve all duplicate occurrences for unscoped decoded-scalar FTS, and final-member-wins behavior for logical/field-scoped access. A later duplicate may replace an earlier match with a nonmatch, null, or an empty container, including removal of descendant contributions.

Reuse the current resolution machinery. Compact provisional match state or deferred selected source ranges are acceptable; retained owned text is unnecessary. Do not accept a field-scoped match before later relevant duplicates are resolved. Do not rescan all preceding segments on every duplicate or add an unconditional second structural pass.

Evidence must reflect final visible occurrences. Consume temporary paths synchronously and retain only required durable evidence. Membership-only early exit is allowed once all remaining query obligations are resolved; evidence mode may need to continue.

## 8 Preserve explicit index validation

This follow-up inherits the user's consistency policy:

- Indexed records retain their recorded validation state until an explicit index operation updates it, even if source files have changed.
- Search adds no freshness probes, modification-time/size/hash checks, revision comparisons, automatic refresh, or revalidation of previously validated records.
- Records indexed as invalid keep their recorded fallback handling.
- Search without recorded validation uses the existing validating path and does not silently update persistent state.
- Explicit additions, content changes, and rebuilds validate affected records. Path-only moves preserve state; deletion removes it.

Keep memory-safe access, I/O error handling, cancellation, and resource limits. An extraction failure must not trigger diagnostic full reparsing, validator retry, or implicit index mutation. Necessary decoding/conversion is not permission to validate unrelated content.

When an unvalidated search requires validation, complete it before accepting a result. A validated search need not scan trailing input solely to re-establish validity.

Do not expand this task into a lifecycle rewrite. Audit the predecessor's policy, repair a narrow omission encountered on the changed path, or report a concrete prerequisite blocker. Do not reintroduce validation as a workaround for ownership changes.

## 9 Scope exclusions and fallback rules

Do not change public ownership types, query syntax, index format, segmentation, result ordering, or reader semantics. Do not add parser libraries, replace the matcher engine, require AVX2, introduce unsafe lifetime conversions, or redesign caching, indexing, sorting, relations, the UI, or output scheduling.

A fallback must name the unsupported capability, not merely state that a request is complicated. Evidence, escapes, Unicode, built-in normalization, and the existence of an already required `Value` are not blanket reasons to bypass the applicable required route.

Existing owned build/output APIs may remain. Keeping them is not permission to copy text for supported matching-only query paths. Do not attempt to eliminate every allocation in the repository.

## 10 Tests and measurable completion criteria

Use existing tests and instrumentation. Compare unchanged validated inputs with the predecessor; preserve the deliberate stale-index behavior. Add the following focused assertions:

| Requirement | Required evidence |
| --- | --- |
| A: existing text | Identity matches receive the same string storage owned by `Value::String`/`Value::Id`; no text clone or re-decoding. Include nested values, arrays, and logical output. |
| A: paths | Traversal borrows key names; after path-stack capacity warm-up, no per-scalar owned path construction when evidence is disabled. Retained evidence remains correct. |
| A: producer | The query path no longer constructs redundant owned segments before invoking the borrowed consumer. |
| B: source text | Unescaped identity matches borrow from the source buffer; eligible membership queries do not construct a logical tree or owned segment strings. |
| C: reuse | Escaped, normalized, and folded cases reuse capacity across multiple segments and records. No per-segment temporary text allocation within warmed capacity. |
| Boundaries | Cross-escape matching succeeds; cross-field/array-element matching fails; long strings remain single segments. |
| Semantics | Unicode folding/normalization, empty patterns, wildcard escapes, missing/null/non-string FILTER inputs, and duplicate replacement in both directions. |
| Outputs | Evidence on/off, locator/bytes/logical output, combined supported FTS/FILTER, counts, ordering, pagination, streaming, and cancellation remain correct. |
| Compatibility | Documented complex/extension fallbacks and existing regex behavior still work; original source bytes remain unchanged. |
| Validation | Changed paths respect section 8, including unknown-validation search and explicit updates. |

Measure allocations at the extraction/transformation/matching boundary separately from source-buffer allocation, required logical values, duplicate bookkeeping, retained evidence, and output. Warm path/scratch capacities before steady-state assertions. Do not claim the entire query is allocation-free.

Use test-only allocation/copy counters or the existing allocator tooling; keep instrumentation out of the production inner loop. Check scratch reuse with alternating long/short, plain/escaped, and identity/transformed text so stale buffer contents cannot affect subsequent matches.

Completion requires A, B, and C for the defined coverage, either implemented here or verified in the predecessor. An agent may not replace this with an optional prototype or claim that source-byte borrowing also proves existing-value cloning was removed.

## 11 Bounded performance verification

Use the completed predecessor as the baseline. Record exact commits, commands, compiler/profile, worker count, data, output modes, and evidence/cache settings. No new benchmark framework or large dataset download is required.

Measure these focused cases in memory and end to end with the existing Musenalm/query harness:

1. Existing required values, identity matching: isolate removal of text and path copies.
2. Existing values, normalization/folding: isolate reusable transformed output.
3. Source-only verification, unescaped identity text.
4. Source-only verification with escapes and transformations.
5. Field-scoped duplicates and many scalar values.
6. High-hit-rate logical output as a materialization regression control.

Disable verified-result reuse for verification runs; warm OS caches, keep candidate sets and outputs equivalent, and run comparisons serially. Include the existing full query suite and repository-required workspace checks. Run a relevant build comparison if shared build code changed.

Report allocations/copied bytes, scratch growth and retained capacity, candidate/result counts, in-memory and complete query times with dispersion, source reads/bytes, and fallback frequency. Separate allocation-instrumented runs if instrumentation affects timing.

A required path must demonstrate its specified allocation reduction or show that the predecessor already achieved it. Do not promise a wall-time gain. Investigate repeatable regressions above roughly 5% and any substantial absolute slowdown; simplify the implementation or report the concrete blocker. Do not hide a regression by silently routing all required cases back to the allocating path.

## 12 Execution and delivery

Keep the original checkout on `master`. Follow current repository task-document and commit rules. Use:

- Integration branch: `integration/borrowed-segment-verification`.
- Integration worktree: `/home/simon/source/objekte-integration-borrowed-segment-verification` when the original checkout is `/home/simon/source/objekte`.
- Temporary notes: original checkout's `docs/ai/memory/borrowed-segment-verification/`.
- Report: `docs/ai/generated/borrowed_segment_verification_results.md`.
- Measurements: the existing benchmark-results location under this task identifier.

If the predecessor is merged, use mainline containing its work. Otherwise use its completed final integration commit and report that dependency. Do not overwrite its branch or integrate unfinished work blindly.

Use one supervisor. Delegate only separable work: existing-value traversal, source extraction, and independent verification are reasonable boundaries once the shared consumer/buffer interface is agreed. Every implementing agent gets its own sibling worktree under `/home/simon/source`; never use `/tmp`. Only the supervisor writes the integration worktree. Avoid parallel benchmark runs.

Implement in this order, skipping only requirements already verified:

1. Audit predecessor and record the baseline and exact remaining gaps.
2. Implement A and its producer/path-copy tests.
3. Implement or complete C, shared by both routes.
4. Implement B using existing extraction machinery; add required coverage.
5. Run semantic/allocation checks, benchmarks, and required workspace validation.
6. Review the combined diff, append the implementation report, and complete the task document.

Use separate reviewable commits, for example `perf(core): borrow existing value strings during verification`, `perf(core): reuse verification transformation buffers`, and `perf(core): consume borrowed source segments`. Push immediately after each commit, including integration/report commits. Never commit to or merge into `master`.

The final report must contain a requirement-by-requirement A/B/C disposition, predecessor/base/implementation commits, retained owned callers and reasons, exact optimized coverage and fallbacks, test results, allocation/copy reductions, timings, and unresolved limitations. Mark existing implementations as verified rather than inventing changes.

Move the completed task document to `docs/ai/done/` and append its report according to repository rules, preserving user content. Keep one integration branch and worktree for the user's merge. Remove only safely merged subordinate worktrees. If a required outcome is blocked, retain its work and report it explicitly rather than declaring the whole task complete.

## Implementation report

Task identifier: `borrowed-segment-verification`. Completed 11 November 2026 on `integration/borrowed-segment-verification` in `/home/simon/source/objekte-integration-borrowed-segment-verification`. Predecessor and task base: `5166876c7228f4fe2f8f064861cdc8a88275ad18`.

A now visits required Json/JsonLines `Value` strings and object keys by reference, while the query projection omits matching-only owned text segments. C writes Unicode folding into reusable worker-local storage and preserves decoded/folded buffers separately. B scans eligible built-in JSON source segments directly, borrowing unescaped strings and scalar lexemes, decoding escapes into scratch, and resolving relevant duplicate members before accepting field-scoped hits. The public owned APIs and extension/complex evaluator fallbacks remain. Logical output uses the required value/combined parse. No search-time index freshness or mutation behavior changed. See [the detailed implementation and performance report](../generated/borrowed_segment_verification_results.md).

Implementation commits: `1f906cb1be` (A), `c3b01ab22d` (C), `47931f0cdf` (B and focused tests/benchmark harness). Measurement/report commit: `b6c0bcba00`. One supervisor made the implementation; no subagent branches or contributions were used.

Validation: `cargo test --workspace` passed on the final source (486 core passed, 12 ignored; 158 desktop passed, one ignored). An initial full run hit one timing-sensitive desktop scheduler assertion; the isolated retry and final full run passed. `cargo clippy --workspace --all-targets` passed with existing warnings. Task-owned Rust spacing and `git diff --check` passed. The repository-wide spacing check still identifies an unrelated existing desktop file.

Performance: the exact predecessor and final commits ran the same extended `bench_candidate_verification` release harness with four workers, seven focused repetitions after warm-up, and zero verified-result cache. Forty focused comparisons matched membership and source-work counts. Direct-source cases reduced gross query allocations, including 287,225 to 6,101 for a field-scoped duplicate fixture. The full 194-query Musenalm suite passed in four cache/alphabet profiles; all 776 case/profile result/work rows matched. A relevant shared-code build comparison found no repeatable build regression. Commands, manifests, timings with dispersion, allocations, source reads, and fallback coverage are in [benchmark results](../../../bench/results/borrowed-segment-verification/README.md).

Remaining limitation: on the synthetic 8 KiB escape-heavy fixture, direct-source combined and scoped queries showed roughly 5–14% higher median times in repeated serial runs, while substantially reducing allocations. Safe decoding into reusable scratch is the remaining cost; returning to per-segment owned decoding would lose the required B allocation reduction. High-hit logical output stays on the combined parse and has no repeatable regression.

## Repair reopened on 11 November 2026

The user supplied `docs/ai/tasks/objekte_borroed_segments_repair.md` and explicitly authorized its implementation. The earlier completion report above is historical; root-scoped FTS, combined value/source ownership, relation target ownership, traversal overhead, and decoder/scratch behavior have been repaired on `integration/borrowed-segment-verification` through `f20fe5c1c7a5`. The full query suite has passed result and source-work parity across predecessor, reviewed, and repaired revisions. Seven-repetition alternating rechecks of the flagged timing rows and final validation are still running. The repair task remains open until those checks and its report are complete.

Cleanup: the detached benchmark baseline worktree was restored and removed after its results and harness source were preserved. The integration branch and worktree remain for the user's merge. The five generated fixture directories remain at `/home/simon/source/objekte-bench-borrowed-segment-verification` for local reproduction.
