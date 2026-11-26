# Objekte storage backend repair review and remaining corrections

## Decision

Hold the merge of `integration/index-storage-backend-repair` at `177fba5c2d00b3d6c25051ef313ae61a62cc5f8e`. The six implementation commits end at `47d17ef504`. The seventh commit adds the report. The parent integration branch is `9aca99af25`, and `master` is `cf7253d048`. Continue from the repair head. The main release blocker is an incorrect exact-match claim that can discard a valid record. The remaining items concern the promised ability to replace one storage component while keeping existing consumers and producers.

## 1. Fix the exact equality proof on an empty array ancestor

**High priority, wrong results.** For an independently stored numeric posting, `placed_equality` accepts an inferred scalar field when its observed terminals are scalar or null and no record selects several values. Neither check excludes a traversal that selected zero elements. For a dynamically bound multivalued path, default equality is true for such a traversal, whereas `?=` requires a matching element. When the provider claims exact membership, the runner can omit a valid record before source verification. The native posting path already checks this condition. [Independent proof](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/index/memory/access/segment.rs#L120-L178>) · [Applied membership](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/index/memory/access/segment.rs#L525-L549>) · [Native proof guard](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/index/memory/access/catalog.rs#L146-L156>) · [Reference semantics](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/reference/filter.rs#L99-L121>)

Use one collection with auto inference and put these records in the same all-logical segment. Configure the independent writer to accept the inferred numeric posting on `items.year`. The current test writer only accepts the top-level `score` path. [Existing numeric fixture](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/inference/tests.rs#L2953-L2979>) · [Current writer selection](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/index/storage/segment_catalog/tests/segments.rs#L1112-L1119>)

| Record | Logical content | `items.year = 1800` | `items.year ?= 1800` |
|---|---|---:|---:|
| A | `{"items":{"year":1800}}` | yes | yes |
| B | `{"items":[]}` | yes | no |
| C | `{}` | no | no |

Share the proof eligibility rule between both providers. If a posting cannot account for successful empty traversals, decline the exact offer or include those rows for verification. Preserve the fast path for existential equality. Assert IDs and exact counts against the reference evaluator, with the result cache disabled and evidence off. Check explain or instrument the selected provider so the regression actually exercises the independent posting. Repeat after compaction.

## 2. Apply directory scope to all collection-scoped operations

**High priority, unrelated storage failure.** Ordinary segmented search, explain and streaming now select their participating directories. `IndexView::select_exact` and `posting_values` still request `self.0.segment_groups()` up front. A broken directory for B therefore prevents enumerating facet values for healthy A. The outgoing and backlink page APIs also request all directories. Incremental search reads every old and new directory and compares placement `Arc` addresses. A backend that decodes fresh handles for an unchanged prefix can consequently lose the incremental fast path. [Exact selection and facets](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/lifecycle/view.rs#L1257-L1294>) · [Relation pages](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/lifecycle/view.rs#L1297-L1363>) · [Incremental search](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/lifecycle/view.rs#L993-L1045>)

Use the requested collection plus the relation dependencies of the specific operation. Apply the same selection to both revision pins. Compare pinned placement or revision identity in a way that survives independent decoding and distinguishes a changed live mask or replaced payload. Reuse the existing failure-injecting directory fixture. Verify zero placement reads for unrelated B, propagation of failures in a required directory, and an incremental result after an append when the unchanged prefix is independently decoded. Include a negative test with a changed prefix. Repository-wide graph and statistics requests can retain their existing scope. [Existing directory test](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/index/storage/segment_catalog/tests/segments/transfer.rs#L1086-L1235>)

The selected-directory fallback still collects every placement page before the runner starts. Add a separate read-counter test for an early-stopping, unordered streaming query against one large independent directory. Allow the runner to stop fetching placements when it has enough rows and the request does not require a complete count. Operations requiring a merge across all segments may need all segment descriptors. [Fallback collection](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/index/storage/segment_catalog.rs#L165-L195>)

## 3. Bound worker memory before emitting gram blocks

**High priority, construction memory.** The independent gram worker pushes a `GramFact` into a vector for every adjacent and skip occurrence. It hands off the complete vector, and the aggregate subsequently slices it into 256-fact writes. For a 1 MiB ASCII string consisting entirely of `a`, this produces **2,097,149 facts for two distinct channel/key memberships in one record**. Several workers or indexed text fields can multiply this cost. This is a count from the implementation, not a measured RSS figure. The reported native-backend build does not exercise this path. [Occurrence emission](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/index/memory/bigram/mod.rs#L42-L52>) · [Worker accumulation](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/index/memory/bigram/placement.rs#L69-L108>) · [Later batching](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/index/memory/bigram/placement.rs#L156-L175>)

Give the selected backend a worker-local writer or bounded staging facility at extraction time. Flush when the item or byte limit is reached. An opaque prepared contribution can carry the result through ordered slot assignment. Preserve logical record boundaries, folded-text behavior and the native builder’s fast path. Apply the same working-set check to selected posting and relation writers, which also accumulate facts before splitting them. [Posting facts](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/index/memory/posting/placement.rs#L82-L102>) · [Relation facts](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/index/memory/relation/placement.rs#L44-L81>)

Test repetitive and high-diversity text. Instrument peak pending facts or bytes **inside the worker**, before the sink receives them. Check query equivalence, cancellation, and that increasing a text by an order of magnitude does not grow the unflushed occurrence vector proportionally. Sink callback sizes alone cannot verify this.

## 4. Preserve usable index capabilities across component placements

**Backend contract.** Ordinary posting planning still looks in the native declared, named and inferred maps. The separate path for an independently placed posting offers a narrow scalar equality operation. Consequently a complete, ordered numeric posting can lose indexed range queries when moved to another store. The query can fall back to source verification. With a detached source, a locator or count request can lose results that the native index could answer alone. Keyed membership also requires the complete list for a frequent key as a bitmap or sorted slice, even when the operation input is selective. The existing block traversal is for the whole posting index. [Native catalog lookup](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/index/memory/access/catalog.rs#L416-L451>) · [Placed operations](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/index/memory/access/segment.rs#L120-L151>) · [Key lookup and traversal](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/index/storage/posting.rs#L250-L280>) · [Membership representations](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/query_access/membership.rs#L243-L251>)

Use reader-advertised capabilities and shared proof rules. Allow a keyed posting read to return bounded blocks, a pinned view or a restricted intersection. Keep the option to decline unsupported operations. Demonstrate a numeric range with locator delivery and zero source opens on a complete independent posting, preserve residual verification on partial coverage, and exercise a selective input against a frequent key with the whole-list operation disabled.

Declared sort placement also skips metadata paths and every constituent of a composite order, even when the selected store accepts the column. Honor placement for accepted columns and bind composite comparison to their readers. Verify ascending and descending ties and refresh metadata keys after moving a linked file. If a particular backend lacks a capability, expose that in selection. [Sort placement exclusion](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/index/memory/sorting/mod.rs#L417-L433>) · [Metadata and composite refresh](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/index/memory/sorting/mod.rs#L330-L360>)

**Accounting.** Native segment accounting sums native posting, gram and relation containers. Independently placed families can leave those containers empty. The byte-triggered consolidation policy then operates on an underestimate. Provide a cheap resident-byte estimate on sealed component handles, count shared allocations once according to existing policy, and avoid attributing an entire on-disk file to resident memory. Inject a known estimate in the fixture and check both inspection and the size-triggered maintenance decision. [Segment accounting](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/index/memory/segment/mod.rs#L1411-L1465>) · [Maintenance consumer](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/lifecycle/publication.rs#L574-L591>)

## Repairs verified and performance

The completion flag now comes from the source catalog. Cached payload delivery propagates backend errors and handles unavailable source files separately. Paged liveness exposes checked operations. The memory importer covers retained and versioned-provider records, and automatic consolidation tries another eligible pair after a decline. Existing tests cover these paths. Keep them as regression coverage. The foreign multipart importer still walks headers and file directories per output partition, a documented scaling limitation distinct from the improved posting and gram traversal. [Catalog restore](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/index/memory/file_catalog.rs#L405-L421>) · [Delivery regression](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/index/storage/segment_catalog/tests/segments.rs#L1638-L1680>) · [Liveness regression](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/index/storage/segment_catalog/tests.rs#L1463-L1550>) · [Transfer regression](<https://github.com/Simon-Martens/objekte/blob/177fba5c2d/crates/core/src/index/storage/segment_catalog/tests/segments/transfer.rs#L924-L1083>) · [Implementation report](https://github.com/Simon-Martens/objekte/blob/177fba5c2d/docs/ai/done/objekte_index_storage_backend_repair_specification.md#L94-L129)

| Measurement | Master | Repair | Evidence |
|---|---:|---:|---|
| Stored residual, first row | 23.924 µs | 24.242 µs | Median of seven paired run medians, 64 calls per run |
| Eight-file update batch | 4.720 ms | 3.581 ms | Seven pairs, repair faster in four |
| ASCII auto-worker build | 722.50 ms | 671.53 ms | One build per revision |
| Peak RSS for that build | 665.00 MiB | 613.81 MiB | Same build |

The raw samples reproduce the medians and the two recorded build signatures agree. The branch reports 150 matching query digests, 706 passing core tests and 16 ignored in each workspace configuration, plus nine release transfer tests. The earlier timing concerns do not independently block this merge. This review traced the source at the pinned commit and recalculated the benchmark statistics. A Rust toolchain was unavailable here, so the suite and proposed regressions were not executed independently. [Benchmark data](https://github.com/Simon-Martens/objekte/blob/177fba5c2d/docs/ai/generated/index-storage-backend-repair-results.json) · [Release comparison](https://github.com/Simon-Martens/objekte/blob/177fba5c2d/docs/ai/generated/index-storage-backend-repair-results.md)

Run the new focused regressions, the locked workspace checks from the repair report and the standard query replay before resubmission. Measure the alternate writer on the two construction fixtures above. Another full benchmark campaign is unnecessary unless the native hot path changes materially.

## Implementation report

Completed 2026-11-26 on `integration/storage-backend-repair-review`,
starting from `177fba5c2d00b3d6c25051ef313ae61a62cc5f8e`. The integration
worktree is `/home/simon/source/objekte-integration-storage-backend-repair-review`.

| Commit | Verified change |
| --- | --- |
| `918c35b8f3` | The native and independent posting plans share the empty-traversal proof rule. The independent `items.year` regression first reproduced a count of one against the reference count of two. Default equality now returns A and B, and existential equality returns A using the placed reader. Both results survive compaction. |
| `ae56a0d6de` | Exact selection, facets, outgoing references, backlinks and incremental search open the collections they need. The failure fixture covers an unrelated broken directory, a required broken directory, an unchanged prefix returned in new handles, and a changed prefix after retirement. |
| `26e783f4ae` | Selected gram, posting and relation writers receive bounded extraction batches and contribute their backend-local state at ordered slot assignment. The gram fixture compares query results with native storage and injects cancellation on a worker write before publication. |
| `bea406550d` | The unordered finite-page fixture starts with 24 independent placements. The one-row page reads one placement. The page at offset one with a limit of two reads three placements. The complete-count request surfaces the injected second-page failure. |
| `a40bd0f830` | Complete placed postings answer numeric ranges with zero source opens after detaching the file. A selective equality query uses only its restricted input against a 300-slot key with whole-list reads disabled. Partial coverage uses residual verification. Composite sorts use placed scalar and metadata keys, preserve ties in both directions, and refresh after a move. A shared 1 MiB resident estimate is charged once across the fixture's two placements, appears in inspection, and changes the size-triggered consolidation decision. |
| `c5dcec2b28` | Each restricted posting block is validated against the effective logical input and checked for cancellation. The extraction measurement reaches a full 1 MiB of text. |

The combined implementation passed these commands.

```sh
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo test --locked -p objekte-search-core --lib index::storage::segment_catalog::tests -- --nocapture
cargo test --locked --workspace
cargo test --locked --workspace --features objekte-search-core/test-scan-instrumentation
cargo test --locked --release -p objekte-search-core --lib transfer -- --nocapture
```

The focused group passed 25 tests. Both workspace runs passed 712 core tests
with 16 ignored. The release transfer selection passed 11 tests.

### Worker measurement and query replay

The release fixture measured each worker's pending, unflushed gram facts and
their payload bytes. Each input also passed the native query comparison.

| ASCII input | Text bytes before the search marker | Peak pending facts | Peak pending bytes |
| --- | ---: | ---: | ---: |
| Repetitive | 104,857 | 256 | 4,096 |
| Repetitive | 1,048,576 | 256 | 4,096 |
| High diversity | 104,857 | 256 | 4,096 |
| High diversity | 1,048,576 | 256 | 4,096 |

The standard Musenalm replay matched all 150 query digests against the repair
parent using the locked release build and `ascii-off` profile. The canonical
digest map SHA-256 is
`53fbe3252acb0a8e9d4715adc7d695ec8623c1eb5c9319f274960ef482e4fc28`.

```sh
cargo build --locked --release -p objekte-cli --example bench_musenalm --example bench_build_musenalm
bench_musenalm --root examples/musenalm --profiles ascii-off --repetitions 1 --result-dir <output>
bench_build_musenalm --root examples/musenalm --alphabet ascii --build-workers auto
```

A representative ASCII auto-worker build used 32 workers on an Intel Core
i9-13900K with Rust 1.100.0. Both revisions indexed 148,486 records and
produced the same index signature. The parent took 686.67 ms with 613.19 MiB
peak RSS. This branch took 650.00 ms with 619.23 MiB peak RSS. These are
single-run observations.

Finite-page streaming can stop early when one selected directory certifies
non-overlapping record-ID ranges in placement order and disjoint file
associations. Other layouts use the existing merge path. Placed posting
readers opt into bounded scalar restrictions and retain residual verification
where coverage or semantics need it. Foreign multipart import still
revisits headers and file metadata per output partition, as documented
by the parent task.

The integration worktree is retained for review. The temporary task notes
and obsolete untracked source specification are removed after the report
push and the preserved source bytes have been checked.
