# Objekte: unified query planner and index capability model

## 1. Goal, scope, and execution

Implement one query planner using one index capability model. Planning must depend on what an index represents and can prove, never on whether a schema hint, automatic inference, extension, or snapshot load produced it.

Continue the requested direct-on-master workflow for this query-optimization work. Record the actual starting commit; preserve unrelated changes; make coherent commits and push each one. Do not reset or force-push. No integration worktree is required for this task.

Inspected master: `90b52907c7f9c9364305e38e1b689c9766cc84d0`, which already adds explicit `value_complete`, `length_complete`, and `empty_string_complete` flags. Recheck current code before implementation and incorporate subsequent changes.

This is a consolidation of planning and index access, not a redesign of index construction or storage algorithms. Automatic indexing remains opt-in. Explicit indexing hints remain supported. Preserve current build policies, structural-index improvements, query syntax, matching semantics, output modes, and extension contracts.

Definition of success: equivalent index contents, capabilities, coverage, and statistics yield equivalent planning decisions regardless of origin. Every query execution surface uses the same compatibility, coverage, proof, and costing rules.

## 2. Keep parsing, binding, and planning distinct

Do not put index selection into syntax analysis.

| Stage | Responsibility |
| --- | --- |
| Parsing | Produce expressions, operators, literals/parameters, paths/selectors, modifiers, and Boolean structure. |
| Binding | Resolve collection/field identity, declared or dynamic types, literal conversion, transformations, quantifiers, text representation, and operation semantics. |
| Planning | Find available access paths; assess applicability, coverage, exactness, cardinality and cost; select indexes and residual evaluation. |
| Execution | Perform selected accesses and Boolean composition, verify unresolved records, and produce the existing output/evidence. |

Use the existing bound expression types where practical. The meaning of a query must not change when an index is added, omitted, partially populated, or loaded from disk. An observed numeric posting does not redefine an entire mixed-type field as numeric. Preserve existing parameter, temporal, relation, and custom-binding validation.

Coverage and cardinality are not properties the parser can discover. Exactness is usually a property of an index applied to a particular bound expression over a particular domain.

## 3. One descriptor model and one access catalogue

Introduce a small internal catalogue of borrowed index/access references for each published collection and segment. Prefer adapting existing structures over duplicating them. Professional suggested module names are `index_catalog.rs` and `query_planner.rs`; retain existing names when that produces a smaller coherent change.

Each registered access must expose the following information, directly or through cheap methods:

| Information | Required meaning |
| --- | --- |
| Identity and scope | Stable identity within its published revision, collection, segment/record-slot namespace, and eligible domain. |
| Target | Canonical field path or collection text binding; metadata paths remain distinguishable from data paths. |
| Types and representation | Source field shape, indexed key type, and transformation/binding identity. A length index has numeric keys although its source field is an array. |
| Supported operations | Parameter-sensitive capabilities: scalar equality/range, presence, explicit null, array length, exact empty string, text prefilter, ordering, or existing relation access. |
| Coverage | Which records and value selections are represented, separately for each capability where necessary. Include unresolved domains and relevant completeness/version information. |
| Proof limitations | Scalar versus multivalued selection, position/correlation information, quantifier restrictions, and exact versus candidate-only operations. |
| Statistics | Applicable counts, distinct keys, bounds and existing cost summaries, with exact counts distinguished from estimates. |

Important requirements:

- Do not replace the new per-capability completeness flags with one global `complete` Boolean. A field may have complete lengths and incomplete scalar values.
- `EmptyStringEquality` must not advertise unrestricted string equality. A partial value dictionary must never prove absence outside its covered domain.
- A count of covered records is insufficient when the identity of covered records matters. Reuse existing coverage bitmaps/domain references.
- Different representations of one field can coexist. Lookup returns all applicable alternatives, not the first entry encountered.
- Provenance may appear in diagnostics and build reports; it must not determine eligibility, exactness, or selection priority.
- Publish a catalogue once per index revision, or provide equivalent constant-time borrowed views. Do not rebuild it by walking all fields on each query.
- Reuse postings, bitmaps and statistics. Do not clone dictionaries, duplicate membership storage, or allocate a dynamic object for each record/predicate evaluation.

Provide two logical lookup forms:

1. Field lookup for FILTER and field-scoped FTS, including selectors and binding requirements.
2. Text-binding lookup for unscoped FTS, respecting the configured searchable corpus and fallback representation.

