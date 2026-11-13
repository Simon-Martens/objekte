# Plan 2 — Boolean field predicates across the library and clients

Date: 2026-11-13  
Task identifier: boolean-field-predicates  
Integration branch: integration/boolean-field-predicates  
Integration worktree: /home/simon/source/objekte-integration-boolean-field-predicates  
Prerequisite: validated, pushed result of Plan 1, incremental-index-availability.  
Next task: Plan 3, indexed-field-structural-postings.

## Objective and boundary

Implement the agreed field predicates and Boolean binding rules consistently in the parser, evaluator, planner, CLI/TUI and desktop. They must work whether a field has an index or requires normal source evaluation.

This task defines semantics and client support. It does not add compact state columns, automatic field indexing, a second construction pass or a separate planner. Plan 3 will accelerate these predicates through the existing posting architecture.

The user's scope is every indexed field for new structural indexing, not every discovered field. Query syntax itself remains available for unindexed fields.

## Execution and dependency contract

Read AGENTS.md and docs/ai/README.md. Keep the original checkout on master; perform all changes in the named sibling worktree. Preserve unrelated work and user task-document edits. Push immediately after every commit. Never commit to or merge/push master without the user's explicit merge command.

Create this integration branch from Plan 1's final validated, pushed tip and record the SHA. If the user has already merged Plan 1, use the current master containing that result and record the ancestry. Do not merge the rejected staged branch.

When all three plans are assigned, continue from a passing Plan 1 without another approval request. A supervisor may delegate source semantics/tests and client support to separate task worktrees after defining the shared contract. Subagents are optional; only the supervisor integrates.

After this plan passes, Plan 3 starts from this branch's validated tip. Keep both integration branches. The final Plan 3 branch is the cumulative result; no master merge is implied.

## Reusable work and compatibility review

Reference branch: integration/staged-index-construction at bee59df5ed952e28b41f6b422e0c6bcc89afce1f.

Useful reference commits:

| Commit | Relevant work |
| --- | --- |
| 0ada21b88998ca04b642f6f2fce982b1cecf9ee0 | Predicate syntax, binding, source semantics and truth-table tests |
| 5e87733f7f3065670501ebd3f75777eb8cc6e78a | Prevent old singleton-null postings from falsely proving multivalued null predicates; exhaustive enum handling |
| 88e0c572bf42a17829b8d0c7f039ca2a18ddff31 | Desktop modifier and Boolean-value completion |
| 6e4d11affc4365a442c6c11abfd7c8e72205638b | Desktop filter-builder representation and round-trip tests |
| f57b2b222a3f188b7f423eb351b5551baa2612e7 | TUI modifier labels |

Port the relevant final code and tests into the new base. These commits are review references, not an instruction to cherry-pick them wholesale. The initial predicate commit includes a compact column that is explicitly outside this task. Its later null correction matters for correctness.

Inspect query.rs, reference.rs, posting.rs, index_catalog.rs, executor.rs, query validation, prepared-query/cache identity, documentation and all modifier formatting sites. Prefer a shared semantic definition over independent copies of truth rules.

## Syntax and binding contract

Support terminal Boolean predicates:

~~~text
annotation:? = true
annotation:? = false
annotation:string = true
annotation:number = false
annotation:array != false
annotation:null = true
annotation:object = true
annotation:exists = false
~~~

The permitted comparisons are equality and inequality with a Boolean operand. Preserve existing parameter support when an operand is bound as Boolean. Reject incompatible operands and operators at the normal binding/validation boundary, not during record evaluation.

Preserve existing registered conversions named string, number, array or object for their ordinary non-Boolean comparison forms. A terminal Boolean type comparison has the agreed predicate meaning. Explicitly test name collisions and report any unavoidable compatibility difference; do not silently reinterpret unrelated custom operations.

Reject incompatible modifier combinations using existing error conventions. Do not add arbitrary conversions, Boolean ordering, field-to-field state comparisons or a new shorthand language. Keep quoted/escaped field paths working, including colons and question marks inside quoted names.

Do not add a () state or a new :bool predicate in this task.

## Authoritative semantics

Predicates inspect logical values after the adapter's existing logical/schema mapping, not normalized full-text segments.

| Selected value at x | x:exists | x:null | x:? | Positive primitive type predicate |
| --- | --- | --- | --- | --- |
| Missing | false | false | false | None |
| null | true | true | false | Null |
| Empty string | true | false | false | String |
| Whitespace-only string | true | false | true | String |
| Nonempty string | true | false | true | String |
| Numeric zero, including equivalent signed/decimal zero | true | false | true | Number |
| Other number | true | false | true | Number |
| false or true | true | false | true | None of the requested string/number/array/null/object predicates |
| Empty array | true | false | false | Array |
| Nonempty array, including [null] | true | false | true | Array |
| Empty object | true | false | false | Object |
| Nonempty object, including an object containing only null values | true | false | true | Object |

Logical ID values count as strings, matching the reviewed predicate contract. Logical Date, Datetime and Unixtime values are present and nonempty; they are not automatically treated as primitive string or number merely because their serialized form was textual or numeric. Preserve existing explicit conversion behavior.

