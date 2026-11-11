# Objekte candidate verification optimization plan

Date: 11 November 2026  
Task identifier: `candidate-verification-optimization`  
Inspected base: `bebff5d2f82ac8c3230f0adff9365bb1bfc0d41b`  
Deliverable: one measured, tested integration branch for the user to merge.

## Objective and priorities

Reduce the cost of verifying source records after candidate selection, preserving every existing query and output feature. The main targets are uncached queries with many candidates: decoded-scalar FTS and FILTER predicates requiring source evaluation. Exact-index answers and verified-result cache hits are controls, not the primary target.

Implement and measure these changes separately:

1. Make search use the validation state recorded by explicit index operations, with no source freshness checks or revalidation of records indexed as valid.
2. Combine logical-value construction and decoded-scalar text extraction in the JSON query reader.
3. Prepare reusable substring matchers and accelerate JSON delimiter searches where measurements justify them.
4. Reduce case-folding allocation without changing Unicode semantics.
5. Only if profiling still justifies it, prototype selective verification that avoids constructing text segments or unnecessary values.

Keep the current query evaluator, query language, UI, and `ExtractedRecord` contract. Reuse the index's existing validation classification where it records whether validation was performed. If readers or persisted indexes cannot distinguish validated from unvalidated records, add the smallest explicit state needed, with conservative compatibility for older indexes and extensions. Do not introduce proof generation or a freshness framework. A broad parser replacement is a later experiment, not a prerequisite.

The user's validation policy is mandatory: search trusts the index's recorded validation state until an explicit index lifecycle operation changes it. That state may be stale relative to source files. Search must not test its freshness. This is an intentional change from the current implementation, which repeats JSON syntax checks during parsing and has source-revision checks.

This specification provides an implementation handoff. Its existence does not itself start repository work; follow the user's assignment and `docs/ai/README.md`.

## What the second pass means

In `source.rs`, `parse_json_decoded_for_query_diagnosed()` first calls `parse_json_for_query_diagnosed()` with text collection disabled. That parses JSON through serde, retaining selected top-level members where projection is available, and converts the result into Objekte `Value` objects.

When `projection.collect_text` is true, the function then calls `json_scalar_text::scalar_segments()` on the same original byte buffer. This is a second structural traversal in memory, not a second filesystem read. Text matching follows extraction. FILTER-only requests with text collection disabled do not incur this particular second traversal.

The second traversal retains information a normal logical object tree does not preserve:

| Source property | Required observable behavior |
| --- | --- |
| Escaped strings such as `"hist\u006fry"` | Search the decoded value `history`. |
| Number spellings such as `1.0`, `1e0`, and `-0` | Preserve their original spellings in the scalar text corpus. |
| Duplicate object members | Unscoped scalar FTS sees every occurrence; logical field access and resolved field text use the last occurrence. |
| Object member names and JSON punctuation | Do not add them to decoded-scalar value text. |
| Scalar boundaries and source order | Preserve the segment behavior expected by matching and evidence. |

For example, `{"title":"old","title":"new","n":1e0}` produces a logical object whose title is `new`, while unscoped scalar text retains `old`, `new`, and `1e0`. Reconstructing text only from the resolved value tree would change that behavior.

Collect both representations while parsing the bytes once. The existing `build_record()` and `ScalarWalker::build_value()` already combine them for fresh index construction. They are a starting point, not a direct query replacement: the build path does not supply the query path's field projection, populated field paths, and resolved-only text behavior.

Here, one traversal means one application-level structural parse. It does not forbid token-local decoding, later evaluation over extracted values, or an SIMD parser's internal processing stages.

## Execution workflow

Read current `AGENTS.md`, `docs/ai/README.md`, relevant task notes, and this complete specification. Record the actual base revision; recheck the cited functions if the repository has advanced.

Keep the original checkout on `master`. Assuming its location is `/home/simon/source/objekte`, create:

- Branch: `integration/candidate-verification-optimization`.
- Worktree: `/home/simon/source/objekte-integration-candidate-verification-optimization`.
- Temporary notes: the original checkout's `docs/ai/memory/candidate-verification-optimization/`.
- Benchmark results: `crates/bench/results/candidate-verification-optimization/`, following existing benchmark conventions.
- Requested report: `docs/ai/generated/candidate_verification_optimization_results.md`.