A collection-wide text index may be a safe candidate source for a field predicate, but cannot prove the field predicate merely because its text occurs somewhere in the record. A set of field indexes can cover unscoped FTS only when its coverage of that searchable corpus is established.

Existing metadata scans, scanners, sorting and relation strategies remain physical access alternatives. The catalogue must distinguish an actual index from a source or resident-metadata scan; storing metadata is not automatically an ordered index.

## 4. One per-expression applicability and estimate contract

For a bound expression and active input domain, an access provider must return either a specific rejection reason or a prepared access proposal containing:

- The selected index reference and lookup operation.
- The expression-specific coverage and unresolved domain.
- Whether the access proves results or only restricts candidates on that coverage.
- Estimated output cardinality, lookup work and residual work.
- Any residual expression still required.

This assessment must be shared by planning, execution preparation, and EXPLAIN. Do not independently reimplement compatibility in estimation and candidate execution.

Use existing enums and borrowed references for built-in providers. Do not build a generic optimizer framework, a new query language, or a mandatory public trait hierarchy. Specialized dictionary, bitmap, bigram, relation and sorting implementations can remain separate behind the common contract.

Cardinality is predicate-dependent: `x = 3`, `x >= 3`, and a restricted-input lookup have different counts. Persist/reuse sufficient statistics; calculate exact counts cheaply when available, otherwise provide an estimate. Do not materialize result bitmaps, read source records, or walk a large dictionary merely to select an access path. Preserve bounded existing estimate methods and account for approximation.

Choose by estimated total work, including unresolved verification, not index provenance. Apply current restrictions and small input domains before broad copies/unions wherever existing primitives permit.

## 5. Share proof and Boolean-composition rules

Reuse the existing `SegmentCandidateBounds` principle throughout planning and execution:

For the eligible active input domain D, maintain `must ⊆ actual_matches ⊆ may ⊆ D`.

- `must`: records proved to satisfy the expression.
- `may`: records not yet ruled out.
- Residual verification is required for `may - must`.
- A fully exact answer has `must == may`.

For an access covering C within D:

- Exact matches M on C produce `must = M` and `may = M ∪ (D - C)`.
- Candidate matches P on C produce `must = ∅` and `may = P ∪ (D - C)`, unless additional positive proof is available.
- Unsupported access produces `must = ∅`, `may = D`.

Existing representation-specific proofs may be stronger; preserve them without weakening soundness. Coverage must include the relevant selection semantics, not merely the presence of a field.

Compose bounds consistently:

| Expression | Must | May |
| --- | --- | --- |
| A AND B | must(A) ∩ must(B) | may(A) ∩ may(B) |
| A OR B | must(A) ∪ must(B) | may(A) ∪ may(B) |
| NOT A | D - may(A) | D - must(A) |

Never complement an approximate candidate set and call it an exact answer. Apply the correct active/logical/fallback domains; exclude deleted slots and preserve existing invalid-record policy. Preserve established query-error and cancellation behavior; proof shortcuts must not silently change the defined evaluation semantics.

Array-element membership cannot generally prove positions, multiplicity, same-element correlation, or universal comparisons. Preserve empty-selection behavior and the current length exactness restrictions. Missing, explicit null, empty arrays, empty objects, empty strings, zero and false remain distinct. This task changes no null/existence semantics.

## 6. FILTER, FTS, and all execution entry points

FILTER and FTS must participate in the same planning context and cost/proof model. They can keep their existing bound types and specialized matching operators.

For text access:

- Preserve raw source text, decoded scalar text, logical field text, custom normalization, case folding and malformed fallback as distinct representations where their semantics differ.
- A transformation mismatch is usable only with a documented conservative implication; it must never eliminate a true match.
- Bigrams ordinarily yield candidates, even with complete coverage. Their counts are not exact query match counts.
- Preserve field scope, whole-value segment boundaries, Unicode behavior, case-sensitive verification, wildcard rules, and regex necessary-literal restrictions.
- No usable grams, short/empty literals, unsupported custom operations or incomplete corpus coverage must produce a safe residual route.
- Correctly combine multiple selected fields and covered/uncovered text domains using existing FTS semantics.

Use the common planning core for ordinary search, count, pagination, sorted search, streaming, relation target predicates, and both estimate-only and analyzing EXPLAIN. Share it between full collection generations and incremental lifecycle segments.

Keep specialized relation traversal and sorting algorithms, but obtain their local predicate/index decisions through the same catalogue and applicability rules. Do not rewrite join algorithms or computed sorting in this task.

