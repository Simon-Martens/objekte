# Objekte native segment final corrections specification

## 1. Assignment and fixed scope

Implement exactly three corrections on the existing **`integration/native-segment-construction-repair`** branch. This is the final correction pass for this assignment. Implement the changes, prove them with focused regression tests, run the required checks, commit a results report, and stop.

Repository: `Simon-Martens/objekte`.

Reviewed branch revision: **`7e88607d1e3ed929e4ac9693e1f56a064d8ac9b1`**. The implementation described by that report is **`93210b976fedd45d9f1b1ea4e8eeb293a3b1c21d`**. If the branch has advanced, inspect the intervening commits and continue from its actual head. Do not reset it to either revision.

| ID | Required correction | Required result |
| --- | --- | --- |
| F01 | Related-field FTS opens target files through the original inventory | Resolve target paths and revisions through the same pinned current catalog as the relation query |
| F02 | The presence of some short strings bypasses general dictionary admission | Distinguish partial short-string coverage from a complete value dictionary |
| F03 | Final private-segment draining returns after the first worker error | Consume all outstanding completions before returning the failure; publish none of the failed file group |

The user accepts the current performance. **Memory optimization remains deferred.** Preserve native maps, Roaring postings, bigram filters, segment construction, existing parallelism, and public query semantics. Add no dependency or storage backend. Do not redesign the planner, allocation accounting, public snapshot API, or general scheduler architecture. Do not undertake a new performance optimization campaign.

This document defines the remaining work. It does not reactivate deferred requirements from earlier specifications. Existing accepted fixes must remain intact, but unrelated cleanup is not a fourth correction.

### Repository procedure

Read the applicable `AGENTS.md` and `docs/ai/README.md`. Find the existing repair worktree using `git worktree list`; the expected location is `/home/simon/source/objekte-integration-native-segment-construction-repair`. Use that worktree and branch. If the worktree is absent, attach the existing branch to a sibling worktree under `/home/simon/source`, following repository instructions.

Leave the original master checkout and unrelated local changes untouched. Fetch and inspect divergence before updating the repair checkout. Do not force-push, discard user changes, create another integration branch, or merge into master. Record the actual starting SHA. Follow the repository's task-document conventions, including its treatment of user-provided uncommitted specifications.

## 2. F01 — Read relation targets from the pinned current file catalog

### Failure and relevant code

In `crates/core/src/relation_semijoin.rs`, `verify_qualified_targets` resolves candidate records through `MergedRelationView`, but constructs its reader as:

```rust
let mut access = SourceAccessSession::new(&view.repository().files);
```

`SourceAccessSession::handle` in `source_access.rs` then obtains the file entry, native path, and revision from the original `FileInventory`. Lifecycle changes are represented in the current `FileCatalog`. Consequently, relation membership can refer to a correct current record while its physical read uses an absent or outdated inventory entry. Added files can produce `unknown source file`; target moves and replacements can use outdated paths or revision metadata.

The existing test `qualified_relation_semijoin_survives_file_lifecycle_and_compaction` does not establish correctness here: its principal request is the structured predicate `entry.title = 'Needle'`, its verified cache is enabled, and its move operation moves a source file. Add coverage of actual target-file reads through related-field FTS.

### Implementation decision

Extend the existing `SourceAccessSession` with a small internal borrowed source selector. Preserve its current inventory constructor for callers that intentionally use an immutable inventory. Add a constructor for a current catalog plus repository root. Suggested interface names:

```rust
enum SourceFileTable<'a> {
    Inventory(&'a FileInventory),
    Catalog { root: &'a Path, catalog: &'a FileCatalog },
}

// Within SourceAccessSession:
fn from_catalog(root: &'a Path, catalog: &'a FileCatalog) -> Self;
```

This is a design sketch; adapt visibility and imports to existing conventions. Use the existing catalog lookup API, not an invented parallel file map. `MergedRelationView` already exposes `repository_root()` and `catalog()`.

