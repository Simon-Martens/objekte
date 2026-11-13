# Plan 1 — Incremental availability during single-pass index construction

Date: 2026-11-13  
Task identifier: incremental-index-availability  
Integration branch: integration/incremental-index-availability  
Integration worktree: /home/simon/source/objekte-integration-incremental-index-availability  
Sequence: implement and validate this plan before Plan 2, then Plan 3.

## Objective

Make the file catalogue available after discovery and allow queries to begin while the initial index is being constructed. Preserve one construction read/parse per source record, subject to the existing adapter behavior. Construct validation state, record identity, bigrams, postings and other selected index families together for each work unit.

The rejected staged branch improved warm SearchReady from about 2.55 s to 2.13 s but increased configured completion to about 6.64 s and substantially increased process memory. Those historical numbers are context, not the benchmark baseline for this task. Early usability must not be purchased by another corpus pass, duplicate permanent indexes, or a comparable completion/memory regression.

This plan changes availability and publication. Plan 2 adds query predicates; Plan 3 extends structural postings. Do not implement their features here.

## Execution and branch discipline

1. Read AGENTS.md, docs/ai/README.md and relevant task notes. The reviewed master was 7a6b96684e1fbb849eab7cc20185b942159ed189. Resolve and record current master at execution time; retain subsequent master improvements.
2. Keep the original checkout on master. Implement only in the named sibling integration worktree. Preserve user changes and uncommitted task documents.
3. Never merge into, commit onto, or push updates to master without the user's explicit merge command.
4. Push immediately after every task commit, including integration and report commits. Do not force-push.
5. One supervisor owns integration. Optional subordinate agents may own core publication, clients, or verification in separate task branches/worktrees under /home/simon/source. Begin client integration after the core contract is agreed. Do not delegate overlapping edits.
6. When all three plans are assigned together, execute them in order without requesting approval between successful plans. Plan 2 starts from this plan's validated, pushed integration tip; Plan 3 starts from Plan 2's validated tip. Record the base SHA for each. The last branch consequently contains all three plans, while each earlier branch remains separately reviewable. This does not authorize merging master.

## Inspect before implementation

Read current construction, lifecycle and client code rather than importing the previous architecture. Relevant starting points are:

- crates/core/src/repository.rs: discovery, construction coordination and final repository assembly.
- crates/core/src/build.rs: BatchContribution, batch limits, worker coordination and admission.
- crates/core/src/lifecycle.rs, lifecycle_catalog.rs and mutation_segment.rs: immutable views, file membership and existing segment ownership.
- crates/core/src/executor.rs, index_catalog.rs and run.rs: coverage, planning, counts, streaming, sorting and caches.
- The current CLI/TUI opening path and desktop opening/query controllers.

At the reviewed revision, BatchContribution already groups records, postings, bigrams, sort keys and relations. This is an implementation opportunity, not evidence that partial publication is already safe. Check finalization, local-to-global slots, inference admission, duplicate-ID checks and cross-record dependencies.

The unmerged integration/staged-index-construction at bee59df5ed952e28b41f6b422e0c6bcc89afce1f is reference material only. Reuse small tested client/status ideas where appropriate. Do not merge its second-pass builder, extended batch overlay, separate query proof route or partial snapshot machinery.

## Required availability contract

Expose these independent facts through a small public status contract, using existing API conventions:

- Discovery status and discovered file count.
- Whether the file catalogue is complete.
- Processed file/record counts and whether any searchable batch exists.
- Whether initial construction is complete.
- Terminal cancellation/failure information.

A file catalogue row represents a physical file. It may expose its stable FileId, root-relative path, size and modification time after discovery. It must not invent RecordIds, record counts, byte ranges, source validity or schema validity. Multi-record files and intrinsic record IDs require content processing.

Allow the desktop/TUI to list files and accept queries after catalogue availability. Do not call the repository fully search-ready merely because file discovery finished. Use ordinary status wording such as “Indexing — 12,000 of 148,486 files”.

