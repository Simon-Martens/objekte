# Query extensions: base, contracts, and task ledger

## Pinned base and evidence

- Base: `e9d22a405c151dca1a3a8c5d87a39cb9de708db2` on `development/local-file-search-core`, selected from the current checkout on 2026-11-06 at the user's direction. The main checkout has pre-existing deletions of `implement.md` and `docs/development/corrective-results.md`, plus the untracked specification. This integration worktree starts clean from the base; the specification is copied here without changing the main checkout.
- `fix/parallel-index-construction` and development both point at this base. Its work is already in the ancestry of the current branch (`d15b689d1a` is an ancestor); it is not a separate implementation dependency. Later scheduling experiments and their retained/reverted outcomes are also in the selected base.
- Foundation verification on this exact base was completed immediately before this assignment: `cargo test --workspace --all-targets`, benchmark tooling unit tests, the doc test, ASCII/Unicode 150-case optimized/reference acceptance, 300 jq oracle ID-set comparisons, and the 3,000-query full correctness workload all passed. The recorded run is `/tmp/objekte-correctness-dhDxzx4q/`. Its quick/full timing samples were not latency-gated and are not a performance baseline.
- The applicable recorded performance evidence is in `docs/development/foundation-performance-repair-results.md`, `docs/development/casefold-results.md`, `docs/development/parallel-index-construction-results.md`, and the subsequent build experiment reports. Their code parents precede this base. Recheck corpus, schema, compiler, machine, workers and cache policy before using any as a quantitative baseline. No new timing baseline is claimed here.

## Planner prerequisite

`TASK-01` is already implemented in the pinned base by `dbf4777adf` (`fix(planner): preserve indexed filtering over logical record domains`). `plan_expr_restricted` partitions `input` into `logical_records` and raw fallback slots, plans healthy records through `plan_logical_expr`, and combines fallback three-valued truth through `fallback_truth`. `one_fallback_does_not_disable_selective_logical_postings` checks indexed selection, Boolean operators, metadata branches, unavailable values, optimized/reference parity, and build/restore in both alphabets. The selective equality has one candidate and zero source opens/parses in a collection with a malformed sibling. This existing, tested behavior is the first-wave gate; no duplicate implementation or empty commit is needed.

## Shared contracts for first-wave workers

- **Bound operands and paths:** the query worker owns syntax, `Term` operand migration, source spans, parameter slots and path steps. Literal comparison semantics remain in the bound evaluator. `FieldPath`/schema representation may change only with an explicit compatibility path for existing callers. The relation-index worker does not add an independent query operand or parser. Array occurrence identity must survive binding; a value posting cannot prove a positional match.
- **Inline order:** parse one outermost `SORT BY` into the existing `Vec<SortField>` before binding or execution. Reject a nonempty independent sort together with inline order. Existing programmatic sort and fixed workload entries retain their meaning. Registration and compatibility are checked by `validate_sort`; no on-demand sort column construction.
- **ID:** the relation-index worker owns `LogicalType::Id`, `Value::Id(String)` and a distinct `ScalarKey::Id(String)`. IDs preserve spelling and compare exactly in their declared target collection namespace. Source text extraction must preserve the reader's text coverage when conversion yields typed IDs. The query worker may add a contextual binding hook but must not define a second ID representation.
- **FTS mode:** the regex worker owns a typed `Literal`/`Regex` mode for FTS requests, pattern options, compilation and candidate extraction. Literal remains the default; case sensitivity remains independent. The query worker must leave FTS pattern parsing alone. Regex never passes its pattern through an ordinary literal transformer; incompatible bindings fail at preparation.
- **Relations:** the relation-index worker owns declarations and immutable forward/reverse storage in generic collection/schema state. It exposes a read-only accessor and records its exact public shape in the handoff. Query traversal and semijoin planning are later tasks; neither first-wave query syntax nor regex work may infer relation semantics from matching ID text.
- **Unavailable data:** preserve known true/false/unavailable on raw fallback and dangling targets through NOT, AND and OR. Approximate grams and positional postings only restrict candidates; they never become exact proofs or complemented membership.
- **Identity and persistence:** a prepared template can be shared across parameter values, but verified membership includes the resolved typed values, regex mode/options, case mode, normalized sort, and repository generation. Version snapshot/cache contracts in the commit that first changes them. The current repository-wide generation scope is conservatively sufficient for future relation queries.
- **Shared files:** query worker owns `query.rs`, `prepared.rs` and `reference.rs` changes; regex worker owns `regex_query.rs`/`bigram.rs` with minimal FTS hooks; relation-index worker owns `model.rs`/`posting.rs`/`storage.rs` and relation storage. Each worker reports edits to `run.rs`, `executor.rs`, `repository.rs`, `Cargo.lock` and cross-owned API files for coordinator review. Each branch must compile independently from the gate. Integration requires a reviewed commit and authorization as specified in the task document.