Implement these steps:

1. Replace the session's inventory-only field with the selector. Both constructors initialize the same existing handle cache, counters, read windows, execution context, and read limits.
2. In `handle`, resolve the file entry through the selected source. The inventory case retains current behavior. The catalog case retrieves the entry from the borrowed pinned catalog and constructs its native path from the root and that entry's relative path.
3. Pass the resolved entry's revision, including its size, to `RecordRangeReader::open_for_search`. Preserve range validation against that revision. The repository root may come from the repository; mutable file metadata must come from the catalog.
4. Construct the session in `verify_qualified_targets` with `view.repository_root()` and `view.catalog()`. Keep `configure_target_execution` and the existing target read accounting.
5. Keep one session bound to one pinned relation view for its entire lifetime. Do not fetch a newer global view during a query. FileId-based handle reuse is valid within that pinned session.

Preserve the existing bounded handle count, batching, reusable range windows, cancellation checks, resource limits, and error/diagnostic behavior. Preserve grouping by collection and file and the qualified record addresses used to map bytes back to targets. Do not turn this into one file open per target record.

Do not rebuild or clone the complete inventory for each query. Do not mutate `Repository.files` to make it resemble the current catalog. Do not disable the native semijoin optimization to avoid the failing read.

### Required regression tests

Use `relation_semijoin.test.rs` and the existing `selected_fts` helper. The central request must be related-field FTS, such as `selected_fts(None, "Needle", false)`, which selects `entry.title`; a structured equality request is not a substitute.

Use `verified_cache_bytes: 0`, locator return mode, and no computed sort. Ensure the request actually executes `try_semijoin_relation_page` through qualified target verification. Assert an existing execution counter that proves this, or add a narrowly scoped test-only execution marker. An estimated planner strategy alone is insufficient.

Exercise the following sequence through the normal lifecycle API:

1. Build an initial source/target fixture and establish its expected related-text result.
2. Create a target file after construction, add it to the index, and add a source referring to its logical ID. Assert the new target's FileId is absent from the original inventory. Related FTS must return the new source without a source-open error.
3. Replace that target's text while keeping its logical ID. Use old and new literals and a changed byte length so stale revision/range metadata is detectable. The old literal must cease matching and the new literal must match.
4. Rename the **target** file, remove the old physical path by the rename, and apply the supported lifecycle move/update operation. The related-text result must remain correct.
5. Compact while a mutated matching target remains live, then repeat the related-text assertions. Do not test compaction only after deleting every target.
6. Delete the target and verify that its source no longer obtains a related-text match.

After each operation, acquire a new current view. Assert explicit expected source IDs and counts, plus relevant matched-field evidence. Compare with the reference evaluator where appropriate, but keep explicit expectations so a shared reader defect cannot make both sides pass incorrectly. Retain the existing qualified-address collision and relation strategy tests.

Keep or add a small regression for inventory-backed `SourceAccessSession` callers. Do not impose historical file-byte retention on an old pinned view after its underlying file is overwritten; that is not the contract being repaired.

**F01 acceptance:** Related-field FTS reads added, replaced, and moved targets correctly, including after compaction, through the optimized path with the verification cache disabled.

## 3. F02 — Separate complete dictionary coverage from partial short-string coverage

### Failure and deterministic reproducer

In `crates/core/src/inference_build.rs`, aggregate `value_dictionary_complete` currently includes:

```rust
decision.string_admissible || aggregate.short_values_present
```

`short_values_present` is accumulated with OR across segments. It establishes that some short-string values exist, not that the short-string dictionary covers the field. Local candidate construction uses relaxed semantic minima, so locally complete candidates cannot establish collection-wide admission by themselves.

Create this deterministic two-segment fixture:

| Parameter | Value |
| --- | --- |
| Segment record limit | 512 |
| Logical records | 513 |
| First 512 field values | `"category_alpha"` |
| Final field value | `"b"` |
| `short_string_max_utf8_bytes` | 4 |
| `max_string_bytes` | At least the UTF-8 length of `"category_alpha"` |
| `min_nonblank_occurrences` | 514 |
| `min_nonblank_records` | 514 |

Set other policy limits permissively enough that these minima are the intended rejection reason. Assert that construction really produces the intended partition. Global general-string admission must fail. The single short value must not make the general dictionary complete. The existing all-long threshold test does not cover this exception.

### Implementation decision

Keep the existing shared field-admission decision as the authority for semantic policy. Add an explicit aggregate fact describing whether the **retained short-string component covers the entire required domain**. Do not create a second policy ladder in lifecycle finalization.

The existing single-domain construction establishes the relevant coverage predicate:

```rust
let short_covered = &field.short_seen - &field.short_uncovered;
let required = &field.present - &field.exact_null;
let all_short = has_short_values && short_covered == required;
```

Use this domain definition, including its treatment of nulls and empty selections. Do not replace it with `covered == eligible_records`, because missing and null records have different roles. Do not OR local bitmap integers across different segment domains.

Implement the repair in this order:

1. Introduce a small shared coverage helper or equivalent local calculation used by single-domain finishing and aggregate observation processing. Prefer calculating equality within each segment's domain and combining boolean coverage facts, avoiding a new global bitmap.
2. Initialize the aggregate all-required-domains-covered fact to true. For each field observation, compute the required local domain and whether the actual short-string component covers it. A nonempty required domain with missing, dropped, or incomplete short postings makes the fact false. An absent field or an empty required domain must not independently invalidate coverage elsewhere.
3. Keep the existing separate ANY-short-values fact to describe partial candidate availability. Require an actual retained short component somewhere before claiming a complete short dictionary. Observed short values or distinct-value summaries alone are not evidence that postings were retained.
4. Derive final completeness from the global admission decision, compatible scalar kind/shape, actual component availability, and local completeness. A complete short dictionary may substitute for a rejected general string dictionary only when every required local domain is covered. Partial short coverage may never make this substitution.
5. Preserve existing handling of numeric and boolean dictionaries, declared fields, explicit opt-outs, resource truncation, empty strings, structural facts, and array-length indexes. Short-string coverage is an additional justified route to completeness, not a reason to weaken another gate.
6. Follow the corrected diagnostic into `lifecycle.rs` construction finalization and the actual published posting capabilities. Ensure `posting.value_complete` cannot retain the old true value when collection-wide completeness is false. Preserve partial short-string coverage for candidate filtering and residual verification.

The intended relationship is:

```text
complete value dictionary =
    existing status/type/shape/completeness gates
    AND actual full local coverage
    AND (global general admission OR complete retained short dictionary)

partial short candidates = separate, weaker capability
```

This expresses the logic, not a requirement to introduce these exact variable names or a new public API. Check both report diagnostics and physical segment capabilities; repairing only report text leaves the planner defect intact.

The field can legitimately remain `Built` because partial short postings or structural information are useful. Do not turn every general-dictionary rejection into removal of the entire inferred field. Retaining unused candidate entries temporarily is acceptable under the memory deferral, provided they do not create an unsupported exact proof or an incorrect result. Their removal is not required by this task.

### Required regression tests

Add focused tests alongside the existing inference and segmented construction tests:

- **Mixed long/short rejection:** the 513-record fixture above. Assert failed general admission, false final dictionary completeness, retained short candidate capability, and correct query results for both values. Inspect the published capabilities used by the planner, not only the report's `Built` status.
- **Ordering independence:** put the short record in the first segment and long records in the remaining domain. Admission and coverage conclusions must be identical despite changed partition contents.
- **All-short positive control:** use a field whose applicable values are all within the short limit while the general minima remain unmet. With complete retained short postings, full dictionary coverage must remain available.
- **General-admission positive control:** lower the minima to an attainable value while keeping the mixed values and other policy limits permissive. The complete general dictionary must remain available.
- **Missing/null and incomplete coverage:** cover missing and exact-null records using the existing semantic domain; reuse the established truncation/overflow fixture to ensure a nonempty uncovered domain cannot be reported as exact. This can be a compact extension of existing tests rather than a new exhaustive matrix.

