# Objekte query execution — remaining repairs

Continue `integration/query-execution-path-repair` in `/home/simon/source/objekte-integration-query-execution-path-repair`. The reviewed tip is `4ec025d7264c1d894aa653c5cc52fcc93df04b15`. This document supplements the completed query execution addendum on the same branch. Follow `docs/ai/README.md`, retain the pushed history, and leave the cumulative branch for the owner to merge.

Complete the four corrections below in `crates/core`. Reuse the existing fixtures, reference evaluator and release benchmark. The ordinary indexed boolean OR already returns the right fields on both a cold stream and a warm page. The mixed flat-base page construction has been removed. Keep those regressions passing. The findings below come from source inspection. Capture each failure or excess-work count on the starting revision before applying its fix.

## 1. Make metadata comparison evidence agree with reference execution

The test for `@collection.name != @record.id` already checks that record `other` matches in collection `items`. It sets `evidence: false`. With evidence enabled, `segment_static_evidence_fields` excludes the left operand for inequality, then appends the right metadata operand unconditionally. The independent evaluator puts both operands under the same condition. [Static evidence](https://github.com/Simon-Martens/objekte/blob/4ec025d7264c1d894aa653c5cc52fcc93df04b15/crates/core/src/query_execution/runner/mod.rs#L1101-L1129) · [Reference rule](https://github.com/Simon-Martens/objekte/blob/4ec025d7264c1d894aa653c5cc52fcc93df04b15/crates/core/src/reference/filter.rs#L126-L143) · [Existing fixture](https://github.com/Simon-Martens/objekte/blob/4ec025d7264c1d894aa653c5cc52fcc93df04b15/crates/core/src/query_execution/runner/tests.rs#L1049-L1123)

Apply the evidence rule before adding either operand. Preserve deduplication and stable field order. Exercise the existing fixture with locator output and evidence enabled. Assert full-hit and count parity with the reference and these literal results:

| Query | Record | Matched fields |
| --- | --- | --- |
| `@collection.name != @record.id` | `other` | `[]` |
| `@collection.name = @record.id` | `items` | `["@collection.name", "@record.id"]` |

Exercise the exact ordered route through ordinary search and cold streaming. An OR that combines the inequality with a positive indexed field must report only fields contributed by true, evidence-producing branches. Verify a different page through an actual verified-result cache hit. This catches incorrect fields persisted for off-page rows.

## 2. Charge simultaneously retained bytes throughout replacement delivery

At a replacement callback the page, cursor heap, previous identities and newly copied identities can all coexist. The current runner checks the page plus heap. Delivery separately checks the heap plus identity copies. After the callback, the saved identities stay live while the runner budgets another cursor or source read. Two successful partial checks can admit an over-budget request. The previous report’s 4,230-byte observation also adds the page charge to another estimate of those same rows. [Page and callback](https://github.com/Simon-Martens/objekte/blob/4ec025d7264c1d894aa653c5cc52fcc93df04b15/crates/core/src/query_execution/runner/mod.rs#L4062-L4105) · [Identity ownership](https://github.com/Simon-Martens/objekte/blob/4ec025d7264c1d894aa653c5cc52fcc93df04b15/crates/core/src/query_execution/streaming.rs#L672-L715) · [Tail observation and read](https://github.com/Simon-Martens/objekte/blob/4ec025d7264c1d894aa653c5cc52fcc93df04b15/crates/core/src/query_execution/runner/mod.rs#L3793-L3843)

Give each quantity one owner and use checked addition under the existing byte-estimation convention. Compute the required size before allocating new identity copies, calling the sink or starting a bounded read. Apply the same convention to every caller of `Delivery::replacement`, including callers passing zero. Preserve `max_event_bytes` as the separate event-size limit.

| Boundary | Required charge while those objects are live |
| --- | --- |
| Replacement callback | Current page + cursor/reader state + old identities + new identities |
| After callback, before next read | Retained page + cursor/reader state + saved identities + upcoming source bytes |
| Final ordered-row callback has returned | Release delivered rows before counting the tail. Charge any state the engine still owns. |

Extend the public streaming fixture with sufficiently long record IDs and a later nonmatching source record larger than the first page. Construct two independent boundaries. At the callback, choose a limit where both old partial checks pass and their simultaneous total exceeds it. At the tail, choose a limit that admits the first callback and fits the read only if the saved identities are omitted. Keep the event limit valid in both runs. Assert the typed resource error, number of callbacks already delivered, sequence and emitted-row progress, and absence of a partial verified entry. At the exact sufficient limit, assert success and agreement with the reference. If a repair eliminates an allocation, derive the boundary from the objects it actually retains. Record the component sizes separately and compare their sum with the reported peak on the successful run. Preserve the cancellation test that observes zero subsequent reads.

## 3. Evaluate identical segment restrictions once when the preferred route declines

With a single collection, an explicitly stored sort, locator output and cache disabled, an evidence-bearing `a = true || b = true` can enter three probes. The exact-page helper computes bounds and declines row-dependent evidence. The ordered probe computes bounds and declines the exact owned page. The coordinator calls the first helper again before entering general execution. The current single-predicate test returns from the first helper and misses this sequence. [First two attempts](https://github.com/Simon-Martens/objekte/blob/4ec025d7264c1d894aa653c5cc52fcc93df04b15/crates/core/src/query_execution/runner/mod.rs#L1953-L1985) · [Late decline](https://github.com/Simon-Martens/objekte/blob/4ec025d7264c1d894aa653c5cc52fcc93df04b15/crates/core/src/query_execution/runner/mod.rs#L3555-L3568) · [Second exact-page call](https://github.com/Simon-Martens/objekte/blob/4ec025d7264c1d894aa653c5cc52fcc93df04b15/crates/core/src/query_execution/runner/mod.rs#L2048-L2064)

Remove the duplicate dispatch. Choose the executor before reading postings wherever eligibility follows from the request. When the decision needs calculated bounds, pass the bounds forward by borrow or move. For heterogeneous OR evidence, reuse branch membership already obtained on the same input where possible. Keep reuse scoped to this execution, pinned revision, segment, predicate and effective input. A genuinely narrowed input is a distinct operation. Keep the storage interface and the independent reference evaluator intact.

Reuse the four-row OR fixture. Exercise one segment and two **populated** segments, with a stable stored order, cold execution and evidence on. Measure actual restriction applications by predicate, segment and input across the complete public call, including helper variants with and without a predicate provider. Report counts for the combined filter and its branches. Require at most one physical application per identical operation and one sort binding and preparation when binding is required. Verify the ordered IDs, literal fields, count on/off, and zero source reads for this indexed locator fixture. Retain the simple exact-route test. Exercise the public stream fallback with a fixture that actually declines its first route and verify the same reuse rule.

## 4. Stop building cache members after admission closes

`LifecycleCacheBuilder::push` discards its membership list when it exceeds budget. The ordered loop continues to calculate a rank key, collect and sort fields, and construct a member for each later match. The general runner already asks `is_accepting()` before doing that work. [Builder](https://github.com/Simon-Martens/objekte/blob/4ec025d7264c1d894aa653c5cc52fcc93df04b15/crates/core/src/query_execution/runner/mod.rs#L679-L717) · [General runner guard](https://github.com/Simon-Martens/objekte/blob/4ec025d7264c1d894aa653c5cc52fcc93df04b15/crates/core/src/query_execution/runner/mod.rs#L1789-L1816) · [Ordered loop](https://github.com/Simon-Martens/objekte/blob/4ec025d7264c1d894aa653c5cc52fcc93df04b15/crates/core/src/query_execution/runner/mod.rs#L3898-L3944)

Decide separately whether the current row needs output evidence and whether the cache can accept a member. After admission closes, continue membership verification and counting as required. Skip cache-only key construction, field reconstruction, cloning, sorting and allocation. Reuse the popped cursor’s existing rank key while admitting members when the ordering is identical. Rows entering the requested page still need correct evidence, including when admission closed before the requested offset.

Use a small, positive verified-cache budget that admits at least one member and then closes during a broad ordered query. Use enough later matches to expose work in the count tail. Instrument cache-only work at the point it is performed. Require **zero** cache-only key or member constructions and **zero** evidence reconstructions solely for cache admission after rejection. Exclude necessary heap-ordering work from this counter. Check a later page whose rows still need evidence, complete count, and a subsequent cache miss for the rejected entry. Repeat with a sufficient budget and verify a warm hit with the expected fields.

## Merge criteria and handoff

- Record baseline and candidate outcomes for each regression. Include the actual field lists, replacement-byte components and event progress, restriction counts per input, and cache-only operations after rejection. A green workspace suite alone cannot establish these properties.
- Run `cargo test --locked -p objekte-search-core --all-features`, `cargo test --locked --workspace`, `cargo fmt --all -- --check` and `git diff --check`. Record the inherited formatting result for `model/json.rs` separately if it persists. For performance, use the affected OR and counted-tail fixtures with the timing pattern in `crates/core/examples/bench_query_execution_addendum.rs`. Three short alternating baseline/candidate pairs suffice. Report completion time and, for streaming, time to first nonempty callback, with units, cache state, result parity and work counters.
- Append the results to this specification under `docs/ai/done/`. Correct the previous report’s 4,230-byte interpretation with the measured components. Use the existing `count_core_rust_lines.py` to report physical production and test deltas from both `4ec025d726` and `891d377f32`. Identify the duplicate calls removed and any outstanding condition. Push the integration commits and provide the final code and report revisions for review.

## Implementation report

- Task identifier: `query-execution-remaining-repairs`
- Completed: 20 November 2026
- Starting revision: `4ec025d7264c1d894aa653c5cc52fcc93df04b15`
- Final code revision: `ca0f3451fafa56b09cc11273f6efa62a8cc56e22`
- Integration branch: `integration/query-execution-path-repair`
- Integration worktree: `/home/simon/source/objekte-integration-query-execution-path-repair`

### Implemented repairs

- `176fa6b35d` — metadata comparison evidence now applies the evidence rule before either operand is added. For `@collection.name != @record.id` on record `other`, the starting implementation produced `Some(["@record.id"])`; the reference result and repaired implementation are `Some([])`. Equality on record `items` retains `["@collection.name", "@record.id"]`. Ordinary search, cold streaming, an evidence-bearing indexed OR, and a later verified-cache page agree with the reference.
- `1a8b9cb05f` — replacement delivery and ordered-tail reads now use one checked retained-byte total. A replacement callback owns page-event bytes, cursor/reader state, old identities and new identities. After the callback, a bounded read owns the retained page, cursor/reader state, saved identities and upcoming source bytes. `max_event_bytes` remains a separate event limit. The callback and tail regressions cover both old partial checks passing while their simultaneous total exceeds the request, typed resource failures, progress, successful exact limits and absence of partial cache publication.
- `eb5e198650` — heterogeneous exact OR branch memberships are computed once and reused for candidates and per-row evidence. The ordinary one-populated-segment case performs two predicate-bound applications; two populated segments perform four. It uses one preparation and one sort binding in ordinary and streaming invocation, opens no source files and returns the reference IDs, fields and counts.
- `70697da94a` and `3ea4531377` — once verified-cache admission closes, ordered execution stops cache-only rank-key, member and evidence construction while it continues filtering and counting. A page-row check is now position-aware, so evidence is still built for rows that enter the requested page after admission closes. With the 512-byte cache fixture, 17 candidates remain after closure and the repaired counters are 17 closed candidates, zero cache-only keys, zero cache-only members and zero cache-only evidence. A later page has correct evidence and is a cold miss; a sufficient cache produces a warm hit with identical fields.
- `ca0f3451fa` — a public stream whose first ordered probe declines now passes that decision into the fallback executor instead of probing the same ordered route again. On the fallback fixture the starting revision performed six predicate-bound applications; the repaired complete public call performs three, exactly one for each predicate, with one preparation and one sort binding. The route remains `bounded_top_k` and the streamed hits equal the independent reference.

### Baseline and repaired observations

The starting implementation was replayed for the route-sensitive cases before applying each production correction. The metadata inequality evidence mismatch and the fallback excess work were captured directly on `4ec025d726`. The replacement-budget fixture shows that checking page plus heap and then identity copies separately permits an over-budget callback; the repaired callback has one checked sum. In the successful ordered-tail observation the components were:

| Live component | Bytes |
| --- | ---: |
| Page-event bytes | 2,291 |
| Saved and new identities | 143 |
| Cursor/reader state | 384 |
| Upcoming source record | 1,838 |
| Checked total | 4,656 |
| Reported peak | 4,888 |

The earlier report's 4,230-byte value was not the total simultaneously retained quantity. It combined a page charge with another estimate of the same rows; its interpretation has been corrected in `objekte_query_execution_path_repair_addendum.md`.

The cache replay leaves 17 post-rejection candidates in the 20-row fixture. On the starting loop each such match still constructed a rank key, reconstructed and sorted fields, constructed a member and called `LifecycleCacheBuilder::push`, even though the closed builder discarded it. The repaired loop reuses the popped cursor key only while admission is open and skips all of that cache-only work after closure. It still verifies membership, preserves evidence for emitted page rows, completes the count and leaves no partial verified entry.

The duplicate paths removed are the second ordered-stream probe after a stream fallback had already declined it, the separate exact-OR recount after page construction, and post-rejection cache key/member/evidence construction. The ready-state eligibility check also avoids entering the exact evidence page helper when an OR needs row-dependent evidence; if exact branch evidence cannot be formed, the ordered route declines and the general executor owns the request.

### Validation

- `cargo test --locked -p objekte-search-core --all-features`: 630 passed, 16 ignored, 0 failed; all integration and doc tests passed.
- `cargo test --locked --workspace`: passed for core, CLI and desktop; core ran 629 passed and 16 ignored under the workspace feature set.
- Focused `rustfmt --edition 2024 --check` passed for every task-owned Rust file.
- `cargo fmt --all -- --check`: the only reported difference is the inherited extra blank lines in `crates/core/src/model/json.rs`, present before this task.
- `git diff --check`: passed.

### Release comparison

Three baseline/candidate pairs used the same Musenalm root, four workers, cold verified-result cache and release builds from `4ec025d726` and final code `ca0f3451fa`. The benchmark example was identical in both revisions. Values are medians of the three alternating runs, with first nonempty `OrderedRows` callback and completion shown for streams.

| Case | Baseline first / completion | Candidate first / completion |
| --- | ---: | ---: |
| Q141 count off | 13.825 / 13.841 ms | 13.547 / 13.562 ms |
| Q141 count on | 13.338 / 13.353 ms | 13.127 / 13.142 ms |
| Multi-field relation FTS | 0.021278 ms search | 0.021248 ms search |
| Stored residual count off | 0.066400 / 0.071748 ms | 0.032066 / 0.034427 ms |
| Stored residual count on | 0.025521 / 0.835122 ms | 0.025435 / 0.814713 ms |
| Indexed heterogeneous OR | 0.787725 ms search | 0.056905 ms search |

Q141 returned the same count `823`, returned 50 rows, traversed 982 candidates/982 files, read 2,309,880 bytes, parsed 982 records, and produced result digest `3b411ddf4dba8863` in every baseline and candidate run. The multi-field relation case returned three rows with the same counters in every run. Stored residual count-on observed one match at the first callback and completed at all 1,000 candidates with count `500`; count-off stopped after its one-row page. The indexed OR returned 1,000 matches to the reference evaluator. Its baseline scanned 1,000 records and read 32,549 bytes; the candidate returned 50 ordered locator rows from 50 candidates with zero source files, bytes, parses or residual evaluations. All benchmark searches were cold (`verified_result_hit: false`).

### Source-size delta

The repository's physical-line classifier reports:

- `4ec025d726..ca0f3451fa`: production 175 added, 89 removed, **+86 net physical lines**; tests 555 added, 14 removed, **+541 net physical lines**.
- `891d377f32..ca0f3451fa`: production 764 added, 688 removed, **+76 net physical lines**; tests 1,380 added, 30 removed, **+1,350 net physical lines**.

The production growth is the four bounded retention/evidence checks and their counters. The cumulative production growth remains 76 physical lines from the original `891d377f32` baseline.

### Delivery and cleanup

Code was pushed before this report at `ca0f3451fa`. The temporary detached baseline worktree and benchmark target are disposable and were removed after measurement. Task memory was deleted after transferring its findings here. The original checkout's uncommitted task source is removed only after this committed report is verified to preserve the 54-line specification exactly. The integration branch and worktree remain for owner review and merge. No acceptance condition remains unmet.
