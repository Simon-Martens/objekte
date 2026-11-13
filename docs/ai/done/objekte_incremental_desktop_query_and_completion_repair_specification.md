# Objekte: Incremental desktop query and completion repair

## Objective

Repair two defects in the cumulative implementation of Plans 1–3:

1. Incremental desktop refreshes materialize every matching record in newly published segments before discarding results beyond the visible page.
2. Autocomplete and the filter builder obtain inferred fields from stale opening metadata, so fields discovered during incremental construction can remain unavailable after construction completes.

Deliver both repairs, regression tests, and a focused performance report on the existing cumulative integration branch. Preserve the implemented predicate semantics, structural postings, and incremental availability model. Do not merge into master.

## Branch and worktree contract

- Repository: `Simon-Martens/objekte`.
- Target branch: `integration/indexed-field-structural-postings`.
- Existing integration worktree: `/home/simon/source/objekte-integration-indexed-field-structural-postings`.
- Reviewed cumulative commit: `7a9dee532e75bf167b6e4eabfd52277c79bb4df1`.
- Earlier integration tips: Plan 1 `c54afc3eb1`; Plan 2 `7c8ce508d5`.
- Reported master baseline: `7a6b96684e`.

Inspect the current worktree, branch, remote, and uncommitted changes before editing. If the integration branch has advanced, review those changes and adapt this task; do not reset it to the reviewed commit or duplicate an existing repair. Record the actual starting commit.

This task explicitly repairs the existing cumulative Plan 3 branch. It does not require a fourth integration branch. Leave the earlier Plan 1 and Plan 2 branches as historical checkpoints; their implementations are already included in Plan 3. Do not independently apply conflicting repairs to all three branches.

Keep the original checkout on master and unchanged. Preserve unrelated staged and untracked files. Push immediately after each commit; do not force-push or rewrite published history. If subordinate worktrees are useful, place them under `/home/simon/source`, integrate their changes into the target branch, and retain the integration worktree for review. No merge into master is authorized.

## Repair 1: Bound incremental result materialization

### Confirmed defect

At the reviewed commit:

- `crates/desktop/src/controller/runtime.rs::spawn_incremental_delta` builds a query, calls `search_added_since_with_cancellation`, and only afterwards truncates the returned hits to the remaining visible capacity.
- `crates/desktop/src/backend/query.rs::build_query` requests `ReturnMode::Bytes` and evidence. The unpaged query template uses limit zero when the user has not supplied a limit; callers must supply a page bound.
- `crates/core/src/lifecycle.rs::search_added_since_with_context` replaces the requested limit with the number of newly added records.
- `crates/core/src/run.rs` retrieves selected result payloads before the desktop receives and truncates the hits.

Consequently, a publication containing thousands of matching records can cause thousands of payload reads and allocations for a page needing only one more row. A full page can perform the same work and discard every result.

### Required behavior

1. Determine the remaining visible capacity before executing an appendable delta. Reuse the existing page-size definition and query-limit semantics; do not introduce another independent hardcoded page size.
2. Enforce the bound in result selection before materializing returned record bytes or evidence. Truncating a fully materialized result is not a repair.
3. For a record-local, appendable query whose visible page is already full, perform no delta search or payload retrieval. Advance the applicable publication bookkeeping cheaply so the controller does not repeatedly reconsider the same publications.
4. Keep query and open generations correct. A changed query must evaluate the current available domain, including segments skipped for the previous full page. Skipping delta work must not mark an incomplete result complete, enable premature pagination, or suppress the final exact refresh.
5. Preserve the correct replacement/deferred behavior for explicit sorting, offsets, counts, relation/global dependencies, changed lineage, and unsupported deltas. A full page does not justify skipping a replacement when new records could change that page. Retain cancellation and stale-result rejection.
6. Preserve the existing public all-delta contract for callers that require all newly matching records. Prefer a narrow bounded operation or explicit internal limit parameter shared with the existing implementation. Do not silently change an existing public method to return an incomplete delta, and do not duplicate the query engine.
7. Handle zero remaining capacity explicitly. Do not pass zero to an execution API if it means unlimited results there.
8. Preserve ordering, result identity, evidence, provisional status, and final result correctness. Do not fabricate a complete match count from a bounded delta.

