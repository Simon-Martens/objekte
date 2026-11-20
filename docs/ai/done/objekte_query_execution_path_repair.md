# Objekte query execution path repair

## Assignment

Fix per-record matched fields in relation full-text search. Remove repeated query preparation, count dispatch and hit copying from ordered page delivery. Consolidate base-record and segment search wherever both own the same request-level decisions. Deliver one integration branch for the owner to merge.

The reviewed `master` revision is `891d377f32f601d0234776fb05995504ff1783bf`. Start from current `origin/master`, record the actual branch point and verify the named paths if it has advanced. Follow `docs/ai/README.md`. Use `integration/query-execution-path-repair` in the sibling worktree `/home/simon/source/objekte-integration-query-execution-path-repair`. Make the evidence correction independently reviewable, then integrate the execution changes. [Reviewed source](https://github.com/Simon-Martens/objekte/tree/891d377f32f601d0234776fb05995504ff1783bf/crates/core/src)

**Prepared-query lifetime.** The reviewed core builds fresh bindings in `query::prepared::prepare`. The CLI type named `PreparedQuery` carries a parsed desktop request into one execution. An archived benchmark harness mentions `prepared_cache_bytes`. The production workspace has no cross-request prepared-query cache. Bind once when an execution needs bindings, borrow them across that execution, then drop them. Separate identical requests bind independently when the verified-result cache is disabled. Preserve existing verified-result hits, including any path that can return before binding. If the implementation branch has since acquired a cross-request prepared-query cache, remove its storage, lookup, insertion and production configuration. Keep the verified-result cache, which stores completed membership and ordering. [Core preparation](https://github.com/Simon-Martens/objekte/blob/891d377f32f601d0234776fb05995504ff1783bf/crates/core/src/query/prepared.rs#L77-L94) · [Verified results](https://github.com/Simon-Martens/objekte/blob/891d377f32f601d0234776fb05995504ff1783bf/crates/core/src/query_results/verified.rs#L7-L38)

## R1. Preserve matched fields for each relation hit

**Defect.** `qualified_fts_witnesses` already produces a set of matching field paths for each `SegmentRecordAddress`. In `try_semijoin_relation_page`, those sets are combined into one vector per segment. `exact_ordered_page` copies that vector to every emitted hit from that segment. A record outside the requested page can therefore change the highlights on a visible record. The same caller also treats unavailable static filter evidence as an empty list. An exact membership bitmap can prove that an OR matched without saying which branch matched a given record. [Witness production](https://github.com/Simon-Martens/objekte/blob/891d377f32f601d0234776fb05995504ff1783bf/crates/core/src/query_access/relations/semijoin.rs#L1035-L1108) · [Page selection](https://github.com/Simon-Martens/objekte/blob/891d377f32f601d0234776fb05995504ff1783bf/crates/core/src/query_execution/runner/relations.rs#L1135-L1254) · [Hit construction](https://github.com/Simon-Martens/objekte/blob/891d377f32f601d0234776fb05995504ff1783bf/crates/core/src/query_execution/runner/mod.rs#L1347-L1361)

1. Keep membership bitmaps for counting and ordering. Let the ordered page obtain evidence for the particular winning record. Preserve the full source address, including collection and segment, when looking up witnesses. Reuse the existing address-to-fields map. Materialize owned strings only as needed for returned evidence. The shared exact-page code also serves callers with genuinely uniform evidence. Keep their cheap constant-field case.
2. Include local filter evidence when requested. Reuse `segment_static_evidence_fields` only where it proves a common set. For row-dependent evidence, use the existing per-record evaluator for the winners or decline this optimization before emitting rows and use the evidence-correct route. Avoid building additional field lists in the evidence-off and count-only paths. Routes using the verified-result cache must return the same fields.
3. Add a regression to `crates/core/src/query_access/relations/semijoin/tests.rs`. Put three source records in one segment with forward relations `author` and `editor`. Search for one token in `author.name` and `editor.name`. Arrange the targets so A matches only the first, B only the second, and C both. With locator output and evidence enabled, expect A → `{author.name}`, B → `{editor.name}`, C → both, count three. Select A alone with a one-row page and assert the same fields. Disable verified-result admission to exercise the optimized semijoin. Confirm through a route probe that it supplied the result. Assert literal expected sets and compare with the independent reference scan. Show the first assertion failing on the branch point. Extend the fixture with an exact local OR whose true branch differs by row and a segment with reused local slot numbers. Check evidence off yields identical IDs and empty field lists.

## R2. Prepare and execute an ordered request once

**Current path.** `search_segmented_impl` binds the sort and prepares the query. `try_lifecycle_ordered_page` probes filter bounds, clears the count flag and constructs a streaming sink that clones hits from borrowed events. `try_lifecycle_ordered_stream` binds again and recomputes bounds. Both the page wrapper and the public streaming path have a count branch that can invoke `search_segmented_with_context_lifecycle` again. The public stream entry point also validates and prepares before calling the ordered helper. Address all of these call sites. [Page wrapper](https://github.com/Simon-Martens/objekte/blob/891d377f32f601d0234776fb05995504ff1783bf/crates/core/src/query_execution/runner/mod.rs#L1602-L1757) · [Ordered stream](https://github.com/Simon-Martens/objekte/blob/891d377f32f601d0234776fb05995504ff1783bf/crates/core/src/query_execution/runner/mod.rs#L3439-L3526) · [Stream count and entry point](https://github.com/Simon-Martens/objekte/blob/891d377f32f601d0234776fb05995504ff1783bf/crates/core/src/query_execution/runner/mod.rs#L3996-L4158)

| Responsibility | Required behavior |
| --- | --- |
| Preparation | Use the same frozen clock, bound sort, prepared predicates, term IDs, execution context and pinned view across eligibility, traversal and count. Pass prepared inputs to fallback after a declined route. Keep lazy work lazy for cache hits and count-only operations. |
| Segment selection | Test inexpensive eligibility conditions first. If eligibility needs filter bounds, pass the computed bounds and exactness onward. Compute a given restriction once for the same term and input domain. Narrower inputs can require different bounds. Borrow immutable proofs and bitmaps wherever possible. |
| Page delivery | Give the traversal a direct path to return its owned hit vector. Let streaming borrow rows for the duration of the callback. Remove the synthetic sink and the hit clones it creates. Preserve ordered batches, replacement snapshots, stable tie order, offsets and the final sequence. |
| Count | Use exact bitmap cardinalities when available. For residual predicates, continue with membership-only work or use an existing cheaper count operation on the same prepared inputs. Reuse established proofs and verified membership. Avoid materializing payloads or sorting the unreturned tail solely for the count. Remove both recursive count dispatches from the ordered path. |
| Completion | Without a count, stop once the requested ordered page is complete where the existing contract permits. With a count, emit ready rows before finishing a residual count. Honor callback stop, cancellation, and resource limits across both phases. Preserve source-unavailable or changed-source outcomes when payload delivery encounters them. A locator-only count cannot upgrade incomplete evidence of success. |
| Accounting | Preserve the meanings of public work counters, count completeness and trace phases. Charge actual page and count work once. Keep distinct-file accounting correct across phases. Carry the real observations through completion instead of replacing page counters with counters from a second request. |

**Acceptance.** Extend the existing runner and streaming fixtures. For an eligible ordered request with the verified cache disabled, use the existing test-only `production_prepare_calls()` delta to show exactly one preparation for `search` and for a separate streaming invocation. Repeat an identical request and show it prepares afresh. Cover count on/off and a probed route that declines. A targeted test probe should also establish one sort bind and one bounds calculation per identical input on the formerly duplicated route. A second public search dispatch for an exact count or a streaming-event-to-owned-hit clone fails review even if the output is correct.

Use enough records to fill a small page well before exhaustion. Check an exact indexed predicate and a residual predicate. Assert ordered hits and payloads against reference for locator, bytes and logical modes. Check early stop with count off, first row batch before residual count completion, exact totals with count on, zero-limit counts, deep offsets, replacement snapshots and all-results batching. Exercise cancellation after the first batch, a tight resource budget, and an unavailable source during bytes or logical output. Reuse existing cases for these contracts and add only missing coverage. [Early-delivery test](https://github.com/Simon-Martens/objekte/blob/891d377f32f601d0234776fb05995504ff1783bf/crates/core/src/query_execution/streaming/tests.rs#L684-L729)

## R3. Give base and segment searches one coordinator

**Current path.** `Repository::search` and the published lifecycle view independently choose a base or segmented runner. On a mixed view, `search_segmented_impl` clones the query, adjusts the offset and limit, calls the full base search for its top candidates and merges the result with segment winners. A storage choice therefore controls preparation and pagination at two levels. [Repository dispatch](https://github.com/Simon-Martens/objekte/blob/891d377f32f601d0234776fb05995504ff1783bf/crates/core/src/repository/search.rs#L13-L31) · [Published dispatch](https://github.com/Simon-Martens/objekte/blob/891d377f32f601d0234776fb05995504ff1783bf/crates/core/src/lifecycle/view.rs#L627-L655) · [Mixed execution](https://github.com/Simon-Martens/objekte/blob/891d377f32f601d0234776fb05995504ff1783bf/crates/core/src/query_execution/runner/mod.rs#L2010-L2051)

1. **Establish reachability first.** Trace production constructors and readers from initial publication through completed build, repository adoption, supported load/reopen operations, mutation and compaction. Record which can produce active base records, active segment records, both, or an empty view. Distinguish compiled production uses from fixtures and the independent reference scanner. A compatibility generation containing metadata does not establish an active base search domain. Start at `construction/initial.rs`, `construction/repository_state.rs`, `lifecycle/open.rs` and `lifecycle/publication.rs`. Include supported historical input formats in this check where applicable.
2. **If populated base records are supported**, have the repository and published-view optimized entry points call one coordinator. It owns preparation, the global offset/limit, ordering, count completeness, result delivery and request-level cache decisions. Let each storage representation provide candidates, order, verification and qualified record identity through the narrowest existing internal interface. A local top-`offset + limit` can still be an efficient contribution. The base operation must accept the prepared request and feed the common merge without entering another complete search or constructing a second public page. Reuse existing ordering, ranking, cache and materialization code. Keep per-segment borrowing, pruning and early termination.
3. **If only empty base views are reachable in production**, use the common coordinator for the empty case, including ordinary query validation, then retire the unreachable optimized runner and its dispatch after checking callers. Preserve any code required by the supported reference API or compatibility readers. If a populated base path is confined to a supported legacy reader, that is a supported base path and takes the preceding implementation branch.
4. **Demonstrate parity and deletion.** Exercise empty and segment-only views through the public entry points. For supported base-only and mixed views, include overlapping local slot numbers, tied sort keys, offsets spanning partitions, retired records and count on/off. Compare full ordered identities, matched fields and payloads with explicit expected rows or the independent reference evaluation. Verify repository, pinned view, and context-aware entry points. Check partial publication retains its availability and count semantics. List retired functions and their replacements. A new dispatcher forwarding to both complete runners leaves this repair unfinished.

## Validation and delivery

- Run `cargo fmt --all -- --check`, `cargo test -p objekte-search-core --all-features` and `cargo test --workspace`. Keep the reference evaluator independent so shared defects cannot make equivalence tests pass.
- Compare baseline and candidate release binaries on the same data, configuration, worker count and cache state. Use a focused Musenalm subset from `crates/core/benches/musenalm_queries_extended.json`. Q126 covers a shallow ordered filter, Q136 a deep offset, and Q141 filtered FTS with sorting. Confirm the selected routes. Include the new multi-field relation case and a residual sorted page with count both off and on. Distinguish cold verified-result misses from warm hits. Alternate baseline and candidate in three short batches on the same host. Report median time to first rows and completion where applicable, along with candidates, files opened, bytes read and record parses. Investigate repeatable regressions beyond measured baseline variation. A fallback that turns an indexed page into a full scan requires correction.
- Quantify the change using the repository's existing `docs/ai/generated/query-engine-deduplication-repair/count_core_rust_lines.py --from <branch-point> --to <final-code-commit>`, run with Python. Report net production and test deltas separately, plus the full execution paths removed. The review measured 66,795 nonblank, noncomment production Rust lines at the pinned revision. The repository script measures physical line deltas. Report those measures with their units. Aim for a net production reduction and explain any growth against the behavior it implements.
- Append the implementation report to the completed specification under the repository workflow. Include the baseline failure and passing regression, the base reachability findings, preparation counts, focused benchmark results, source-size delta, final commit and any unresolved condition. Deliver the integration branch for review.

## Implementation report

- Task: `query-execution-path-repair`
- Completed: 19 November 2026
- Base commit: `891d377f32f601d0234776fb05995504ff1783bf`
- Integration branch: `integration/query-execution-path-repair`
- Integration worktree: `/home/simon/source/objekte-integration-query-execution-path-repair`

### Delivered changes

`c2c5b3a948` makes relation FTS evidence independently reviewable. The qualified witness map is keyed by the complete `SegmentRecordAddress`; ordered pages look up evidence only for the winning physical record. Uniform static evidence remains the cheap constant case. Row-dependent exact-member membership declines the optimized page before output and follows the evidence-correct route. The regression covers A/B/C branch-specific evidence, a one-row page, local OR evidence, reused local slots, cache-on/off output, evidence-off IDs, and the independent reference scanner.

`4280acdf8d` executes an ordered request with one preparation. `try_lifecycle_ordered_page` now returns an owned hit vector directly instead of constructing a synthetic stream and cloning callback rows. Page and streaming callers share the same bound sort and prepared predicates. Eligibility bounds are computed once in the ordered traversal. The recursive locator count requests are gone: exact bitmap cardinality is used when complete, while residual counts continue in the same traversal without materializing the unreturned page payloads or evidence. Replacement streams continue checking source availability after the visible page is filled. Cancellation, sink stop, resource failures, cache publication, public counters and source-unavailable outcomes remain distinct.

The test-only preparation probes establish one prepared-query call, one production sort bind and one predicate-bounds calculation for exact search, exact streaming and residual streaming. Repeating an identical request prepares again. A declined ordered route reaches fallback with the existing bindings and still prepares only once. Existing tests cover locator/bytes/logical payloads, early delivery before residual count completion, exact totals, zero-limit count, deep offsets, replacement snapshots, all-results batching, cancellation, resource limits and unavailable source behavior.

`f585c142c1` gives base and segment storage one coordinator. Reachability established that normal initial build, completed build/open/reopen, mutation and compaction publish active native segments with an inactive base domain. The public `Index::from_repository` adoption path can instead receive a populated flat base with no native segments, so a coordinator is required rather than retirement of the base runner.

Repository search, context-aware search, explain/analyze, and streaming now enter the same segmented coordinator with the actual active base and segment domains. Repository and pinned-view dispatch no longer chooses between two complete search runners. For a mixed view, the coordinator passes its bound sort and prepared predicates into the base contribution, which returns only its local top contribution and shared cache membership. The common merge owns offset, limit, ordering, count completeness, payload delivery and cache publication. Base-only and mixed lifecycle cache entries are published from the same complete traversal.

The full paths removed are the page wrapper's synthetic streaming delivery and borrowed-event-to-owned-hit clone loop, both recursive ordered count searches, repository/view base-or-segment search dispatch, repository/view base-or-segment explain dispatch, and the nested complete base search inside mixed execution. Internal base context wrappers with no remaining production callers were removed; the test/reference base search helpers remain. No supported public API or independent reference path was retired.

R3 tests cover a populated flat base through direct Repository calls, repository adoption, pinned view, context-aware search and streaming. Mixed-view tests cover overlapping base/segment local slots, computed tied sort keys, offsets across partitions, count on/off, locator/bytes/logical modes, cache cold/warm behavior, pinned-view and context-aware calls, and page streaming. Existing lifecycle tests continue to cover retired records, partial publication and source availability.

### Baseline failure and passing regression

At branch point `891d377f`, only the new semijoin regression was applied in a disposable worktree. The first full-hit comparison failed because S_A and S_B both reported `["author.name", "editor.name"]` while the independent reference expected only their individual branches. The integration worktree's passing regression is:

`cargo test --locked -p objekte-search-core query_access::relations::semijoin::tests::relation_fts_page_evidence_is_keyed_by_the_winning_segment_address`

### Validation

- `cargo test --locked -p objekte-search-core --all-features`: 624 passed, 16 ignored, 0 failed; all integration and doc tests passed.
- `cargo test --workspace`: passed for core, CLI and desktop, including 81 CLI unit tests, 33 CLI process tests, 161 desktop unit tests and the core all-feature-equivalent suites. No inherited desktop failure reproduced.
- `cargo fmt --all -- --check`: the only failure is the pre-existing formatting diff in `crates/core/src/model/json.rs` (extra blank lines). All task-owned files pass `rustfmt --edition 2024 --check`, and `git diff --check` passes.

### Focused Musenalm comparison

Baseline binary `806df616...` was built from `891d377f`; candidate binary `3c42fa5a...` was built from `f585c142`. Both used the same 148,486-record corpus (`9f93a4ba...`), four query workers, source cache off, and ASCII/Unicode verified-cache profiles off or 64 MiB warm. Six short runs alternated baseline/candidate order for three batches. The following values are the median of the three per-batch medians; each batch median came from three timed repetitions.

| Query/profile | Baseline ms | Candidate ms | Change |
| --- | ---: | ---: | ---: |
| Q126 ASCII off | 0.038221 | 0.033232 | -13.05% |
| Q126 ASCII warm | 0.008377 | 0.008189 | -2.24% |
| Q126 Unicode off | 0.034331 | 0.036554 | +6.48% |
| Q126 Unicode warm | 0.008511 | 0.008389 | -1.43% |
| Q136 ASCII off | 4.423588 | 3.927365 | -11.22% |
| Q136 ASCII warm | 1.509022 | 1.556802 | +3.17% |
| Q136 Unicode off | 4.505450 | 4.164164 | -7.58% |
| Q136 Unicode warm | 1.536481 | 1.592231 | +3.63% |
| Q141 ASCII off | 6.480462 | 5.889090 | -9.13% |
| Q141 ASCII warm | 0.028787 | 0.024405 | -15.22% |
| Q141 Unicode off | 5.118190 | 5.986256 | +16.96% |
| Q141 Unicode warm | 0.027144 | 0.025131 | -7.42% |

Q126 returned 203 matches and selected 50 locator rows with 203 candidates, zero files, bytes and parses, and zero residual evaluations. Q136 returned 133,948 matches and 100 rows with 133,948 exact candidates and zero files, bytes and parses. Q141 returned 823 matches and 50 rows from 982 candidates, opening 982 files, reading 2,309,880 bytes, parsing 982 records and evaluating no unproven residual. Warm profiles returned zero candidates and source work with 3/3 verified-result hits. Baseline and candidate result digests were identical in every batch.

The maintained `bench_musenalm` checkpoint reports completed ordinary search latency; it has no separate time-to-first-row column. Streaming callback ordering is covered by the new early-delivery test, which observes rows before the residual count completes. The multi-field relation and residual count-on/off cases are covered by deterministic reference tests rather than a separate maintained corpus timing case.

The one over-threshold cell, Q141 Unicode cache-off, was rechecked in alternating seven-repetition runs: baseline/candidate medians were 4.819/5.605 and 5.297/4.345 ms in the two pairs, followed by an 11-repetition baseline/candidate pair of 5.020/4.359 ms. The direction reversed, with identical route work and results, so no repeatable implementation-caused regression remains.

### Source-size delta

At code commit `f585c142`, `count_core_rust_lines.py` reports physical lines:

- Production: 537 added, 484 removed, **+53 net physical lines**.
- Tests: 519 added, 7 removed, **+512 net physical lines**.

The review's independent baseline measure is **66,795 nonblank, noncomment production Rust lines** at `891d377f`; the repository script's delta above is physical-line based and is not that metric. Production growth is the narrow coordinator interface, prepared base contribution and shared cache publication needed to preserve supported flat-base and mixed-view behavior while removing the duplicated complete-search dispatches. The gain is concentrated in tests and in deleted execution paths rather than net physical production text.

### Delivery

Implementation commits:

- `c2c5b3a948` — Preserve per-record relation FTS evidence
- `4280acdf8d` — Execute ordered queries with one preparation
- `f585c142c1` — Coordinate base and segment search once

All commits and this completed task record are on `integration/query-execution-path-repair` and pushed to `origin`. The integration worktree is retained for review and merge. No unresolved implementation condition remains.