## Task status

| Task | State | Base / evidence | Owner |
| --- | --- | --- | --- |
| TASK-00 contracts and foundation verification | complete | this ledger and pinned base checks | coordinator |
| TASK-01 logical/fallback planner partition | complete in base | `dbf4777adf`, `run.test.rs` mixed-domain fixture | coordinator |
| TASK-02 typed operands, paths, inline sort | reviewed feature branch; pending approved integration | code `a770ed9140`, handoff `b96e8e3cb7` | query worker |
| TASK-04 regex FTS | reviewed feature branch; pending approved integration | code `d4445fa545`, handoff `3115e188a4` | full-text worker |
| TASK-05 ID and relation indexes | reviewed feature branch; pending approved integration | code `a42840c07a`, handoff `47118d2c49` | relation-index worker |
| TASK-03 temporal/size | blocked by TASK-02 integration | — | later worker |
| TASK-06 relation traversal | blocked by TASK-02 and TASK-05 integration | — | later worker |
| TASK-08 relation inspection | blocked by TASK-05 integration | — | later worker |
| TASK-07 semijoin planner | blocked by TASK-06 integration | — | later worker |
| TASK-09 cross-feature verification | blocked by integration | — | coordinator |
| TASK-10 final measurements | blocked by TASK-09 | — | coordinator |

Every worker handoff must include the fields in Section 12.7 of the specification, exact test output, cache/snapshot effects and any shared-file edits. This ledger is updated at review checkpoints.

## G2 integration proposal (pending approval)

All three feature branches descend from gate `258575ff5e` and have been pushed. The proposed integration order is:

1. Merge `feature/relation-indexing` through handoff `47118d2c49` (implementation `a42840c07a`), retaining its typed ID model, relation storage, and snapshot/model version bumps.
2. Merge `feature/typed-query-operands` through handoff `b96e8e3cb7` (reviewed implementation `a770ed9140`), preserving both the relation branch's ID model and the query branch's typed operand and cache identities. Add the narrow contextual ID/parameter bridge here.
3. Merge `feature/full-text-regular-expressions` through handoff `3115e188a4` (implementation `d4445fa545`), reconciling FTS mode with typed parameters and both branches' exact evaluation and CLI hooks.

This order follows the dependency on the ID type before contextual query binding. A read-only `git merge-tree` inspection at the first-wave gate found textual conflicts between relation storage and typed operands in `reference.rs` and `repository.rs`, and between relation storage and regex in `lib.rs` and `reference.rs`. Both combinations also edit `executor.rs`; all three change public request and execution surfaces. These are expected integration work, not permission to discard either side. No merge has been performed.

Branch-local checks: the relation branch passed `cargo test --workspace --all-targets`, Clippy, formatting, and its focused build/restore fixtures; the regex branch passed the same commands plus conservative candidate fixtures. Read-only review found three typed field-comparison issues (ambiguous nested arrays, null inequality direction, and non-array derived length). Commit `4eda0b6782` corrected them with independent expected-answer and optimized/reference tests. Commit `a770ed9140` additionally preserved null through dynamic case folding. Post-fix `cargo test --workspace --all-targets` passed independently at that exact code in `/tmp/objekte-typed-final-a770-all-targets.log`, and the worker's Clippy and formatting checks passed. Each branch's Section 12.7 handoff is in its named documentation commit. New feature timing and cross-branch correctness have not been measured. The current development checkout is unchanged.

The specification's Section 12.6 requires explicit user approval before any branch merge into this integration worktree and separate approval before any merge into development or master. After authorization, the coordinator resolves conflicts, runs cross-feature checks, and starts the dependent tasks from accepted integration commits.
