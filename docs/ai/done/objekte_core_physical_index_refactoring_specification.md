# Assignment 3 — Physical indexes and segment transformations

**Task ID** `core-physical-indexes`  
**Integration branch** `integration/core-physical-indexes`  
**Sibling worktree** `/home/simon/source/objekte-integration-core-physical-indexes`  
**Reviewed master** `407410cbc28ae2a2c6fa89ba0e6af2458157c472` (includes Assignment 1)

## Outcome

Give the physical representation of a finished index one owner under `crates/core/src/index/`. A maintainer changing the storage of a numeric posting, the layout of a gram index, or the translation of local slots during compaction should find the implementation there. A maintainer changing which index a FILTER or FTS term uses should continue to work in query planning and query access. Preserve lookup results, costs, publication behavior and the current external Rust API at the integrated base.

| Operation | Owner after this assignment |
| --- | --- |
| Read files, extract records, schedule workers, seal batches and create a segment | Assignment 2, `construction/` |
| Represent a finished segment, look up stored membership, project survivors into new slots, consolidate finished segments | Assignment 3, `index/` |
| Select segments for compaction, resolve current file versions, enforce work budgets, handle cancellation, publish a revision | Lifecycle |
| Offer access paths, select by estimated cost, apply an operation to the current input, verify residual predicates | Existing query providers, planner and runner |