Use one supervising integrator. If delegation is available, assign the following separable work to subagents, each on a `task/candidate-verification-optimization/<subtask>` branch and a sibling worktree under `/home/simon/source`:

| Subtask | Ownership and handoff |
| --- | --- |
| `json-query-extraction` | JSON traversal, query-reader integration, parser differential tests. Owns `json_scalar_text.rs` and associated source-reader changes. |
| `text-matching` | Prepared matchers, folding buffers, matching tests. Owns the relevant `reference.rs` changes. |
| `verification-benchmarks` | Existing benchmark harness integration, workloads, result comparison. Avoid simultaneous performance runs with other agents. |

The JSON agent owns delimiter-search changes too; do not have two agents edit the walker concurrently. The integrator owns shared interfaces, dependency changes, conflict resolution, combined checks, and the completion report. Subagents branch from a recorded integration commit and return pushed commits, test results, and measurements.

Push immediately after every commit, including integration and report commits. Preserve user changes and task-document state as required by the repository workflow. Never merge into or push changes to `master`. Keep the integration branch and worktree for the user's merge.

## Phase 0 Establish a trustworthy baseline

Use the existing benchmark commands and workload files. Record the precise command, commit, build profile, compiler, dependency versions, CPU, thread count, dataset, and cache configuration. Confirm which reader each workload uses; the built-in `Json`, `JsonDecodedScalars`, and `JsonLines` modes do not have identical text semantics.

Record complete result membership, ordering, counts, payloads, evidence, and diagnostics for the focused regression corpus. Use the old reader/evaluator as a differential oracle for unchanged validated records and the validating path for unvalidated records, preferably under test-only configuration. Tests involving externally changed sources must follow the explicit lifecycle policy below, rather than require the old search-time revision rejection or revalidation. Do not maintain two production implementations indefinitely.

Measure two levels:

1. In-memory parse/extract/evaluate work over a fixed candidate sequence, excluding file opens and reads.
2. Complete repository searches with the verified-result cache disabled and OS file cache warmed, keeping candidate sets, requested outputs, and worker settings equivalent.

Retain the existing full query suite as a regression check. Use additional focused queries to expose the changed paths; fast exact-index queries alone cannot demonstrate this optimization.

## Phase 1 Combine JSON query extraction

### Implementation

Introduce one internal query extraction operation, for example `extract_json_query_record(bytes, projection, validation_state)`, returning the existing `ExtractedRecord`. Its name and exact location should follow repository conventions. Pass the index's recorded state from the execution context; do not derive it by inspecting the source during search.

Extend or refactor the existing combined build traversal to support query extraction, sharing token handling where practical. Avoid duplicating a third JSON grammar implementation. The operation must perform the following work during a single structural traversal:

1. Dispatch from recorded validation state: use trusted extraction for records indexed as valid, preserve the recorded raw-fallback path for records indexed as invalid, and perform validating extraction only when the index records no validation. Do not run source freshness checks in any search path.
2. Construct Objekte logical values directly for the required projection. Do not introduce an intermediate full `serde_json::Value` tree merely to convert it immediately afterward.
3. Decode each relevant string token once during extraction, then populate the required logical-value and text outputs. Retain numeric lexemes separately from exact numeric values.
4. Track field paths for text segments using a reusable path stack, copying only when the owned output requires it.
5. Preserve source-order scalar occurrences, applying the appropriate duplicate-member policy to the logical and text outputs separately.
6. Return the same shapes and semantics consumed by `prepare_query_record()`, necessary value conversion, FTS evaluation, FILTER evaluation, sorting, and payload construction. For records indexed as valid, keep conversion separate from repeated schema validation, including logical output.

Keep `parse_json_decoded_for_query_diagnosed()` as the integration boundary if useful. Implement the combined extraction first where text is requested, while preserving narrow projections for FILTER-only queries. The validation policy applies to both paths: leaving an unconditional validating serde parse in the indexed FILTER-only path is not an acceptable final implementation. Keep text collection disabled when unnecessary and measure projection performance independently.

### Projection and duplicate members

Treat logical-value projection and text collection as separate requirements. A field omitted from the logical projection can still contribute to unscoped FTS. Do not discard its text because its value is not retained.