For a path selecting multiple values:

1. Compute each predicate by existential aggregation over selected occurrences.
2. Apply the Boolean comparison to that aggregate.
3. Comparing the aggregate to false negates the aggregate; it does not mean “some occurrence is false”.

Thus a path selecting both null and a nonempty string satisfies both :null = true and :string = true, and satisfies :? = true. “Something overrides nothing” applies to nonemptiness only; it must not erase the independent null/type facts.

An empty selection has no positive occurrence. Preserve the distinction between a selected empty array and selecting its nonexistent elements.

For example:

~~~text
{"items": [null, "", "hello", 0, {}]}

items:array = true          # true: the field itself is an array
items:string = true        # false: its elements do not change its own type
items[*]:string = true      # true: explicit element selection
items[*]:null = true        # true
items[*]:? = true           # true
items[*]:? = false          # false
~~~

Do not invent same-element correlation from independent existential predicates. Preserve the query language's existing explicit selectors, quantifiers and correlation rules; reject combinations outside its supported grammar rather than manufacturing a new meaning.

## Source evaluation and existing indexes

The source evaluator is authoritative. Reuse borrowed selected values and short-circuit positive predicates once their truth is established. Avoid cloning strings or logical values to classify type or emptiness.

Existing presence/null postings are not automatically compatible with the new aggregate semantics. At the reviewed master, nulls records only a single selected null. A selection containing null plus another value can therefore be absent from that bitmap.

For this task:

- Use an existing posting only when its coverage and semantic contract prove the requested predicate.
- Conservatively fall back for incompatible mixed selections until Plan 3 supplies the necessary fact.
- Implement safety restrictions in the shared capability assessment so estimate, execution, EXPLAIN and all search routes agree.
- Do not silently alter literal-null comparisons or other legacy operations by changing the meaning of a shared null bitmap.
- Do not append a second predicate evaluator after the ordinary query planner.

Predicates remain usable in Plan 1's incomplete views. Missing posting coverage is unknown; absence can only be proved inside known coverage. The record/result-domain completeness contract from Plan 1 still applies.

Maintain validation state at construction and explicit mutation boundaries. Source fallback must not introduce a new full-document validation pass or per-query freshness scan.

## Client work

CLI:

- Accept the new predicates wherever structured FILTER expressions are accepted.
- Preserve error formatting, parameter handling, output and exit status conventions.
- Update help/examples without introducing indexing switches.

TUI:

- Render modifier names correctly and retain query editing/status behavior.

Desktop:

- Complete modifier names and Boolean operands, including escaped paths.
- Extend the existing filter-builder model to round-trip the Boolean predicates with the authoritative query text.
- Restrict the operator/value controls appropriately.
- Use existing standard selection/menu controls for choosing the predicate; do not reproduce a cycling-through-seven-options button merely because it exists on the old branch.
- Keep unsupported complex expressions intact in the text editor instead of rewriting or dropping them.
- Preserve the early-availability behavior from Plan 1.

Update the FILTER grammar and public examples. Clearly distinguish missing from null, and nonempty from truthy. In particular, false and zero are nonempty.

## Verification

Port and extend meaningful semantic tests:

- The complete truth table above through source evaluation.
- Mixed null/string/number/object occurrences, implicit array traversal and explicit selectors.
- Empty selections versus selected empty containers.
- Boolean equality/inequality and composition with AND, OR and NOT.
- Raw fallback/malformed records according to existing logical-field semantics.
- ID and temporal logical values, exact numeric zero and whitespace strings.
- Quoted path names, Unicode names and modifier-looking names.
- Custom conversion-name compatibility, parameters and invalid modifier/operator combinations.
- Old singleton-null postings cannot produce false exact answers for aggregate null predicates.
- Result equivalence with indexes enabled, disabled, absent or incomplete.
- CLI parsing/errors and desktop filter-builder/completion round trips.
- Prepared queries, verified-result caches and persisted index compatibility after any changed predicate semantics.

If persisted derived data or cached semantics need a version change, follow the existing compatibility mechanism. Do not conceal an incompatible snapshot by trusting stale proof semantics.

Run the required workspace tests, formatting/lints and native desktop checks. Existing valid queries must retain their behavior except for explicitly documented aggregate-state corrections supported by the new contract.

## Performance and acceptance

Use Plan 1's final revision as the immediate baseline and retain the original master baseline for cumulative comparison.

This task must add no construction pass and no permanent state-index payload. Compare full construction time and memory to ensure parser/API changes did not accidentally activate indexing. Investigate a repeatable construction increase above 5% or any material permanent-memory increase.

Run the current full query correctness suite and add the predicate cases. Benchmark:

- Existing structural queries, especially presence, null, empty strings and array length.
- New predicates on indexed and unindexed fields.
- Mixed multivalued null queries whose previous postings cannot prove the new semantics.
- Queries during construction and after completion.

Record source verification counts. New predicates using source fallback are an expected interim state before Plan 3; report those costs explicitly. Do not label them index-accelerated. Existing unrelated queries must not acquire source scans or regress materially. Investigate repeated regressions exceeding both 10% and 0.5 ms.

