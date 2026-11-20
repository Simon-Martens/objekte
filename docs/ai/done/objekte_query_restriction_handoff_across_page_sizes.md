# Objekte repair specification · Reuse segment restrictions across page sizes

**Merge blocker.** A residual query can still apply the same indexed predicate twice when the requested page crosses a size boundary. Query results agree with the reference, so checking hits and count alone misses this defect.

Continue `integration/query-execution-path-repair` in `/home/simon/source/objekte-integration-query-execution-path-repair`. Reviewed branch tip `1a82e74d80dc310987a465da45ae9fae872d7da9`. Its production revision is `e2ba8a8962e9a9e37526ca626c66ad9532ba160e`. The cursor allocation repair, nested evidence regression and equal-sized 260-record fixtures are complete. Reuse those fixtures. The production work belongs in segmented query execution.

## 1. Reproduce the remaining failure on the existing data

Use the existing one-segment, 65-file JSONL fixture, four workers and a disabled verified-result cache. Query `a = true || note = 'needle'` with ascending stored `sort`, locator output, evidence off and count on. There are 257 true matches across all 65 files. IDs `1`, `5` and `9` fail the residual check. The other three records in each affected file still qualify.

| Offset | Limit | Page bound | Reference page in ascending order | Why run it |
| ---: | ---: | ---: | --- | --- |
| 1 | 50 | 51 | 50 hits, first ID 2, last ID 53 | Existing case. Keep its 65 ordinary worker dispatches. |
| 14 | 50 | 64 | IDs 17 through 66 | Last page size covered by the new early decision. |
| 15 | 50 | 65 | IDs 18 through 67 | One-row offset change disables that decision. |
| 0 | 260 | 260 | All 257 hits | Existing full-result check, currently outside posting observation. |