The bound applies to returned-hit payload materialization. Residual verification for full-text or unindexed predicates can legitimately examine more sources than the page size. Do not claim a general source-read bound for those queries, or weaken matching to meet one. With a blank or exact indexed predicate that needs no residual source verification, payload reads must be bounded by the remaining visible capacity.

### Required tests

Use deterministic publication boundaries or test synchronization, not timing sleeps. Exercise the actual desktop query settings: bytes and evidence, not only locator results.

| Scenario | Required assertion |
|---|---|
| Large new segment; empty page | At most one page of payloads is materialized. |
| Large new segment; one row remaining | At most one additional hit payload is materialized. |
| Large new segment; full appendable page | No delta query or payload retrieval; publication tracking advances. |
| User limit smaller than page size | Remaining capacity respects that limit. |
| Several publications coalesced | Correct bounded results, no duplicate rows or replay of consumed publications. |
| Query changes after a full-page skip | The new query can find records in previously skipped segments. |
| Cancellation or repository replacement | Obsolete work cannot update the current grid. |
| Construction completes | Final refresh and final pagination/count behavior remain correct. |
| Sorted/global/offset query | Existing correct replacement or deferred behavior remains intact. |
| Existing all-delta caller | Its documented full-result behavior is preserved. |

Use counters or narrow test instrumentation to prove the materialization bound. Assertions solely on the returned vector length are insufficient: that already passed before the repair.

## Repair 2: Refresh field suggestions from the current index view

### Confirmed defect

At the reviewed commit:

- `crates/desktop/src/controller/filter_validation.rs::field_choices` iterates `session.repository.collections()` and reads `collection.inference` directly.
- `crates/core/src/lifecycle_caller.rs::IndexedRepository` retains opening metadata for its repository dereference, while searches can use later index views.
- `crates/core/src/repository_incremental.rs` publishes completed inference through `global_inference` in the final repository state.
- `crates/core/src/repository.rs::inference_report` already understands both the completed global summary and conventional inference reports.
- Suggestions derived from visible columns can conceal the defect. A late or uncommon field outside the displayed rows remains missing, especially when no rows match.

Reading only `collection.inference` from opening metadata is therefore insufficient, even when search itself correctly uses the final postings.

### Required behavior

1. Obtain configured fields and available inference observations from one current, pinned index view. Reuse the canonical inference-report or capability APIs. If an accessor is missing, add the smallest read-only accessor needed; do not expose mutable internal index structures.
2. Include the completed global inference summary. Preserve conventional nonincremental inference behavior and repositories opened from existing indexes.
3. Refresh both text autocomplete and the filter-builder field list when relevant metadata becomes available. Completion must trigger a refresh even when visible columns have not changed, the query has no results, or the user has not typed or reopened the repository.
4. Refresh appropriately on repository opening/replacement and collection-scope changes. Check initialization ordering: an `Opened` state reset must not erase suggestions just delivered for that opening.
5. Associate suggestion updates with the correct repository/open generation, scope, and metadata revision. Discard delayed updates from obsolete sessions or scopes. Coalesce redundant refreshes; do not rebuild every suggestion list on every progress tick.
6. Keep configured fields and reserved metadata available during construction. Inferred fields become available when their inference metadata is actually published. This task does not require new per-batch inference publications or a redesign of inference aggregation.
7. Preserve canonical field-path formatting, quoted keys, nested paths, collection filtering, and conservative type handling for mixed observations. Both completion and the builder must consume consistent choices.
8. Retain the existing modifiers and Boolean binding rules, including `:?`, `:string`, `:number`, `:array`, `:null`, `:object`, and `:exists`. Suggested queries must parse and execute with the same semantics as manually entered queries.