Metadata-only file listing/filtering can be complete over the discovered file catalogue. Existing record-query APIs must retain record semantics: a record count or record-level metadata query is not complete until the relevant record inventory is known.

## Construction and publication

1. Keep existing bounded worker scheduling and input/scratch reuse. Do not reduce construction to a single worker merely to implement publication.
2. Read and validate each construction work unit once. Build all applicable local index families from the same logical values and release decoded records at the existing bounded lifetime.
3. Publish only immutable completed units with consistent record slots, source bindings, validation state, bigrams, postings, coverage and necessary metadata. No visible half-written structures.
4. Move completed payloads into their owning representation and share them by immutable references. Do not clone all earlier postings, records, paths or ID maps for each publication.
5. Publication may group several work units to bound segment count and overhead. Do not emit one publication per record or repeatedly merge the entire accumulated index.
6. Reuse the current segment/lifecycle execution model. Do not add an “initial construction” planner beside the existing planner.
7. A family requiring collection-wide finalization, such as an inferred admission decision, global uniqueness check or relation resolution, must remain explicitly incomplete until its actual prerequisites are satisfied. Never claim exact coverage prematurely.
8. Source-format validation retains its existing scope. In particular, malformed skipped fields and trailing input must still be detected. Do not publish a portion of a single logical document as validated before its required validation has finished. Preserve existing JSONL framing and malformed-record behavior.
9. File moves, additions, deletions and replacements retain the explicit lifecycle contract. Reuse existing serialization/version checks for concurrent publication; cancel or restart affected build work safely rather than introducing source-stat checks on every query.
10. Final construction must reuse completed payloads. Do not produce a second complete repository while retaining the first complete representation. Any final consolidation must have bounded temporary memory and no source reread.

Discovery should expose the existing inventory without making a second corpus-sized path catalogue. Catalogue notification must not repeatedly copy all previously discovered files.

## Query execution while construction continues

Use the existing planner and index-capability model for the view actually being queried. Coverage and result completeness are separate:

- Missing capability on an already validated record can use ordinary source evaluation.
- A record outside an index's covered domain is unknown, not a negative match.
- An unprocessed file is pending construction. Do not launch a competing query-time validation or indexing pass over it.

Pin each evaluation to an immutable published view. An ordinary one-shot search over an incomplete view must either wait for completion or explicitly return the existing results together with an unmistakable incomplete status through a new opt-in API. It must never silently masquerade as a complete repository search.

The interactive query session may follow subsequent publications:

- Stream or display available matches immediately where semantics permit.
- Continue evaluating newly available records using the current capability coverage.
- Avoid reevaluating all earlier records after every publication for record-local predicates when no new dependency requires it.
- Final counts, exported complete results and complete pagination wait for completion.
- Sorting may reorder provisional results. Keep the selected record identified by RecordId, not by its temporary row number. Do not issue final continuation tokens against a moving result domain.
- Cross-record relations, backlinks, aggregates or global ordering may require additional work or deferral until their dependencies are complete. Do not present a provisional negative as final.
- Changing the query cancels its old subscription/results. Construction continues unless explicitly cancelled separately.

Follow queries need an initial view plus an ordered publication cursor/event contract that cannot miss a batch between subscribing and obtaining the view. Deduplicate publications, handle terminal events and bound queued notifications.

Growing record membership must affect the data revision/domain used by result caches. Never reuse a result cached for an earlier subset as a complete result for a later subset. Preserve cache reuse only where its existing validity requirements actually hold. Do not repeatedly clear unrelated completed-query caches for mere progress text updates.

## Desktop, CLI and TUI

Desktop/TUI:

- Show the file catalogue as soon as available.
- Keep query controls usable during construction.
- Label provisional results and counts, show progress, and transition automatically to complete results.
- Keep long work off the UI thread; coalesce progress updates.
- Preserve query edits, tabs, selection and cancellation behavior across publications.
- Display terminal build errors/cancellation without falsely showing completion.

CLI:

