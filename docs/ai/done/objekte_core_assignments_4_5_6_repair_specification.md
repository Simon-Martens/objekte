# Objekte core repair specification for Assignments 4, 5 and 6

**Repository** `Simon-Martens/objekte`
**Starting revision** `601a8e597513701d5933094eece7b0ad9ce555e2` (verified GitHub `master`, 19 November 2026)
**Task** `core-query-and-lifecycle-repairs`
**Delivery branch** `integration/core-query-and-lifecycle-repairs`

Deliver one integration branch. Five findings concern query execution and one concerns concurrent shutdown. Assignment 4 supplies the reader contract for this work and requires regression verification. Implement the sections in order. The owner will review and merge the completed branch.

## 1. Establish the baseline

Use the local repository at `/home/simon/source/objekte` and a sibling integration worktree. Follow `docs/ai/README.md` for task notes, commits, immediate pushes to the integration branch and the final report. For a fresh start:

```sh
cd /home/simon/source/objekte
git fetch origin
repair_base=601a8e597513701d5933094eece7b0ad9ce555e2
git rev-parse --verify "$repair_base^{commit}"
git merge-base --is-ancestor "$repair_base" origin/master
git worktree add -b integration/core-query-and-lifecycle-repairs \
  ../objekte-integration-core-query-and-lifecycle-repairs "$repair_base"
cd ../objekte-integration-core-query-and-lifecycle-repairs
git rev-parse HEAD
```

Check the exit status of each command. If resuming this task, inspect its existing worktree, commits and notes and continue there. If `origin/master` has advanced, record its full SHA and the divergence. The SHA above is the reproducible base for the repair and its tests. Keep work off the original checkout and the previous repair branches. Publish the integration branch using ordinary pushes.

