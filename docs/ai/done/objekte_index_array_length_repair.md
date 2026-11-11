# Objekte: array-length and empty-string index repair

## Execution and scope

Implement directly on `master`, as explicitly requested for this task. Do not create an integration branch or worktree. Record the actual starting commit, preserve unrelated working changes, and make small coherent commits; push after each commit. Do not reset or force-push. The master revision inspected for this specification is `95f1736d448a01d94650afd237149c158c0e7a8a`; inspect newer changes before implementation and reuse any completed work.

Fix index coverage for array lengths and exact empty strings. Preserve query semantics, public query syntax, and the explicit index-update lifecycle. This is a core-library change, not a benchmark-only configuration adjustment.

## Evidence and required outcome

The latest complete Musenalm benchmark measured `49ac3f503ac63f8303f85122a1ec4f0043949cda`. These Unicode, verified-cache-disabled medians are historical context, not a substitute for measuring current master:

| Case | Collection and predicate | Median, ms | Source records parsed | Matches |
| --- | --- | ---: | ---: | ---: |
| Q115 | entries: `items:length >= 2` | 20.059 | 4,928 | 244 |
| Q116 | contents: `contributors:length > 0` | 441.719 | 133,948 | 70,428 |
| Q117 | contents: `contributors:length = 0` | 420.410 | 133,948 | 63,520 |
| Q118 | contents: `contributors:length >= 3` | 391.493 | 133,948 | 254 |
| Q120 | contents: `incipit_statement = ""` | 387.572 | 133,948 | 42,700 |

Each contents case reads 149,889,026 source bytes. The benchmark already has indexed child paths such as `contributors.type`, but no parent `contributors` posting; `incipit_statement` also has no value posting. Conversely, indexed `places:length = 0` and `places:length > 0` took 0.297 and 0.658 ms.

After rebuilding with the repaired library, all five target cases must return identical results with **zero query source opens, bytes read, record parses, and source scans**, with the verified-result cache disabled. Use the unchanged benchmark queries and normal collection configuration, including counting, evidence, and locator output. Bytes/logical output may still require source materialization.

Sources: `bench/results/borrowed-segment-verification/repair-2026-11-11/suite-repaired-final-off/{query-results.csv,queries.json,run-manifest.json}`.

## 1. Reuse existing structures and separate index capabilities

Inspect these existing boundaries before changing them:

- `crates/core/src/posting.rs`: `PostingIndex.lengths`, `add_values`, `exact_set_restricted`, `exact_set`, `estimate_exact`, and `length_posting_is_exact` already implement array-length postings. `PostingCollection::new` currently selects fields using `FieldDefinition.indexed`.
- `crates/core/src/schema_adapter.rs`: `indexable` rejects arrays whose element type is Object. Do not make arbitrary objects scalar-indexable to work around this.
- `crates/core/src/inference_build.rs`: length collection exists, but retention is coupled to general value-posting admission/drop decisions. Inspect the current inference and short-string coverage mechanisms before adding another representation.
- Follow the existing posting construction, query planning, incremental segment, snapshot, and compaction callers.

Represent structural coverage separately from a complete scalar-value dictionary. A field with only length or empty-string facts must never be advertised as having exact postings for arbitrary scalar comparisons. An absent value in an incomplete dictionary is not proof of no match. Cost estimates and planner diagnostics must reflect the actual available capability.

Reuse existing postings when they already provide complete coverage. Avoid retaining duplicate facts or implementing a second general indexing subsystem.

## 2. Array-length coverage

Retain `length -> record bitmap` for declared array fields whose indexing is permitted, including arrays of objects, independently of whether element values receive scalar postings. Integrate this into normal declared-schema construction; do not require the user to index every child or manually annotate these five benchmark fields.

Use the same capability in the existing opt-in schema-free auto-index path. Do not enable schema-free inference globally. Respect explicit indexing prohibitions and configured resource limits. Distinguish an omitted scalar-value index from an explicit prohibition.

Build these facts while the record is already parsed. Read the array's element count; do not traverse or serialize its elements merely to determine length. Support existing equality and range operators through the existing length-posting implementation.

Preserve the existing exactness restrictions for nested paths, repeated reached arrays, quantifiers, and explicit array selectors. In particular, a union of records containing one array of length N does not prove every reached array satisfies a predicate. Retain the established evaluator where coverage cannot prove the answer. The five target cases require exact index execution.

An empty array, missing field, null, and wrong-type value must retain their existing distinct semantics. Do not implement `length > 0` as an unrestricted complement of `length = 0`.

## 3. Exact empty-string coverage

For declared string fields whose indexing is permitted, retain an exact `value == ""` bitmap independently of full string-dictionary admission. Apply the same rule to eligible strings in the existing auto-index path. High string cardinality or rejection of long-string dictionaries must not discard this small fact.

For ordinary scalar string equality, return the exact bitmap intersected with the active, covered logical-record domain. An existing complete value dictionary can supply the fact without additional storage.

Do not index every nonempty string to solve this case. Distinguish `""` from whitespace-only strings, missing fields, null, arrays, and objects. Do not infer an empty string from an absence of bigrams. Normalized text and custom transformations may change emptiness; optimize only expressions for which the stored representation proves the result.

Other comparisons and multivalued paths must keep their established behavior. Do not extend the empty-string capability to arbitrary equality, inequality, or quantification without an exactness proof.

## 4. Lifecycle, persistence, and bounds

Construct/update facts during explicit initial build, add, content update, and rebuild. Deletion removes active visibility; path-only movement preserves content facts. Keep postings, active domains, and cache invalidation consistent through incremental segments, compaction, and save/load.