Preserve behavior for object, array, scalar, and empty roots. The existing top-level projection applies to object roots; do not accidentally replace non-object roots with empty objects.

For duplicate members, maintain last-member-wins values while retaining every scalar occurrence for unscoped text. For resolved text, retain only occurrences belonging to the final member, including when an entire earlier object or array subtree is replaced. A later empty object or array can erase all earlier text for that member. Preserve the current scalar order after resolution.

Use per-object occurrence bookkeeping or equivalent bounded traversal state. Do not repeatedly rescan all prior segments for each duplicate member or clone the complete path at every structural token. Measure heavily duplicated and deeply nested inputs to detect quadratic behavior.

### Validation state belongs to explicit index operations

The recorded state is an index snapshot, not a claim about the current source bytes. Search must trust that snapshot until an explicit lifecycle operation replaces it. Do not condition trusted extraction on a fresh index, matching modification times, matching sizes, matching fingerprints, a source hash, or a fresh validation certificate.

| Operation or recorded state | Required behavior |
| --- | --- |
| Explicit full build or rebuild | Validate supported syntax, encoding, and declared types; record the resulting state and diagnostics. |
| Explicit add or changed-record update | Validate the newly indexed record; publish its value/text indexes and validation state together. |
| Explicit partial rebuild | Revalidate the affected records and update their state with the rebuilt index parts; retain unrelated records' state. |
| Explicit move | Update the indexed path; preserve validation state for a path-only move. If the operation also replaces content, handle it as a changed-record update. |
| Explicit delete | Remove or deactivate the record and its validation state; no source-content validation is required. |
| Explicit refresh or synchronization | Perform change detection here, then apply the corresponding lifecycle operations. |
| Search of a record indexed as valid | Decode/extract what the query needs, with no document or schema revalidation. |
| Search of a record indexed as invalid or raw fallback | Use the recorded fallback behavior and diagnostics; do not retry validation because the source might have been repaired. |
| Search where the index provides no validation | Validate during the source parse needed for the query. This includes unindexed search and readers/indexes that did not perform validation. |

Use existing state where sufficient. The fact that an index exists is not enough to label all extension records as validated; validation must actually have been performed during an index operation. Conversely, age does not downgrade a recorded valid state. Dispatching on an in-memory state flag is sufficient; do not recompute schema signatures, reader fingerprints, or source metadata per candidate. Any validation-relevant configuration change is applied through an explicit index update or rebuild.

Search must not stat files for freshness, compare source revisions or fingerprints, rehash content, trigger refresh/rebuild, promote or invalidate recorded validation state, or invalidate query caches merely because externally changed bytes are encountered. Existing filesystem access needed to open and read the requested record remains necessary; retain ordinary I/O failure handling and range arithmetic checks. Audit the search call chain through source-access and record-range helpers so hidden revision checks do not survive beneath the new extractor. Lifecycle operations retain the checks needed to update the index explicitly.

For indexed valid records, do not perform a complete grammar, encoding, or schema validation pass, including inside a parser dependency. Do not scan skipped fields or trailing input solely to re-establish validity. Structural traversal and decoding required to locate fields, resolve duplicates, evaluate predicates, or produce requested output still occur. Apply needed temporal and other value conversions without rerunning declared-type validation. The logical-return path must follow this policy too; split its current validation/conversion routine where necessary.

For records without recorded validation, use the validating extraction path and preserve existing malformed-input handling, including skipped content and trailing input. A search-time validation result is local to that query; it does not silently update persistent index state. Queries satisfied entirely by indexes or caches need no source read just to validate.

The existing scalar walker's assumption of valid input can be supported by the recorded state on the trusted path. Reuse it where useful, with the projection, value-building, and duplicate-resolution additions described above. Keep checked memory access, valid Rust string construction, cancellation, and runtime resource limits in all paths. Trusted extraction must remain memory-safe even when externally edited bytes are malformed; it may report an extraction or I/O error when it cannot proceed. Do not respond by performing a diagnostic full reparse, retrying through the validator, or changing the index's recorded classification.

Preserve exact numeric semantics and original number spellings. Token decoding and required conversions may fail; they are not permission to validate unrelated fields or the whole document. Keep text finalization semantics unchanged.