- Preserve existing default result correctness, stdout format, sorting and exit-status behavior.
- Accept a query while opening; start internal work when batches become searchable. Existing streaming modes may emit provisional data only if their protocol explicitly conveys incompleteness.
- A normal complete query/export waits for full relevant coverage. Diagnostics/progress belong on stderr, not mixed into machine-readable stdout.
- Reuse existing controls where possible; do not introduce the old two-pass readiness flag or a new staged configuration system.

Persistence:

- Preserve completed index save/load and blocking build compatibility.
- Do not add resumable partial snapshots in this task. Existing save operations must wait for a complete initial build or return an explicit incomplete-build status; they must not write a partial index as complete.
- Cancellation must preserve existing completed snapshots. A later opening may restart unfinished initial construction.

## Verification

Use deterministic barriers around discovery and batch publication, not timing-dependent sleeps.

Required cases:

1. Catalogue available before the first searchable batch; no invented record metadata.
2. Query submitted before the first batch, between batches and at final publication: no missed or duplicated results.
3. At every observed publication, source truth over the published validated domain agrees with index results. Final results/counts/evidence/order agree with the ordinary complete baseline.
4. Missing capability and partial coverage under AND, OR and NOT do not discard uncovered records.
5. Multi-record files, escaped paths, non-UTF-8 paths where supported, intrinsic IDs and duplicate-ID failure behavior.
6. Malformed skipped content, trailing content and raw fallback preserve validation and search behavior.
7. Query replacement, cancellation, terminal build failure, mutation during construction and safe stale-publication rejection.
8. Provisional sorted results, final pagination/export, relation queries and cache keys across growing membership.
9. Desktop/TUI interaction remains responsive and a CLI query never reports partial completion as success.
10. Completed snapshot round trip and existing lifecycle mutation tests.

Run the repository-required workspace tests, formatting/lint checks and native desktop checks using its documented commands. Do not substitute compile success for interactive state-transition verification.

## Performance experiment and acceptance

First record a fresh current-master baseline, then compare the completed task branch. Use the existing Musenalm construction/query benchmark and complete current query suite; preserve schema, selected fields, inference setting, alphabet, compiler, worker settings and cache conditions.

Measure:

- Discovery/catalogue availability, first searchable batch, first text result and full completion separately.
- Construction source reads/bytes and parse counts separately from query verification.
- Worker utilization, publication count, segment count and publication/finalization time.
- Actual peak RSS in a fresh process, post-build RSS, estimated live index bytes and snapshot bytes. Do not label a single RSS sample or summed nonexclusive heap estimates as peak memory.
- Quiescent complete-index queries with cache off, including Q108, Q116 and Q120 if those cases remain in the suite.
- A fixed foreground query workload during construction: response latency and construction slowdown, compared with the no-query run.
- Memory with an old view held during several publications, followed by release, to detect accumulating full copies.

Use seven paired warm construction trials with alternating order after a short smoke run. Report medians and IQRs. Cold/advisory-cache trials are supplementary and must be labelled accurately. Run every required correctness query; repeat performance measurements only for relevant comparisons or confirmed outliers.

Default acceptance budgets for construction without foreground queries:

- No additional normal corpus read/parse pass.
- Median full completion no more than 5% above the recorded master baseline.
- Peak RSS and completed live index estimate no more than 10% above baseline.
- No changed field-selection policy, missing existing acceleration or material complete-index query regression.

Investigate query regressions exceeding both 10% and 0.5 ms on repeated matched runs. Do not hide a slower query behind a geometric mean. These are task acceptance limits, not a predicted speedup. If the limits cannot be met after bounded implementation repairs, retain the branch and measured findings for review; do not mark it complete by merely filing an issue.

## Suggested commits and handoff

1. Record baseline and implement catalogue/build-status contract.
2. Publish completed single-pass units with bounded ownership.
3. Integrate incomplete-domain query sessions and cache/completeness handling.
4. Integrate desktop/TUI/CLI behavior.
5. Complete correctness/performance verification and resolve in-scope regressions.
6. Commit the completed specification with an appended Implementation report and benchmark report.