Do not scan source files to discover suggestions, enable inference implicitly, clone the full index for UI metadata, or rebuild postings. A suggested field is not proof that every predicate on it has an exact index: the existing planner remains responsible for capability and coverage decisions.

### Required tests

Construct an inference-enabled fixture in which a nested field appears only in a later publication and never appears in the first displayed page. Include a query with zero results to remove visible-column discovery as a fallback.

Assert that:

- Before the relevant metadata is published, configured and metadata suggestions remain usable.
- After construction completes, the late field appears in text autocomplete and the filter builder without reopening, editing the query, or changing columns.
- An unambiguous observed type is retained; mixed observations do not acquire an unjustified single type.
- Selecting the field and a new predicate produces a valid query and the same results as manual entry.
- Collection scope excludes fields belonging only to another collection.
- Nested/quoted field names survive suggestion insertion and query parsing.
- A late response from a previous repository or scope cannot replace current suggestions.
- Inference-disabled opening does not initiate inference or construct additional indexes for autocomplete.
- Initial `Opened` processing does not erase the final applicable suggestion list.

## Scope boundaries

Keep the existing incremental construction pipeline, validation policy, index-admission policy, structural representation, and unified query planner. Add no source parsing pass or query-time freshness/revalidation checks.

Do not expand this task into snapshot-size reduction, index compression, general scheduling changes, UI redesign, or unrelated formatting cleanup. The previously reported snapshot-size increase is not part of these two repairs.

## Validation and performance evidence

Record a before/after baseline using the actual starting integration commit, identical fixtures, and identical build settings.

1. Run focused core/controller/UI-model regression tests for both repairs, followed by the workspace test suite, Clippy, and relevant desktop checks. The earlier report recorded 920 passing and 30 ignored tests; report current actual totals rather than treating those numbers as a substitute for execution.
2. Preserve the existing query-campaign counts and result digests, including the new predicate/path cases. Run the established campaign where its data is available. State explicitly if a dataset or native environment is unavailable; do not report an unexecuted check as passed.
3. Benchmark the real desktop incremental refresh path with bytes and evidence. Use large matching publications and remaining capacities of a full page, one row, and zero rows. Report elapsed time, selected/materialized hit counts, source opens, and payload retention/allocation evidence. Explain any reads required for residual verification separately.
4. Repeat with a substantially larger publication to demonstrate that returned payload work is bounded by visible capacity, rather than growing with every match. Avoid fragile wall-clock assertions in unit tests.
5. Perform a native desktop smoke covering provisional rows, completion, a late inferred field in both completion surfaces, and execution of the inserted predicate. A fixture-open smoke alone does not cover this defect.
6. Confirm construction still has no additional parse/read pass. Since these repairs target query materialization and UI metadata, avoid adding a large unrelated construction benchmark project. Run a comparable build sanity measurement; if shared construction code changes or measurements suggest a regression, investigate with repeated paired measurements before declaring completion.
7. Format touched code and compare against the inherited formatting baseline. Do not reformat the unrelated files already reported by the previous task.

Source-free locator benchmarks alone are insufficient for Repair 1. Merely showing configured fields or fields from visible rows is insufficient for Repair 2.

## Delivery

Make reviewable commits separating the two repairs, each with its regression tests. Push after each commit. Commit the final evidence and completion record on the same integration branch.

- Completion record: `docs/ai/done/objekte_incremental_desktop_query_and_completion_repair_specification.md`, preserving this specification and appending the implementation outcome.
- Report: `docs/ai/generated/incremental_desktop_query_completion_repair_results.md`.
- Raw measurements, if generated: a task-specific directory under the existing benchmark results hierarchy.

The report must identify the starting and final commits, changed behavior, API compatibility decision, tests and commands, materialization counters, observed performance, and limitations. Preserve the previous three task specifications and reports.