Externally edited source content may produce stale candidates, stale cached answers, missed matches, changed payloads, or extraction failures until the user explicitly updates the index. That is the selected consistency model. Do not introduce search-time repair or freshness checking to conceal it. Publish updated postings, validation state, diagnostics, and applicable cache invalidation through the existing explicit lifecycle operation.

### Compatibility and acceptance

Keep `SourceReader`, `ExtractedRecord`, `TextSegment`, query syntax, return modes, evidence, and extension behavior compatible. Do not change `Json` or `JsonLines` into decoded-scalar readers as a side effect.

The owned output types can still require copying a decoded string into both the value and text outputs. That is acceptable in this phase. Removing a traversal and removing every allocation are different changes; do not introduce shared string ownership or a public lifetime redesign to combine them.

Acceptance requires equivalent query results on unchanged validated data, correct validating behavior when no validation is recorded, demonstrated removal of repeated document/schema validation and search-time freshness checks for indexed valid records, removal of the second structural traversal, and a useful measured improvement without material projection or build regressions. The explicitly selected stale-index behavior is a policy change, not a differential-test failure.

## Phase 2 Prepared text matching and delimiter search

Benchmark these changes independently of Phase 1, then measure the combination.

For literal contains and eligible literal FTS, construct a `memchr::memmem::Finder` once per prepared query literal and reuse it across candidate segments. Use an owned finder or another ordinary safe ownership arrangement; do not introduce self-referential structures or lifetime transmutation. Keep preparation and retained-memory accounting consistent with the query objects.

For wildcard contains, prepare the existing literal pieces and preserve their ordered, non-overlapping search behavior. Retain escape handling, empty-pattern behavior, empty pieces, case sensitivity, and all existing `%` semantics. Keep regex queries on their existing compiled-regex path. Do not route custom matcher operations around their declared semantics.

`Finder` explicitly supports reuse across haystacks; its owned form copies the needle once. The benefit is workload-dependent. Measure short and long strings, short and long needles, misses, early hits, and late hits. Do not assume the standard library's current substring implementation performs no optimization.

In the scalar walker, compare a `memchr2` search for the next quote or backslash with the current byte loop. Preserve decoding and the validation-state dispatch defined in Phase 1: indexed valid records use trusted extraction; unvalidated records use validating extraction. Never introduce unchecked reads or vector loads past the input boundary. Retain portable builds and instruction-set fallback rather than globally requiring AVX2.

Use the repository's dependency conventions and compatible pinned versions. The proposed `memchr` experiment is part of this plan's implementation scope; do not add unrelated parser libraries or dependencies under it.

## Phase 3 Reduce case-folding allocation

Preserve the existing full Unicode case-folding operation, including expansions such as `ß` to `ss`, rather than replacing it with ordinary lowercase conversion.

Prepare the query literal once using the current transformation. For ASCII candidate text, benchmark folding into reusable worker-local scratch storage. Use an ASCII shortcut only where it is equivalent to the current operation and all selected representation transforms. For other text, stream the existing fold operation into the same reusable buffer where the API permits.

Scratch buffers belong to a worker or scan context, not a global mutex. A mutable shared query object must not serialize independent workers. Bound retained capacity so one unusually large record does not cause every worker to retain an oversized allocation indefinitely. Do not allocate a full folded copy when no case-insensitive matching is required.

Keep transforms and representation selection in the same order. Measure allocation count, allocated bytes, throughput, and worker memory; reject a faster microbenchmark if memory retention materially worsens real searches.

## Phase 4 Optional selective verification prototype

Start only after the earlier phases establish that materialization remains a significant cost. First keep this internal to the built-in reader and executor; a new required public extension method is outside the initial implementation.

A restricted verifier may consume decoded scalar events directly, evaluate the existing prepared matcher, and retain only membership and requested evidence. Unescaped strings can borrow the input slice; escaped strings need decoding into reusable storage. Follow recorded validation state rather than adding revalidation to either case. Borrowed data must not outlive the candidate byte buffer or survive scratch-buffer reuse.

Eligibility must account for the whole request: residual predicates, schema conversions, text representations, relation evaluation, computed sorting, evidence, and return mode. Reuse existing bound matchers rather than writing a second query language interpreter. Unsupported combinations continue through the combined parser and established evaluator, preserving all user-visible features.

