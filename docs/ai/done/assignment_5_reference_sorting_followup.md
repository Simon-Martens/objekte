# Assignment 5 follow-up specification — Independent reference sorting and record resolution

Start a new sibling worktree under /home/simon/source from repair/core-schema-binding at 18f877b11d3b2565212b41a495d4904a5996d2a8. Use repair/core-reference-sorting. Keep the existing pushed repairs and master available for review at their current revisions. Preserve the previous source-registration, macro resolution, parameter substitution and FILTER/FTS independence repairs.

## Remove the remaining production index reads

1. Cover both base-only and segmented reference requests with the same source-derived ranking and relation traversal. Keep the public IndexView and Repository reference method signatures, revision and result stamping, sorted tie-breakers, total count, paging, evidence, payload modes, and source error contract. The base-only reference call to repository.rs:1948 currently enters sort_rows, which reads stored sort keys at :2098 and optimized computed keys at :2060. A storage-layout-neutral reference scan using the published active masks, source readers and catalogue can serve both cases. Establish equivalent error behavior before routing the legacy entry through it.
2. In query/sort_binding.rs:629–670 the segmented reference path for First/Last still calls forward_edges and can return Unavailable based on its physical unavailable mask. Use the logical input record for reference traversal with every reducer. For First/Last keep source occurrence order and visit both ID and null occurrences, honor all position selectors, retain duplicates and distinguish an unavailable target from a null occurrence. Keep the optimized sort path as-is. Parse local sort paths and construct collators once per selected collection and sort term. Read locale and collation options from logical configuration and evaluate keys from record values and metadata.
3. Resolve reference forward target IDs from active record metadata. Do the same in related sort and carry selected record addresses through ranking into payload reads. Currently relation_query.rs:363 reads both production ID maps, reference forward traversal calls it at :1326, and query_execution/runner/relations.rs:2499 resolves the winning record ID again during materialization. A lazily built request-local identity map from visible records is acceptable. Detect duplicate visible IDs using the existing error contract and keep inactive records excluded. Use physical slot addresses only to retrieve record metadata/bytes and test pinned visibility.
4. Remove reference bound FILTER/FTS cloning at query_execution/runner/relations.rs:2602. Pass borrowed bound terms to a neutral scan or move the binding data once. Keep production planner, provider ordering, proof cache and borrowed membership behavior unchanged. Replace the wildcard re-export chains in reference/mod.rs, query/binding/mod.rs and query/semantics/mod.rs using canonical imports. Keep the existing test suites and alternate matcher fix.
5. The reference backlink scan at relation_query.rs:598 builds and sorts the entire source collection for each queried target. On the first use of each relation, lazily build a request-local backlink lookup from visible logical occurrences. Order source addresses once by the same identity order, deduplicate one source per target while keeping the required selector order, and retain the unavailable-source diagnostics. Track source reads and cache retention. Limit retained work to what the requested relations need. Preserve the reference execution resource/error behavior.

## Acceptance evidence

- Extend relation_query.test.rs:1104 with a segmented FIRST/LAST sort case whose targets have different sort values and whose tested source row includes a null and repeated references. Set the tested forward contribution's unavailable bit in the isolated copy. Verify the optimized/direct diagnostic observes the perturbation and reference sort still returns hand-derived order, count and a correctly materialized page. An edge-only deletion cannot catch this bug because First/Last uses the source values after the unavailable check.
- Add a base-only pinned fixture. Perturb a stored sort key and a relation forward or reverse contribution independently in isolated copies. Assert reference order, IDs, count and evidence before running an optimized control. Cover both stored and related sort, and an unavailable source record with the expected diagnostic behavior.
- Independently omit a visible target's entry from the optimized ID map while leaving its active metadata intact. Assert reference forward FILTER, related sort and selected locator/logical payload still derive from the metadata and bytes. Keep the existing preparation/planner probes and predicate regressions.
- Add a deterministic multi-target backlink case. Check the reference order and results against fixture truth and assert logical source scanning is bounded to once per requested relation. Keep the workload small and verify read counts, not wall time.
- Run the focused tests with at least one executed test in each invocation, the original R5 provider-cost and restricted-domain tests, the workspace formatting/check/test/doc commands and the all-feature core suite. Document the tested code SHA, selected test names and counts, and the final report-only head. Compare changes with the frozen R5. Once accepted, integrate the follow-up into the R6 stack and rerun its workspace gates, focused lifecycle suite and ignored 262,144-record release regression with the environment override unset.