Read the A4, A5 and A6 specifications and repair reports in `docs/ai/done/`, including `assignment_5_reference_sorting_followup.md`, `assignment_5_reference_sort_execution_repair.md` and `assignment_5_reference_sort_collation_repair.md`. All implementation changes from those deliveries are already ancestors. Commit `9cfd8b2c906caef24cb9214c6b5fa2b7e4f14efe` adds the combined validation report to the starting revision. Read its evidence with `git show`; begin implementation at the specified master commit. [Baseline](https://github.com/Simon-Martens/objekte/commit/601a8e597513701d5933094eece7b0ad9ce555e2) · [Historical combined test results](https://github.com/Simon-Martens/objekte/blob/9cfd8b2c906caef24cb9214c6b5fa2b7e4f14efe/docs/ai/done/assignment_5_reference_sort_collation_repair.md#L79)

For each defect, first run a regression that exposes the baseline behavior, then apply the fix and run the same test. During development, tests and fixes may be staged together in one buildable commit per section. Use fresh pins/requests and disable the verified result cache for execution-path tests. Assert hand-derived answers before comparing optimized and reference results. The paths and line numbers below are relative to this baseline.

## 2. Assignment 4 — verify the reader contract

Start by running `source::registry::tests::custom_readers_under_builtin_names_never_use_builtin_scalar_verification`. Check its positive execution count. The fixture installs non-JSON readers under `json`, `jsonl`, `json_decoded_scalars` and a neutral ID. It verifies the actual custom reader runs for residual FILTER, field FTS and the intersection, with expected IDs, evidence and payload. The installed JSON and JSONL readers resolve to `ResolvedStrings`; the decoded-scalar reader resolves to `DecodedScalars`. The custom registrations resolve to no built-in scalar capability.

Use the pinned collection's prepared reader and schema everywhere this pass reads a logical record, including relation sources and targets. Keep capability selection at registration resolution, the borrowed scanner paths, fused traversal, and the existing seven root-file removals. Add an A4 production change only if a new regression identifies a concrete reader failure. Keep its evidence separate from the A5 read-accounting tests. [Registry](https://github.com/Simon-Martens/objekte/blob/601a8e597513701d5933094eece7b0ad9ce555e2/crates/core/src/source/registry.rs#L118) · [Custom-reader fixture](https://github.com/Simon-Martens/objekte/blob/601a8e597513701d5933094eece7b0ad9ce555e2/crates/core/src/source/registry/tests.rs#L151)

## 3. Assignment 5 — repair the query paths

### 3.1 Use the pinned publication for every reference search

`lifecycle/publication.rs:92–99` under `crates/core/src/` transfers the active and segment vectors out of an adopted repository. `lifecycle/view.rs:1047–1064` passes the correct publication for a view containing native segments. Its base-only branch passes the active masks to `repository.rs:1635–1651`, which obtains the segment vector, catalogue and revision from the underlying repository. The merged runner checks the number of collection domains at `query_execution/runner/relations.rs:2659`. A valid base-only view can therefore fail with `segment query collection count mismatch`.

1. Use one `IndexView::search_reference` dispatch for both layouts. Check initial completion, resolve the request clock once, then supply the view's repository, active masks, segment groups (including empty groups), catalogue and revision to the existing reference evaluator. Stamp hits and remap diagnostics through the view once. Preserve the standalone `Repository::search_reference` and parameterized APIs using the state owned by their receiver. Inspect callers before removing any internal helper.
2. Keep parameter substitution before clock resolution and binding. A request containing a clock macro must use one instant across all collections and rows. Preserve the borrowed request for macro-free queries. Keep visibility, identity resolution, related evaluation, sort keys and payload addressing independent of production postings, ID maps, order columns and proof caches.
3. Extend the base fixture in `relation_query.test.rs:2023`. First obtain the expected standalone result. Then consume the repository via public `Index::from_repository`, acquire a view, and call its public reference API. Assert that it really is a nonempty base generation with empty native groups. Query a sorted pair with known IDs and evidence and check count, page, file locator, generation, logical payload and original bytes across requests. The baseline failure must occur on the adopted view.
4. Pin that view, publish a logical retirement through the index while retaining the source bytes, and compare it with a fresh pin. The retired row must remain visible on the old pin and disappear on the new pin. Assert revisions on hits. Include a catalogue-sensitive case using the content-preserving move fixture or a test-local publication with a changed file catalogue entry. Assert the new pin uses the new path; an old pin retains its own path and follows the normal missing-source contract if the backing file moved. Also test a configured empty collection and retain the existing segmented parameter/frozen-clock regression and public unresolved-macro smoke test.

### 3.2 Guard reuse of stored related sort keys by collation

`query_results/ranking.rs:592–644` reuses stored ICU key bytes for an eligible one-hop sort without checking the terminal column's collation. `query_execution/runner/relations.rs:563–571` already performs a compatibility check in a different optimized route. Sharing ICU key bytes is valid only when both sides use the same collation settings.

1. Preserve the distinction between sorting a configured local field and sorting through a related expression. A simple local field uses its configured override. The existing related expression uses the terminal collection's default sort configuration. Bind both optimized and reference calculations to those semantics.
2. Before borrowing the key in the merged ranking shortcut, verify that the actual column's locale and complete sort options match the effective related expression configuration. Check both base and segment columns. Reuse the existing compatibility rule across the two optimized routes where practical. On mismatch, decline that shortcut and use the existing computed evaluator. On a match, retain stored-key reuse without reading or decoding the target. Keep direction, tie-breaking and null/missing placement consistent. Keep ICU collator construction out of optimized stored-key binding.
3. Use two source rows `S2 → T2` and `S10 → T10` in a declared to-one relation `entry`. Give the targets titles `item2` and `item10`, with English nonnumeric defaults and a numeric stored override on `title`. Query the **source** collection, `SORT BY entry.title ASC`, with no related predicate, first with limit 1 and then with a full page. Expected IDs are `[S10]` and `[S10, S2]` with count 2. Sorting the **target** collection directly by `title ASC` must return `[T2, T10]`. Verify each through optimized and reference public APIs. Cover a segment target and the base-column selection arm via a mixed publication or a focused owner-local test.
4. Repeat with compatible configurations. Prove the general merged ranking route still obtains a stored key and incurs zero sort-target reads. Retain `lifecycle_related_sort_uses_target_stored_column` as the control for the other optimized route. Use positive route evidence or a scoped probe if another optimization could make the assertion pass without exercising the repaired code.

### 3.3 Restrict forward sort shortcuts to forward hops

`cached_filter_target_sort_key` and `direct_related_target_sort_key` in `query_results/ranking.rs:525–645` call `forward_edges`. A backlink's declaration ordinal belongs to the referencing collection. Looking it up on the ranked target can report a missing relation or use an unrelated relation at the same ordinal.

1. Require `hop.direction == HopDirection::Forward` before either helper touches a forward contribution. Keep the one-hop, to-one, terminal-field, transform, selector/position and projection conditions appropriate to each shortcut. If the shortcut cannot prove its edge satisfies the bound occurrence selectors, use normal traversal. Resolve backlinks using the normal backlink walker and its source-ID ordering.
2. Create target records `A` and `B` with **no outgoing relation**. Declare `mentions.entry → entries` as cardinality one with reverse alias `mentions`. Add `m1 → A` titled `Zulu`, `m2 → B` titled `Bravo` and `m3 → A` titled `Alpha`. Query targets with `SORT BY @backlinks.mentions[0].title ASC`. Identity order makes `m1` the first source for A. Expect `[B, A]`, count 2, and `[B]` at limit 1. Use the existing typed sort expression API if necessary to isolate ranking from parser spelling; use parser-supported syntax for the public test. Exercise optimized and reference execution.
3. Exercise both shortcuts. A sort-only query reaches the direct-key case. For the cached-filter case, ensure its field-projection predicate is satisfied (use a related field-to-field comparison patterned on `related_sort_does_not_reuse_an_incomplete_filter_target_projection`, or call the helper in an owner-local test with such a bound filter). Assert it declines before reading any forward relation. Include a target with an unrelated outgoing relation at the same ordinal as a wrong-answer control. Preserve the existing compatible forward-key and projected-cache reuse counters.

### 3.4 Make incomplete backlink discovery deterministic

The reference lookup at `relation_query.rs:604–652` caches known source addresses. An unreadable eligible logical source means its outgoing IDs are unknown. The ranking loop at `query_execution/runner/relations.rs:2172–2186` currently infers row failure from an increase in a request-wide diagnostic count. Only the first target triggers discovery. The assertion at `relation_query.test.rs:1728–1745` consequently expects only B after one unrelated source file disappears.

**Required result policy.** A ranked row that actually depends on an incomplete backlink lookup has an unverified sort key. Exclude that row from the ranked page and from the tally of verified ranked rows. Apply the rule to every dependent row, including cache hits. Keep independently rankable rows. Return the existing incomplete-count signal and the underlying source diagnostic. Before reading the missing file, the reference evaluator cannot establish which targets it referenced. A zero-length backlink list or a list with a known minimum is insufficient to establish a complete ordered key.

1. Store completeness alongside the request-local address map, per relation and pinned view. Track failure while visiting eligible, visible logical sources. Normal raw-fallback exclusions, known empty arrays and absent relation values are handled by their existing semantics. Preserve ordering by source identity, deduplication per source/target and streamed decoding with bounded retained memory. One unreadable logical source adds one discovery diagnostic on the first actual read failure. Cache hits report the stored completeness without rescanning or replaying diagnostics.
2. Make reference ranking explicitly propagate an unverifiable outcome through nested traversal, reducers and multiple sort terms to the row admission decision. Keep ordinary missing/null/dangling keys distinct from an unavailable input file that prevents a ranking decision. Keep existing handling of other source errors. Preserve FILTER/FTS sharing of the lookup and available witnesses. A lookup first populated while evaluating a predicate must still carry its completeness when sorting later. Preserve ordinary query errors, resource errors and cancellation.
3. Extend `reference_backlink_sort_streams_discovery_without_target_cache_budget`. With the existing healthy fixture require ordered IDs `[A, B]`, count 2, one discovery read per visible logical source across both targets and the existing three actual sort-target reads. After removing the single large unlinked file, both target keys depend on the incomplete lookup. Require `hits=[]`, `count=None`, `total_matches=0`, `match_count_complete=false` and a diagnostic identifying the missing path. Compare two **actual** target visitation orders (e.g. arrange stable IDs A and B in opposite physical traversal order in a second fixture and verify visitation with a test-local probe). Reversing the requested sort direction alone does not change discovery order.
4. Exercise reuse after a predicate or owner-local prelookup has already built the partial map, a target with no known sources, and a control lookup for a different complete relation. Verify a positive independent result stays rankable. Keep the existing healthy large fixture's sort-cache-budget control and its discovery/read counters. Keep diagnostics bounded at the existing limit.

### 3.5 Reuse decoded forward sources and account for actual reads

`query/sort_binding.rs:613–759` receives an optional decoded source at each hop. The ordinary reference forward branch calls `reference_forward_edges(address, relation, None)` even when it already has that record. `relation_query.rs:525–584` can then open and decode the same record a second time without charging its work.

1. When the source value is supplied, extract the declared ID occurrences directly from that borrowed logical value. Reuse the exact source-based extractor used by reference forward traversal. Preserve selectors, duplicates, logical null/missing behavior, custom-reader conversions, and the separate source occurrence order needed by FIRST/LAST. Keep production edge contributions out of the reference path.
2. For an ordinary forward intermediate with no supplied value, reuse an available complete, appropriately decoded cached value or load it through the existing source reader with live relation work accounting. Charge actual source reads/bytes to the relation source counters, apply read limits and cancellation where a context exists, and fold the physical opens, parses and bytes into the aggregate counters **once**. A borrowed value or cache hit adds zero physical reads. Terminal key materialization retains its existing sort-target classification and cache budget. Preserve the existing distinction between unavailable-source diagnostics and hard query errors. Keep counters that describe logical operations distinct from physical I/O.
3. Use a filter-free locator query, one logical record per file, a single selected source collection, a fresh request, and no unrelated source reads. In the one-hop case, use two roots pointing to different readable targets with distinct keys. Assert the exact order and count, one decode per root, zero forward source rereads and two sort-target reads. Counters alone cannot expose the old uncharged reread. Use a per-fixture counting reader or scoped read probe that identifies each path/record; reset observations after build. Check result byte totals against actual record-range lengths.
4. Add a two-hop sort where two distinct roots `R1` and `R2` point at the same small intermediate and it points at one terminal with a data-field sort key. Expected IDs at equal key are `[R1, R2]`. Require two original root reads, one intermediate relation-source read, one terminal sort-target read, and total bytes equal to the sum of those four record ranges. Verify warm reuse on the second root, no duplicate charging, and no new retained source set proportional to the number of roots. Keep the removed-intermediate and removed-terminal diagnostic paths covered with fresh requests.

## 4. Assignment 6 — make concurrent shutdown callers wait for the same worker

`lifecycle/service.rs:20–35` takes the maintenance job from its admission slot before waiting. While the first caller is blocked, a second `quiesce()` or `shutdown()` can observe an empty slot and return. Quiescence prevents publication, but the caller can still proceed with teardown while the worker is alive.

1. Set the quiescing flag under the publication write lock and release that lock. Stop the timer using its current protocol. Under the maintenance admission mutex, clone the current job's `Arc` and release the mutex. Each caller that observes the job requests cancellation and waits through `MaintenanceTask::wait`. Keep the shared job published in the slot until its worker has been joined.
2. After the wait, reacquire the admission mutex briefly and clear the slot **only if it still holds the same job** (`Arc::ptr_eq`). This cleanup applies to success, cancellation and a terminal worker error after the join. Preserve lock-poisoning and worker error reporting. A delayed waiter must never remove a newer job admitted after resume. Repeated calls after cleanup must retain the existing behavior for completed work.
3. Keep joins outside the publication, admission and result locks needed by the worker. Retain the handle-slot mutex across taking/joining the worker so that concurrent waiters synchronize with the actual join. Keep separate reconciliation admission and its cancel/wait sequence, maintenance/reconciliation error precedence, and `shutdown` delegation. Use the existing locks and task handles. Keep resume semantics and worker count. Preserve expected-revision and quiescence checks inside the publication write lock and retired-root destruction after unlock.
4. Add a deterministic concurrency test beside `lifecycle/maintenance/tests.rs`. Use the real compactable fixture, worker-entry barrier, bounded waits and release-on-unwind guard. Hold the worker before its computation. Start caller Q1 via `quiesce()`, wait until it has inspected the maintenance slot and obtained the job, then start Q2 via `shutdown()`. Add a per-index, test-only, nonblocking observation point immediately **after** slot inspection and lock release if needed. Have it report each caller's observed job identity or absence before waiting. Both observations must identify the compacting job. On the baseline, Q2 deterministically observes absence while the worker is held.
5. Before releasing the worker, verify neither caller has returned and a fresh maintenance or reconciliation admission is rejected. Release and bounded-join both callers and the test harness threads. Verify one worker entry, terminal cancellation, no new publication, a stable pinned revision and the outcome of a repeated task wait. A subsequent shutdown succeeds; an explicit resume permits a new, different operation. Reuse the existing admission test for same-operation sharing, conflicting-operation rejection, independent reconciliation and subsequent readmission. Use per-test channels and bounded arrival/completion waits. A sleep, polling delay or two threads that happen to run at the same time does not establish this interleaving.

## 5. Acceptance and performance evidence

Run the following from the integration worktree on the **same final code commit**. Record the full SHA first. The historical report at the pinned base recorded 594 default core tests, 595 all-feature core tests and 88 focused lifecycle tests passed; these numbers provide context only. New tests increase the inventory. Record actual results and investigate lost or newly ignored tests.

```sh
git rev-parse HEAD
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
cargo test --locked -p objekte-search-core --all-features
RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc --locked --workspace --no-deps
cargo test --locked -p objekte-search-core --lib lifecycle::
env -u OBJEKTE_LARGE_COMPACT_RECORDS cargo test --locked --release \
  -p objekte-search-core --lib \
  lifecycle::maintenance::compaction::tests::large_source_free_compaction_cancels_and_retries_a_concurrent_commit \
  -- --exact --ignored --nocapture
```

The release run must show the default build of **262,144 records**, **two attempts** after the injected concurrent commit, and **exactly one test passed**. Retain the output line with before/after slot totals and retired count. The regression verifies the independent mutation survives, the pinned view stays valid and cancelled work does not publish. Also retain the unaffected-segment sharing assertion (`Arc::ptr_eq`) from its separate focused test.

Run all newly added regression functions individually. Also run the following existing controls. Use `cargo test --locked -p objekte-search-core --lib '<qualified name>' -- --exact` for an individual function. For a module or prefix filter, omit `--exact` and capture the actual matching test names. Check the default/all-feature inventory with `-- --list` to resolve qualified paths and feature-gated controls. A successful command with zero matching tests supplies no evidence.

| Area | Existing test functions or group |
|---|---|
| A4 source capability, exact output and scan costs | `custom_readers_under_builtin_names_never_use_builtin_scalar_verification`, `borrowed_source_verification_matches_reference_for_outputs_and_duplicates`, `root_scoped_fts_matches_reference_across_readers_and_outputs`, `decoded_scalar_fts_combines_source_text_with_required_values` |
| A5 independent provider selection and borrowed domain | `gate_a_public_search_selects_either_provider_in_either_order`, `selective_text_reduces_the_input_to_the_structured_lookup`, `selective_candidate_and_broad_exact_or_share_the_restricted_domain`. Preserve cost inversion and the second provider's restricted input. Inspect `query_access/membership.rs` for any new clone on borrowed or restricted results. |
| A5 visibility, binding, independence and payload | `segmented_reference_parity_with_retired_and_new_rows_and_parameter_clock`, all `relation_query::tests::base_reference_` tests, `segmented_reference_relations_ignore_perturbed_forward_and_backlink_contributions`, `segmented_reference_first_last_ignores_unavailable_contribution` |
| A5 stored, computed and related sorting | `reference_stored_sort_uses_numeric_and_null_override`, `cross_collection_sort_uses_effective_override_compatibility`, `optimized_binding_defers_reference_sort_preparation`, `lifecycle_related_sort_uses_target_stored_column`, `related_sort_does_not_reuse_an_incomplete_filter_target_projection`, `reference_backlink_sort_streams_discovery_without_target_cache_budget`, `related_sort_stream_reports_target_budget_as_resource_error` |
| A6 admission, cancellation and publication | `maintenance_admission_shares_only_the_real_worker`, `stopping_periodic_schedule_does_not_cancel_current_reconciliation`, `background_reconciliation_reports_completion_and_joins`, `background_rebuild_waits_and_cancelled_preparation_never_publishes`, `publication_rejects_foreign_and_skipped_revisions_without_advancing_status`, `quiescence_rejects_prepared_publications_until_close_is_cancelled` |
| A6 pinning, concurrent mutation and sharing | `pinned_view_stays_stable_when_a_new_root_is_published`, `deleting_shared_multi_record_file_retires_every_collection_atomically`, `compatible_move_keeps_index_contributions_and_advances_file_version`, `rebuild_retries_after_concurrent_file_commit_without_losing_it`, `compaction_reuses_unaffected_native_segment` |

Record exact hand-derived orders, counts and physical-work counters for sections 3.2–3.5. In particular, compatible stored-order queries must keep zero extra target reads; safe fallback must perform only the source work needed for correctness; backlink discovery must remain one pass per referenced relation; borrowed root values must incur zero additional opens and decodes. Use fixture-local measurements. If production work increases beyond the intentional incompatible-key fallback, compare the same query, data and build profile at the pinned base and repair, give both figures and diagnose the increase. Preserve direct FTS/FILTER selection and borrowed membership; a performance change cannot be justified solely by equal output.

Compare the final diff with the pinned base. Preserve A4's seven source-root removals, A5's six schema/binding-root removals and A6's thirteen lifecycle-root removals. The original assignments list the paths. Keep public method signatures, diagnostic limits, snapshot format, query grammar and dependencies compatible. Confine production changes to the identified query and lifecycle paths. If a gate flakes, record the original failure, the isolated rerun, and the full affected command rerun on the same code SHA.

## 6. Completion report and handoff

Move the issued specification into `docs/ai/done/objekte_core_assignments_4_5_6_repair_specification.md` and append one completion report according to the repository workflow. On partial delivery use its blocked-task procedure and identify each pending section. Include:

- Verified starting SHA, current remote master at validation time, ordered implementation commits and final tested code SHA.
- For each of the six findings: baseline failure or observed counters, exact regression name, expected answer/outcome, result after repair and changed production paths. Record A4's reader and work assertions separately.
- For the concurrent shutdown regression: actual ordering of Q1, Q2, cancellation, worker release, join and readmission; identify which lock protects each transition and when it is released.
- Measured read/parse/byte and shortcut counters, focused results with positive execution counts, all workspace gate outcomes and the complete ignored release-test summary. Distinguish runs performed for this delivery from earlier reports.
- Final integration diff scope; independently confirm that the reporting commit changes only the completion document. Give the full pushed delivery SHA in the handoff after the commit exists. Verify the remote integration ref, list outstanding issues if any, and leave the merge to the owner.

## Implementation report

- **Task:** `core-query-and-lifecycle-repairs`
- **Completion date:** 2026-11-19
- **Verified starting revision:** `601a8e597513701d5933094eece7b0ad9ce555e2`
- **Remote `master` at validation:** `601a8e597513701d5933094eece7b0ad9ce555e2` (no divergence)
- **Integration branch:** `integration/core-query-and-lifecycle-repairs`
- **Integration worktree:** `/home/simon/source/objekte-integration-core-query-and-lifecycle-repairs`
- **Tested code revision:** `8ee3cb1fdddfa9ccd4c25c95149f2ba8a5a46e10`
- **Implementation commits:** `69c27a9515`, `e9cda124b1`, `15a76bfacd`, `0f613fd161`, `a0d640f0fe`, `c161f94321`, `b0b25b9d71`, `8ee3cb1fdd`
- **Status:** complete and ready for owner review; no merge into `master` was performed

### Summary

The public pinned reference route now uses one `IndexView` dispatch for base-only
and segment-bearing publications. It resolves parameters before the clock, then
builds the reference request from that view's active masks, segment groups,
catalogue and revision. Hits and diagnostics are stamped through the view once.

Related stored-key reuse is limited to compatible collation configurations.
The incompatible-key fallback reads only the target records needed to compute
correct keys, while compatible stored keys retain zero target reads. Both
forward-key shortcuts now require a forward direction and decline occurrence
selectors that they cannot prove.

Backlink discovery stores completeness with its request-local source map.
Unreadable logical sources invalidate every row whose related key depends on
that incomplete lookup, including cache hits and nested/multi-term sorts.
FILTER/FTS still retain the available witnesses, and complete independent
relation lookups remain rankable.

Reference forward sorts extract occurrences directly from supplied decoded
root values. Unsupplied intermediate values use the query-local decoded cache
or the registered source reader, with actual opens, parses and bytes charged to
relation-source work once.

Concurrent `quiesce()` and `shutdown()` callers now observe and wait for the
same retained maintenance job. The slot is cleared after the worker join only
when it still contains that same `Arc`, so a delayed waiter cannot remove a
newer admitted job.

### Finding results

#### 3.1 Pinned reference publication

- **Baseline:** `relation_query::tests::adopted_base_view_reference_search_uses_pinned_publication`
  failed on the adopted view with `segment query collection count mismatch`.
- **Regression:** the same test now checks a nonempty base generation with one
  empty native group per collection, count 2, limit 1, ID `m2`, `id` evidence,
  view revision, locator path `mentions/m2.json`, logical payload and original
  bytes.
- **Pins and catalogue:** `adopted_base_view_reference_search_keeps_retired_rows_on_the_old_pin`
  keeps `[m2, m1]` on the old pin after a logical retirement and returns only
  `m1` on the new pin. `adopted_base_view_reference_search_uses_each_pins_catalogue`
  keeps `entries/a.json` on the old locator and moves the new pinned locator and
  bytes to `entries/renamed-a.json`; byte reads on the old pin report the missing
  old path rather than following the new catalogue.
- **Empty collection:** `adopted_base_view_reference_search_handles_a_configured_empty_collection`
  returns count 0, no hits and zero scanned records.
- **Production paths:** `crates/core/src/lifecycle/view.rs`.

#### 3.2 Related sort-key collation

- **Baseline:** `query::sort_binding::tests::related_stored_sort_uses_the_target_fields_effective_collation`
  returned `["S2"]` at limit 1; the required answer is `["S10"]`.
- **Expected and fixed results:** limit 1 is `["S10"]`, the full page is
  `["S10", "S2"]`, count 2. Direct target title order remains `["T2", "T10"]`
  under its configured numeric override. Optimized and reference APIs agree.
- **Fallback work:** the incompatible target column is declined; the optimized
  fallback performs 2 actual sort-target reads and computes the correct keys.
- **Compatible reuse:** the compatible fixture returns `["S2", "S10"]` through
  optimized and reference search, with optimized `sort_target_reads=0`.
- **Both layouts:** `relation_query::tests::base_related_sort_declines_an_incompatible_stored_key`
  covers the base-column arm with `["S10", "S2"]`, 2 fallback reads and direct
  target order `["T2", "T10"]`; the primary regression covers segment columns.
- **Production paths:** `crates/core/src/query/sort_binding.rs`,
  `crates/core/src/query_results/ranking.rs` and
  `crates/core/src/query_execution/runner/relations.rs`.

#### 3.3 Forward-only shortcuts

- **Baseline:** `relation_query::tests::backlink_sort_does_not_use_a_forward_relation_at_the_same_ordinal`
  failed with `merged segment relation missing` because the backlink sort used
  a forward relation at the same declaration ordinal.
- **Expected and fixed results:** the parser-supported public query returns
  `["B", "A"]`, count 2, and `["B"]` at limit 1. The related-filter projection
  case also succeeds. The wrong-answer control has B pointing forward to A at
  the same ordinal and is not used for the backlink key.
- **Work evidence:** `sort_target_reads=2`, `relation_edge_visits=3`,
  `relation_target_reads=0`. Direct calls to both optimized helpers return
  `None` for the backlink hop before a forward contribution can be used.
- **Production paths:** `crates/core/src/query_results/ranking.rs`.

#### 3.4 Deterministic incomplete backlink discovery

- **Baseline:** `relation_query::tests::reference_backlink_sort_streams_discovery_without_target_cache_budget`
  returned `["B"]` with `total_matches=1` after one unreadable unlinked source.
  The required result is no hits and no verified rank tally because that source
  could have affected either target's ordered key.
- **Healthy result and work:** `["A", "B"]`, count 2, one discovery read for each
  of 18 visible logical sources, `relation_bytes_read=16777698`, and exactly 3
  actual linked-target sort reads.
- **Incomplete result:** `hits=[]`, `count=None`, `total_matches=0`,
  `match_count_complete=false`, one source diagnostic identifying
  `mentions/unlinked-00.json`. A predicate-first lookup, a nested sort and a
  two-term sort all propagate the same unverifiable outcome.
- **Caching and controls:** a source-less target is also excluded, the cached
  lookup for a second target performs zero additional reads and replays no
  diagnostic, and the complete `related_from` relation remains rankable as
  `["A", "B"]`.
- **Physical order:** healthy target visitation in the original fixture is
  `["A", "B"]`; a reverse physical fixture visits `["B", "C", "A"]` and still
  returns no verified rows after discovery becomes incomplete.
- **Production paths:** `crates/core/src/relation_query.rs`,
  `crates/core/src/query/sort_binding.rs`,
  `crates/core/src/query_results/ranking.rs` and
  `crates/core/src/query_execution/runner/relations.rs`.

#### 3.5 Decoded forward sources and actual reads

- **Baseline:** `query::sort_binding::tests::reference_forward_sort_reuses_sources_and_accounts_actual_reads`
  observed four source-range reads for the one-hop fixture instead of the two
  target reads. The two roots were reopened and decoded without accounting.
- **One hop:** result `["R1", "R2"]`, count 2,
  `relation_source_reads=0`, `sort_target_reads=2`, `files_opened=4`,
  `record_parses=4`; the path/range probe identifies only `t1.json` and
  `t2.json`. Total bytes are 101, exactly
  `24 + 24 + 27 + 26` for the four record ranges.
- **Two hops:** equal-key result `["R1", "R2"]`, count 2,
  `relation_source_reads=1`, `sort_target_reads=1`,
  `sort_target_reuse=1`, `files_opened=4`, `record_parses=4`; the probe
  identifies one `i.json` read and one `t.json` read. Total bytes are 92,
  exactly `23 + 23 + 22 + 24`.
- **Fresh failures:** removed-intermediate and removed-terminal requests return
  no rows, `count=None`, incomplete counts and diagnostics for the exact missing
  path.
- **Production paths:** `crates/core/src/relation_query.rs`,
  `crates/core/src/query/sort_binding.rs`; the test-only scoped read observation
  is in `crates/core/src/source/access/mod.rs`.

#### 4 Concurrent shutdown

- **Baseline:** `lifecycle::maintenance::tests::concurrent_quiesce_and_shutdown_wait_for_the_same_worker`
  had Q1 observe the compacting job identity but Q2 deterministically observed
  `None` while the worker was held.
- **Fixed order:** Q1 enters `quiesce`, sets `quiescing` under the publication
  write lock and releases it, stops the timer under `periodic_transition`, then
  clones the maintenance `Arc` under `maintenance_operation` and releases that
  mutex. Q2 enters `shutdown`, observes the same retained `Arc`, requests the
  same cancellation and waits. The worker is released; the shared handle lock
  stays held across taking and joining the worker. After the join, each caller
  briefly reacquires `maintenance_operation` and clears the slot only if
  `Arc::ptr_eq` still names the joined job.
- **Assertions:** both threads remain blocked before release; new maintenance
  and reconciliation admissions return `Cancelled`; there is one worker entry,
  terminal `Cancelled`, unchanged publication revision and pinned revision; the
  repeated task wait reports cancellation. A later shutdown succeeds, resume
  reopens admission, and a new rebuild completes.
- **Production paths:** `crates/core/src/lifecycle/service.rs`,
  `crates/core/src/lifecycle/mod.rs` and `crates/core/src/lifecycle/open.rs`;
  the real-worker test is in `crates/core/src/lifecycle/maintenance/tests.rs`.

### Assignment 4 verification

No A4 production change was required. The final exact source capability test ran
one positive case and retained the custom registrations under `json`, `jsonl`,
`json_decoded_scalars` and the neutral ID with no inferred scalar verification.
Its residual FILTER, field FTS, intersection, expected IDs/evidence/payload and
locator assertions all passed.

The four existing A4 controls each ran exactly once with one positive result:

- `source::registry::tests::custom_readers_under_builtin_names_never_use_builtin_scalar_verification`
- `query_execution::runner::tests::borrowed_source_verification_matches_reference_for_outputs_and_duplicates`
- `query_execution::runner::tests::root_scoped_fts_matches_reference_across_readers_and_outputs`
- `query_execution::runner::tests::decoded_scalar_fts_combines_source_text_with_required_values`

These positive scanner-output, fused/borrowed traversal, duplicate and read-cost
assertions remain separate from the A5 sort accounting probes.

### Focused controls

Every named control below executed positively on the tested code revision.
Single functions reported 1 passed; the `base_reference_` prefix reported 3
passed.

- A5 provider/domain: `gate_a_public_search_selects_either_provider_in_either_order`,
  `selective_text_reduces_the_input_to_the_structured_lookup`,
  `selective_candidate_and_broad_exact_or_share_the_restricted_domain` — 1 each.
- Visibility/binding: `segmented_reference_parity_with_retired_and_new_rows_and_parameter_clock`
  — 1; all `relation_query::tests::base_reference_` tests — 3;
  `segmented_reference_relations_ignore_perturbed_forward_and_backlink_contributions`
  and `segmented_reference_first_last_ignores_unavailable_contribution` — 1 each.
- Sorting: `reference_stored_sort_uses_numeric_and_null_override`,
  `cross_collection_sort_uses_effective_override_compatibility`,
  `optimized_binding_defers_reference_sort_preparation`,
  `lifecycle_related_sort_uses_target_stored_column`,
  `related_sort_does_not_reuse_an_incomplete_filter_target_projection`,
  `reference_backlink_sort_streams_discovery_without_target_cache_budget`,
  `related_sort_stream_reports_target_budget_as_resource_error` — 1 each.
- Lifecycle: `maintenance_admission_shares_only_the_real_worker`,
  `stopping_periodic_schedule_does_not_cancel_current_reconciliation`,
  `background_reconciliation_reports_completion_and_joins`,
  `background_rebuild_waits_and_cancelled_preparation_never_publishes`,
  `publication_rejects_foreign_and_skipped_revisions_without_advancing_status`,
  `quiescence_rejects_prepared_publications_until_close_is_cancelled` — 1 each.
- Pinning/mutation/sharing: `pinned_view_stays_stable_when_a_new_root_is_published`,
  `deleting_shared_multi_record_file_retires_every_collection_atomically`,
  `compatible_move_keeps_index_contributions_and_advances_file_version`,
  `rebuild_retries_after_concurrent_file_commit_without_losing_it`,
  `compaction_reuses_unaffected_native_segment` — 1 each.

All nine new regressions also ran individually with 1 passed, 0 failed:

- four `adopted_base_view_reference_search_*` regressions;
- `related_stored_sort_uses_the_target_fields_effective_collation`;
- `base_related_sort_declines_an_incompatible_stored_key`;
- `backlink_sort_does_not_use_a_forward_relation_at_the_same_ordinal`;
- `reference_forward_sort_reuses_sources_and_accounts_actual_reads`;
- `concurrent_quiesce_and_shutdown_wait_for_the_same_worker`.

### Validation actually run

All gates below were run on `8ee3cb1fdddfa9ccd4c25c95149f2ba8a5a46e10`:

```text
cargo fmt --all -- --check                                      passed
cargo check --locked --workspace --all-targets                  passed
cargo test --locked --workspace                                 passed
cargo test --locked -p objekte-search-core --all-features       passed
RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc \
  --locked --workspace --no-deps                                passed
cargo test --locked -p objekte-search-core --lib lifecycle::    passed
```

The default core unit suite reported 603 passed, 0 failed, 16 ignored. The
all-feature core unit suite reported 604 passed, 0 failed, 16 ignored. CLI was
81 passed; process integration was 33 passed and one ignored; desktop was 161
passed and one ignored; all remaining core integration suites and the
compile-fail doc test passed. The focused lifecycle run was 89 passed, 0 failed,
6 ignored.

The complete ignored release regression was rerun with the environment override
unset:

```text
large compaction build records=262144 elapsed=241.69846ms
large compaction records=262144 elapsed=258.122538ms attempts=2
  before=262146 after=262145 retired=1
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 618 filtered out
```

Default and all-feature inventories are 619 and 620 entries respectively,
compared with the prior delivery's 610 and 611. The +9 delta is exactly the nine
named new regressions; no unrelated function was removed or duplicated.

No new production benchmark applies. The repairs either remove redundant target
reads, select an existing stored key, or add no execution path beyond the
intentional incompatible-collation fallback. The fixture-local counters above
show zero extra compatible target reads, one charged shared intermediate read,
zero root rereads, one discovery pass per relation and no increase beyond the
two-read incompatible-key fallback.

### Integration scope and cleanup

Before the report-only commit, the complete implementation diff from the pinned
base modifies 13 core source/test files and adds or deletes no file. The report
commit adds only this completed task document. All former A4 source-root
removals, A5 schema/binding root removals, and A6 lifecycle root removals remain
removed. Public method signatures, query grammar, snapshot format and dependency
set are unchanged. Production changes are confined to the cited query and
lifecycle paths; the only additional source-access code is a `cfg(test)` scoped
read observation.

No subordinate worktrees were created. The integration worktree and branch are
retained for owner review. There are no outstanding issues. The final
report-only commit follows tested code revision `8ee3cb1fdd`; its only delta is
moving this specification into `docs/ai/done/` and appending this report.
