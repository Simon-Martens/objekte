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

## Implementation report

- **Task:** `core-reference-sort-execution`
- **Completion date:** 2026-11-19
- **Reviewed source:** `5cffd5df18a92ad0b4af272bb7c51dcc2b3c9ac0`
- **Specification commit:** `3f5a4fc45939bc147887dd19ccb07f1d0c69b83c`
- **Tested code commit:** `4935431b3fde7b7ec4d121871f1f8d81fe6d6978`
- **Branch:** `repair/core-reference-sorting`
- **Worktree:** `/home/simon/source/objekte-repair-core-reference-sorting`
- **Status:** complete and ready for review; no merge into `master`

### Implemented

- Replaced the retained sort-cache backlink loader with one streaming discovery path shared by reference FILTER, FTS and sort execution.
- Discovery borrows a previously decoded value when available; otherwise it reads and decodes one logical source, extracts relation occurrences into the request-local target lookup, and releases the decoded value.
- Discovery never enters `MergedSortTargetCache`. Source addresses remain ordered by identity, repeated references from one source are deduplicated, and the relation lookup is reused for later target visits.
- Discovery reads and bytes are recorded as relation source work and folded once into aggregate I/O counters. Actual key evaluation retains the existing target cache and its limits.
- Split optimized and reference sort binding. Ordinary `BoundSort::bind` now leaves resident stored columns on the optimized path without constructing an ICU collator.
- Added `BoundSort::bind_reference`, used only by `search_segmented_reference_records`, which prepares one source-derived evaluator per selected collection and term while retaining compatibility and tie-breaking checks.
- Populated `record_address` only for reference winners. Optimized base and segment materialization keep their original ID lookup and payload routes.
- Extended locator, logical and bytes assertions for reference payload materialization.

### Regressions and counters

`reference_backlink_sort_streams_discovery_without_target_cache_budget` builds two targets with distinct linked keys and nine valid unlinked source records whose old retained-cache estimate exceeds 64 MiB. It verifies:

- the former loader fails with `sort target cache budget exceeded`;
- reference search returns hand-derived order `["A", "B"]` with complete count `2`;
- one discovery read occurs for each of the 18 visible logical sources across both target lookups;
- discovery bytes are relation source work;
- actual linked-target key evaluation uses three sort-target reads;
- removing one discovery source reports a diagnostic, returns `count=None`, and does not claim complete matches.

`optimized_binding_defers_reference_sort_preparation` proves ordinary binding leaves the stored source representation untouched and records no reference collator construction. Explicit reference binding constructs exactly one source evaluator in the same test.

The existing stored-key perturbation, segmented FIRST/LAST perturbation, relation forward/backlink perturbation, provider-order and restricted-domain tests remain passing.

### Validation actually run

Each focused invocation executed at least one test:

- `relation_query::tests::reference_backlink_sort_streams_discovery_without_target_cache_budget`: 1 passed.
- `query::sort_binding::tests::optimized_binding_defers_reference_sort_preparation`: 1 passed.
- `relation_query::tests::base_reference_`: 3 passed.
- `relation_query::tests::segmented_reference_first_last_ignores_unavailable_contribution`: 1 passed.
- `relation_query::tests::segmented_reference_relations_ignore_perturbed_forward_and_backlink_contributions`: 1 passed.
- `query_execution::runner::tests::gate_a_public_search_selects_either_provider_in_either_order`: 1 passed.
- `query_execution::runner::tests::selective_candidate_and_broad_exact_or_share_the_restricted_domain`: 1 passed.

Final gates at tested code commit `4935431b3fde7b7ec4d121871f1f8d81fe6d6978`:

- `cargo fmt --all -- --check`: passed.
- `cargo check --locked --workspace --all-targets`: passed.
- `cargo test --locked --workspace`: passed; 591 core unit tests passed, 16 ignored, plus all workspace integration and doc tests.
- `cargo test --locked -p objekte-search-core --all-features`: passed; 592 core unit tests passed, 16 ignored, plus all integration and doc tests.
- `RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc --locked --workspace --no-deps`: passed.

No separate benchmark was required. The regression measures source rediscovery and retained-cache work through exact counters rather than wall time, and the existing optimized provider/order tests cover the unchanged production baseline.

### R6 integration validation

- Source report head: `e226c6c1293bae57976da9af809df4e925dcab6b`
- Tested R6 merge revision: `33d4b9085f44fc920f47dba791aa974692c57c02`
- R6 branch: `repair/core-lifecycle-coordination`

The corrected source branch merged into the reviewed R6 head without conflict.
The combined tree passed:

- `cargo fmt --all -- --check`
- `cargo check --locked --workspace --all-targets`
- `cargo test --locked --workspace` (592 core unit tests passed, 16 ignored, plus all workspace integration and doc tests)
- `cargo test --locked -p objekte-search-core --all-features` (593 core unit tests passed, 16 ignored, plus all integration and doc tests)
- `RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc --locked --workspace --no-deps`
- `cargo test --locked -p objekte-search-core --lib lifecycle::` (88 passed, 6 ignored)

The ignored release regression used the default record count with the
environment override unset. It reported `records=262144`, `attempts=2`,
`before=262146`, `after=262145`, `retired=1`, and exactly one passing test.

### Review notes

No unresolved functional issue or blocked check remains. The source report-only
commit follows tested code commit
`4935431b3fde7b7ec4d121871f1f8d81fe6d6978`; this R6 report-only commit follows
tested merge `33d4b9085f44fc920f47dba791aa974692c57c02`. Each report commit's
only delta is completion documentation.
