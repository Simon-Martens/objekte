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
| TASK-02 typed operands, paths, inline sort | complete on feature branch, pending integration review | `25b103f92e`–`c504fdb493`, repair `4eda0b6782` | query worker |
| TASK-04 regex FTS | ready | first-wave gate | full-text worker |
| TASK-05 ID and relation indexes | ready | first-wave gate | relation-index worker |
| TASK-03 temporal/size | blocked by TASK-02 integration | — | later worker |
| TASK-06 relation traversal | blocked by TASK-02 and TASK-05 integration | — | later worker |
| TASK-08 relation inspection | blocked by TASK-05 integration | — | later worker |
| TASK-07 semijoin planner | blocked by TASK-06 integration | — | later worker |
| TASK-09 cross-feature verification | blocked by integration | — | coordinator |
| TASK-10 final measurements | blocked by TASK-09 | — | coordinator |

Every worker handoff must include the fields in Section 12.7 of the specification, exact test output, cache/snapshot effects and any shared-file edits. This ledger is updated at review checkpoints.

## TASK-02 handoff

- **Task identifier:** TASK-02, milestones 3, S1, 4, 5 and 6.
- **Owning branch and worktree:** `feature/typed-query-operands`, `/home/simon/source/objekte-typed-query-operands`.
- **Pinned base commit:** `258575ff5efac601df7ee9ac648a90531f8650bd`.
- **Ordered implementation commits:** `25b103f92e` typed operand AST; `fc3a68e64b` inline sorting; `fb5312bded` typed parameters; `2be7f8d06e` explicit array paths; `c504fdb493` correlated field comparisons. Each was pushed separately.
- **Changed implementation and test paths:** `crates/core/examples/musenalm_cli.rs`; core `src/{executor,lib,parameters,prepared,query,query_path,reference,repository,run,verified}.rs` and corresponding test files; `crates/core/tests/{array_positions,inline_sort,parameters}.rs`.
- **Implemented behavior:** Typed right operands with source spans; top-level and sort-only `SORT BY` normalized into registered sort options; checked typed parameter bindings without text interpolation; positional array selection; scalar and same-occurrence field comparisons with scalar broadcast; metadata-only comparison without source reads. Field comparisons retain residual verification, union projection, both-field evidence and unavailable fallback truth.
- **Deliberately excluded behavior:** Dynamic `:exists`/`:null` field comparisons are rejected because per-occurrence derived scope is not prepared. Independent multivalued branches are rejected. Custom operations must explicitly opt in to dynamic comparison. ID parameter transport and contextual ID literal binding await TASK-05 integration; regex pattern parameters await cross-feature integration.
- **Public API, cache and snapshot changes:** `Query::with_parameters` and repository search/reference/explain parameter wrappers; CLI `--param NAME=JSON`; structural serialized prepared and verified cache keys include bound typed values. `QueryPath` preserves array steps. No snapshot format change.
- **Test commands and observed results:** At `c504fdb493`, `cargo test --workspace` passed (core 123 passed, 1 ignored; array positions 2, inline sort 1, parameters 2, TUI 25, CLI 1, doctest 1). `cargo clippy --workspace --all-targets` exited 0 with advisory warnings. `cargo fmt --all`, the repository spacing script on touched files, and `git diff --check` completed; unrelated baseline formatting drift was restored.
- **Independent oracle / optimized-versus-scan evidence:** The deterministic field fixture asserts independently expected IDs for scalar, positional, paired array, broadcast, wildcard and metadata comparisons, then compares optimized and reference counts, hits and evidence. Malformed JSONL tests check null and unavailable truth. Parameter tests alternate values in a warm verified cache. No new jq oracle was added for these new semantics.
- **Measured performance, memory and resource counters:** No timing or memory run was made for this milestone. The metadata-only field comparison test asserts zero source file opens with locator return mode.
- **Known failures, warnings and open risks:** Workspace tests have no failures. Clippy reports advisory warnings, including existing general warnings. Dynamic derived `:exists`/`:null` remains a stated unsupported combination. The ID and FTS mode branches require the integration bridges above.
- **Shared-file edits requiring integration:** `query.rs` adds serialization derives to FTS structs but no FTS mode; its `FtsQuery` shape must be reconciled with TASK-04. `musenalm_cli.rs` adds inline-sort/parameter handling and overlaps regex flags. `reference.rs` adds bound dynamic operands and a custom-operation capability method; `run.rs` changes projection/evidence/fallback, `executor.rs` changes residual and metadata comparison paths, and `repository.rs` adds request wrappers. TASK-05 changes to `Value::Id` need contextual binding and tagged JSON transport.
- **Prerequisites for the next task:** Review and integrate TASK-02, then add the TASK-05 ID bridge before temporal or relation query work relies on the unified binder.

### Independent review repair

`4eda0b6782` rejects ambiguous nested arrays in unknown-typed paired paths at runtime, while explicit selected elements such as `items[*].tags[0]` remain comparable. Field-to-field `!=` treats one null and one non-null value symmetrically; literal null comparison behavior is unchanged. Invalid dynamic `:length` values now remain noncomparable instead of becoming null. The focused fixture checks these cases against independent expected records and optimized/reference parity. After the repair, `cargo test --workspace` passed (core 123 passed, 1 ignored; array positions 2, inline sort 1, parameters 2, TUI 25, CLI 1, doctest 1). `cargo clippy --workspace --all-targets` exited 0 with advisory warnings; formatting and `git diff --check` passed for the changed files. No new performance measurement was made. Only `reference.rs` and `tests/array_positions.rs` changed in the repair.