Do not add file freshness checks, metadata probes, content hashes, automatic refresh, or revalidation to ordinary search. Preserve the current handling of malformed/raw-fallback records and inputs without indexed validation.

Persist capability and coverage information, including a complete fact with zero matching records. Old snapshots lacking these facts must remain explicitly unsupported for the optimized route until an authorized rebuild; never interpret missing serialized data as a complete empty posting. Use the project's existing snapshot compatibility/version mechanism.

Account for memory under the existing policies. If coverage is dropped or incomplete, fall back correctly and expose the reason. Ordinary Musenalm configuration must retain the target facts. Avoid allocating or cloning complete text values for the new bookkeeping.

## 5. Verification and performance acceptance

Add focused differential tests against the reference evaluator:

- Empty/nonempty arrays of scalars and objects; lengths 0, 1, 2, and 3; missing, null, nested/multivalued selections, and existing quantifier semantics.
- Empty strings, spaces, tabs/newlines, Unicode text, missing/null/wrong types, and duplicate JSON members under the established logical-field policy.
- A structural-only index must not incorrectly answer an unsupported value predicate. Cover complete-but-empty postings, explicit index prohibitions, and budget/coverage fallback.
- Add, update across empty/nonempty and length boundaries, delete, move, save/load, and compaction preserve result membership and counts. An unrefreshed source edit must not introduce query-time freshness checks.

Before implementation, capture a baseline from the actual starting master on the same machine and unchanged corpus. After an explicit index rebuild, run Q115–118 and Q120 with verified cache disabled and assert the zero-source-work counters. Compare membership as well as counts.

Run the complete 194-query suite under ASCII/Unicode, cache off/warm, using the existing three-repetition protocol. Measure the five target cases with at least seven repetitions after warm-up. Record median and dispersion, exact commits, candidate/source-work counters, build time, resident/index memory, and snapshot size. Preserve all query limits and counting settings.

Require a substantial uncached improvement, supported by elimination of source work; do not use warm result-cache hits as proof. Investigate repeatable unrelated regressions above both 5% and 0.5 ms, and repeatable build or memory growth above 5%. Report the cause and cost rather than hiding it in aggregate medians. Do not promise a fixed sub-millisecond result across hardware.

Run formatting and the applicable core, lifecycle, storage, and adapter tests; complete the repository's required checks for the touched components.

## 6. Q119 and exclusions

Q119, `@file.modified >= "2026-01-01T00:00:00Z"`, already opens and parses zero source records. Its 26.734 ms is a separate metadata/result-processing cost. Include a brief profile or counter-based diagnosis in the report. Fix an obvious local redundant operation only if demonstrated and covered by the relevant tests; otherwise record a separate follow-up. Do not introduce a metadata-index redesign here.

Parser/SIMD changes, relation sorting repairs, new query syntax, general string dictionaries, and a broad inference redesign are outside this task.

## Delivery

Suggested commit sequence: (1) array structural coverage, (2) empty-string coverage, each with its required lifecycle/persistence changes and tests; (3) benchmark results and report.

Write `docs/ai/generated/structural_predicate_index_repair_results.md` with the exact before/after table for Q115–118/Q120, the coverage/opt-out behavior, complete-suite outcome, construction/storage costs, and the separate Q119 finding. Follow the repository task-document convention. Leave master containing the completed fixes and pushed commits.


## Implementation report

- **Task identifier and date:** `index-array-length-repair`, completed 2026-11-11.
- **Base and integration:** started from `95f1736d448a01d94650afd237149c158c0e7a8a`; integrated directly on `master` in `/home/simon/source/objekte`, as this task expressly required. No subordinate branches or worktrees were created.
- **Implementation commit:** `90b52907c7f9c9364305e38e1b689c9766cc84d0`, pushed to `origin/master` immediately after commit. It adds independent length and empty-string posting capabilities, planner guards against treating partial postings as complete dictionaries, opt-in inferred coverage within budgets, lifecycle and snapshot handling, tests, and a reproducible reference verifier.
- **Correctness and checks:** `cargo test --workspace --locked` passed. The focused differential, inference, lifecycle, snapshot compatibility, CLI, and desktop tests are included. Task-owned Rust formatting checks and `git diff --check` passed. Prior snapshot formats 19/20 are rejected until explicit rebuild; new versions are 21/22.
- **Benchmarks:** the unchanged 194-query Musenalm suite passed 776/776 profile rows with identical counts and returned-page sizes. Seven-repetition Unicode cache-off medians for Q115/Q116/Q117/Q118/Q120 changed from 19.395/472.949/429.785/402.704/403.198 ms to 0.108/10.051/18.109/0.699/9.672 ms. Each target now has zero query source opens, bytes, parses, and scans. Full-corpus counts and first-page membership agree with the reference evaluator in both revisions. Build time rose 0.83% ASCII and 0.89% Unicode; estimated index size rose about 1%; snapshot size about 1.3%. Commands, dispersion, raw results, and cost detail are in [`structural_predicate_index_repair_results.md`](../generated/structural_predicate_index_repair_results.md).
- **Limitations and follow-up:** partial postings deliberately fall back for unsupported value and multivalue expressions. Q135 ASCII timings remain variable without a changed plan or source-work count. Q119's separate metadata/result-processing cost is recorded in [`q119_metadata_result_processing.md`](../issues/perf/q119_metadata_result_processing.md); no metadata redesign was made.
- **Deviation and cleanup:** direct `master` integration follows this task's explicit exception to the usual branch/worktree workflow. No task worktree requires removal. The unrelated untracked `docs/ai/tasks/objekte_borroed_segments_repair.md` was preserved untouched. Temporary task memory and task-owned baseline build artifacts are removed after delivery.
