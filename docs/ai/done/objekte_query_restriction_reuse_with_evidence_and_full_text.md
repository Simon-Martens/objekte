# Objekte repair specification · Preserve restriction reuse with matched fields and full-text search

**Merge condition.** For one query on one pinned view, reuse each structured predicate restriction already computed for the same segment and effective input slots. Two branches of the new implementation bypass that handoff. Both affect execution work. The source review found no incorrect result rows in the cases below.

Continue `integration/query-execution-path-repair` at reviewed tip `41e913e6b3d7523b201f92707ea1c1a19fd49a74` (code `5aa761cf2bd48ccb7401b04ce6ac5d8bd31caf29`), in `/home/simon/source/objekte-integration-query-execution-path-repair`. Follow `docs/ai/README.md` and leave the merge to the owner. Keep the existing request-local lifetime. The four evidence-off page bounds, selective first callback, cursor allocation protection and untimed benchmark validation already have regressions.

## 1. Carry the result of the OR evidence probe into fallback

**Reproduce first.** In `ordered_exact_multi_file_or_does_not_reapply_restrictions_in_fallback`, keep the 65-file fixture, four workers, disabled verified-result cache, stored numeric sort, locator output and count. Use `a = true || note = 'needle'`, offset 1, limit 50. Change only `evidence` to true. Run the reference separately, then start and consume predicate observations around each cold public call. Capture baseline observations on the reviewed tip. [Fixture](https://github.com/Simon-Martens/objekte/blob/41e913e6b3d7523b201f92707ea1c1a19fd49a74/crates/core/src/query_execution/runner/tests.rs#L7169-L7203) · [Current evidence-off test](https://github.com/Simon-Martens/objekte/blob/41e913e6b3d7523b201f92707ea1c1a19fd49a74/crates/core/src/query_execution/runner/tests.rs#L7351-L7409)

**Fault.** The ordered runner calls `exact_or_branch_evidence_fields` before consulting the shared segment restriction. The helper applies the indexed `a` branch and the `note` branch on the active slots. When `note` requires verification, it returns `None`. The caller immediately declines the route before inserting bounds. The fallback then evaluates the OR on the same slots, including `a`. With several segments, a later decline can also discard evidence and membership work completed for earlier segments. [Branch applications and decline](https://github.com/Simon-Martens/objekte/blob/41e913e6b3d7523b201f92707ea1c1a19fd49a74/crates/core/src/query_execution/runner/mod.rs#L1326-L1366) · [Early return before handoff](https://github.com/Simon-Martens/objekte/blob/41e913e6b3d7523b201f92707ea1c1a19fd49a74/crates/core/src/query_execution/runner/mod.rs#L3626-L3679) · [OR input and union](https://github.com/Simon-Martens/objekte/blob/41e913e6b3d7523b201f92707ea1c1a19fd49a74/crates/core/src/query_execution/restrictions.rs#L1462-L1484)

**Implementation.** Give one operation ownership of bounds and optional evidence for a top-level OR. For each child, preserve the possible and proven bitmaps returned by the restriction provider. For the OR as a whole, form the union of possible rows and the union of proven rows. Keep per-child field contributions where exact membership proves them. If one branch requires source verification, the complete aggregate still has value for fallback. Store it in the existing request-local entry before declining the ordered route, or decline before physical work if eligibility is known cheaply. If the probe stops partway through the children, preserve completed results and calculate the remainder once. An exact match in `a` proves inclusion but cannot establish whether `note` also belongs in the row’s matched fields. Preserve the reference evaluator’s rule of reporting every true evidence-producing branch.

**Acceptance.** Both `IndexView::search` and `IndexView::search_stream` must apply the indexed `a = true` posting exactly once on the fixture’s full 260-slot input. Using the existing observation hook, also show each successful `note` restriction application on that input. Measure source verification separately. Require 257 matches and exact ordered hit parity. In the offset-1 page, ID `2` must have `["a","note"]` and ID `13` must have `["note"]`. The existing fully indexed `a || b` evidence control must retain zero source reads. Reuse the page-bound test table for evidence on and off so a change in offset cannot conceal another repeat.

## 2. Reuse structured bounds when FTS leaves the filter input unchanged

**Fault.** The ordered attempt stores structured bounds computed on the full active domain before doing full-text restriction. A broad multi-file FTS can make that attempt decline. In the general executor, the handoff is read only under `fts.is_none() && !text_first`. Thus a filter-first plan reapplies the filter on the entire active domain when FTS is present. A text-first plan also discards the entry when FTS returns every active slot. The decisive fact is which slots the structured predicate receives. [Producer](https://github.com/Simon-Martens/objekte/blob/41e913e6b3d7523b201f92707ea1c1a19fd49a74/crates/core/src/query_execution/runner/mod.rs#L3660-L3709) · [Multi-file decline](https://github.com/Simon-Martens/objekte/blob/41e913e6b3d7523b201f92707ea1c1a19fd49a74/crates/core/src/query_execution/runner/mod.rs#L3791-L3824) · [Consumer input selection](https://github.com/Simon-Martens/objekte/blob/41e913e6b3d7523b201f92707ea1c1a19fd49a74/crates/core/src/query_execution/runner/mod.rs#L2224-L2271)

**Implementation.** Use the same pinned collection, segment and bound filter as today. Reuse the full-domain entry when the general executor supplies that same active-slot set. A filter-first plan knows this without comparing bitmaps. A text-first plan may also establish it, for example by shared identity or by the full-domain subset invariant and equal cardinality. Equal sizes of arbitrary slot sets do not prove equality. When the text-first input genuinely narrows, compute the restriction on that input or explicitly restrict both saved bounds if applicable to that provider. Keep downstream candidates inside the text input and preserve source verification and cancellation. Scope this correction to structured restriction reuse.

**Acceptance.** Use the same 260 records, four workers, offset 1, limit 50, count on, stored sort and locator output. Combine the fully indexed structured expression `a = true || b = true` with literal FTS over the searchable `note` text. Set evidence off to isolate this path. Run two independent controls:

| FTS literal | Expected reference result | Required observation |
| --- | --- | --- |
| `needle` | All 260 records, including the three containing `needle-extra`. Page IDs 1–50. | In both public APIs record the actual execution route and filter input sets. When the ordered attempt and fallback both need the full domain, apply each structured branch exactly once across the call. |
| `extra` | Only IDs 1, 5 and 9 match. At offset 1 the page contains IDs 5 and 9. | Check full-hit and count parity. Exercise the general text-first consumer with a genuinely narrower input and assert its actual slot set. If the selective ordered route finishes directly, add a focused test at the consumer to cover that handoff. |

Use the existing literal FTS API and verify the reference counts before enabling observations. Record provider applications by field, operator, literal, segment and effective slot set. Keep candidate generation distinct from final text verification. The examples establish identical and narrower domains respectively. If an index returns a broader safe candidate set for `extra`, use a controlled selective text provider or fixture for the narrow-domain assertion.

## Reporting and verification

1. For each new regression, record the failing baseline observations and the repaired results. Report applications per term and slot set for ordinary and streamed searches. Keep existing controls for page bounds 51, 64, 65 and 260, 65 ordinary file-worker jobs at bound 51, one scanned row for the selective stream, and zero source reads for the exact OR.
2. Run the focused regressions, `cargo test --locked -p objekte-search-core --all-features`, `cargo test --locked --workspace` and `git diff --check`. Record the inherited formatting result separately if it persists. The physical work counts and reference hits decide these two issues. The full dataset timing suite is unnecessary for this repair.
3. Append the observed numbers and final code revision to the completed spec under `docs/ai/done/`. Push the integration commits and provide the code and report revisions for review.

## Implementation report

- Task identifier: `query-restriction-reuse-evidence-full-text`
- Completed: 20 November 2026
- Starting revision: `41e913e6b3d7523b201f92707ea1c1a19fd49a74`
- Final code revision: `bd10419a8ee8791084e664658da4bc7a69a95ae7`
- Integration branch: `integration/query-execution-path-repair`
- Integration worktree: `/home/simon/source/objekte-integration-query-execution-path-repair`

### Implemented repairs

`bd10419a` changes the evidence-bearing top-level OR probe to retain the complete aggregate `may` and `must` bounds for every child. It also retains per-child evidence only when every child has exact membership. The aggregate is inserted into the request-local restriction cache before an order-dependent evidence failure declines the ordered route, so fallback reuses the completed restriction work. The independent evaluator still supplies all true branch fields, including later branches that source verification proves.

General execution now asks the cache for bounds restricted to its actual effective input, rather than reusing only when FTS is absent or filter-first. Both possible and proven bitmaps are intersected with that input. A broad FTS that leaves the active domain unchanged reuses the full-domain entry; a selective text-first consumer reuses a genuine bitwise-narrowed input without another structured posting application. The cache remains scoped to one pinned view, request, collection and segment.

### Evidence-on OR observations

The fixture uses one segment with 260 active slots, 65 files and segment ID `1`. Both public APIs started observations immediately before the cold call.

| Entry point | Baseline `a` | Baseline `note` | Repaired `a` | Repaired `note` | Result |
| --- | ---: | ---: | ---: | ---: | --- |
| `search_stream` | 2 | 2 | 1 | 1 | count 257, reference page |
| `search` | 2 | 2 | 1 | 1 | count 257, reference page |

Every observed input was segment `1`, slots `[0..259]`. Fallback source verification opened more than 64 files and scanned at least 257 records. ID `2` has `["a","note"]` and ID `13` has `["note"]`; all 50 hit objects equal the reference.

The fully indexed `a || b` evidence control still has zero source reads and per-record fields intact.

### FTS and structured restriction observations

| FTS literal | Entry point | Baseline `a`, `b` | Repaired `a`, `b` | Route | Result |
| --- | --- | ---: | ---: | --- | --- |
| `needle` | stream | 2, 2 | 1, 1 | general fallback | 260 matches; 50-row page IDs 1–50 |
| `needle` | search | 2, 2 | 1, 1 | general fallback | 260 matches; 50-row page IDs 1–50 |
| `extra` | stream | 1, 1 | 1, 1 | ordered completion | 3 matches; IDs 5,9 |
| `extra` | search | 1, 1 | 1, 1 | ordered completion | 3 matches; IDs 5,9 |

For broad `needle`, the general consumer reused the cached full-domain entry with input slots `[0..259]`. For the selective ordered control, structured branches originally received the full 260-slot input and applied once each. A focused general-consumer assertion then restricted the saved possible and proven bounds to the genuine text-first input `[1,5,9]`; both bounds became that slot set with zero additional posting applications. Full hits and count match the reference in every case.

### Existing controls and benchmark validation

- Evidence-off page bounds 51, 64, 65 and 260 still apply `a` and `note` once in both APIs.
- Ordinary bound 51 still dispatches 65 parallel file jobs.
- The selective non-FTS stream still emits ID 12 after one candidate and one source verification.
- One-file and 65-file indexed OR controls still return 260 matches, a 50-row page, one application per indexed branch and zero source reads.
- The release benchmark's separate untimed stream collected complete ordered hits, including matched fields, and compared them with the reference. Both layouts returned 260 matches and 50 rows with zero files, bytes or parses.
- The full timing suite was intentionally omitted because this repair changes no benchmark-timed path and the specification states it is unnecessary.

### Validation

- `cargo test --locked -p objekte-search-core --all-features`: 633 passed, 16 ignored, 0 failed; all integration and doc tests passed.
- `cargo test --locked --workspace`: passed for core, CLI and desktop; core ran 632 passed and 16 ignored under the workspace feature set.
- Focused evidence-OR, FTS, page-bound, selective-route, cursor-allocation, nested-evidence and benchmark-validation tests passed.
- Focused `rustfmt --edition 2024 --check` passed for every task-owned Rust file.
- `cargo fmt --all -- --check`: only the inherited extra blank lines in `crates/core/src/model/json.rs` remain.
- `git diff --check`: passed.

### Delivery and cleanup

The code is pushed on the integration branch. The detached baseline worktree and release target used for observation were removed. Task memory was removed after its findings were transferred here. The original checkout's untracked task source was removed only after the committed 36-line specification was verified byte-for-byte. The integration branch and worktree remain for owner review and merge.