For records indexed as valid, stop as soon as query membership and every requested output are determined; do not scan the remaining bytes to validate them. For records without recorded validation, complete the required validation before accepting the result. In either mode, field-scoped results cannot be committed before relevant later duplicate members have been resolved. Collect every field required by evidence mode; preserve three-valued FILTER logic, NOT behavior, and quantifiers.

Compare any successful-hit materialization cost with the current path. High hit rates, logical payloads, computed sorting, or extensive evidence can erase the savings of a selective verifier. Report the eligible query fraction and total end-to-end gain; do not advertise only the favorable rejection path.

If this requires substantial public interface or ownership changes, deliver Phases 1–3 with the measured prototype findings and a separate proposal. Do not make completion of the low-risk optimization depend on a new extension architecture.

## Differential test matrix

Extend existing tests rather than writing a separate duplicate suite. Include parser-level equivalence and representative executor-level searches so the current build/index/query agreement is exercised.

| Area | Required cases |
| --- | --- |
| Strings | Plain ASCII; escapes; escaped quotes/backslashes; control characters; BMP and supplementary Unicode; surrogate errors; invalid UTF-8. |
| Numbers | Zero and negative zero; decimals; exponents; huge exact integers; large exponents; invalid leading zeros, signs, and fractions; current resource limits. |
| Duplicate members | Scalar replacement; nested object replacement; array replacement; later empty container; escaped and unescaped equivalent keys; nested duplicates. |
| Structure | Root object, array, string, number, boolean, null; empty containers; heterogeneous arrays; nesting boundaries; illegal whitespace; trailing input; malformed skipped fields. |
| Text corpus | Values without names or punctuation; source order; scalar boundaries; unscoped all occurrences versus resolved field paths; exact numeric spellings. |
| Queries | FILTER alone; FTS alone; both combined; field-scoped and unscoped search; sensitive/insensitive matching; regex; wildcard escaping; empty patterns; AND/OR/NOT; array selectors and quantifiers. |
| Integration | Value conversions without revalidation on trusted paths; custom readers/bindings; recorded malformed raw fallback; missing/deleted/changed sources; cancellation; resource limits; JSONL record ranges. |
| Validation lifecycle | Full/partial rebuild, add, update, move, delete; state persisted and loaded without freshness probes; no-validation readers; query-local validation without persistent state changes. |
| Stale sources | External valid-to-invalid and invalid-to-valid edits do not cause validation, refresh, revision comparisons, or classification changes during search; explicit updates apply the new state. |
| Results | Locator, bytes, logical payload; evidence on/off; counts; pagination; indexed and computed sort; relation queries; cached and uncached results. |

Use deterministic generated valid documents for output equivalence. Test malformed mutations through build/update validation and through search with no recorded validation. For trusted extraction of externally corrupted input, require bounded memory-safe handling, not full malformed-document diagnostics or equivalence with the validating parser.

Instrument or mock the relevant boundaries to assert that searches of indexed valid records call neither a full document/schema validator nor source freshness probes. Cover FILTER-only, FTS, logical output, streaming, and selective verification. Check that source-access helpers perform no search-time stat/hash/revision comparison. Explicit lifecycle operations must validate changed content and publish the resulting state; path-only moves and deletion must avoid unnecessary content validation. Retain runtime resource, cancellation, and memory-safety checks in both extraction modes.

## Benchmark design and decisions

Use Musenalm and the existing full query suite. Add controlled in-memory records and, if available, an existing larger-file dataset. A synthetic large record is useful but must be identified as synthetic. Do not block the work on downloading a new multi-terabyte corpus.

Cover many small files and larger records, selected fields in large objects, plain and escaped strings, ASCII and Unicode, mostly rejected candidates, frequent matches, and early/late/no matches. Include FILTER-only queries as a projection control, exact-index/cache paths as execution controls, and a fresh-build check because walker code is shared.

Record per variant:

- Candidate IDs/counts, verified result IDs/counts, file opens, bytes read, and logical record parses.
- In-memory verification time and complete query wall time, with median and dispersion across repeated runs.
- Allocations and allocated bytes per candidate in a separate instrumented run; retained/peak memory where practical.
- Matcher preparation cost and amortized search cost.
- Full suite results and relevant build timing.
- Full-document/schema validator invocations and search-time freshness probes, separated from necessary token decoding and I/O; both must be zero for records indexed as valid.