No claim that the historical staged branch's benchmark results apply to this implementation. Use seven paired warm construction trials when needed to establish a material build change; do not run unrelated benchmarks repeatedly.

## Suggested commits and completion

1. Add shared predicate semantics, parser/binder support and truth-table tests.
2. Make existing posting proofs safe for the new aggregate semantics and update compatibility handling.
3. Integrate CLI/TUI/desktop support and grammar documentation.
4. Validate correctness/performance and commit the implementation report.

Write docs/ai/generated/boolean_field_predicates_results.md. Follow docs/ai/README.md for task-document handling, immediate pushes and cleanup. The completed task document must include an appended Implementation report with exact base/head revisions, code changes, tests, measured performance, semantic differences and any deferred acceleration assigned to Plan 3.

Retain the integration branch/worktree and provide its validated SHA as Plan 3's base. Do not merge master. Do not declare completion with in-scope correctness failures or unmeasured required performance.


## Implementation report

- **Task:** `boolean-field-predicates` (Plan 2). Completed 2026-11-13. Original untracked specification SHA-256: `ebaf0d675930b41af94a4d40ece8a96dd154d34c0e0884425629e918c42e55c1`; its bytes are preserved above.
- **Base and delivery:** Base is the validated Plan 1 tip `c54afc3eb1cdc496734bfb30bcc9a3ea38988b78`. Integration branch `integration/boolean-field-predicates` and worktree `/home/simon/source/objekte-integration-boolean-field-predicates`. Validated implementation and evidence head is `42820c5a338cfa15f317008d835b4536c8ddf5de`; this completion record is the subsequent branch commit. Master was not merged or changed. The pushed completion commit is Plan 3's exact base.
- **Code:** `920fc4fefd` introduced shared existential source semantics, `:?`/type/exists/null Boolean modifiers and safe multivalued-null fallback. `a7a31feb5d` integrated CLI/TUI/desktop completion, standard filter-builder predicate menu and documentation. `5f2e1f1cc5` added Q195–Q206 and matched benchmark helpers. `d666524f7f667305d3d1c19593835be9d478f2e5` restored exact null postings on fields proven scalar by inference despite conservative schema-free binding. `42820c5a338cfa15f317008d835b4536c8ddf5de` archived the raw measurements, client screenshots and acceptance report. There is no new permanent predicate payload or construction pass.
- **Semantics:** Positive predicates are existential across selected logical values and Boolean false complements that aggregate in the known domain. Missing and null differ; zero and false are nonempty; empty containers retain their own type. A null-plus-text multiselection can satisfy both `:null` and `:string`. Existing singleton-null postings cannot prove aggregate null on multivalued fields, so shared assessment selects source fallback. Exact scalar structural postings remain available. Literal-null comparisons retain their existing behavior.
- **Validation:** `cargo test --workspace --all-targets --locked`: 914 passed, 30 ignored, zero failed. `cargo clippy --workspace --all-targets --locked`: exit 0, 215 nonfatal warning locations, the same count/file distribution as the Plan 1 predecessor on the current toolchain. `cargo fmt --all --check` and `python3 scripts/format-rust-spacing.py --check` still fail on predecessor style drift; rustfmt flags 99 predecessor files and 95 candidate files, zero candidate-only. `git diff --check` passed. A native headless Wayland desktop five-value fixture verified builder serialization and true/false results; CLI process, TUI renderer and desktop round-trip tests passed.
- **Construction:** Three alternating warm fresh-process pairs against Plan 1 on the identical 148,486-file Musenalm corpus, ASCII, 32 workers and incremental build path: median full build 897.922→921.155 ms (+2.59%), actual peak RSS 573.559→576.315 MB (+0.48%), completed live index 69.965692 MB on both sides. Each trial made exactly 148,486 source reads/parses. Against the original master seven-pair medians, Plan 2's separate campaign is approximately 5.36% faster build, 7.35% higher peak RSS and 0.44% higher live index; this cross-campaign view is indicative. No material build change crossed the Plan 2 five-percent investigation threshold.
- **Queries:** The predecessor-matched 194-case × four-profile × five-repetition run passed all 776 status/count/digest rows with no source-open increase. The repaired final revision ran all 206 cases × four profiles × five repetitions: 824 statuses passed; the first 194 digests, counts and source opens still match Plan 1. New predicates on indexed fields mostly use the expected interim source fallback. Three alternating full-suite cache-off pairs and five isolated 15-repetition pairs for each surviving Q003/Q004 timing outlier found no repeatable regression exceeding the acceptance threshold. Raw results, environment, commands, limitations and per-query counters are in [the acceptance report](../generated/boolean_field_predicates_results.md).
- **Remaining limits and handoff:** The source-fallback cost on broad indexed predicate queries is measured and explicitly assigned to authorized Plan 3. No in-scope correctness failure or repeated unrelated query regression remains. Keep this integration branch/worktree for review; use its pushed completion SHA as Plan 3's base. The original untracked task source may be removed only after verifying this pushed copy preserves its original bytes and no newer edits exist.