This assignment changes internal ownership. Keep the native postings, Roaring bitmaps, gram alphabets, ICU collation, relation layout, existing serialization contracts and published segment identities. A storage replacement, new backend trait or query-optimizer rewrite requires its own task. [Roadmap source inventory](https://github.com/Simon-Martens/objekte/tree/407410cbc28ae2a2c6fa89ba0e6af2458157c472/crates/core/src)

## Coordinate with Assignment 2

Start from the current `origin/master` in the task worktree and record the SHA. Family modules, read contracts and their tests can be reviewed and moved immediately. Leave the mixed `mutation_segment.rs`, its test file, and the re-exports in `build.rs` for the integration step if Assignment 2 still owns edits there. Once Assignment 2 is available on master, integrate that master into this branch and extract the physical operations from their actual location. Do not copy the old file over the construction changes. If the assignments are reviewed in parallel, identify which commits were tested together and repeat the combined checks before delivery. Preserve Assignment 2’s single `Index::start_build` API wherever it has landed.

The dividing line on the reviewed revision is precise. `SegmentAdmission`, `CollectionSegment`, `SegmentBatch`, `SegmentAggregate`, and `CollectionSegment::into_indexed_profiled` belong to construction. The first physical transformation is `IndexedCollectionSegment::project_inference_for_partitions` at line 665. Everything from that implementation through `compact_with_mapping` operates on finished indexed data. `SegmentLimits` serves admission. The physical `SegmentPolicy` type and its existing public path remain available to both callers. [Shared source file](https://github.com/Simon-Martens/objekte/blob/407410cbc28ae2a2c6fa89ba0e6af2458157c472/crates/core/src/mutation_segment.rs#L664) · [Original re-exports](https://github.com/Simon-Martens/objekte/blob/407410cbc28ae2a2c6fa89ba0e6af2458157c472/crates/core/src/build.rs#L2618)

## Module and symbol assignments

Add private `mod index;` to `lib.rs`. Keep its `mod.rs` limited to declarations and deliberate re-exports. Use the following destinations, relative to `crates/core/src/index/`. Keep moved methods inherent on their existing types. Move the implementation and its relevant tests together.

| Destination | Contents |
| --- | --- |
| `segment/mod.rs` | Move `native_segment.rs`’s `SegmentPolicy`, `SegmentId`, `SegmentRecordAddress`, `SegmentRecordSet`, `SegmentFile`, `IndexedCollectionSegment`, `PublishedSegment`, `SegmentReadView`, `NativeSnapshotRef`, component descriptors, accounting and accessors. Keep the local-slot namespace and borrowed views intact. |
| `segment/consolidation.rs` | Move `ConsolidatedSegment`, `translate_active`, `merge_bigram_index`, `consolidate_inference`, and `IndexedCollectionSegment::consolidate`. This operation combines compatible segments and returns the offsets needed to translate their original liveness masks. |
| `segment/compaction.rs` | Move `CompactedSegment`, `compact_active`, `remap_dense`, `compact_active_mapped`, `compact_partition_with_postings` and `compact_with_mapping`. This operation constructs the destination records, file entries, diagnostics and indexes using a slot mapping. |
| `segment/projection/mod.rs` and `routing.rs` | In `routing.rs` put `PartitionRouting`, `SlotMap`, bitmap projection/scattering and bounds checks. In `mod.rs` keep `DomainProjection`, `project_domains_for_partitions` and the small `project_relations_for_partitions` orchestration method. Retain the relation-owned occurrence mapper. |
| `segment/projection/{postings,text,sorting,inference}.rs` | Place each family's scalar projection and multi-part scatter together. Postings owns `PostingProjection`, empty projected dictionaries, posting-key scatter and `project_postings_for_partitions`. Text owns `BigramProjection`, dense/partition gram projection and `project_bigrams_for_partitions`. Sorting owns `project_sorting` and `project_sorting_for_partitions`. Inference owns dense projection, `finish_projected_inference` and `project_inference_for_partitions`. Reuse the posting and bitmap helpers needed by inference. |
| `posting/mod.rs` and `posting/lookup.rs` | Move the existing `posting.rs` family. Keep `ScalarKey`, `PostingIndex`, `PostingCollection`, structural facts, population, validation, merge and statistics in the parent. Put value cursors, dictionary lookups, exact/restricted evaluation, range and temporal probes, estimates and their private helpers in `lookup.rs`. Preserve borrowed leaf returns, visit counters and bounded range iteration. |
| `bigram/mod.rs` and `bigram/builder.rs` | Move the existing `bigram.rs` family. Keep folding, gram encoding, `BigramRequest`, `BigramIndex` and lookup in the parent. Move packed columns, block/chunk conversion, construction counters, `BigramContribution` and `BigramBuilder` into `builder.rs`. Construction calls that representation-specific builder. Reuse the same gram encoding for query requests and record ingestion. |
| `sorting/mod.rs` and `sorting/inferred_order.rs` | Move `sorting.rs`. Keep public collation/configuration types, sort keys and columns, composite order, sort construction and comparison in the parent. Put the inferred-order cache key, entry, stats and cache implementation in `inferred_order.rs`. Keep `sort_binding.rs` with its existing query caller. |
| `relation.rs` inside `index/` | Move physical `RelationIndex`, `RelationEdge`, `RelationTarget`, `RelationContribution`, private `PendingReference`, occurrence traversal, index validation and its test counter together. Keep declaration-facing `RelationCardinality`, `RelationDefinition`, `RelationId`, `impl Schema`, and declaration validation in the existing crate-root `relation.rs`. That file re-exports the existing physical types at their former visibility. Keep private-field tests with the physical owner. |

**Compatibility.** Keep public `objekte_search_core::{native_segment, posting, bigram, sorting, relation}` paths working, including constants, methods and serde behavior. For example, preserve `native_segment::SegmentPolicy` with a module re-export, and keep `relation::RelationIndex` as a re-export of the moved type. Use Rust visibility only as wide as a real caller requires. Children can use explicit internal re-exports for siblings. Eliminate the old dependency on `use super::*` from `build` for production transformations. Keep any `crate::build` or construction re-export that Assignment 2 still consumes through a direct re-export of the existing type. An external consumer must not need a new `index` import. [Current public modules](https://github.com/Simon-Martens/objekte/blob/407410cbc28ae2a2c6fa89ba0e6af2458157c472/crates/core/src/lib.rs) · [Relation storage and declarations](https://github.com/Simon-Martens/objekte/blob/407410cbc28ae2a2c6fa89ba0e6af2458157c472/crates/core/src/relation.rs#L67)

Keep `posting_values.rs` and `relation_inspection.rs` with their current public APIs. They coordinate selections, pagination or inspection across published state. Likewise keep `index_catalog.rs` at the existing native query-adapter boundary. It assesses operations and exposes physical lookups there. Keep `relation_query`, `relation_semijoin`, `relation_target_cache`, query execution, and the independent reference evaluator with their present owners. Shared `inventory`, logical model types and schema binding retain their paths.

## Query contract and ownership of results

Trace one structured term and one FTS term through `IndexAccessProvider::propose` and `apply` before the first move. The planner must still receive offers carrying cost, coverage and proof strength. The chosen provider receives the current domain-qualified `SlotInput` and returns a `Restriction` containing possible and proven membership. Preserve unsupported-operation behavior, exact empty results, mismatched-domain rejection, and the verification step for conservative gram matches. The planner must still be able to choose a selective text term first, then pass the narrowed input to a structured provider.

The physical reader can borrow a posting leaf or iterate a bounded dictionary range. Keep that access within the provider and its physical adapter. Do not expose trees, gram maps, relation adjacency, the active bitmap or native segment fields to additional planner or runner code. Preserve public low-level inspection APIs already used by consumers. Do not copy query results across the new boundaries. Forward borrowed or shared membership as supplied, and move owned results. A new `.clone()`, `.to_vec()`, `.collect()` of slots or conversion to an owned bitmap merely to return, compare, or pass a result fails review. Preserve the allocations required to compute a genuine union, intersection or remapped *destination* when the existing operation requires it. [Query access contract](https://github.com/Simon-Martens/objekte/blob/407410cbc28ae2a2c6fa89ba0e6af2458157c472/crates/core/src/query_access/contract.rs#L301) · [Independent provider and mixed-term tests](https://github.com/Simon-Martens/objekte/blob/407410cbc28ae2a2c6fa89ba0e6af2458157c472/crates/core/src/query_access/independent.rs)

## Transformation invariants

1. **Segment addresses.** A slot is local to its collection and segment. Two segments may both have slot zero. Maintain that separation until an explicit route translates each survivor. Preserve record identity, file identity and revision, byte ranges, identity order, and local ID lookup. Retain checked arithmetic and the same errors for overlapping files/IDs, invalid masks, duplicate partition assignment and record-budget overflow.
2. **Consolidation and compaction differ.** Consolidation concatenates compatible physical slot domains and translates the original active masks using offsets. Compaction can remove tombstones and returns the existing old-to-new mapping where applicable. Preserve dense remapping for verified updates, where a replacement must occupy the required slot. Keep file-version selection with lifecycle, including current zero-record files and removal of obsolete file versions. Keep diagnostics attached to the correct surviving record and file.
3. **Project all index families.** Preserve declared/inferred/custom value postings, presence, null and structural facts, short-string and empty-selection coverage, logical/empty-array domains, raw/fallback/field-specific grams, stored sort keys and composite tie order. Preserve relation occurrence positions, logical target IDs and unresolved references through remapping. Keep inference reporting and accounting aligned with the completed payload. Raw-fallback records remain eligible for the same searches.
4. **Keep physical transformations source-free.** They operate on the finished segments, masks and supplied routing data. Preserve the existing single shared partition routing table, multi-destination family scatter, bounds on work, and reuse of unaffected segment payloads. Do not allocate a source-sized slot table for each output part, reparse input, rebuild postings from source, or reconstruct reverse relations to answer an ordinary query. Keep retained views valid while a new physical representation is published.
5. **Leave lifecycle decisions in place.** `lifecycle/publication.rs` still chooses a pair and installs the revision. `lifecycle_partition.rs` determines retained current file memberships, limits and cancellation checkpoints. `lifecycle_compaction.rs` owns the policy and reports. `lifecycle_verified.rs` still decides when a verified replacement may use dense remapping. Only adjust imports to reach the physical operations. [Publication caller](https://github.com/Simon-Martens/objekte/blob/407410cbc28ae2a2c6fa89ba0e6af2458157c472/crates/core/src/lifecycle/publication.rs#L441) · [Partition orchestration](https://github.com/Simon-Martens/objekte/blob/407410cbc28ae2a2c6fa89ba0e6af2458157c472/crates/core/src/lifecycle_partition.rs#L41) · [Verified replacement](https://github.com/Simon-Martens/objekte/blob/407410cbc28ae2a2c6fa89ba0e6af2458157c472/crates/core/src/lifecycle_verified.rs#L352)

## Acceptance evidence

Run the relevant tests on the integration base before moving them. Preserve each test body and its original assertions through the move unless Assignment 2 has migrated fixture construction. Split the mixed segment test file by operation. Admission and the existing ignored admission measurement stay with construction. Transformation tests move with their new owner. Reuse the fixture helpers through test-only imports. Never make a production field public to compile a moved test.

| What must survive | Named tests on the reviewed revision |
| --- | --- |
| Independent backing and FTS/FILTER term ordering | `sorted_scalar_and_gram_backing_match_reference_membership`, `selective_text_reduces_the_input_to_the_structured_lookup`. Run the membership oracle and domain-identity tests too. |
| Slot coverage, fallback and source-free consolidation | `consolidation_matches_direct_admission_without_source_files`, `seventy_small_shards_consolidate_after_sources_are_removed`, `compaction_matches_active_file_oracle_across_indexes`, `compaction_remaps_retained_fallback_diagnostics`. |
| Current file versions and bounded partitions | `historical_file_versions_compact_to_current_zero_record_or_delete`, `compacts_one_large_file_in_source_free_parts`, `partition_and_native_publication_keep_global_inference_diagnostics`. |
| Unaffected segment reuse and pinned readers | `compaction_reuses_unaffected_native_segment`, `background_compaction_preserves_pinned_reader_and_reports_completion`, `bounded_compaction_can_resume_remaining_retired_shards`. |
| Physical lookup, order and relations | `narrow_ordered_ranges_visit_only_matching_dictionary_keys`, `ascii_and_unicode_candidates_are_conservative`, `shifted_blocks_match_incremental_bitmaps_at_chunk_and_container_boundaries`, `mixed_scalar_sort_keys_obey_total_kind_order_and_comparator_laws`, `nested_array_occurrences_retain_every_scope_position`. |

Check that the existing oracles distinguish two segment-local slot-zero records and a retired slot after translation. If that specific case is missing, extend the nearest consolidation fixture with two input segments, retire one local zero, and assert that the other local zero remains live at its translated offset. Keep a second live record in the first segment so an implementation that drops the whole segment also fails. Exercise the production transform and compare stable record IDs and liveness, not just counts.

Run formatting, all core targets, the core suite and the CLI consumer on the combined head. Compile a tiny downstream import check for the five compatibility module paths if existing workspace targets do not cover them. Include the all-features check and compare any failure to the exact base. The reviewed master has a documented `detailed-profiling` compile failure in `query_results/cache.rs`. If an intervening revision fixes it, the final branch must pass. [Baseline issue](https://github.com/Simon-Martens/objekte/blob/407410cbc28ae2a2c6fa89ba0e6af2458157c472/docs/ai/issues/bug/detailed_profiling_missing_module.md)

```sh
cargo fmt -p objekte-search-core -- --check
cargo check --locked -p objekte-search-core --all-targets
cargo test --locked -p objekte-search-core
cargo check --locked -p objekte-cli
cargo check --locked -p objekte-search-core --all-targets --all-features
```

For a mechanical move, compare the moved bodies against the base and report any substantive changes in iteration, cloning, allocation, source work or locking. The existing tests and comparison suffice when the hot paths are identical. If a hot path changes, measure only that operation on base and head in release mode, with identical input, partition limits and worker count. Report median elapsed time over three measured runs per revision and peak resident memory for physical transforms. The complete query suite on Musenalm is unnecessary here.

## Commit order and delivery

1. Record the base, visibility/import inventory and relevant tests. Capture existing test names so a misplaced `#[cfg(test)]` cannot silently drop coverage.
2. Establish the private index owner and public re-exports. Move the native segment definition and family modules. Split storage-specific lookup/build/cache code as specified. Keep each commit compiling.
3. Integrate the Assignment 2 boundary. Move consolidation and its tests, then routing, family projections and compaction. Remove the duplicate module declarations and obsolete physical imports from construction. Verify that constructing a segment never calls projection merely to transfer ownership.
4. Validate the combined result through public queries and lifecycle updates. Compare the commit diff for copies, extra traversals and public API changes. In the completion report record the base SHA, Assignment 2 revision integrated, implementation SHA, symbol destinations, test outcomes and any baseline-only failure. Follow `docs/ai/README.md`, commit and push each task change on the integration branch, and deliver the tested head for the owner's merge decision.

## Implementation report

- Task identifier: `core-physical-indexes`
- Completion date: 2026-11-18
- Base commit: `1811f49160e45e9e5a827cfa8b9bfca42cbf9a5f`
- Assignment 2 revision integrated: `1811f49160e45e9e5a827cfa8b9bfca42cbf9a5f`
- Implementation commit: `8b6b881f`
- Integration branch: `integration/core-physical-indexes`
- Integration worktree: `/home/simon/source/objekte-integration-core-physical-indexes`

The finished-index representation now has one private owner under
`crates/core/src/index/`. `lib.rs` re-exports `bigram`, `posting`, `sorting`
and `segment` as `native_segment`, while the crate-root `relation` module keeps
relation declarations and re-exports the physical relation storage types.
Existing external Rust paths and visibility are preserved; callers do not need
an `index` import.

Symbol destinations:

- `index/segment/mod.rs`: native segment policy, identity, record sets, files,
  indexed segment payloads, published/read views, component descriptors,
  accounting, accessors and native physical tests.
- `index/segment/consolidation.rs`: `ConsolidatedSegment`,
  `translate_active`, shifted bigram merging, inference consolidation and
  `IndexedCollectionSegment::consolidate`.
- `index/segment/compaction.rs`: `CompactedSegment`, dense remapping,
  partition compaction and slot-mapped destination construction.
- `index/segment/projection/{mod,routing,postings,text,sorting,inference}.rs`:
  domain projection, shared slot routing/maps and bounds, family-specific
  scatter/projection helpers, and the small relation orchestration method.
- `index/posting/{mod,lookup}.rs`: posting representation, population,
  validation, shifted merge and statistics in `mod.rs`; borrowed cursors,
  dictionary probes, estimates, exact/restricted/range/temporal lookup and
  helpers in `lookup.rs`.
- `index/bigram/{mod,builder}.rs`: folding, gram encoding, requests,
  `BigramIndex`, lookup, validation and statistics in `mod.rs`; packed columns,
  chunks, blocks, construction counters/contribution and `BigramBuilder` in
  `builder.rs`.
- `index/sorting/{mod,inferred_order}.rs`: collation, sort keys, columns,
  builders, composites and comparison in `mod.rs`; inferred-order cache key,
  entries, statistics, bounded cache and inferred column construction in
  `inferred_order.rs`.
- `index/relation.rs`: physical edge/target/CSR types, contribution state,
  occurrence traversal, projection/resolution, index validation and the
  reverse-reconstruction test counter. The crate-root `relation.rs` retains
  cardinality, definitions, IDs, `Schema` methods and declaration validation.

The transformation test file was split by operation. Consolidation tests now
live in `index/segment/consolidation.test.rs`; compaction tests live in
`index/segment/compaction.test.rs`. The admission and profiling-gated admission
tests remain with construction. A new regression,
`consolidation_translates_independent_local_zero_slots`, starts with slot zero
in both input segments, retires only the second local zero, keeps another live
record in the first segment, and verifies the surviving zero at offset zero and
the surviving second-segment record at offset three.

The moved implementation bodies retain their existing iteration, cloning,
allocation, source access and locking behavior. Changes are limited to module
placement, explicit imports/visibilities, test registration, declaration
re-exports and the added slot-translation regression. No physical transformation
hot path changed, so a before/after transform benchmark was not required.

### Validation

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed. |
| `cargo check --locked -p objekte-search-core --all-targets` | Passed. |
| `cargo test --locked -p objekte-search-core` | Passed: 581 unit tests, 16 ignored, all core integration tests and the doc test. |
| `cargo check --locked -p objekte-cli` | Passed; one pre-existing unused-import warning in CLI code. |
| `cargo check --locked -p objekte-search-core --all-targets --all-features` | Passed. The detailed-profiling baseline issue no longer fails. |
| `cargo test --locked --workspace` | Passed. |
| External import check | Passed for `native_segment`, `posting`, `bigram`, `sorting` and `relation`, including `SegmentPolicy`, `SegmentId`, `PostingIndex`, `ScalarKey`, `BigramIndex`, `SortOptions`, `RelationCardinality`, `RelationDefinition`, `RelationIndex` and `RelationTarget`. |

The complete suite covers the required independent FTS/FILTER ordering,
membership oracles, domain identity, source-free consolidation and compaction,
historical file versions, bounded partitions, unaffected segment reuse,
pinned-reader behavior, lookup visit bounds, mixed scalar order, gram
boundaries and nested relation occurrence positions. No baseline-only failure
remains. Temporary compatibility-check data was removed. The task memory is
removed after the report commit. The integration branch and worktree are
retained for the owner's merge decision.