Store results in docs/ai/generated/incremental_index_availability_results.md. Follow docs/ai/README.md for task document lifecycle, immediate pushes and subordinate worktree cleanup. Retain this integration worktree and branch.

The handoff must state exact base/head SHAs, measured budgets, remaining limitations, no master merge, and the SHA to use as Plan 2's base. A passing test suite with failed performance gates is not completion.


## Implementation report

- **Task:** `incremental-index-availability` (Plan 1). Completed 2026-11-13.
- **Base:** `7a6b96684e1fbb849eab7cc20185b942159ed189` (then-current `master`/`origin/master`). **Integration branch:** `integration/incremental-index-availability`. **Worktree:** `/home/simon/source/objekte-integration-incremental-index-availability`. Master was not merged or changed.
- **Implementation:** Discovery publishes a physical-file catalogue; bounded single-pass construction publishes complete immutable segments and explicit progress/terminal status. Queries pin a consistent view, can follow ordered publications and surface provisional results with honest completeness. Complete search/export/save waits for full coverage. Desktop and TUI remain interactive during construction and transition to exact results; CLI accepts early direct input and preserves final stdout semantics. Snapshot load/save and lifecycle mutation behavior remain covered.
- **Subtasks and commits:** Core publication/query branch `task/incremental-index-availability/core-publication` through `0c3333e299`; desktop branch `task/incremental-index-availability/desktop` through `8dec99240a`; CLI/TUI branch `task/incremental-index-availability/cli-tui` through `d770085bec`. Each was reviewed and integrated/pushed. Integration code culminates at `ad542f6e9ad3ede9fca0e2235df61476ec721749`; final acceptance artifacts/report were pushed at `01ffdd8f75dd12732065be996ab7465f7b894256`. Key core repairs are recorded in Git history, including `2aa6fb49b4`, `6cf6be401b`, `dc4bc27358`, `75ca39f549`, and `0c3333e299`.
- **Validation:** `cargo test --workspace --all-targets --locked`: 905 passed, 30 ignored, zero failed. `cargo clippy --workspace --all-targets --locked`: passed with no warnings. Earlier integrated `cargo check --workspace --all-targets --locked`: passed. `git diff --check`: passed. Workspace-wide `cargo fmt --all --check` and `python3 scripts/format-rust-spacing.py --check` still fail from preexisting style drift on master; affected-file comparison shows no candidate-only rustfmt failures. Native desktop and PTY TUI state transitions were exercised, including provisional→exact results, query replacement, cancellation and final-code desktop launch.
- **Benchmark:** Seven alternating paired warm fresh-process Musenalm construction trials, same absolute 148,486-file corpus/schema, ASCII, 32 workers, measured builder `526ae413db`: median completion master 973.344 ms, candidate 910.233 ms (6.48% faster); fresh-process peak RSS 536.842→568.316 MB (+5.86%); completed live index 69.663→69.966 MB (+0.44%); exactly 148,486 candidate construction reads/parses. Catalogue median 208.079 ms, first searchable batch 286.082 ms, first result 286.264 ms, eight publications. Snapshot 129,139,458→178,287,528 bytes (+38.1%). Nine alternating foreground/held-view runs measured +4.54% median build time with 885–906 fixed queries/run and zero query source opens; no full-view copy accumulation observed. Full 194-case × four-profile × five-repetition query comparison passed all 776 status/count rows, no source-open increase. Two apparent full-suite latency outliers did not reproduce in three alternating seven-repetition matched pairs. Commands, environment, raw data, medians/IQRs, limitations and digest-format caveat are in [the acceptance report](../generated/incremental_index_availability_results.md).
- **Deviations and remaining limits:** No approval deviation. Saved snapshots grew 38.1%; this is a documented material persistence cost without a specified size gate. Provisional ordering/counts can change until complete; clients label them. No open in-scope acceptance failure or issue report.
- **Cleanup:** Task-owned subtask worktrees are removed after verified merge/push and preservation checks; this integration worktree and branch remain for review. The original untracked task source is removed only after this completion record is committed/pushed and its original bytes verified.