## 7. Concrete consolidation targets

At the inspected revision, examine and consolidate:

- `executor.rs::AccessPathKind::Posting` versus `InferredPosting`.
- `InferenceState::compatible_index`, `estimate_candidates`, and `candidates` in `inference.rs`.
- Manual/custom/inferred dispatch in `executor.rs` estimation, access execution, and segment filtering.
- `segment_fts_candidates`, `fts_candidates`, `estimate_fts_candidates`, and their regex/field-index helpers.
- FILTER/FTS ordering and access selection repeated across `run.rs` query entry points.
- Existing `PostingIndex` methods, `AccessPathEstimate`, `PlanContext`, `Plan`, `Proof`, and `SegmentCandidateBounds`.

Move reusable capability, coverage and costing rules into the common layer. Inference-specific observation, admission and reporting stay in the builder/inference modules. Query execution must not consult `InferenceMode` to determine whether an already published index is usable.

The final implementation must remove live origin-specific planning policy, not merely place a wrapper around two planners. If an exported diagnostic label needs compatibility, preserve it as presentation/provenance metadata while execution uses the shared access kind. Temporary migration wrappers are acceptable only if removed or reduced to policy-free forwarding by completion.

## 8. Construction, hints, persistence and caches

Keep automatic indexing disabled by default and keep explicit hints authoritative. Do not add new hint syntax or broaden index admission as part of this consolidation.

All producers publish the same capability meaning. Adapt declared indexes, inferred indexes, structural-only indexes, custom indexes, loaded snapshots and incremental segments. The new structural indexes already on master must retain their coverage and zero-source-work routes.

Store authoritative capability/coverage information with the index; derive the runtime catalogue from it. Do not serialize another redundant registry when existing persisted fields suffice. Any necessary format migration follows existing version/compatibility policy. Missing old metadata means unknown capability, never invented completeness.

Build/add/update/rebuild publish data, capabilities and statistics consistently. Delete restricts the active domain; movement preserves content facts while existing path-dependent state updates appropriately. Compaction and loading retain equivalent capabilities.

Keep prepared bindings and physical accesses scoped to the appropriate published revision and binding/configuration identity. An index handle must not outlive its owning snapshot. Existing verified-result and ordering-cache invalidation remain authoritative.

No file freshness checks, content hashes, source revalidation, or automatic reconciliation on ordinary search. No query-time type-inference pass.

## 9. Verification and measurements

Use the reference evaluator and existing regression suites. Add meaningful tests for the new boundary:

1. Equivalent declared and automatically produced indexes with identical capabilities, contents, coverage and statistics produce equivalent access choices and results, ignoring identity/provenance labels.
2. Partial hinted/custom indexes receive the same safety treatment as partial inferred indexes. Complete inferred indexes receive the same optimizations as complete declared indexes.
3. Equality, ranges, presence/null, lengths, empty strings and transformation compatibility behave correctly for full, partial, absent and complete-but-empty coverage.
4. AND/OR/NOT over exact and approximate providers preserve bounds and results, including uncovered records, missing/null values, empty selections, arrays and malformed fallback.
5. FTS literals, regex, field scope, raw/decoded/logical representations, Unicode/case folding, short/empty needles and custom bindings retain matching/evidence semantics.
6. Ordinary, count, streaming, pagination, sorted, relation and EXPLAIN routes use the shared planning rules. Save/load, add/update/delete/move and compaction retain parity.
7. Catalogue lookup and estimate-only EXPLAIN do no source I/O and do not materialize candidate bitmaps. Repeated queries reuse immutable catalogue state.

Use construction fixtures that equalize capabilities for provenance-parity tests; do not incorrectly demand the same plan for a complete manual index and a partial automatic one.

Capture baseline results from actual starting master on the same machine/corpus. Run the complete 194-case Musenalm suite with ASCII/Unicode and result cache disabled/warmed using the established protocol. Check membership, counts, evidence and ordering, plus candidate/source-work counters. Preserve Q115–118/Q120 zero-source-work behavior wherever the starting index capabilities provide it.

Measure planning separately: prepared-cache on/off, narrow/broad filters, mixed FILTER/FTS, partial coverage, many available field indexes, and incremental segments. Record planning time, dictionary visits, bitmap constructions and allocation counts where instrumentation exists. Use at least seven repetitions for focused timings and three for the complete suite; keep OS/cache/worker conditions matched.

