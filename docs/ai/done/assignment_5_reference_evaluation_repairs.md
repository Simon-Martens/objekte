# Assignment 5 repair specification — Independent reference evaluation and binding ownership

**Start** on `repair/core-schema-binding` from the accepted R4. Integrate the original A4..A5 commits in order (A4 `5b3804be019ddb55447c7ad7810786e801a553a5`, A5 `a8a1db7d89186e7b34d2c18e25c621ba7f1b8cf6`). Use an ordinary merge of frozen A5 into the R4-based branch to retain ancestry; resolve source import conflicts by retaining R4 capability behavior with A5 canonical imports. Confirm R4 and A5 are both ancestors. Review the new material against R4.

## Restore an independent public oracle

1. At lifecycle/view.rs:1047 the segmented path bypasses resolve_current_clock. At query/binding/filter.rs:395 the binder rejects unresolved macros. Resolve the clock exactly once at the outer reference request, after parameter application and before either storage-layout path binds; preserve borrowed Cow for macro-free requests. Validate against a fixed-clock request and also call the public reference API with an unresolved supported macro. Use dates far from the current boundary for that smoke test.
2. Replace the delegation at query_execution/runner/relations.rs:2558. The reference currently calls production prepare and search_segmented_relations with pruning disabled, which still enters relation_page_plan at :1802. Factor an explicit reference entry under reference/ that binds selected collections once, shares the logical bound terms and exact scalar routines, and enumerates visible base records and each pinned segment's active records exactly once. Borrow revision, catalogue, active masks, record metadata, source reader, byte ranges and prepared matchers. Exclude retired entries. Preserve public signatures, output ordering/ties, paging/count, requested evidence, payload materialization, unknown values, diagnostics, query errors, frozen clock and parameter semantics. A stored search posting may be used by production but cannot establish truth in this entry. Keep optimized dispatch, planner costs, access offers, cache admission and worker behavior unchanged.
3. The existing merged relation visitor calls forward_edges/resolve_id/backlink_sources (relation_query.rs:1173–1216); backlink_sources reads posting fields (:477). Provide the reference visitor a pinned record enumeration and source-based hop resolution. Read declared relation values through the record's reader/schema. Resolve forward IDs against visible metadata; find backlinks from the referencing values of visible records, preserving the established identity ordering, multiplicity, array positions, missing targets, selectors, quantifiers and three-valued logic. For efficiency a reference-request-local lookup built solely from visible metadata/logical occurrences is acceptable. Construct it only when the query needs it. Use the same independent resolution for relation FTS, FILTER and related sort keys. Check the base-only active reference route too (relation_query.rs:1578). Avoid production adjacency, postings, order indexes, provider calls, plan selection, candidate bitmaps and verified/proof caches. Pinned active-slot masks are authoritative visibility metadata. Share exact routines and neutral output construction with optimized execution only where this preserves independence.
4. Add a focused parity test through IndexView with a base/segment fixture that includes a retired row, a surviving row and a new row. Use an indexed term plus unindexed residual, non-default parameters and a frozen clock; include an FTS false positive, sort tie with deterministic tie-breaking, limit and count. Assert hand-derived ordered IDs, total and field evidence before comparing optimized results. In a separate reference-only regression use test-local provider/planner probes and deliberately omit or perturb a relation contribution in an isolated test publication. Prove reference results for both forward and backlink queries derive from source truth; verify the altered contribution is exercised by an optimized control or direct diagnostic. Scope instrumentation to the fixture, cover both storage paths, and preserve unrelated tests running in parallel.

## Finish schema and binding ownership

Move PreparedContains, its matcher methods, wildcard splitting and associated construction counter from query/binding/filter.rs:31 to query/semantics/comparison.rs. Bind once and borrow the prepared finder at evaluation. Replace wildcard re-export chains in reference/mod.rs, query/binding/mod.rs and query/semantics/mod.rs with direct imports at their consumers. Keep public extension traits in schema::operation and use canonical imports rather than compatibility forwarding. Split reference/tests.rs by actual responsibility (schema validation and conversion, binding, folded scratch, comparison, FILTER, FTS). Keep gated and ignored registrations intact. In query/semantics/test_matcher.rs:11–52 match the installed literal as well as the field, operator and transform, and handle the empty needle without windows(0). Exercise an unrelated literal while the guard is live; its invocation count and normal execution must be unaffected.

Re-run gate_a_public_search_selects_either_provider_in_either_order and selective_text_reduces_the_input_to_the_structured_lookup; verify cost inversion changes selection/order and the second provider sees the restricted domain. Preserve query_access/membership.rs borrowed, restricted and owned cases; compare move/clone points against A4. Keep six named root removals and R4 regressions. Record R5 for Assignment 6.

