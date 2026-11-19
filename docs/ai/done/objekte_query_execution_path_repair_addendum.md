# Objekte query execution path repair addendum

Continue `integration/query-execution-path-repair` from `d063a91c7167731d4bffb9252fafd74622421b4a`. This is a supporting assignment for the existing repair. Use its worktree at `/home/simon/source/objekte-integration-query-execution-path-repair`. Push each reviewable commit there under `docs/ai/README.md`. The owner merges the cumulative branch.

The reviewed tip corrects benchmark units. Its production code is identical to `f585c142c1`. The four findings below were traced in that source. Capture the observable failures before editing, using the same fixtures for the repaired revision. Keep the original qualified relation-FTS regression, the independent reference evaluator, and the verified-result cache. Prepared bindings and any reusable restrictions live for one execution. [Reviewed implementation](https://github.com/Simon-Martens/objekte/tree/d063a91c7167731d4bffb9252fafd74622421b4a/crates/core/src) · [Original assignment and corrected report](https://github.com/Simon-Martens/objekte/blob/d063a91c7167731d4bffb9252fafd74622421b4a/docs/ai/done/objekte_query_execution_path_repair.md)

## 1. Return the correct matched fields for an exact local OR

In `query_execution/runner/mod.rs`, `segment_static_evidence_fields` returns `None` when OR branches name different fields. In `try_lifecycle_ordered_stream`, exact membership bypasses verification. The runner then emits an empty field list and can save that list for every matching slot. A later page request can return incorrect highlights even when its own execution would have produced the correct fields. [Exact-match and cache-member construction](https://github.com/Simon-Martens/objekte/blob/d063a91c7167731d4bffb9252fafd74622421b4a/crates/core/src/query_execution/runner/mod.rs#L3837-L3894)

Use uniform evidence where it exists. Otherwise recover the matching branches for the specific collection, segment and slot. Reuse available branch postings or complete per-term proofs. If the selected route lacks sufficient information, use the existing evaluator for the rows that need evidence, or select an evidence-capable indexed route before emitting anything. Preserve indexed membership and the cheap evidence-off path. When admitting a complete verified result, account for off-page members that a later request may return. If admission stops, stop calculating evidence solely for admission.

Build one small segment with declared, indexed boolean fields. Give four records a deterministic identity or stored order. Run `a = true || b = true` with locator output, evidence on and count on.

| Record | a | b | Expected matched fields |
| --- | --- | --- | --- |
| A | true | false | `[a]` |
| B | false | true | `[b]` |
| C | true | true | `[a, b]` |
| D | false | false | Excluded |

Start with an empty, enabled verified-result cache, default execution limits and a one-row page. Let the cold `OrderedRows` stream finish. Assert A's fields and total three. Request the remaining two rows through ordinary search on the same view, changing only pagination. Assert an actual verified-result hit and the literal fields for B and C. Also compare with the reference and record the selected route before and after the fix. With admission disabled, verify all three rows directly. With evidence off, verify the same IDs and empty field lists. An exact count-only request with admission disabled should require no source reads for evidence.

## 2. Account for the page while the residual count continues

When the page fills, the callback receives a borrowed slice. On return the vector can still own all the payloads, but `page_bytes` is unconditionally set to zero. Cursor replenishment and the next residual read use the understated total. The owned-page variant moves the rows into an output vector, which has a different lifetime. [Page delivery](https://github.com/Simon-Martens/objekte/blob/d063a91c7167731d4bffb9252fafd74622421b4a/crates/core/src/query_execution/runner/mod.rs#L4035-L4055) · [Subsequent source-read budget](https://github.com/Simon-Martens/objekte/blob/d063a91c7167731d4bffb9252fafd74622421b4a/crates/core/src/query_execution/runner/mod.rs#L3773-L3799)

After the final ordered-row callback returns, release row payloads and their unused allocation before reading another candidate. Where a mode requires retained data, include it in the next budget check. For page replacements, preserve identity comparison, event sequence and final progress. For an owned page, trace its lifetime across the move and apply the existing budget wherever that API promises one. Use the accounting conventions of `Delivery::check_buffer`, `check_record_bytes` and the ordered reader. Keep caller-owned copies outside engine accounting.

Exercise the public ordered stream with a small `Bytes` page, large matched payloads, and a later large nonmatching record that must be read to finish a residual count. Choose a byte limit that accommodates each phase individually but cannot hold the page, cursor heap and tail read together. At the tail-read boundary, use a narrow test observation of the actual retained rows/payload bytes or their destruction. The test must detect the old combination of live payloads and a zero charge. A successful query or a peak derived solely from the disputed counter cannot demonstrate this repair.

Assert one final page and the correct count. Make the tail record larger than the delivered page. Set `max_event_bytes` high enough for that page and at or below `max_buffered_bytes` in both runs. Lower the buffer limit until the required tail read alone cannot fit and assert a typed resource error, accurate already-delivered progress and no partial verified entry. Stop from the row callback in a separate run and assert zero subsequent tail reads. Cover replacement and owned-page behavior where ownership changed.

## 3. Calculate segment restrictions once across route selection

The ordered probe computes per-segment filter bounds before deciding whether to handle the page. For a single selected collection, exact filter and explicit stored sort, the owned-page probe can decline after this work. The exact-page or general runner then computes equivalent bounds again. Reusing a prepared query does not eliminate the posting lookups and bitmap operations. [Probe and late decline](https://github.com/Simon-Martens/objekte/blob/d063a91c7167731d4bffb9252fafd74622421b4a/crates/core/src/query_execution/runner/mod.rs#L3465-L3538) · [Exact-page computation](https://github.com/Simon-Martens/objekte/blob/d063a91c7167731d4bffb9252fafd74622421b4a/crates/core/src/query_execution/runner/mod.rs#L1427-L1467)

Decide before expensive work when eligibility is already known. Where eligibility depends on computed bounds, pass those bounds to the selected executor by borrow or move. Reuse applies to the same bound predicate, pinned revision, segment, active domain and effective input. A narrower candidate input can legitimately require another evaluation. Keep route selection and bitmap ownership local to the request.

Use a named collection, a simple indexed filter, an explicitly stored sort, locator output and a disabled verified-result cache through the normal public wrapper. Confirm the old late decline. Count actual restriction evaluations beneath both helper variants, including the path without a predicate provider. Count preparation and sort binding separately. Require one evaluation for each identical input, one preparation and one sort binding on the uncached route. Repeat with two segments. Exercise the stream fallback where it shares that work. Check IDs, order, counts and no-count early completion. A test that only counts preparation misses this defect.

## 4. Resolve whether production needs the populated flat-base runner

The mixed coordinator currently requests a base page of `offset + limit` public hits with the caller's payload mode, looks up their IDs, recomputes their merge keys and applies global pagination. A base row can therefore incur a payload read and then lose to a segment row. The existing populated-base fixture writes crate-private active sets and clears the segment list. `Index::from_repository` accepts an already-constructed repository. Neither shows how a user obtains that state. [Base contribution](https://github.com/Simon-Martens/objekte/blob/d063a91c7167731d4bffb9252fafd74622421b4a/crates/core/src/query_execution/runner/mod.rs#L1954-L2033) · [Fixture](https://github.com/Simon-Martens/objekte/blob/d063a91c7167731d4bffb9252fafd74622421b4a/crates/core/src/lifecycle/tests.rs#L590-L615)

Trace safe production construction, initial and partial publication, adoption, supported reopen/restore formats, mutation and compaction. Check the direct runner, repository and pinned-view entry points and their workspace callers. For base-only and mixed states separately, record the constructor chain and where it installs active base slots. Compatibility records retained as metadata are insufficient. A synthetic fixture may still test an internal invariant.

| Reachability result | Required change and acceptance |
| --- | --- |
| No supported constructor produces active base slots | Remove the unreachable optimized base dispatch and code used exclusively by it. Route supported public entry points through the remaining coordinator. Retain the independent reference and any helpers with live callers. Exercise public empty, partially published and segment-only views, including validation of an invalid query on an empty view. List the functions removed. |
| A supported constructor or reader produces active base slots | Demonstrate it from public input without private field writes. Keep the reachable case. Supply qualified identities, existing rank keys and requested evidence to the common winner selection. Materialize output payloads after global ordering and pagination. Eliminate the intermediate public page and ID lookup. If only base-only is reachable, retire mixed-only machinery after checking callers. |

For a supported mixed state, use tied keys, overlapping local slot numbers and a deep offset with segment rows displacing base candidates. Use indexed membership and ordering to isolate output-only reads. Trace actual source reads by record. For `Bytes` and `Logical`, require zero output-only reads for losing and skipped base rows. Compare final payloads, order, fields and count with the reference. Preserve legitimate predicate verification reads, revision-qualified locators, existing unavailable-source behavior and verified-cache eligibility.

## Verification and handoff

1. Run `cargo fmt --all -- --check`, `cargo test --locked -p objekte-search-core --all-features`, `cargo test --locked --workspace` and `git diff --check`. If the earlier formatting difference in `crates/core/src/model/json.rs` remains, compare it with the starting tip and report it separately. Keep changes scoped to this repair.
2. Compare release builds at `d063a91c71` and the final code commit on the same machine and input. Time the first nonempty `OrderedRows` callback and completion from the same start for a stored-order residual page with count off and on. The count-on case must actually evaluate later candidates. Time the multi-field relation-FTS case as well. Use three short alternating baseline/candidate pairs. Label units explicitly. Record cache settings, routes, result digests, candidates, source files, bytes and parses. Separate cold execution from warm verified hits. Run Q126, Q136 and Q141 as focused parity and timing checks. Investigate a repeatable slowdown above baseline variation.
3. Append the implementation report to this addendum under `docs/ai/done/`. Include the before/after regressions, cold and warm matched fields, observed retained bytes and budget, restriction calls per input, the base reachability decision and focused timings. Run the existing `count_core_rust_lines.py` for both `891d377f32..final` and `d063a91c71..final`. Report production and test physical-line deltas separately and name the duplicate execution paths removed. The earlier repair added 53 net physical production lines. Explain any remaining growth in terms of observable behavior. Record the final code and report commits and any unmet acceptance condition.

## Implementation report

- Task: `query-execution-path-repair-addendum`
- Completed: 19 November 2026
- Starting/base commit: `d063a91c7167731d4bffb9252fafd74622421b4a`
- Final code commit: `6ff3e568bd`
- Integration branch: `integration/query-execution-path-repair`
- Integration worktree: `/home/simon/source/objekte-integration-query-execution-path-repair`

### Implemented repairs

`077079d66b` fixes exact local-OR evidence. When static evidence is not uniform, the ordered route now builds exact branch membership bitmaps for each segment. Matching fields are recovered per winning slot for emitted rows and for every verified-cache member, including off-page members. Unsupported branch layouts decline the ordered route before output. The regression starts from an empty enabled cache, cold-streams A with a one-row page, then requests B and C through a verified-cache hit. The baseline emitted A as `[]`; the repaired path emits A=`[a]`, B=`[b]`, C=`[a,b]`. Evidence off keeps IDs A/B/C with empty fields, and count-only evidence-off opens no source files.

`c73d1c9fc1` fixes residual-count page accounting. Ordered-row pages are cleared and deallocated after their callback, so the next candidate sees zero actual page rows and zero page bytes. Page replacements retain the page and keep its retained bytes in every later budget check. The regression observation saw the old ordered path retain one page row at the tail read. At the fixed boundary the ordered run observed zero rows, zero bytes and three tail reads. The replacement run observed one row and 4,230 actual retained bytes at a 3,678-byte budget, returned an accurate sequence-1/one-row resource error and did not publish a partial verified result. Callback cancellation produced zero tail reads. The owned-page path moved rows to the caller and observed zero engine-retained page rows.

`0e7e2d83e1` fixes duplicate segment restrictions. Exact stored-locator pages whose base is empty now select the exact-page executor before the ordered probe. The two-segment regression observed the old route perform 4 bounds evaluations and the fixed route perform 2, with one preparation and one sort binding in both the ordinary page and separate stream invocation. IDs, order, no-count completion and the stream fallback match the reference.

`03742d6a95` resolves populated flat-base reachability. Normal initial build, partial publication, open/reopen, mutation and compaction install active rows only as native segments. `Repository` fields are crate-private, its only builder is `pub(crate)`, and no supported public API returns a populated repository for `Index::from_repository`. A synthetic populated flat-base view may use the independent reference path, but optimized search explicitly rejects it. The mixed base contribution, base cache admission, ID-based base winner lookup, `search_active_with_prepared`, and `base_rank_key_merged` were removed. Public empty, partially published and segment-only views continue through the segment coordinator; empty-view valid query/count and invalid-query validation are covered.

`6ff3e568bd` adds a release benchmark probe for callback-level ordered timing and the multi-field relation case. It is an example outside `crates/core/src` and does not affect the source-size metric.

### Focused benchmark

Baseline executable `0323cc0a...` was built from `d063a91c`; candidate executable `c225750e...` was built from `03742d6a`. Both used four workers and the same 148,486-record corpus (`9f93a4ba...`). Three alternating pairs ran ASCII and Unicode cache-off/warm profiles with three query repetitions. Values are medians of the three per-batch medians in milliseconds.

| Query/profile | Baseline ms | Candidate ms |
| --- | ---: | ---: |
| Q126 ASCII off | 0.031332 | 0.028268 |
| Q126 ASCII warm | 0.007994 | 0.008099 |
| Q126 Unicode off | 0.039518 | 0.029660 |
| Q126 Unicode warm | 0.009107 | 0.008685 |
| Q136 ASCII off | 3.960411 | 3.548089 |
| Q136 ASCII warm | 1.605058 | 1.553367 |
| Q136 Unicode off | 3.979779 | 3.553498 |
| Q136 Unicode warm | 1.603618 | 1.563478 |
| Q141 ASCII off | 5.385385 | 5.057970 |
| Q141 ASCII warm | 0.024609 | 0.029127 |
| Q141 Unicode off | 6.460272 | 5.308823 |
| Q141 Unicode warm | 0.025060 | 0.029047 |

All baseline/candidate result digest files were identical (`10c91300...`). Q126 returned 203 matches with no source work. Q136 returned 133,948 posting-proven matches with no source work. Q141 returned 823 matches from 982 candidates, 982 files, 2,309,880 bytes and 982 parses. Warm profiles returned zero candidates/source work and three verified hits. The small warm-profile differences are below run variation.

The task-specific release probe used alternating `baseline/candidate/candidate/baseline/baseline/candidate` runs:

| Case | Baseline first/completion | Candidate first/completion |
| --- | ---: | ---: |
| Q141 Unicode count off | 13.992 / 14.008 ms | 14.030 / 14.045 ms |
| Q141 Unicode count on | 13.401 / 13.416 ms | 13.551 / 13.568 ms |
| Multi-field relation FTS | 0.020640 ms | 0.020463 ms |
| Stored residual count off | 0.031264 / 0.033718 ms | 0.039562 / 0.042745 ms |
| Stored residual count on | 0.023604 / 0.815386 ms | 0.025973 / 0.839709 ms |

For stored residual count-on, the first callback observed one match while completion scanned all 1,000 candidates and counted 500, proving later count work. Count-off stopped after the one-match page. Q141 digests matched baseline in every run. The sub-millisecond and low-millisecond differences are within host variation; no repeatable slowdown was found.

### Validation

- `cargo test --locked -p objekte-search-core --all-features`: 628 passed, 16 ignored, 0 failed; all integration and doc tests passed.
- `cargo test --locked --workspace`: passed for core, CLI and desktop.
- `git diff --check`: passed.
- `cargo fmt --all -- --check`: only the pre-existing extra blank lines in `crates/core/src/model/json.rs` remain, identical to the starting tip. All task-owned files pass focused `rustfmt --edition 2024 --check`.

### Source-size delta

The repository's physical-line classifier reports:

- `891d377f32..6ff3e568bd`: production 614 added, 624 removed, **−10 net physical lines**; tests 839 added, 30 removed, **+809 net physical lines**.
- `d063a91c71..6ff3e568bd`: production 141 added, 204 removed, **−63 net physical lines**; tests 562 added, 265 removed, **+297 net physical lines**.

Cumulative production growth from the original repair is no longer positive. The addendum changes reduce production text while adding branch-specific evidence, retained-page accounting, restriction reuse and the reachability invariant.

### Delivery and cleanup

Pushed commits on the cumulative integration branch:

- `077079d66b` — Preserve exact OR evidence per record
- `c73d1c9fc1` — Keep residual count page memory charged
- `0e7e2d83e1` — Select exact stored pages before ordered probing
- `03742d6a95` — Retire unsupported flat-base search dispatch
- `6ff3e568bd` — Benchmark ordered callback and relation paths

The temporary baseline worktree was removed. The existing integration worktree and branch are retained for owner review and merge. No acceptance condition remains unmet.
