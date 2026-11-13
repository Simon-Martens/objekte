# Plan 3 — Structural and type facts in existing indexed-field postings

Date: 2026-11-13  
Task identifier: indexed-field-structural-postings  
Integration branch: integration/indexed-field-structural-postings  
Integration worktree: /home/simon/source/objekte-integration-indexed-field-structural-postings  
Prerequisite: validated, pushed Plan 2, including Plan 1.  
Sequence: final implementation task in the three-plan series.

## Objective

Extend the existing per-field posting architecture to answer presence, null, empty-value, nonempty and primitive-type queries for every indexed field. Reuse existing value/length postings where their semantics are identical.

The user explicitly means every indexed field, not every field discovered in the repository. Do not enable automatic indexing or inference on unconfigured repositories.

The result must retain single-pass construction, incremental availability from Plan 1, predicate semantics from Plan 2 and one index-capability model/planner. Do not import the rejected packed-column overlay or its second construction pass.

## Execution and branch discipline

Read AGENTS.md, docs/ai/README.md and both predecessor plans/reports. Keep the original checkout on master. Create the named sibling worktree from Plan 2's validated, pushed integration tip; record its SHA. If the predecessors have already been merged by the user, use current master containing them.

Preserve subsequent master improvements and unrelated user work. Push immediately after every commit. Do not merge into, commit onto or push master without the user's explicit command.

A supervisor owns integration and may delegate bounded construction/storage and planner/client diagnostics work to separate task worktrees once the semantic representation is agreed. Subagents are optional. Do not run competing corpus benchmarks simultaneously.

This branch includes both predecessor plans. At handoff, identify all three integration tips and the cumulative final tip. The user may merge the final branch or review the sequence individually.

## Selection policy: fixed scope

Use the existing effective field-index selection/admission policy:

- Declared fields selected for indexing receive the new structural/type facts.
- Fields actually admitted by explicitly enabled inference receive them.
- A named/transformed value index does not silently redefine the underlying logical field's type. Record facts for the agreed logical selection domain.
- Fields not selected/admitted remain queryable through source evaluation.
- Unconfigured repositories with inference disabled receive no newly automatic field indexes.
- Preserve existing implicit structural acceleration and explicit exclusions. Do not remove master’s existing array-length or empty-string coverage merely because those schema fields are not marked for full value indexing.
- Preserving those legacy structures does not authorize building all new type facts for every unselected field.
- Do not expand inference to arbitrary nested/dynamic keys or change admission thresholds in this task.

Document how current configuration, inferred admission, legacy structural-only fields, sort-only fields and named indexes map to this rule. Resolve that mapping from the current capability/configuration model; do not create another independent selection policy.

## Starting facts and required inventory

At reviewed master 7a6b96684e1fbb849eab7cc20185b942159ed189, PostingIndex contains presence, nulls, a typed scalar-value dictionary and array-length postings. Empty strings may already have a value entry and empty arrays may be represented by length zero.

The unmerged staged branch retained conventional postings alongside its compact field-state columns. Its matching_slots implementation also decoded individual candidates with a block-map lookup. Neither structure is the target implementation.

Before editing, produce a concise internal representation inventory:

| Fact | Existing representation to inspect | Required action |
| --- | --- | --- |
| Present | presence bitmap | Reuse where the selected-occurrence meaning agrees |
| Missing | covered active domain minus presence | Do not allocate an explicit absence posting |
| Null | singleton-null bitmap and literal-null behavior | Supply aggregate null occurrence without breaking old semantics |
| Empty string | values at the empty-string key | Reuse only for the same logical selection domain |
| Empty array | array length zero | Reuse when that length posting is complete and semantically equivalent |
| Empty object | No general explicit entry | Add an exact structural fact |
| Numeric zero | Ordinary numeric value posting | Reuse; no parallel zero-state column |
| Primitive type | Schema/dictionary kind is insufficient for general record-level tests | Add or derive exact per-record occurrence facts |
| Nonempty | No general aggregate occurrence proof | Add or derive a positive nonempty occurrence fact |

Do not assume that a field’s declared/inferred type proves every record has that type; nullability, missingness and multiple occurrences still matter.

## Representation requirements

Keep structural facts within the existing field posting owner and persistence/lifecycle architecture. A small structural-key enum mapped to existing compressed bitmaps, or fixed optional bitmap members, is acceptable. Pick the simpler representation that meets measurements.

These keys belong to a field’s posting structure. They are not synthetic user field paths, separate B-trees per fact, or a new per-record packed state table.