Retain the existing all-long threshold regression. For the mixed rejection case, assert that the rejected general dictionary is not used as a complete proof for the long-string predicate; the evaluator must retain the appropriate residual work. Do not demand source reads for a short predicate when its separate coverage already justifies an exact answer.

Use explicit expected IDs and the existing reference semantics. Keep test strings nonblank and the controlling policy values explicit. No random fixture may determine whether this boundary is exercised.

**F02 acceptance:** One short value cannot override failed general admission; fully covered short dictionaries and useful partial short candidates continue to work.

## 4. F03 — Finish draining private workers before returning failure

### Failure and relevant code

In `crates/core/src/repository_incremental.rs`, `PrivateSegmentGroup::drain` currently uses:

```rust
while scheduler.outstanding != 0 {
    self.consume_ready(scheduler, true)?;
}
```

`consume_ready` can consume an error completion and return `Err` while other tasks remain outstanding. `publish_private_segment_group` propagates this error. Because it owns the private group, that group is then dropped without the cleanup used by some extraction-error paths. Outstanding Rayon jobs can continue after the build has returned.

The existing collection-name failure injection fails all private jobs quickly. It does not reliably exercise a failure first observed during the final drain after all contributions have been submitted.

### Implementation decision

Unify final draining and discard draining around one private completion loop, with a small retain-successes/discard-successes choice if needed. Do not solve this with a blocking `Drop` implementation or by replacing Rayon.

The loop must:

1. Retain the first observed worker/completion error.
2. Consume each buffered completion exactly once and update outstanding accounting exactly once, including failed completions.
3. Continue receiving until every normally submitted outstanding job has reported, even after an error or cancellation prevents publication.
4. Retain successful builds only while the group can still be published. Once failure is known, clear retained private builds and discard later successes.
5. Return the retained failure only after normal completion accounting reaches zero. Return successful ordered builds only when no error occurred.

Separate **receiving a message** from **processing a worker result** so the loop can distinguish a worker's `Err` from a transport failure. A worker error is a completion to drain past. A disconnected receiver is terminal and must not be retried indefinitely. Do not implement an unconditional retry of every error from `consume_ready`. Preserve an earlier worker error when reporting cleanup failure, adding context if useful.

Keep the existing bounded submission window and asynchronous normal progress. The change belongs in cleanup/finalization behavior; it must not serialize successful construction. Preserve stable segment ordering on successful publication.

Audit the narrow call chain: final publication, extraction failure, submission failure, cancellation, and explicit discard. All paths that abandon an already-submitted private group must use the same draining behavior. When a caller already has an extraction or cancellation error, preserve that primary error while draining; do not replace it with a later cleanup error.

No segment from a failed private file group may be published. Its relation routes, inference contribution, and searchable record membership must not be committed. Preserve the existing catalog/error representation and any independent files already published; do not roll back unrelated successful files. Publication after an error is forbidden even if some worker builds succeeded.

The required guarantee covers the existing worker protocol, including workers returning errors and normal cancellation paths. Do not add a new general panic-recovery or worker-termination framework in this assignment.

### Deterministic final-drain regression

Add a test in the existing private construction test area, using test-only synchronization hooks:

1. Use at least two workers and a private file group producing at least two segment jobs. Configure the fixture so all intended jobs can be submitted without forcing the producer to consume the injected failure during an earlier submission-window wait.
2. Block the selected failing job before it sends its completion. Block another submitted job independently before completion.
3. Let the producer finish extraction and validation. Observe a test-only event at entry to the final drain, after final submission. Only then release the selected job to return a recognizable injected error.
4. Observe that the drain has consumed that error while the other job remains blocked. Assert that the build has not returned. Use channels/barriers and a bounded `recv_timeout`; a sleep followed by a timing guess is insufficient.
5. Release the remaining job. Assert that the build now returns the original injected error, all scheduled completions were consumed, and outstanding accounting is zero.
6. Assert no publication event or searchable membership from the failed private file group, including no committed relation routes or inference contribution. Observe these through existing test access or a narrow publication observer.

Use a test-local job ordinal/identity to select the failing job. Do not make every job fail and hope scheduling reaches the desired phase. Ensure gates are released and the build thread joined even when an assertion fails; use an RAII guard or equivalent bounded cleanup so a failing regression cannot hang the test suite. Keep hooks under `cfg(test)` and avoid global state that makes parallel tests interfere.

Keep the existing successful atomic publication, cancellation, and invalidation tests. A small direct test of completion accounting may supplement the lifecycle regression but cannot replace it.

**F03 acceptance:** An error first observed in the final drain does not allow an early return, leave outstanding normal worker completions, or publish any part of the failed file group.

## 5. Validation and commit sequence

Make three focused implementation commits, each including its regression tests:

1. `fix(core): resolve relation target reads through the current catalog`
2. `fix(core): require complete short-string coverage for dictionary admission`
3. `fix(core): drain private segment workers after finalization errors`

Push each commit according to the repository workflow. A final documentation commit records completion. Do not amend unrelated historical commits.

For each correction, show that its regression detects the reviewed defect. Run a compatible reproducer against the reviewed implementation or temporarily revert only the relevant production correction in an isolated check. The F03 test hooks may be retained while restoring the old drain behavior. Do not disrupt the working repair checkout. Preserve short failure output and then the passing output; if a baseline test cannot compile because it needs new interfaces, adapt the reproducer or use the narrow production revert rather than claiming baseline failure without evidence.

After the three commits, run the repository's required checks once. The previous report used these commands; preserve the same coverage unless repository instructions require more:

```bash
cargo test --workspace --locked --no-fail-fast
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked
cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked
git diff --check
```

Run focused core test filters during implementation. The final workspace test run need not be followed by redundant full core runs. Follow the repository formatting procedure without reformatting unrelated files. Report existing warnings separately from newly introduced warnings.

Run one bounded performance comparison using the established comparable build/query procedure, dataset, worker settings, and query result digests. Its purpose is to detect a material regression caused by these corrections, not to optimize unrelated query variance. Use the existing repeat policy if one is specified. Record raw commands and results; repeat only if a new failure or suspected correction-induced regression needs resolution. Do not add RSS reduction or memory measurements as acceptance gates.

## 6. Final report and stopping criterion

Commit `docs/ai/generated/native_segment_final_corrections_results.md` with:

- Actual starting SHA and final tested implementation SHA; the report commit is separately identified.
- One row each for F01, F02, and F03: changed functions, regression names, baseline failure evidence, final outcome.
- Exact commands and test results, including ignored tests and existing warnings.
- A short comparable performance result and confirmation of query digest agreement, with links to any raw artifacts required by the repository workflow.
- Explicit confirmation that memory work remains deferred and master was not merged.

## Implementation report

- Task identifier: `native-segment-final-corrections`
- Completion date: 2026-11-16
- Actual starting revision: `7e88607d1e3ed929e4ac9693e1f56a064d8ac9b1`
- Final tested implementation revision: `170eaeca03efa4cbc591a43fc3e611dd333b6ad2`
- Integration branch: `integration/native-segment-construction-repair`
- Integration worktree: `/home/simon/source/objekte-integration-native-segment-construction-repair`

### Implemented corrections