The refactor must not add a field scan, heap allocation per candidate, repeated catalogue construction, or eager bitmap unions during estimation. Recheck repeatable end-to-end regressions above both 5% and 0.5 ms, and repeatable planning/build/memory growth above 5%. Correct systematic regressions before completion; record uncertainty and any unresolved result explicitly.

Run formatting and relevant core, extension, lifecycle, storage, CLI and desktop integration checks required by touched behavior. Do not change benchmarks, result limits, query semantics or cache settings to conceal regressions.

## 10. Delivery sequence and completion criteria

Use sequential, reviewable commits on master:

1. Add the common descriptor/catalogue and adapters, preserving current behavior and adding provenance-parity tests.
2. Consolidate posting/structural applicability, estimation and access; remove inferred-versus-declared planning policy.
3. Integrate FTS and Boolean proofs in the shared planner; migrate full-generation and segment query entry points.
4. Finish snapshot/cache/extension compatibility, remove obsolete dispatch/wrappers, and complete verification and benchmarks.

Every phase must keep the repository functional; do not leave two independent planners as the final architecture. Adjust commit boundaries if dependencies require it, without widening scope.

Deliver `docs/ai/generated/unified_query_planner_results.md` containing:

- Exact baseline and final commits.
- The public/internal capability contract and a construction-to-query flow description.
- A table of migrated entry points and removed duplicate decisions/functions.
- Before/after performance and work counters, including structural and partial-coverage cases.
- Snapshot, extension and diagnostic compatibility decisions.
- Any remaining limitation expressed as a capability/coverage limitation, not index origin.

Completion means one common planning policy for FILTER and FTS, safe per-expression proofs, preserved explicit indexing controls, and removal of the separate inferred-index planning route. A smaller policy surface is required; a new abstraction that leaves all duplicated decisions in place is not sufficient.


## Implementation report

- **Task identifier and completion:** `unified-query-planner`, 11 November 2026. Base `6ef66ec8f4bf15bfcaaf6cc4c7493bcf9bb592fc`; direct-on-`master` execution explicitly authorized by the user and this specification. No integration branch or integration worktree was required.
- **Implementation commits:** `05c00a3cbfa2f218ed063681eccbd9769386aef5` unified borrowed posting/text access, shared estimation and execution decisions, FTS generation/segment access, provenance parity test, and verified-cache sort guard. `00f501902c8aee2a66da5cb829b56ab89dbf0ac8` exposed the prepared-cache size in the Musenalm harness for matched measurements. Both were pushed to `origin/master` immediately after committing. There were no subordinate branches or agents.
- **Validation:** `cargo test --workspace --locked` passed (503 core and 158 desktop tests, plus other workspace crates); `cargo clippy --workspace --all-targets --locked` passed with existing warnings; `cargo check --locked --example bench_musenalm` passed; `git diff --check` passed. New source files were checked with rustfmt. Repository-wide rustfmt reports unrelated preexisting formatting differences from module recursion, so it was not applied across the workspace.
- **Benchmark:** Exact baseline and implementation binaries ran the unchanged extended 194-query Musenalm suite, three repetitions, ASCII/Unicode, verified-result cache off and 64 MiB warm, four query workers. All 776 query/profile rows passed and all nine result/source-work fields matched. Q115–118/Q120 remained source-free. Build times and retained-size estimates were stable; 7- and 21-repetition alternating rechecks and prepared-cache on/off planning measurements are documented with commands, corpus, results and raw evidence in [the results report](../generated/unified_query_planner_results.md) and `bench/results/unified-query-planner/overnight-2026-11-11/`.
- **Regression and limitations:** A preliminary Q189 warm-cache source-work regression came from letting a declared sort posting bypass verified FILTER membership reuse; the implementation guard and a focused test fixed it before the acceptance suite. Full-suite timing flags were rechecked in paired and isolated runs; remaining outliers varied with the shared machine and did not reproduce consistently, with no work-counter difference. Musenalm has no controlled partial-coverage or incremental-segment planning timing case, so those paths were covered by core/lifecycle tests; the harness exposes no per-query allocation count. A partial dictionary cannot prove absence outside covered selections, and raw bigrams still require field verification.
- **Worktree and cleanup:** Implementation ran in `/home/simon/source/objekte` as required by the specification. A detached baseline benchmark worktree at `/home/simon/source/objekte-bench-unified-base` was created only for the identical harness comparison and will be removed after benchmark evidence is committed. Temporary task memory will be removed after push. The obsolete untracked task source is removed only after this committed document is verified to contain its exact original bytes.