Required properties:

1. No simultaneous permanent bitmap and packed column for the same fact.
2. Share/derive semantically identical existing empty-string and length-zero entries rather than duplicating them.
3. Preserve distinct physical facts when their meanings genuinely differ, and document the reason. For example, singleton-null and any-null occurrence are not equivalent.
4. Derive missingness from a known covered domain and presence. Uncovered is never encoded as missing.
5. Use sparse allocation for facts with no occurrences; avoid a dense field-by-record matrix or per-record heap object.
6. Coverage should reuse the owning batch/segment domain when complete. Allocate additional coverage data only for actual exceptions, using the existing capability contract.
7. Collect cardinalities during construction or obtain them from bitmap metadata. Planning and EXPLAIN must not scan source files or materialize all matching record IDs.
8. Preserve primitive type facts for empty values. Empty array/object/string are still array/object/string.
9. Logical ID and temporal behavior must follow Plan 2. Temporal logical values must not create unnecessary coverage holes for facts such as presence and nonempty that can be decided exactly.
10. Serialization and validation must reject inconsistent out-of-domain postings through existing validation mechanisms.

Do not add a second value dictionary, whole-record tree, list of selected values retained after construction, or permanent source text solely for this feature.

## Semantics that must survive optimization

Use Plan 2’s shared predicate definitions.

Each path’s positive Boolean predicate is existential across selected logical occurrences; Boolean false is the complement of that aggregate within known coverage.

Consequences:

- A path selecting both null and text can satisfy both :null and :string.
- A path selecting both an empty value and a nonempty value is nonempty.
- Therefore nonempty cannot generally be computed as presence minus the union of empty/null postings.
- A terminal array's type differs from its element types. Existing value dictionaries may flatten array elements; those entries do not automatically prove the terminal field's type or emptiness.
- A terminal empty array differs from an explicit element selection producing no values.
- Independent type/value facts do not prove that multiple conditions matched the same array element.
- Missing, null, empty string, empty array, empty object, zero and false remain distinct.

Literal comparisons keep their existing language semantics. Do not assume that literal-null equality, aggregate :null, inequality and negation all use the same complement rule. Where possible, add an explicit capability/operation distinction rather than changing the interpretation of an old bitmap.

Empty-object and empty-array facts must serve the predicates and any already supported literal comparisons. Do not broaden the FILTER literal grammar to arbitrary object/array equality as part of this task.

## Construction integration

Extend existing posting insertion at the point where the selected values are already available:

1. Select a configured field once using borrowed values and the existing small-buffer/visitor mechanism.
2. In the same visit, update scalar/length postings and structural/type occurrences.
3. For inference, record the required occurrence facts during the existing observation traversal. Do not first traverse for inference and then look up every observed path again.
4. Respect existing provisional observation budgets. Once a field is rejected or prohibited, release unnecessary provisional facts; do not retain them as an undeclared all-field state index.
5. Reuse worker-local scratch and small collections. Do not introduce data_values(...).into_vec allocations per field/record where the current path uses SmallVec or a visitor.
6. Deduplicate record membership as required by existing bitmap insertion, without cloning strings or allocating a per-record set merely to classify values.
7. Finalize and publish the facts with that record batch's other index families under Plan 1. Do not reread records for structural indexing.
8. Preserve bounded finalization, source validation, raw fallback and exact record-slot ownership.

Instrument temporary construction storage as well as retained payload. A smaller serialized result is not sufficient if per-worker builders cause excessive peak RSS.

## Unified planner and execution

Expose the new operations through the existing IndexCatalog/capability model for field-path lookup. Declared and inferred origins must not choose different semantic/planning implementations.

Every applicable access reports path/selection identity, logical value/type semantics, transformation, supported operations, coverage, exactness and cardinality using the current model.

The planner must:

- Use exact bitmap intersections/unions/complements inside the covered domain.
- Intersect with active candidate records; never complement against all possible slot numbers.
- Preserve unknown records for residual evaluation where coverage is incomplete.
- Handle AND, OR and NOT correctly with exact/candidate bounds.
- Integrate costing and predicate ordering with existing accesses.
- Prefer a genuinely cheaper equivalent existing posting where appropriate.
- Avoid enumerating an entire scalar dictionary just to estimate a type predicate.
- Restrict unsupported array positions, explicit selectors and correlation contexts conservatively. Add exact support only where the stored domain proves it.
- Leave ordinary scalar equality/ranges, array lengths and sorting fully functional.