- F01 adds a borrowed inventory-or-catalog selector to `SourceAccessSession`. Qualified related-field target verification now resolves the current relative path and revision through the same pinned `FileCatalog` used by relation membership. The regression executes related-field FTS with cache disabled through target addition, replacement, target rename, compaction, and deletion.
- F02 calculates complete short-string coverage independently in every segment's semantic required domain, combines boolean domain facts without unioning local bitmaps, requires retained short postings, and applies the result consistently to report diagnostics and physical segment capabilities. Focused mixed, ordering, all-short, general-admission, missing, null, and uncovered-domain controls pass.
- F03 separates private completion receipt from worker-result processing. One shared drain loop retains the first worker error, consumes every submitted completion, discards retained successes after failure, records final outstanding count zero, and publishes no part of the failed file group. The test gate independently blocks two jobs and proves the build cannot return after only the failing completion is consumed.

### Commits

- `5344b4f374`: `fix(core): resolve relation target reads through the current catalog`
- `dee6d5f545`: `fix(core): require complete short-string coverage for dictionary admission`
- `5103218c13`: `fix(core): drain private segment workers after finalization errors`
- `170eaeca03`: `refactor(core): preserve duplicate private completions without replacement`

Each required correction has its own pushed implementation commit and focused regression. The fourth commit is a narrow warning-free cleanup of the F03 duplicate-completion branch; it does not add another correction.

### Validation

The final workspace test run passed at `170eaeca03`: core reported 543 passed, 16 ignored, and 0 failed; CLI and desktop unit/integration targets and all doctests passed. The private-window regression was subsequently corrected because `build_peak_segment_jobs` measures entered but not-yet-finished jobs rather than worker threads. The test now checks that the scheduler's outstanding-job window remains within `2 * workers`.

The following final checks passed:

```bash
cargo test --workspace --locked --no-fail-fast
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked
cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked
git diff --check
```

Clippy retains repository-existing advisory warnings and reports no warning from the changed paths.

### Performance

At eight workers, three-run construction medians were 876.767 ms ASCII and 887.279 ms Unicode, versus 814.378 ms and 899.332 ms in the prior comparable run. All runs retained 148,486 records, 15 native segments, stable per-alphabet index signatures, and the configured scheduler bounds.

The 300-case repository query matrix had sum-of-medians ratio `1.0103`, median per-case ratio `1.0081`, and geometric mean ratio `1.0109`. Progressive execution had ratios `1.0204`, `1.0232`, and `1.0519`. Targeted repeats kept plans and counters identical and found no correction-induced material regression. Repository and progressive result-digest files both have SHA-256 `aa907b96bb7960d361da4301aab269651285d80a3e17fcf09f9f856c8a049f63`, identical to the prior baseline.

The complete report and raw links are in [native_segment_final_corrections_results.md](../generated/native_segment_final_corrections_results.md).

### Deviations and limitations

- The F03 implementation includes test-only synchronization hooks in `SegmentJobGate` and one narrow lint-clean duplicate-completion cleanup commit.
- Memory optimization, compact summaries, allocator work, storage-backend changes, planner redesign, and general scheduler redesign remain deferred.
- Native maps, Roaring postings, bigram filters, segment construction, existing parallelism, and public query semantics were preserved.
- `master` was not merged.

### Cleanup

No subordinate worktrees were created. The integration worktree and branch are retained for review. The completed specifications in this file and `objekte_native_segment_correctness_completion_specification.md` remain in `done/`, and the generated report and raw evidence remain under `docs/ai/generated/`.

Make only claims directly established by the tests. For example, a structured relation test is not evidence for related-field FTS source reads, and a failure during extraction is not evidence for final-drain cleanup.

**The assignment is complete when F01–F03 meet their acceptance criteria, required checks pass, the report is committed, and all repair-branch commits are pushed. Stop at that point.** Report the branch and final commit to the user. Do not initiate another audit, write another repair plan, reactivate deferred work, or merge into master.