Run each page through both `IndexView::search` and `IndexView::search_stream`. Start predicate observations immediately before each public call and consume them immediately after completion. Run the reference query separately. For this fixture, assert **exactly one successful application of `a = true` on segment 1’s full 260-slot input** per call, using the fixture’s actual segment ID in code. Check literal, operator and actual input slots. Preserve full-hit and count comparisons. Record baseline multiplicities before modifying production code. [Existing measured page and full-result control](https://github.com/Simon-Martens/objekte/blob/1a82e74d80dc310987a465da45ae9fae872d7da9/crates/core/src/query_execution/runner/tests.rs#L7353-L7437) · [Current observation point](https://github.com/Simon-Martens/objekte/blob/1a82e74d80dc310987a465da45ae9fae872d7da9/crates/core/src/query_execution/restrictions.rs#L354-L386)

**Why the boundary matters.** `prefer_general_file_fallback` returns false above 64. The ordered stream then computes both OR branches over active slots. At least 257 rows can match and they span 65 files, which satisfies the later file-count rule for declining residual queries. The caller receives `Ok(None)` and passes only the prepared query, sort and a Boolean to the general executor. The latter applies the filter on the full active domain again. Ordinary search can also call an exact-page probe that computes and discards bounds on decline. The existing 260-row test verifies the 257 results but leaves these applications uncounted. This is a source trace. Supply the actual per-call counts from Rust in the implementation report. [64-row guard](https://github.com/Simon-Martens/objekte/blob/1a82e74d80dc310987a465da45ae9fae872d7da9/crates/core/src/query_execution/runner/mod.rs#L957-L995) · [First calculation](https://github.com/Simon-Martens/objekte/blob/1a82e74d80dc310987a465da45ae9fae872d7da9/crates/core/src/query_execution/runner/mod.rs#L3615-L3644) · [Late decline](https://github.com/Simon-Martens/objekte/blob/1a82e74d80dc310987a465da45ae9fae872d7da9/crates/core/src/query_execution/runner/mod.rs#L3744-L3779) · [Stream handoff](https://github.com/Simon-Martens/objekte/blob/1a82e74d80dc310987a465da45ae9fae872d7da9/crates/core/src/query_execution/runner/mod.rs#L4647-L4659) · [General calculation](https://github.com/Simon-Martens/objekte/blob/1a82e74d80dc310987a465da45ae9fae872d7da9/crates/core/src/query_execution/runner/mod.rs#L2229-L2256) · [Exact-page probe](https://github.com/Simon-Martens/objekte/blob/1a82e74d80dc310987a465da45ae9fae872d7da9/crates/core/src/query_execution/runner/mod.rs#L1687-L1713)

## 2. Calculate each applicable restriction once

1. For the resident stored-sort, structured-filter path, put the per-segment restriction result under one request-local owner. Produce it once before consumers that need physical membership. Alternatively, return the result along with a declined ordered attempt and pass it through both public entry points. Use `SegmentCandidateBounds` with its existing shared bitmaps. Preserve `may` (possible rows) and `must` (proven rows), and per-branch evidence where the query needs it. A copy of only the final candidate bitmap loses the proof needed by downstream execution.
2. Have the exact-page probe, ordered traversal and general execution consume the same compatible result. Account for **both** ordinary calls to `try_exact_segment_ordered_page`. A declined attempt can have prepared some segments or none. Compute only the missing segments. Keep the ownership tied to the pinned publication, collection, segment, bound expression and effective input. A narrower FTS-first input genuinely can require different work. Preserve cancellation, source-read limits and the existing OR counter based on proven membership. [First ordinary probe](https://github.com/Simon-Martens/objekte/blob/1a82e74d80dc310987a465da45ae9fae872d7da9/crates/core/src/query_execution/runner/mod.rs#L2052-L2083) · [Second ordinary probe](https://github.com/Simon-Martens/objekte/blob/1a82e74d80dc310987a465da45ae9fae872d7da9/crates/core/src/query_execution/runner/mod.rs#L2147-L2167)
3. Base the ordered-versus-general decision on restricted candidates when selectivity matters. The current shortcut counts **all** active records and sums segment file counts, then guesses source requirements from schema flags. A large collection can have four candidates in one file. Widening the page-size cutoff leaves that error intact. Keep file-worker eligibility in the scheduler. For an eligible broad residual page, ordinary search should still dispatch workers. A stream fallback must retain its execution context. Neither worker count nor page capacity changes an indexed predicate’s truth set. [Current shortcut](https://github.com/Simon-Martens/objekte/blob/1a82e74d80dc310987a465da45ae9fae872d7da9/crates/core/src/query_execution/runner/mod.rs#L936-L995) · [Worker eligibility](https://github.com/Simon-Martens/objekte/blob/1a82e74d80dc310987a465da45ae9fae872d7da9/crates/core/src/query_execution/runner/mod.rs#L2552-L2576)
4. Exercise the opposite routing case on the same data. Use the stored numeric sort column to restrict `sort >= 12 && sort <= 15 && note = 'needle'`. Only IDs 12–15 qualify, all in a single file. With offset 0, limit 1, count off and the cache disabled, the ordered stream can deliver ID 12 after verifying its first candidate. Check the candidates’ file count and actual source verification work at the first callback. Preserve early completion or demonstrate an equivalent amount of work. This catches a change that suppresses duplicate postings merely by sending every small page in a large repository through exhaustive verification. [Ordered early completion](https://github.com/Simon-Martens/objekte/blob/1a82e74d80dc310987a465da45ae9fae872d7da9/crates/core/src/query_execution/runner/mod.rs#L4300-L4326)

The exact indexed OR control must continue to produce 260 matches, a 50-row reference page, one application per indexed branch and zero source reads in both layouts. Keep the existing lower-level observation for worker jobs on the 51-row case. The 65- and 260-row cases need correct posting counts and result parity regardless of worker eligibility.

## 3. Correct the benchmark verification claim

The timing harness compares the streamed **number of rows** and total count, then compares the **ordinary** result to the reference. Its report says the benchmark also checked streamed order and evidence. The unit regression does check them. Add a separate untimed streaming pass in the benchmark that collects hits in emission order and compares them, including matched fields, with the independent reference. Keep the measured callback lightweight. Identify the existing baseline correctly as `9b63f56d5a4759e25d09ce1ea032f2dd8158dea4`; the previous timing paragraph contains an invalid abbreviated SHA. [Benchmark assertions](https://github.com/Simon-Martens/objekte/blob/1a82e74d80dc310987a465da45ae9fae872d7da9/crates/core/examples/bench_query_execution_addendum.rs#L595-L626) · [Previous report](https://github.com/Simon-Martens/objekte/blob/1a82e74d80dc310987a465da45ae9fae872d7da9/docs/ai/done/objekte_ordered_cursor_allocation_and_residual_restriction_reuse.md#L117-L131)

## Acceptance and handoff

- Show a before/after table for all four page bounds and both public APIs. Include indexed-field applications on each distinct input, count, returned rows and actual route. Record ordinary file-worker jobs for bound 51 and first-callback verification work for the selective query. A runtime number must come from the observed call.
- Run the focused regressions, the existing cursor-allocation and nested-evidence tests, `cargo test --locked -p objekte-search-core --all-features`, `cargo test --locked --workspace` and `git diff --check`. Run `cargo fmt --all -- --check` and identify the inherited `model/json.rs` difference separately if it persists. Use the existing two release fixtures for a short timing comparison and the corrected stream validation.
- Follow `docs/ai/README.md`. Append results to the completed assignment in `docs/ai/done/`, push the integration commits and give the owner the final code and report revisions. Leave the merge to the owner.

## Implementation report

- Task identifier: `query-restriction-handoff-across-page-sizes`
- Completed: 20 November 2026
- Starting revision: `1a82e74d80dc310987a465da45ae9fae872d7da9`
- Final code revision: `5aa761cf2bd48ccb7401b04ce6ac5d8bd31caf29`
- Integration branch: `integration/query-execution-path-repair`
- Integration worktree: `/home/simon/source/objekte-integration-query-execution-path-repair`

### Implemented repair

`7e84c810` adds one request-local `SegmentRestrictionCache`. Each entry owns the full shared `SegmentCandidateBounds` (`may` and `must`) and exactness for one collection/segment. The ordinary exact-page probes, ordered traversal, and general executor now consume the same entry for the resident stored-sort structured-filter path. A failed first exact-page probe leaves completed segment bounds available, the second probe fills only missing segments, and the ordered-to-general handoff reuses the same bitmaps.

The cache is created anew for each public invocation and is passed through that invocation only. It never crosses pinned publications or requests. FTS-first/narrower paths bypass reuse, and the existing possible/proven semantics, cancellation, source limits, evidence handling, and proven-membership OR counter remain intact.

The schema/active-record routing shortcut was removed. File-parallel eligibility stays in the general scheduler and is based on the actual restricted candidates, not all active records or schema guesses.

`5aa761cf` restores module imports needed by the shared child modules; it does not change query behavior.

### Page-bound public API observations

Each measurement started immediately before the public call and consumed observations immediately after it. All posts below are the literal `a = true` and `note = 'needle'` applications on segment `1` with effective input slots `[0..259]`.

| Offset / limit / bound | API | Baseline `a`, `note` | Final `a`, `note` | Final route |
| --- | --- | ---: | ---: | --- |
| 1 / 50 / 51 | stream | 1, 1 | 1, 1 | general fallback |
| 1 / 50 / 51 | search | 1, 1 | 1, 1 | general fallback |
| 14 / 50 / 64 | stream | 1, 1 | 1, 1 | general fallback |
| 14 / 50 / 64 | search | 1, 1 | 1, 1 | general fallback |
| 15 / 50 / 65 | stream | 2, 2 | 1, 1 | general fallback |
| 15 / 50 / 65 | search | 4, 4 | 1, 1 | general fallback |
| 0 / 260 / 260 | stream | 4, 4 | 1, 1 | general fallback |
| 0 / 260 / 260 | search | 4, 4 | 1, 1 | general fallback |

Every case returned count `257`. The 50-row pages contained the expected reference hits: IDs `2..53`, `17..66`, and `18..67`; the full page returned all 257 hits and omitted IDs `1`, `5`, and `9`. Full hit objects and per-record matched fields equal the independent reference.

The ordinary bound-51 call dispatched 65 parallel jobs for 65 files in both baseline and candidate runs. The residual source path still verifies source records and opens the multi-file sources.

### Selective opposite case

For `sort >= 12 && sort <= 15 && note = 'needle'` with offset 0, limit 1, count off and cache disabled, the reviewed implementation discarded restricted selectivity because the old shortcut counted all active records and sent the query to the general executor. The repaired stream completes on the ordered route:

- first callback: `processed_candidates=1`, `observed_matches=1`, `emitted_rows=1`;
- first and only result: ID `12`;
- counters: 1 candidate, 1 distinct file, 1 file open, 1 scanned record, 1 residual evaluation;
- count remains unknown because count was disabled and execution stopped after the page.

### Exact OR control

The one-file and 65-file indexed OR layouts still return count `260`, a 50-row reference page, one physical application for each indexed branch, and zero source opens, bytes or parses. Stable IDs, order and matched fields agree between layouts.

### Release comparison

The corrected benchmark was built identically against `1a82e74d80dc310987a465da45ae9fae872d7da9` and `5aa761cf2bd48ccb7401b04ce6ac5d8bd31caf29`. Three alternating pairs used four workers, disabled verified cache, offset 1 and limit 50.

| Layout | Baseline first / completion | Candidate first / completion |
| --- | ---: | ---: |
| One file | 44.765 / 50.917 µs | 43.438 / 49.958 µs |
| 65 files | 43.748 / 49.733 µs | 42.950 / 49.075 µs |

Both layouts returned 50 rows and count 260 with zero source work. A separate untimed streaming pass now collects all emitted hits and compares the complete ordered hit list, including matched fields, with the independent reference. The ordinary result is validated separately.

The prior completion report was corrected to name baseline `9b63f56d5a4759e25d09ce1ea032f2dd8158dea4` rather than its invalid abbreviated spelling, and to describe the newly added complete streamed-hit validation accurately.

### Validation

- `cargo test --locked -p objekte-search-core --all-features`: 633 passed, 16 ignored, 0 failed; all integration and doc tests passed.
- `cargo test --locked --workspace`: passed for core, CLI and desktop; core ran 632 passed and 16 ignored under the workspace feature set.
- Focused cursor-allocation, nested-evidence, page-bound, selective-route, benchmark, and exact-OR tests passed.
- Focused `rustfmt --edition 2024 --check` passed for every task-owned Rust file.
- `cargo fmt --all -- --check`: only the inherited extra blank lines in `crates/core/src/model/json.rs` remain.
- `git diff --check`: passed.

### Delivery and cleanup

The code and benchmark commits are pushed on the integration branch. The detached baseline worktree and release target were removed after measurement. Task memory was removed after transferring its findings here. The original checkout's untracked task source was removed only after the committed 39-line specification was verified byte-for-byte. The integration branch and worktree remain for owner review and merge.