The completion criterion is independent reference membership, ordering and payload for both storage layouts. Production query behavior and performance remain the baseline for the optimized path.

## Implementation report

- Task: `core-reference-sorting`
- Completion date: 2026-11-19
- Base: frozen R5 `18f877b11d3b2565212b41a495d4904a5996d2a8`
- Tested code commit: `4b6439989cfcea20b5efdaba35508fd2cf4c8e2a`
- Branch: `repair/core-reference-sorting`
- Worktree: `/home/simon/source/objekte-repair-core-reference-sorting`

### Implemented

- Routed base-layout and native-segment reference requests through the same source-derived reference scan. Removed the base-only `sort_rows` implementation and its reads of stored and optimized sort keys.
- Preserved result revision stamping, tie-breakers, counts, paging, evidence, payload modes, and source diagnostics on the common reference route.
- Changed reference FIRST/LAST forward traversal to visit the logical source record in occurrence order. It retains duplicate occurrences, observes null values, applies every position selector, and distinguishes a missing target from a null occurrence.
- Prepared parsed sort paths and ICU collators once per selected collection and sort term.
- Added a lazily built request-local identity map keyed by active visible record metadata. Reference forward filters, related sorts, and selected payload materialization no longer depend on production ID maps.
- Carried the selected physical record address through ranking so payload reads fetch the chosen record rather than resolving its ID again.
- Moved reference FILTER/FTS binding data into the query scan instead of cloning it.
- Added a lazily built request-local backlink lookup per relation. It scans visible logical sources in identity order once, deduplicates target occurrences per source, preserves selector order, records source reads and bytes, and reuses the lookup for multiple targets.
- Removed the broad reference, binding, and semantics re-export chains and migrated affected consumers to canonical module imports.

### Regression coverage

- `segmented_reference_first_last_ignores_unavailable_contribution` perturbs the physical forward contribution for a source containing a null followed by repeated references. The optimized diagnostic observes the perturbation while the reference FIRST/LAST orders and materializes from logical records.
- `base_reference_sort_ignores_perturbed_stored_and_relation_contributions` uses a base-layout fixture, perturbs a resident stored sort key and relation availability, checks reference order/count/evidence, runs optimized controls, and verifies a missing related source produces incomplete count and diagnostics.
- `base_reference_resolves_omitted_id_map_targets_and_materializes_their_bytes` removes a visible target from the production ID map and verifies relation FILTER, related sort, locator output, and logical payload still use source metadata and bytes.
- `base_reference_multi_target_backlinks_scan_each_relation_once` checks A/B target results and asserts one logical source read per visible source across two target lookups.

### Validation

Focused tests were run separately so each invocation executed at least one test:

- `cargo test --locked -p objekte-search-core --lib relation_query::tests::base_reference_ -- --nocapture`: 3 passed, 0 failed.
- `cargo test --locked -p objekte-search-core --lib relation_query::tests::segmented_reference_first_last_ignores_unavailable_contribution -- --exact --nocapture`: 1 passed, 0 failed.
- `cargo test --locked -p objekte-search-core --lib query_execution::runner::tests::gate_a_public_search_selects_either_provider_in_either_order -- --exact --nocapture`: 1 passed, 0 failed.
- `cargo test --locked -p objekte-search-core --lib query_execution::runner::tests::selective_candidate_and_broad_exact_or_share_the_restricted_domain -- --exact --nocapture`: 1 passed, 0 failed.

Final gates:

- `cargo fmt --all -- --check`: passed.
- `cargo check --locked --workspace --all-targets`: passed.
- `cargo test --locked --workspace`: passed, including 589 core unit tests with 16 ignored and all CLI, desktop, integration, and doc tests.
- `cargo test --locked -p objekte-search-core --all-features`: passed, including 590 core unit tests with 16 ignored and all integration and doc tests.
- `RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc --locked --workspace --no-deps`: passed.

No separate benchmark command was specified for this follow-up. The focused work-counter regression verifies bounded source reads, and the original provider-cost and restricted-domain checks guard the optimized baseline.

### Review notes

- The optimized plan, provider ordering, proof cache, and borrowed membership path remain unchanged.
- This branch intentionally does not merge into `master`; integration into the dependent R6 worktree is performed after this report commit so R5 remains separately reviewable.
- Final report-only head: the first documentation-only commit after tested code commit `4b6439989cfcea20b5efdaba35508fd2cf4c8e2a` on `repair/core-reference-sorting`.