Do not add extended_filter_bounds or another state-specific query phase after the ordinary planner. Search, count, streaming, pagination, evidence, exports and EXPLAIN must use the same access contract.

For exactly covered supported state predicates, membership-only queries must require zero source verification. Evidence or display payload fetching may legitimately read source; distinguish that work in diagnostics.

## Lifecycle, persistence and clients

Implement new facts on initial build and on explicit add/update/delete/rebuild operations. Reuse tombstones/active domains and existing local/global slot remapping. Moves change paths without rebuilding unrelated logical field facts.

Retiring a record must remove it from effective state-query results, including complemented predicates. Replacement must not leave facts from the previous incarnation.

Preserve completed snapshot round trips. Version serialized structures when required and follow existing incompatible-cache handling. Do not add partial snapshot resumption.

Plan 2 already provides syntax and UI controls. This task only adds truthful planner/EXPLAIN diagnostics and any necessary capability presentation. Do not add new indexing settings or change desktop flows.

## Required verification

1. Run the full current query correctness suite plus Plan 2's truth table against an independent source-evaluation oracle.
2. Exercise configured indexing, explicit inference, unconfigured inference-off, prohibited fields and retained legacy structural-only coverage.
3. Verify zero source verification for supported indexed state predicates, including negative forms, empty values and ordinary equality accesses that are safely reused.
4. Verify unindexed fields still evaluate correctly without acquiring unsolicited index payload.
5. Cover mixed array occurrences, nested paths, explicit selectors, missing children, null plus text, empty plus nonempty and terminal versus element type.
6. Test all logical values, including ID and temporal values, without unnecessary presence/nonempty coverage holes.
7. Validate partial publication, unknown coverage, AND/OR/NOT, active-record masking and final completion under Plan 1.
8. Check add/delete/move/update, replacement, compaction if present, save/load and cache invalidation.
9. Retain Q108/Q116/Q120 acceleration and all current scalar/range/length behavior.
10. Verify estimates/EXPLAIN perform no source reads and do not execute a whole bitmap query just to count it.

Use exhaustive small fixtures and bounded randomized mixed-value cases for semantic equivalence. Run repository-required workspace tests, formatting/lint checks and native client checks after integration. Do not write tests that only duplicate the implementation's own helper output as their oracle.

## Performance comparison and acceptance

Compare both against immediate predecessor Plan 2 and original master from Plan 1. Use identical corpus, schema, admission settings, compiler, workers, alphabet and cache conditions.

Measure:

- Catalogue availability, first searchable batch and full construction.
- Source read/parse counts and bytes; no new normal corpus pass.
- Time in posting insertion, inference observation, merge/finalization and allocation-heavy paths.
- Fresh-process peak RSS, post-build RSS, retained field-index bytes and snapshot bytes.
- A net breakdown: old facts retained, old facts removed/shared, new facts added, temporary builder memory. Do not present only the size of a new container.
- Cache-off query latency/source verification for every new predicate, ordinary empty/zero comparisons where supported, existing structural queries and mixed predicates.
- Queries over small restricted candidate sets and broad full-domain sets.
- Foreground queries during construction under the same workload used in Plan 1.
- A bounded sparse/dynamic-field fixture with the same explicit selection policy; demonstrate that unselected keys do not create field-state storage.

Use seven paired warm construction trials with alternating order and report medians/IQRs. Full query correctness remains mandatory; investigate repeated query regressions exceeding both 10% and 0.5 ms. Do not obscure individual regressions with aggregate throughput.

Default acceptance limits:

- No additional normal construction read/parse pass.
- At most 5% median full-build increase relative to Plan 2.
- At most 10% cumulative median full-build increase relative to original master.
- At most 10% cumulative increase in fresh-process peak RSS and completed live index estimate relative to original master for the same configured corpus.
- Existing query coverage is preserved. No source-read regression for previously source-free structural cases.
- Supported indexed predicate membership uses postings, not source scans or per-record column decoding.

Report bytes and timings even if inside the budget. New facts are not automatically free; do not claim the index became smaller unless the measured total did.

If a representation fails the limits, first remove redundant work/storage and compare one bounded alternative within the existing posting model. Do not start a new compression research project, add a dependency or revive the staged architecture to conceal the failure. If the requested facts still cannot meet the limits, retain the concrete measured implementation and explain the blocking tradeoff; do not mark the task complete merely by documenting the regression.

## Suggested commits and handoff

