# Objekte repair specification for interchangeable index storage

**Assignment** `index-storage-backend-repair`  
**Reviewed branch** `integration/index-storage-backends` at `9aca99af2525fbd27973d7494273a50c37dc730b`  
**Implementation checkpoint** `c760904257c05cb50ccc50959021a78408378e12`  
**Comparison baseline** `master` at `cf7253d048c9d6fe5d2a7e9b47441b2207cce932`  
**Review decision** Hold the merge. Preserve the existing memory backend and repair the contracts and production paths below. [Original assignment and implementation report](https://github.com/Simon-Martens/objekte/blob/9aca99af25/docs/ai/done/objekte_index_storage_backend_specification.md)

Start the repair from the reviewed integration head in a sibling worktree, following `docs/ai/README.md`. Production disk storage, mmap, persistence commands and public backend-selection APIs remain future work. Use the existing independent test backend to exercise the repaired contracts. A useful submission consists of focused commits, the regression tests below and a short implementation report. Leave the merge to the maintainer.

## 1. Preserve file catalog construction state during restore

**Defect.** `FileCatalog::restore_revision` preserves file versions, then calls `restore_base` through a narrower source interface. The restored value unconditionally sets `construction_complete: true`. Restoring a paused revision therefore changes its meaning. Strict search reads this flag, and publication uses it to decide whether to track processed file memberships. The defect is demonstrable at the restore boundary even when a particular maintenance caller supplies only completed revisions. [Restore](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/index/memory/file_catalog.rs#L387-L414) · [Strict search](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/lifecycle/view.rs#L742-L746) · [Publication](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/lifecycle/publication.rs#L392-L419)

**Repair.** Read the source revision’s completion flag and pass it explicitly into private catalog construction. For any caller creating a catalog from rows alone, pass the state warranted by that caller. Keep completion changes at the existing lifecycle transition.

**Regression test.** Restore both an incomplete and a completed revision. Exercise native and independent source readers. Compare completion, file status, versions and allocation high-water mark, including a retired ID. On a restored incomplete root, strict search must still reject the request and available search must still report partial construction. Finish construction through the normal path and verify that the earlier pin remains incomplete.

## 2. Open only the segment directories a query needs

**Defect.** Segmented `IndexView::search` calls `segment_groups()` before the runner selects the query collections. It visits every directory. When a directory does not supply `shared_placements()`, the fallback reads its pages to EOF and collects all placements. A search scoped to collection A can therefore fail on a placement read in unrelated collection B. The native shared-vector path conceals the extra work. Streaming and several repository entry points also call this helper. Direct lifecycle record lookup already uses the pinned route and a single placement. [Search](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/lifecycle/view.rs#L749-L772) · [Directory enumeration](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/lifecycle/publication.rs#L140-L152) · [Fallback](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/index/storage/segment_catalog.rs#L166-L197) · [Routed lookup](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/lifecycle/view.rs#L1195-L1212)

**Repair.** Pass the pinned directory handles into planning and execution. Resolve collection scope from bound query information, then obtain placements per participating collection. Resolve relation targets, including backlinks, using the query’s actual dependencies. Share per-query lookups across planning, execution, ordering and delivery. When a scan needs a collection’s segments, consume descriptor pages incrementally where possible. Preserve stable collection and segment identities for cache hits and continuation pages. Retain the shared native path and explicit repository-wide enumeration for operations that require it.

**Regression test.** Give A a short-page directory. Make B count placement reads and return a distinctive error on the first one. A non-relational search, a warm cached repeat, streaming and scoped explain must succeed with zero B placement reads. Check a routed ID hit and miss as controls. Enable B for a cross-collection relation and verify the expected matching IDs. Inject an error into a required B read and verify it reaches the caller.

## 3. Keep storage failures distinct from unavailable source files

**Defect.** Cached delivery first resolves a locator, then reads the payload. An error from the second call can originate in a segment header, origin, catalog lookup or actual source-file read. The interface collapses these into a string. The caller probes the source path after any such error and skips the hit if the path is missing or unreadable. An index-page failure can consequently appear as a successful response with an unavailable-source diagnostic. [Cached delivery](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/query_results/cache.rs#L310-L367) · [Record reads](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/query_execution/runner/mod.rs#L660-L750) · [Error reclassification](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/query_execution/runner/mod.rs#L1278-L1304)

**Repair.** Carry a typed internal outcome from the operation that failed through cached and uncached delivery. Only a source-read failure is eligible for the existing source-unavailable policy. Propagate index storage errors, invalid routes, cancellation and resource errors under their existing public semantics. Inspect the other callers of `record_unavailable_hit` for the same loss of provenance. Keep the public error surface compatible where practical. A cache hit already contains the pinned slot, so reuse it.

**Regression test.** Warm a file-backed query. Allow locator resolution, remove the source file, and inject a distinctive storage failure on the subsequent payload-phase header or origin read. The request must report the storage failure. Repeat with healthy index reads and the same missing file and verify the ordinary unavailable-source diagnostic. Exercise the equivalent cold delivery path if it shares the classification code.

## 4. Select component writers before workers build the index

**Interface gap.** A native segment can query an independently stored declared sort column. Its writer nevertheless receives a complete native column’s keys and both order vectors at seal time. Projection repeats that construction. The factory’s inferred order input also requires complete vectors. Normal native segments own concrete postings, text and relation containers, without an equivalent independent placement hook for those components. Substituting an entire segment does not establish that individual components can be placed independently. [Placement](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/index/memory/sorting/placement.rs#L30-L97) · [Order writer inputs](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/index/storage/order.rs#L112-L145) · [Factory](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/index/storage/mod.rs#L25-L106) · [Native segment fields](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/index/memory/segment/mod.rs#L455-L479)

**Repair.** Resolve placement by logical component identity before its first worker contribution. Supply the selected writer with bounded batches of extracted facts or keys and the established slot assignment. Let it own accumulation, merge and sealing, then attach its immutable reader to the segment. Preserve the native bulk-adoption path, buffer reuse, deterministic ties and current collation. Use this selection for postings (declared, named and inferred), bigrams, ordering and relations in both file and direct ingestion. Late inference may select a writer once its binding is known and feed it bounded batches from existing logical data. Projection must also honor the destination. Keep dispatch at batch boundaries.

**Regression test.** Extend the existing mixed-placement test in both directions. Build native postings with independent sort, then independently written postings with native sort in the same segment. Count input batches and native constructions for the selected component. Use enough rows for multiple batches, require zero native constructions for the independently placed component, and compare exact matches and ordered IDs in both directions, including ties and missing values. Verify compaction preserves the selected placement. [Existing fixture](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/index/storage/segment_catalog/tests.rs#L635-L766)

## 5. Make source-free maintenance honor the selected destination

**Interface gap.** The memory destination’s logical importer accepts only `FileRange` segments without a source registration. Partitioning a foreign segment tries that import once per partition and, on a decline, delegates the whole operation to the source’s maintenance implementation. The returned representation then depends on the source. The record transfer code emits retained bodies in chunks only after asking the source for the complete `RetainedBody`. Unrelated to that limitation, automatic consolidation stops on the first factory-declined candidate pair, even if a later pair is eligible. [Importer](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/index/memory/segment/import.rs#L25-L52) · [Partition fallback](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/index/memory/mod.rs#L30-L61) · [Retained transfer](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/index/storage/segment/transfer.rs#L274-L304) · [Consolidation selection](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/lifecycle/publication.rs#L589-L602)

1. Extend the destination builder to accept file ranges, retained contents and versioned provider descriptors through checked logical blocks. Supply a bounded retained-body read operation so an independent reader can send parts without decoding the entire body first. Preserve provider registration and version without calling the provider. Reuse the native maintenance fast path when it honors placement.
2. Use one source traversal per component where practical when repartitioning. The current foreign import creates a source-sized slot mapping and traverses posting keys per output partition. Build the routing map once and distribute blocks to destination partitions. Keep working buffers bounded by the established limits. [Mapping](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/index/memory/segment/import.rs#L130-L140) · [Posting transfer](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/index/memory/segment/import.rs#L245-L256)
3. Define a factory decline separately from a failed read. When a maintenance operation requires a destination, complete it in that destination or return an explicit error before installation. For optional automatic consolidation, continue to the next policy-eligible candidate after a decline. Keep genuine errors visible and preserve existing size and publication constraints.

**Regression tests.** Extend the existing file-range round trip with retained and provider-backed fixtures. Transfer memory → independent → memory with the original source detached and provider reads counted. Require zero source/provider reads during transfer. Verify the destination reader’s identity, record IDs, versions, optional file links, liveness, order, and coverage. Compare both identity layout and an explicit dense remapping. Split a retained body across several reads and fail or cancel after the first block. The previous pin must remain usable with no partial publication. For partitioning, compare source posting-block reads for one and four outputs over the same input. For consolidation, arrange three eligible segments with a declined first pair and an accepted later pair. Verify the accepted pair merges and all records remain reachable. [Existing round trip](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/index/storage/segment_catalog/tests/segments/transfer.rs#L616)

## 6. Allow fallible paged liveness and membership

**Interface gap.** `LiveSlotReader::contains` returns `bool`, and `visit_blocks` has no error return. With a partially live segment, the backend must already have the relevant data resident to satisfy those contracts reliably. `ActiveRecords::bitmap()` reconstructs a bitmap each time, and the provider restriction interface still requires a full `&RoaringBitmap` as its operation input. These choices matter for sparse live masks and selective relation or predicate queries. [Liveness contract](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/index/storage/liveness.rs#L13-L21) · [Materialization](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/index/storage/liveness.rs#L109-L121) · [Provider input](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/query_access/membership.rs#L298-L325) · [Relation use](https://github.com/Simon-Martens/objekte/blob/9aca99af25/crates/core/src/query_execution/runner/relations.rs#L395-L402)

**Repair.** Give backend-dependent membership tests and block iteration checked results. Keep descriptor counts and the known-all/known-empty shortcuts cheap. Allow provider inputs and results to borrow pinned membership or traverse guarded blocks. A query can still allocate a bitmap for a genuine set operation. A selective lookup should touch only the necessary blocks. Preserve domain checks, complement semantics, and exact versus candidate-only evidence. Keep allocations and dynamic calls out of the native per-slot hot loop.

**Regression test.** Use a sparse independent mask that exposes short blocks and can fail on the second refill. Check a point lookup with untouched pages configured to fail, then a full operation that genuinely requires the failing page. The first must succeed without visiting that page and the second must propagate the injected error. Compare exact counts and complement results against memory, including retired slots.

## Verification and merge criteria

Run focused tests at each checkpoint. For the completed repair run the same checks recorded for the original branch.

```sh
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
cargo test --locked --workspace --features objekte-search-core/test-scan-instrumentation
cargo test --locked --release -p objekte-search-core --lib transfer
```

The original report records 697 passing core tests, 16 ignored and 150 matching Musenalm query digests. ASCII auto-worker construction improved from 665.74 to 631.46 ms and peak RSS from 643.16 to 612.89 MiB. Preserve those gains. The following two timings deserve a focused comparison. [Validation](https://github.com/Simon-Martens/objekte/blob/9aca99af25/docs/ai/done/objekte_index_storage_backend_specification.md#L241-L255) · [Build and digest results](https://github.com/Simon-Martens/objekte/blob/9aca99af25/docs/ai/done/objekte_index_storage_backend_specification.md#L275-L291) · [Stress and first-row samples](https://github.com/Simon-Martens/objekte/blob/9aca99af25/docs/ai/done/objekte_index_storage_backend_specification.md#L339-L381)

| Release workload | Master median | Reviewed implementation median | Evidence needed |
|---|---:|---:|---|
| Eight-file update batch, `stress / batch_small_8` | 4.476 ms | 6.058 ms | All three final pairs were slower. Count touched catalog rows, placements, and any full enumerations per batch. Separate preparation and publication time. |
| Stored residual, first row, complete count disabled | 38.7 µs | 60.5 µs | Six samples overlap, with one record verified and one row emitted on both revisions. Measure directory reads, live-mask materializations and allocations before the first row. |

Use identical release flags, dataset, worker count and query options. Alternate revisions in each pair. First run the two focused comparisons for seven pairs on the same machine, using enough repetitions within each run to resolve the short first-row time. Compare master and the repair, and use the reviewed checkpoint if attribution is needed. Report medians, raw pairs and work counters. Re-run the 150 query digests and one representative auto-worker build/RSS comparison. The original report leaves the cause of both differences open. If a slowdown persists, localize the work and report the residual cost and uncertainty. Store the compact results under `docs/ai/generated/` so the reviewer can inspect them.

Submit for merge review when the defect reproductions pass, the independent backend demonstrates bounded reads and writes through production entry points, and the report distinguishes verified fixes from outstanding performance differences. This review traced the pinned source and checked the committed results. The review environment lacked a Rust toolchain, so the test counts and timings above are the implementing agent’s reported runs.

## Implementation report

Completed 2026-11-26 on `integration/index-storage-backend-repair` in
`/home/simon/source/objekte-integration-index-storage-backend-repair`.
The repair starts at the reviewed revision `9aca99af2525fbd27973d7494273a50c37dc730b`.

| Commit | Change and regression coverage |
| --- | --- |
| `2023052d59` | Restore preserves incomplete construction for native and independent catalogs. The tests check inactive file versions, catalog holes, strict and available queries, and an incomplete pin held through completion. |
| `38513f8ca1` | Typed source-read outcomes preserve storage failures through cached, cold and streaming delivery. The regression injects an origin-read failure after warming the cache and removing the source file. Healthy storage still produces the ordinary source diagnostic. |
| `64045cdd26` | Search, streaming and explain share a query-scoped view of pinned directories. The independent fixture checks scoped and cached search, a routed hit and miss, relation and backlink traversal, and a failure in a required directory. |
| `cae8ed412a` | Destination-controlled maintenance imports file ranges, retained bodies and provider descriptors. Tests verify identity and dense layouts, failure after a retained-body part, zero provider reads, equivalent posting-block counts for one and four outputs, and eligibility retry after a consolidation decline. |
| `9cc8bf205a` | Pinned liveness exposes checked point and block reads. A sparse independent fixture proves that point lookup and a selective predicate leave the second page unread while a full operation propagates its injected error. |
| `47d17ef504` | Component stores receive batches for declared, named and inferred postings, text grams, ordering and relations. The file and direct ingestion tests check mixed native and independent representations, multiple batches, exact matches, ties, missing sort values, failure before publication, and source-detached compaction. |

The following commands passed on the combined implementation.

```sh
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
cargo test --locked --workspace --features objekte-search-core/test-scan-instrumentation
cargo test --locked --release -p objekte-search-core --lib transfer
```

Each workspace run passed 706 core tests with 16 ignored. The release transfer
selection passed nine tests.

The [release comparison](../generated/index-storage-backend-repair-results.md)
records the paired measurements, work probes, digest checks, and remaining
performance uncertainty. The [raw samples](../generated/index-storage-backend-repair-results.json)
include all first-row measurements and eight-file batch timings.

The native directory path shares its placement list. Selected independent
directories still retain the placements collected from checked pages for query
execution. The foreign multipart importer shares routing and traverses posting
and gram blocks once. It walks the source header and file directories per output
partition to assemble the selected rows. Independent structured postings supply
exact scalar equality where coverage permits. Other predicate shapes can still
use source verification. Inferred order can use an independent writer fed by a
native posting dictionary. When the posting dictionary is also independent,
sorting uses the generic query path.

The reviewed repair commits are on the integration branch. The temporary
benchmark checkouts and local task notes are removed after the report push is
verified.