## Integration and acceptance

Use a sibling worktree under /home/simon/source and a new repair branch. Preserve master and all three frozen delivery refs. Record the actual starting commit, original comparison, implementation commits and tested code SHA. Keep the original specification authoritative for unchanged behavior. Use commits that build through normal workspace entry points. Deliver to the owner for review, without merging into master.

Run the following on the final code tree, retain command summaries and exit status, and compare default and all-feature test inventories by function name and multiplicity, accounting for module moves. For every individually filtered test, show the fully qualified test name and the positive number executed. Fix errors within scope before marking the repair ready. In the report distinguish results actually run from historical results. After a report-only commit verify that its only delta is the report and record the final full SHA.

```sh
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
cargo test --locked -p objekte-search-core --all-features
RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc --locked --workspace --no-deps
```

## Implementation report

- **Task:** `core-schema-binding-repairs`
- **Completion date:** 2026-11-19
- **Starting R4:** `7cff39fab80fe583dc25f77f017a265b64995a21`
- **Frozen A5 merged:** `a8a1db7d89186e7b34d2c18e25c621ba7f1b8cf6`
- **Ordinary merge commit:** `fc4f2ab512852458e7996a35bee148e86a15aab1`
- **Repair branch:** `repair/core-schema-binding`
- **Repair worktree:** `/home/simon/source/objekte-repair-core-schema-binding`
- **Tested code revision:** `9dfe5784f0d72d52d12939b919aae3d1459a417a`
- **Implementation commits:** `fc4f2ab512852458e7996a35bee148e86a15aab1`, `e8f1038cc27c23beab91ac3dd01a12ea52c41129`, `9dfe5784f0d72d52d12939b919aae3d1459a417a`
- **Original comparison:** [R4 to tested R5 code](https://github.com/Simon-Martens/objekte/compare/7cff39fab80fe583dc25f77f017a265b64995a21...9dfe5784f0d72d52d12939b919aae3d1459a417a)
- **Status:** complete and ready for review; no merge into `master` was performed

R4 and frozen A5 are both ancestors of this branch. The A5 merge retained
canonical A5 imports and R4 source-provenance behavior. Only the original
`source/json/scalars/scan.rs` import path needed its final canonical update;
the merge otherwise retained R4's exact scanner ownership.

### Summary

The public reference request now resolves the clock once at the outer
`IndexView` entry, after parameter application and before selecting either the
base-only or segmented path. Macro-free queries retain a borrowed query.

A new `reference::relations` entry binds each selected collection once and
passes those bindings directly to an explicit reference scan. The reference
scan:

- enumerates only active base records and active slots in each pinned segment;
- uses source readers to obtain logical values, source bytes and text;
- resolves forward references from the declared relation path and backlinks by
  comparing visible source records' logical relation occurrences;
- resolves related FILTER, related FTS and related sort targets through that
  same source-backed route;
- preserves identity ordering, positions, missing targets, selectors,
  quantifiers, three-valued truth, paging, count, evidence, payload and error
  behavior; and
- does not call production query preparation, relation page planning, access
  providers, posting candidates, verified-result/proof caches or stored order
  indexes for reference truth.

Request-local decoded-target reuse remains bounded and only serves source
values already read for the reference request. Production dispatch, provider
offers, planner choice, cache admission, worker behavior and borrowed
query-access membership are unchanged.

### Binding ownership

`PreparedContains`, wildcard splitting, matcher methods and the construction
counter moved to `query/semantics/comparison.rs`. `BoundTerm` refers directly to
that owner and evaluation borrows the prepared finder. Consumers import the
comparison routine directly rather than through a binding/reference re-export
chain.

The test-only alternate matcher now matches the installed literal in addition
to field, operator and transform. An empty needle is handled without a
zero-width `windows(0)` call. While a matcher guard is live, an unrelated
literal leaves its invocation count unchanged and executes normally.

`reference/tests.rs` was split into owner suites:

| Path | Responsibility |
| --- | --- |
| `reference/tests/schema.rs` | schema validation and conversion |
| `reference/tests/filter.rs` | FILTER, temporal and boolean semantics |
| `reference/tests/folding.rs` | borrowed value visitor, fold scratch and prepared finder use |
| `reference/tests/comparison.rs` | comparison, state composition and prepared contains |
| `reference/tests/fts.rs` | exact/folded FTS and evidence/membership behavior |

The parent test module retains only the shared schema and record fixtures. Test
function names and multiplicity are preserved through module-prefix changes;
one ignored release benchmark remains registered.

### New regressions

`relation_query::tests::segmented_reference_parity_with_retired_and_new_rows_and_parameter_clock`
uses an `IndexView` publication containing one surviving base row, a retired
base row replaced in a segment, and newly added segment rows. It combines an
indexed relation term, unindexed residual FILTER, bound `$needle` parameter,
fixed clock, relation FTS, a false-positive target, a deterministic related
sort tie, limit and exact count. It asserts hand-derived IDs and count before
comparing optimized and reference results.

`relation_query::tests::segmented_reference_relations_ignore_perturbed_forward_and_backlink_contributions`
creates an isolated copy of a pinned source segment, removes target `A` from
both its optimized forward contribution and relation posting, and exercises
direct optimized diagnostics plus source-backed reference diagnostics.
Optimized forward edges and backlinks omit `A`; the reference resolves
`m2 -> A` and backlinks `[m2, m3]` from unchanged source truth.

The public reference API also accepts an unresolved supported `@yearStart`
macro in a current-clock smoke request.

The reference-only publication regression now invokes the full
`reference::relations::search_segmented` entry against the perturbed
publication. Both forward `entry.title = 'Hymn'` and backlink
`@backlinks.mentions.title = 'segment'` requests return their hand-derived source
results `[m2, m3]` and `[A]`. Thread-local test probes count calls to production
`query::prepared::prepare` and `query_plan::operations::relation_page_plan`;
both counts are unchanged across the two reference requests. Instrumentation is
thread-local, so parallel unrelated tests are unaffected.

### Query-access ownership check

Compared with A4, A5's `query_access` changes are canonical import migrations.
No membership representation, borrowed-input lifetime, restricted input or
owned fallback behavior changed. The R5 repair did not edit `query_access`.
All 14 query-access tests passed, including shared borrowed ownership, all
membership representations and independent restricted-provider selection.

### Validation actually run

The following commands passed at tested code revision
`9dfe5784f0d72d52d12939b919aae3d1459a417a`:

```text
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
cargo test --locked -p objekte-search-core --all-features
RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc --locked --workspace --no-deps
```

Workspace results included CLI 81 passed; process integration 33 passed and
one ignored; desktop 161 passed and one ignored; all core integration suites;
and one passing compile-fail doc test. Default core was 585 passed and 16
ignored. All-feature core was 586 passed and 16 ignored.

Individually filtered commands and positive execution counts:

- `cargo test --locked -p objekte-search-core --lib relation_query::tests::`
  — 20 passed, 0 failed.
- `cargo test --locked -p objekte-search-core --lib reference::tests::`
  — 24 passed, one ignored, 0 failed.
- `cargo test --locked -p objekte-search-core --lib query_access::`
  — 14 passed, 0 failed.
- `query_execution::runner::tests::alternate_exact_predicate_implementation_matches_reference_in_boolean_queries`
  — 1 passed.
- `query_execution::runner::tests::gate_a_public_search_selects_either_provider_in_either_order`
  — 1 passed.
- `query_execution::runner::tests::selective_candidate_and_broad_exact_or_share_the_restricted_domain`
  — 1 passed.
- `query_access::independent::tests::sorted_scalar_and_gram_backing_match_reference_membership`
  — 1 passed.
- `query_access::independent::tests::selective_text_reduces_the_input_to_the_structured_lookup`
  — 1 passed.
- `relation_semijoin::tests::seeded_array_semijoins_match_an_index_free_expected_enumeration`
  — 1 passed.
- `source::registry::tests::custom_readers_under_builtin_names_never_use_builtin_scalar_verification`
  — 1 passed.

Frozen A5 and tested R5 contain 628 versus 630 default inventory entries and
629 versus 631 all-feature entries. Function-name and multiplicity comparison
after module-prefix normalization reports exactly the two named regressions
above and no other additions, removals or multiplicity changes.

No production benchmark applies to the new independent oracle path. It is a
correctness/reference route and there is no comparable pre-repair source-backed
segmented oracle to benchmark. Existing borrowed-access tests verify the
production move/clone and reuse points remain unchanged. Focused planner tests
also verify cost inversion changes provider order and the second provider sees
only the restricted input domain.

### Review notes

No unresolved functional issue or blocked check remains. The report-only commit
follows tested code revision
`9dfe5784f0d72d52d12939b919aae3d1459a417a`; its only delta is completion
documentation. The branch and worktree remain for the owner's review.
Assignment 6 is based on this accepted R5 result.