Existing record-parse counters may not count internal traversals, validator calls, or revision probes separately. Use focused tests and temporary local instrumentation to demonstrate their removal; do not reinterpret a counter as evidence of work it never measured. Benchmark searches without recorded validation separately so their required validation cost is not confused with the trusted indexed path.

Run release builds on the same machine, with equivalent compiler settings and worker counts. Warm the filesystem cache deliberately. Disable or clear verified-result caching between measured verification runs without changing query semantics. Avoid simultaneous builds, benchmarks, or other agents' performance runs. Alternate baseline and variant runs and repeat enough times to separate change from noise.

Measure baseline, recorded-state dispatch with search-time revalidation/freshness checks removed, combined extraction, prepared matching, delimiter search, folding reuse, and the selected combination. Separate effects where the implementations permit; state any dependency between variants. Preserve revision identifiers for each comparison. Do not add individual percentage gains together.

The explicit validation lifecycle is a required behavior change, not an optional benchmark experiment. For the additional performance optimizations, proposed decision criteria are engineering gates, not predicted results: require repeatable gains exceeding run-to-run noise on affected workloads and investigate any representative end-to-end regression above 5%. Treat very small absolute changes separately. Prefer retaining a simple optimization with broad modest gains over a complex optimization supported only by a narrow microbenchmark. If an optional optimization has negligible gains, leave it out and retain its experiment report.

Parser substitution with `sonic-rs` or `simd-json` is a separate future comparison if tokenization remains dominant. Require the same logical/text outputs and include conversion, source preservation, and allocation costs. A parser-only throughput number does not establish an improvement to Objekte queries.

## Suggested commits and completion

Use separate reviewable commits, each immediately pushed:

1. `test(core): define explicit index validation lifecycle behavior`.
2. `perf(core): use recorded validation state during search`.
3. `perf(core): combine JSON query value and scalar extraction`.
4. `perf(core): reuse prepared literal substring matchers`.
5. `perf(core): accelerate JSON string delimiter search`.
6. `perf(core): reuse case folding buffers during verification`.
7. `docs: record candidate verification benchmark results`.

Adjust commit boundaries to actual findings; omit optional optimizations that fail the measurement gates. Implement the explicit validation lifecycle regardless of whether a particular SIMD experiment succeeds. Keep optional selective-verifier work separately reviewable.

Run the repository-required workspace checks and relevant performance benchmarks against the final integration branch. Document the exact commands and results. Review the combined diff for accidental API, index-format, corpus, and default-query changes.

The report must state accepted and rejected experiments, independent and combined measurements, correctness coverage, affected and unaffected paths, remaining bottlenecks, and any unresolved limitations. Append the required implementation report to the completed task specification and move it to `docs/ai/done/` according to repository rules. Commit and push it on the integration branch. Preserve the integration worktree; clean subordinate worktrees only after their work and findings are preserved.

Completion means the user receives one integration branch implementing the explicit validation lifecycle, with measured optimizations and equivalent query behavior for unchanged validated data. It does not mean a promised speedup, automatic adoption of a different JSON parser, or a merge into `master`.

## Source references