The final handoff must state which branch contains both repairs, its pushed commit, the integration worktree, and any validation not performed. Confirm that master was not modified or merged. Do not mark the task complete while an in-scope correctness or materialization-bound failure remains unresolved.

## Implementation report

**Task ID:** `incremental-desktop-query-completion-repair`. **Completion date:** 2026-11-13. **Actual base commit:** `f9536d8787a2788c9462cc359fa421c0ad17b9d7`. **Integration branch:** `integration/indexed-field-structural-postings`. **Integration worktree:** `/home/simon/source/objekte-integration-indexed-field-structural-postings`. The reviewed commit in this specification had advanced due to the separately authorized snapshot repair; the actual base was inspected and retained. No fourth integration branch was made, and master was not modified or merged.

The bounded delta repair computes remaining visible capacity before asking core to select results. It adds `IndexView::search_added_since_bounded_with_cancellation`, shares appendability and query execution with the public all-delta API, and uses a zero-capacity appendability check without a delta search. Selected Bytes payload reads and bytes are now counted separately from residual source verification. The field repair reads configured definitions and canonical inference reports from one pinned `IndexView`, refreshes suggestions on completion/opening/replacement/publication/scope changes, and coalesces unchanged keys. It fixes the `Opened` reset ordering and uses the pinned view for initial projection columns. Inferred fields from the completed global summary now feed both text autocomplete and the builder; mixed observations remain untyped. Existing predicate semantics, structural postings, and incremental availability were preserved.

**Implementation commits:** `cae4d834bd` (bounded desktop delta and regression tests), `4cf5d759b9` (current-view field choices and regression tests), `2250398101` (formatting of touched code/tests). There were no delegated subtasks or subordinate branches. A detached exact-base measurement worktree was removed cleanly after its results were archived. The integration worktree and branch remain for review.

**Validation:** `cargo test --locked -p objekte-search-core -p objekte-desktop --lib --quiet` passed (534 core and 182 desktop tests, 16 and one ignored). `cargo test --locked --workspace --all-targets --quiet` passed after the final source commit (929 passed, 30 ignored, zero failed). `cargo clippy --locked --workspace --all-targets` exited 0 with inherited warnings. `cargo fmt --all -- --check` still fails on inherited formatting: the starting and final trees each produce 1,016 diff headers after edited lines were formatted. The production native Wayland desktop smoke showed provisional 100-row results, exact final completion, late inferred field in text completion and the builder with no visible matching rows, and a builder-inserted predicate returning the expected record. The task-owned GUI was stopped and its disposable fixture removed.

**Performance and correctness:** Six alternating fresh-process pairs using the actual starting commit measured the real desktop Bytes/evidence path for 2,048 and 8,192 matching records with capacity for 100, one, or zero rows. Old materialization was 2,048/8,192 payloads in every case; new materialization was 100/one/zero. Median elapsed times and retained payload byte sums are in [the report](../generated/incremental_desktop_query_completion_repair_results.md) and [raw archive](../../../bench/results/indexed-field-structural-postings/2026-11-13-desktop-repair/README.md). The established 880-row release query campaign preserved all result digests, statuses, counts and source-work counters. Fifteen full-campaign latency candidates were rechecked in three alternating exact-base pairs with seven repetitions each; none remained a material regression. Paired construction checks parsed exactly 148,486 records at both revisions and showed no repeatable build regression; the deterministic incremental fixture retained exactly 2,050 construction source reads/parses.

**Limitations and deviations:** A page bound applies to returned-hit payloads; residual verification may read more sources for queries requiring it. Inferred suggestions appear when their metadata is published; no per-batch inference publication was added. The previous snapshot save/restore peak issue (`docs/ai/issues/perf/lifecycle_snapshot_segment_buffer_peak.md`) was outside this task and remains for user review. No required validation was omitted, and no in-scope correctness/materialization failure remains. The original untracked specification was copied verbatim before this appended report; its SHA-256 was `04bc5929695a17dae190f9d348b5a9cec95ad3bada0e861aa4f0cab4a4396c9e`.
