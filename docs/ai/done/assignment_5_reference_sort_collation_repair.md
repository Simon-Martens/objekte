# Assignment 5 repair specification — Reference sort collation overrides

The reviewed Assignment 5 head is `e226c6c1293bae57976da9af809df4e925dcab6b`. Its streaming backlink discovery, separate reference collator preparation and reference-only payload addressing resolve the previous findings. Address the remaining collation regression on `repair/core-reference-sorting` from this task-specification commit, using the existing sibling worktree.

## Defect

In `crates/core/src/query/sort_binding.rs:345–385`, optimized simple stored-field sorting gets locale and options from the stored column. Reference binding bypasses that column and constructs the evaluator from the collection defaults. `SortBuilder::new` in `crates/core/src/index/sorting/mod.rs:232–269` applies `CollectionDefinition::sort_overrides` before building the column. When a field specifies numeric collation and the collection default is nonnumeric, reference search can order `item10` ahead of `item2` while optimized search orders `item2` first. Cross-collection compatibility can also differ because the two paths compare different effective configurations.

## Required change

- In reference mode for a simple stored-field sort, derive the effective `SortConfig` from the logical declaration for that field. Select `definition.sort_overrides.get(&path.field)` when present and fall back to the collection locale and options. Use that same effective configuration for the collator, null and missing placement, and cross-collection compatibility. Keep related sorts, reducers and explicit bindings on their existing configuration path.
- Preserve source-derived reference keys. Keep optimized sorting on a single stored-column lookup and keep reference evaluator construction out of optimized and explain binding. Retain the existing construction-count regression. Preserve the backlink and payload changes from the prior repair.
- Add a public search regression with a stored field that overrides nonnumeric defaults with numeric collation. Verify expected ordered IDs for `item2` and `item10` independently for optimized and reference search. Include a null placement override in the same fixture if the schema allows it. Exercise equal effective overrides across collections with different defaults, and differing effective overrides with equal defaults. Both paths must agree on compatibility. Keep the existing stored-key perturbation regression.

## Delivery

Run the focused regressions with positive execution counts and the workspace formatting, all-target check, workspace test, all-feature core test and documentation gates. Push the implementation and completion report on `repair/core-reference-sorting`. Move this specification to `docs/ai/done/` and record the tested code revision. Integrate the corrected head into `repair/core-lifecycle-coordination`, which currently contains the reviewed source at `9e8c26d9c4e3704aafcc0650c996bbf71d75df6b`. Verify the exact combined tree with the workspace gates, focused lifecycle tests and the ignored release compaction regression at 262,144 records with two attempts. Report the tested combined head for the owner to fast-forward after review.

## Implementation report

- **Task:** `core-reference-sort-collation`
- **Completion date:** 2026-11-19
- **Reviewed source:** `e226c6c1293bae57976da9af809df4e925dcab6b`
- **Specification commit:** `5b7ecef878618591806ceefabd89ca368433af53`
- **Tested code commit:** `d9c44623c1274217c2ad8c9c3ec572c50e34d00c`
- **Branch:** `repair/core-reference-sorting`
- **Worktree:** `/home/simon/source/objekte-repair-core-reference-sorting`
- **Status:** complete and ready for review; no merge into `master`

### Implemented

- Reference binding now recognizes every simple source-derived stored-field sort independently of whether a resident base column exists.
- For those terms it selects the field's `CollectionDefinition::sort_overrides` entry and falls back to the collection `sort_locale` and `sort_options` only when no override exists.
- The effective configuration is used for the ICU collator, null/missing placement, and cross-collection compatibility.
- Related sorts, reducers, explicit bindings, optimized stored-column lookup, reference evaluator preparation isolation, streaming backlink discovery, and reference-only payload addressing remain unchanged.

### Regression coverage

`reference_stored_sort_uses_numeric_and_null_override` uses a nonnumeric collection default overridden by numeric collation with nulls and missing values first. Both public optimized and reference searches return `["null", "two", "ten"]` with count `3`, proving `item2` sorts before `item10`.

`cross_collection_sort_uses_effective_override_compatibility` verifies:

- collections with different defaults but equal effective overrides both search successfully and return `["a2", "b2", "a10", "b10"]` on optimized and reference paths;
- collections with equal defaults but differing effective overrides reject both optimized and reference searches with `incompatible cross-collection sort`.

The construction-count, stored-key perturbation, streaming backlink, segmented relation, provider-order and restricted-domain regressions remain passing.

### Validation actually run

Focused regressions at tested code commit
`d9c44623c1274217c2ad8c9c3ec572c50e34d00c`:

- `query::sort_binding::tests::reference_stored_sort_uses_numeric_and_null_override`: 1 passed.
- `query::sort_binding::tests::cross_collection_sort_uses_effective_override_compatibility`: 1 passed.
- `query::sort_binding::tests::optimized_binding_defers_reference_sort_preparation`: 1 passed.
- `relation_query::tests::base_reference_`: 3 passed.
- `relation_query::tests::reference_backlink_sort_streams_discovery_without_target_cache_budget`: 1 passed.
- `relation_query::tests::segmented_reference_first_last_ignores_unavailable_contribution`: 1 passed.
- `relation_query::tests::segmented_reference_relations_ignore_perturbed_forward_and_backlink_contributions`: 1 passed.
- `query_execution::runner::tests::gate_a_public_search_selects_either_provider_in_either_order`: 1 passed.
- `query_execution::runner::tests::selective_candidate_and_broad_exact_or_share_the_restricted_domain`: 1 passed.

Final gates:

- `cargo fmt --all -- --check`: passed.
- `cargo check --locked --workspace --all-targets`: passed.
- `cargo test --locked --workspace`: passed; 593 core unit tests passed, 16 ignored, plus all integration and doc tests.
- `cargo test --locked -p objekte-search-core --all-features`: passed; 594 core unit tests passed, 16 ignored, plus all integration and doc tests.
- `RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc --locked --workspace --no-deps`: passed.

One initial all-feature-unit run encountered the existing temporary-directory
race in `relation_oracle::independent_json_relation_oracle_covers_quantifiers_negation_periods_and_restore`.
That test passed alone, and the complete all-feature command was rerun successfully.

No separate benchmark was required. The change selects an existing logical
configuration on the reference path and does not add production source reads,
copies, or index data.

### Review notes

No unresolved functional issue or blocked check remains. The report-only commit
follows tested code commit
`d9c44623c1274217c2ad8c9c3ec572c50e34d00c`; its only source delta is moving
this specification to `done/` and appending this report.