- [Decoded-scalar query and build reader paths](https://github.com/Simon-Martens/objekte/blob/bebff5d2f82ac8c3230f0adff9365bb1bfc0d41b/crates/core/src/source.rs).
- [Scalar traversal and combined build extraction](https://github.com/Simon-Martens/objekte/blob/bebff5d2f82ac8c3230f0adff9365bb1bfc0d41b/crates/core/src/json_scalar_text.rs).
- [Existing scalar and build-equivalence tests](https://github.com/Simon-Martens/objekte/blob/bebff5d2f82ac8c3230f0adff9365bb1bfc0d41b/crates/core/src/json_scalar_text.test.rs).
- [Candidate scan and output preparation](https://github.com/Simon-Martens/objekte/blob/bebff5d2f82ac8c3230f0adff9365bb1bfc0d41b/crates/core/src/run.rs).
- [Shared preparation and schema conversion boundary](https://github.com/Simon-Martens/objekte/blob/bebff5d2f82ac8c3230f0adff9365bb1bfc0d41b/crates/core/src/record_preparation.rs).
- [Prepared contains matching](https://github.com/Simon-Martens/objekte/blob/bebff5d2f82ac8c3230f0adff9365bb1bfc0d41b/crates/core/src/reference.rs).
- [Repository agent workflow](https://github.com/Simon-Martens/objekte/blob/bebff5d2f82ac8c3230f0adff9365bb1bfc0d41b/docs/ai/README.md).
- [memchr reusable Finder API](https://docs.rs/memchr/latest/memchr/memmem/struct.Finder.html) and [memchr2 delimiter search](https://docs.rs/memchr/latest/memchr/fn.memchr2.html). Verify the repository-selected version before implementation.

## Implementation report

Task identifier: `candidate-verification-optimization`
Completed: 2026-11-11
Base commit: `2bd4dbb15fd6bc7ddec178d056eefcad9b764362`
Integration branch: `integration/candidate-verification-optimization`
Integration worktree: `/home/simon/source/objekte-integration-candidate-verification-optimization`

Indexed built-in logical records now use their published validation class to enter a trusted projected query extractor. Recorded raw fallbacks retain fallback evaluation and diagnostics; extension registrations and unindexed scans validate. Search range reads no longer compare source revisions, metadata, or fingerprints, and ordinary search no longer triggers reconciliation after external source changes. Explicit build, update, move, delete, rebuild, and reconciliation operations keep their lifecycle responsibilities. Trusted logical materialization applies necessary conversions without repeating schema validation. The decoded-scalar reader combines value and text extraction, literal matchers reuse owned Finder instances, folding reuses scratch storage, and JSON string scanning uses the measured hybrid delimiter strategy plus single-pass decoding of required strings.

Contributing subtask branches and commits:

- `task/candidate-verification-optimization/json-query-extraction`: `3a28f38f7e` (combined JSON extraction), `034ade87de` (delimiter search).
- `task/candidate-verification-optimization/text-matching`: `ed6a59ef75` (prepared Finder and folding scratch).
- `task/candidate-verification-optimization/verification-benchmarks`: `befbeef3` (baseline, fixture generator, focused harness, full-suite archive).
- Integration commits: `525ef56135` (published validation and source read policy), `97f3d75d11` (required string-token decoding), `835a4bee52` (final benchmark archive and generated report). Subtask merge commits were pushed after each merge.

Validation: `cargo test --workspace` passed, including 477 passed and 12 ignored core library tests. `cargo clippy --workspace --all-targets` passed with existing workspace warnings. `cargo build --release --locked -p objekte-search-core --example bench_candidate_verification --example bench_musenalm` passed. All eight focused seven-repetition runs passed complete membership and source-work comparisons against the base. The short-plain 14,757-line result/payload/evidence oracle matched byte-for-byte. Both 194-case Musenalm suites (ASCII and Unicode, uncached and warm cache, five repetitions) had zero failures and zero differences in result or work-counter CSV columns across 388 case/profile rows each. Task-owned Rust files passed the spacing convention and `git diff --check`; the repository-wide spacing check and strict warning-free Clippy are affected by unrelated pre-existing files.

Median warm search examples (base → final): Amazon20k decoded-scalar FTS 13.526 → 9.444 ms, votes/rating FILTER 17.802 → 15.857 ms, combined FTS/FILTER 9.877 → 7.430 ms; synthetic long-escaped FTS 28.737 → 13.841 ms and combined FTS/FILTER 27.904 → 12.956 ms. The full Musenalm median per-case final/base ratio was 0.912 uncached and 0.995 with warm verified-result caching. Fresh build parse counts and estimated index bytes were unchanged; build wall times varied in both directions. Exact commands, environment, corpus digests, raw runs, variant comparisons, accepted and rejected experiments, and limitations are in [the generated results report](../generated/candidate_verification_optimization_results.md) and [the benchmark archive](../../../bench/results/candidate-verification-optimization/README.md).

The task's `crates/bench/results/` destination was corrected to the repository's existing root `bench/results/` convention. The optional restricted early-exit verifier and a sonic-rs/simd-json parser substitution were not selected; the generated report records the measured gate and a future proposal. External edits intentionally may yield stale candidates, changed byte payloads, stale cache hits, or extraction errors until an explicit index operation updates the published state. No unresolved in-scope failure remains. The integration worktree is retained. All three subordinate worktrees and the task memory were removed after the completion document was first pushed.