1. Record the representation/selection inventory and add semantic structural posting definitions.
2. Integrate construction and inference observation without repeated selection.
3. Integrate unified capability costing/execution and exactness restrictions.
4. Complete lifecycle/persistence support and coverage tests.
5. Resolve performance findings and commit the completed task/report.

Write docs/ai/generated/indexed_field_structural_postings_results.md, including a cumulative three-plan comparison with original master. Append the required Implementation report to the completed specification and follow docs/ai/README.md for immediate pushes, document lifecycle and safe subordinate worktree cleanup.

The final handoff must give all three branch tips, the cumulative branch, benchmark/test results, worktree paths and any limitations. Keep integration worktrees for review. State explicitly that master has not been merged.


## Implementation report

Task identifier: `indexed-field-structural-postings`. Completion date: 2026-11-13. Base commit: pushed Plan 2 `7c8ce508d54687afc5eeced93611b5662266b74a`. Integration branch: `integration/indexed-field-structural-postings`. Integration worktree: `/home/simon/source/objekte-integration-indexed-field-structural-postings`. The original checkout remains on `master`; no merge into master was performed.

The implementation stores sparse selected-occurrence structural/type bitmaps inside the existing field posting, with an exceptional empty-selection bitmap for exact `:exists`. It reuses singleton-null, empty-string, length-zero and scalar-value postings where semantics agree. Declared indexed and explicitly admitted inferred fields receive new facts; legacy structural-only, unindexed, prohibited, sort-only and named transformed indexes keep their prior scope. The existing `IndexCatalog` planner provides cost, exactness, coverage and execution for the new predicates. Construction remains single-pass and incrementally published. Merge, compaction, validation and versioned snapshots carry the facts. No packed-column overlay or second source pass was introduced.

One supervising agent implemented and integrated the task; no subtask branches or subordinate worktrees were created. Implementation commits, in order: `491d4b4970` (sparse facts, construction, inference, planner, persistence), `49963f7eec` (extended corpus cases and source verifier), `60936c6981` (verifier formatting), `677c98c673` (lifecycle test and direct-size accounting), `0d4615f34a` (size-accounting lint repair), `5b0302752d` (bounded mixed-value source oracle), `29197eef5b` (benchmark formatting), and `28c0a42360` (measurements and report). Each commit was pushed immediately. The completed specification is committed and pushed separately after these commits.

Validation: `cargo test --workspace --all-targets --locked` passed 920 tests, 30 ignored, zero failed. `cargo clippy --workspace --all-targets --locked` exited zero with 211 underlying warnings, matching the Plan 2 baseline. `git diff --check` passed. Workspace `cargo fmt --all --check` and `python3 scripts/format-rust-spacing.py --check` still report inherited style drift; rustfmt flagged the same 95 files as Plan 2 and no candidate-only file. The native desktop built and opened the five-file fixture in task-owned Wayland; cleanup stopped the app and pointer server. The prior Plan 2 native predicate-builder exercise remains applicable because Plan 3 changed no UI code. CLI and TUI tests passed in the workspace suite.

Benchmark: seven alternating warm Plan 2/Plan 3 construction pairs on the original Musenalm corpus, ASCII, 32 workers and identical schemas had 891.750 to 920.393 ms median build (+3.21%), 568.930 to 576.074 MB fresh peak RSS (+1.26%), and 69.966 to 71.239 MB estimated live index (+1.82%). All 148,486 source reads/parses and 193,297,230 source bytes remained unchanged. Versus original master, published median build is 5.44% lower, peak RSS 7.31% higher and live index 2.26% higher, within the task's cumulative limits. All 880 rows of the 220-case/four-profile/five-repetition final query campaign passed. The first 206 cases preserved all predecessor counts and digests with no source-open regression; the 14 new selected predicate cases were source-free. Q195–Q220 also matched independent source evaluation. Foreground Q108/Q116/Q120 construction completed 891–942 queries per run with zero query source opens. Complete commands, environment, per-case timings, IQRs, attribution, limitations and raw evidence are in [the generated report](../generated/indexed_field_structural_postings_results.md) and [benchmark archive](../../../bench/results/indexed-field-structural-postings/README.md).

There was no approved deviation or unresolved in-scope issue. Explicit array selectors and same-element correlation continue to use source evaluation when independent field postings cannot prove them; arbitrary object/array literal equality is outside the existing FILTER grammar. Direct-build phase attribution is based on one noisy diagnostic pair; the acceptance result uses seven paired incremental builds. The final native smoke opened the fixture but did not repeat Plan 2's full builder interaction. The integration branch and integration worktree are retained for review. No subordinate worktree cleanup is needed.
