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
