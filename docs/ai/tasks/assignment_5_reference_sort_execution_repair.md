# Assignment 5 repair specification — Reference sort execution

## Starting point and scope

Implement on `repair/core-reference-sorting` in its sibling worktree under `/home/simon/source`. The reviewed source is `5cffd5df18a92ad0b4af272bb7c51dcc2b3c9ac0`. Start implementation from the task-specification commit on top of that revision. Preserve the completed source-independence work in `docs/ai/done/assignment_5_reference_sorting_followup.md` and its focused tests. Keep the repair on this branch for review.

Fix reference backlink discovery, isolate reference sort preparation from optimized sort preparation, and keep reference-specific payload addressing out of optimized ranking. Preserve the public query interfaces, the existing source error and count contract, and optimized provider and stored-order selection. No new dependencies or changes to the index format are needed.

## 1. Stream reference backlink discovery

In `crates/core/src/query/sort_binding.rs` at the reviewed head, the reference backlink branch around line 816 gives `reference_backlink_sources_with` the `MergedSortTargetCache::load_source` loader. The loader retains every decoded logical source in the sort-target cache. At lines 162–174 it charges four times source bytes plus 128 bytes per record against the existing 64 MiB target cache limit. Approximately 16 MiB of unrelated source records can make a query fail before any linked record contributes a key.

- Use one discovery path for reference FILTER, FTS and sort. Reuse the existing request-local lookup per `RelationId` in `relation_query.rs`. Give discovery a source-reader path that decodes a record, extracts its relation occurrences, then releases the decoded value. A previously decoded value may be borrowed when already available. Discovery must never populate `MergedSortTargetCache` or consume its retained-byte budget.
- Keep the sorted source addresses per target, using the existing source-identity order. Deduplicate repeated references to the same target from one source record before adding the address. Preserve array selector behavior and reuse the same lookup when the query visits further targets of the relation.
- Retain the target cache and its limits for actual key evaluation, including selected related targets and any intermediate records needed for traversal. Keep the source and target read limits, cancellation checks and unavailable-source diagnostics. Count real discovery reads and bytes as relation source work, and count those bytes once in the aggregate I/O totals. Avoid reporting discovery reads as sort-target reads.
- Add a regression through public pinned reference search using a backlink sort expression accepted by the existing parser. Use at least two visible target rows with distinct keys from small linked records. Include enough valid unlinked source data to exceed the old target-cache threshold by its actual estimate. Each individual source must be below the per-record limit. Assert the hand-derived ordered IDs, complete count and one discovery read per visible logical source across both target lookups. Verify the old implementation fails the fixture with the sort-target cache error. Exercise an unavailable discovery source separately and assert diagnostics and count completeness.

## 2. Construct reference collators only for reference requests

The shared `BoundSort::bind` stored-column branch around lines 332–358 builds a `ComputedSort` and an ICU collator even when an optimized request uses resident sort keys.

- Keep ordinary `BoundSort::bind` and `ResolvedSort::new` on the stored-column path. Add an explicit reference sort-binding entry used only by `search_segmented_reference_records`. In reference setup prepare each source-derived evaluator once per selected collection and sort term. Preserve the bound path, transform, locale, collation options and tie-breaking semantics. Preserve the existing configuration compatibility checks between collections.
- No reference stored-sort evaluator or collator may be prepared on the optimized stored-order or explain paths. Add a test-scoped construction counter or inspect the bound representation to prove this. The same test must exercise reference binding and prove it produces the source evaluator. Retain the existing test that perturbs a stored sort key and checks reference order.

## 3. Keep address-based payload retrieval in reference execution

In `crates/core/src/query_execution/runner/relations.rs` around line 2216, the shared runner sets `record_address` for every winner, which routes optimized fallback page materialization through the new reference address loader at line 2556.

- Populate `record_address` only in reference mode. Keep the existing optimized lookup and payload path, and keep reference materialization independent of production ID maps. Verify locator, logical and bytes output for the affected shared runner. Retain source diagnostics, pinned-view revision stamping and field evidence.

## Acceptance and delivery

1. Run the new tests, the existing `base_reference_` tests and `segmented_reference_first_last_ignores_unavailable_contribution` with positive execution counts. Run `segmented_reference_relations_ignore_perturbed_forward_and_backlink_contributions`, `gate_a_public_search_selects_either_provider_in_either_order`, and `selective_candidate_and_broad_exact_or_share_the_restricted_domain`. Confirm that the altered sort keys, forward unavailable bit and production ID map still exercise the intended controls.
2. Run `cargo fmt --all -- --check`, `cargo check --locked --workspace --all-targets`, `cargo test --locked --workspace`, `cargo test --locked -p objekte-search-core --all-features` and `RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc --locked --workspace --no-deps`. Record the tested commit, command outcomes, positive test counts and discovery counters. Check that the diff is limited to the specified behavior.
3. Push implementation and completion-report commits on `repair/core-reference-sorting` in accordance with `docs/ai/README.md`. Move this specification to `docs/ai/done/` with the completion report. Report the final full SHA for review.
4. After acceptance, integrate the corrected Assignment 5 head into the dependent lifecycle repair and validate the combined head. The reviewed lifecycle head `14f49430d40ccecad67eb1a35aa5179cc30ca79c` already contains `5cffd5df18a92ad0b4af272bb7c51dcc2b3c9ac0`. Re-run the workspace gates and focused lifecycle suite, then run `env -u OBJEKTE_LARGE_COMPACT_RECORDS cargo test --locked --release -p objekte-search-core --lib large_source_free_compaction_cancels_and_retries_a_concurrent_commit -- --ignored --nocapture`. Require 262,144 records, two attempts and exactly one passing test. Treat integration into `master` as a separate owner decision.
