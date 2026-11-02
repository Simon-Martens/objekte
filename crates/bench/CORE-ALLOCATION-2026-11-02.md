# Core allocation and copying changes

Starting revision: `ccf61c814d8d70d308d46196eeae447694ccc844`.
Four changes were measured separately, then tested together. The release
benchmark uses a counting global allocator, wall-clock timing, and Linux
`VmHWM`. Each operation runs in a fresh process. Figures below are medians of
three runs unless noted. Allocated bytes are cumulative requested bytes;
peak live is the increase in outstanding allocated bytes during the measured
operation. RSS is process high-water, so it also includes setup and warmed
indexes. There is no evidence of a large regression in the selected operations.

The [fixture generator](generate_allocation_fixture.py) creates 20,000 nested
JSON records. The real-data run uses the 133,948-record Musenalm `contents`
collection. Both small records and large nested documents were exercised. The
[benchmark executable](src/bin/core_allocation.rs) and
[runner](run_core_allocation.py) are included for reproduction.

## Comparison

| Operation and corpus | Revision | Wall ms | Allocations | Allocated MB | Peak live MB | Peak RSS MiB |
|---|---|---:|---:|---:|---:|---:|
| View fallback, 25 rows, synthetic | baseline | 175.9 | 981k | 167.9 | 68.36 | 112.8 |
| | bounded selection | 128.3 | 981k | 153.1 | 0.13 | 29.0 |
| Direct view, 25 rows, synthetic | baseline | 271.9 | 1,721k | 231.7 | 41.57 | 66.7 |
| | bounded selection | 211.4 | 1,402k | 195.5 | 18.57 | 40.4 |
| Cold sort-index build, synthetic | baseline | 241.4 | 2,016k | 495.0 | 18.99 | 43.9 |
| | Sonic projection | 210.7 | 1,698k | 403.3 | 19.08 | 42.6 |
| Sort-index rebuild, Musenalm | baseline | 4,568.4 | 37,102k | 4,245.7 | 196.32 | 549.9 |
| | Sonic projection | 3,503.3 | 29,151k | 3,568.8 | 196.36 | 547.8 |
| Sorted JSON query, synthetic | baseline | 135.7 | 1,194k | 1,710.5 | 5.01 | 47.6 |
| | optional updates | 134.2 | 1,189k | 1,709.8 | 5.00 | 47.7 |
| Sorted text query, synthetic | baseline | 1,319.5 | 3,812k | 608.1 | 4.96 | 46.0 |
| | optional updates | 1,264.5 | 3,806k | 607.4 | 4.98 | 45.6 |
| Secondary snapshot save, Musenalm | baseline | 442.1 | 2,488k | 274.6 | 69.72 | 320.4 |
| | borrowed view columns | 407.4 | 2,219k | 240.8 | 69.72 | 304.8 |
| Secondary snapshot restore, Musenalm | baseline | 682.5 | 4,004k | 466.1 | 149.34 | 304.5 |
| | borrowed view columns | 690.4 | 4,010k | 465.6 | 149.34 | 306.6 |

The indexed 25-row view stayed at about 0.4 ms. The unindexed filtered and
sorted view with `count: true` had the same before/after performance as the
un-counted case (175.0 to 128.4 ms); it still computes the exact total.
Synthetic snapshot save fell from 61.2 to 54.8 ms and 24.8 to 23.5 MB
allocated. Synthetic snapshot restore fell from 69.6 to 61.3 ms, with no
meaningful allocation change. Restoration was not targeted by change 4.

Raw allocation samples: [baseline](results/core-allocation-baseline-2026-11-02.ndjson),
[change 1](results/core-allocation-task1-2026-11-02.ndjson),
[change 2](results/core-allocation-task2-2026-11-02.ndjson),
[change 3](results/core-allocation-task3-2026-11-02.ndjson),
[paired query runs](results/core-allocation-task3-paired-2026-11-02.ndjson),
and [change 4](results/core-allocation-task4-2026-11-02.ndjson).

## The four changes

1. **Bounded view selection.** The fallback previously retained every matched
   parsed document and its keys, sorted them, and then paginated. Direct mode
   built every projected row. A max heap now retains at most `offset + limit`
   candidates, with O(log K) insertion for K retained candidates. The match
   counter remains independent, so `count` is exact. Selected schema-backed
   documents are retained only until final validation; schema-free selection
   retains projected rows and sort keys. Tie breaking and state checks retain
   their prior behavior. Larger offsets still require K storage and O(N log K)
   work; the page limit does not bound source reads.

2. **One Sonic parse for indexed projections.** Sort keys previously caused a
   second full parse into `serde_json::Value`. Sort projection now traverses the
   Sonic value already used for text, typed, presence, and view projections.
   It handles nested arrays, duplicate object members with last-member-wins,
   exact numeric lexemes, and configured ICU collation. Rebuild workers reuse
   a local source-byte buffer only while projection is active. Peak live memory
   remains dominated by the completed indexes, while cumulative allocations
   and rebuild time fall substantially.

3. **Optional progress snapshots.** Ordinary sorted JSON and text queries
   previously deep-cloned retained records to build intermediate callback
   pages, even for a no-op callback. Internal callbacks are now optional;
   explicit stream-update APIs still receive the same progressive pages.
   Source parsing dominates these broad queries, so the allocation reduction
   is small but measurable. A 50-case benchmark exposed an initial indexed
   JSON regression: the absent callback had also disabled bounded candidate
   batches. Batching is now controlled by the result cap. A regression test
   checks that a collected indexed sort can stop before scanning the whole
   candidate set. The final 50-case relevant-sort workload took 6.27 s versus
   7.22 s in the historical single-pass run. The unscoped original-sort
   workload took 15.31 s versus 14.36 s across dates. A controlled adjacent
   run against the pre-change revision instead measured 15.77 s before and
   15.07 s after. All 50 result counts, truncation flags, index-use decisions,
   and indexed source-read counts matched in that pair. The unsorted
   50-query mix had the same first IDs, counts, and source reads, with 1.439 s
   versus 1.444 s total median case time.

4. **Borrowed view-index serialization.** `ViewIndex::saved()` previously
   cloned the field names, key columns, and ordering arrays before bincode
   wrote them. It now passes borrowed slices to the buffered serializer;
   restoration still deserializes owned values. The field order and encoded
   bytes are unchanged. A snapshot written with the old revision loaded both
   bigram and secondary indexes in the new revision. The old and new
   `.secondary` files were byte-for-byte identical (1,565,988 bytes for the
   synthetic fixture). A corrupted payload was rejected and rebuilt. The
   snapshot format version and atomic publication path are unchanged.

Final 50-case raw runs: [relevant sorts](results/core-allocation-final-sort-relevant-2026-11-02.ndjson),
[original sorts](results/core-allocation-final-sort-original-2026-11-02.ndjson),
the adjacent original-sort [before](results/core-allocation-paired-original-pre-2026-11-02.ndjson)
and [after](results/core-allocation-paired-original-post-2026-11-02.ndjson) runs,
and [query mix](results/core-allocation-final-query-mix-2026-11-02.ndjson).
Timing comparisons to the historical November 1 sort run are indicative:
process state and page cache were not controlled across dates.

## Verification and remaining allocation sources

The complete release core suite and workspace suite pass. New tests cover
large-page view parity, Sonic and serde sort-key parity across nested values,
duplicate keys, Unicode and exact numbers, indexed sort early stopping, old
and new snapshot byte equivalence, old snapshot restoration, and corrupt
secondary snapshot rejection. `cargo clippy -p objekte-core --all-targets --
-D warnings` passes. `python3 scripts/format-rust-spacing.py --check`
passes. Plain stable `cargo fmt --all -- --check` reports the repository's
pre-existing two-blank-line convention as differences; this convention is
specified in `docs/code-guidelines.md` and applied by the spacing script.

The opt-in `all_musenalm_collections_match_direct_execution` test stops on a
pre-existing invalid default view request: the first collection declares no
default columns, while `Plan::compile` requires nonempty columns. The same
validation exists at the starting revision. The separate real-data bigram
oracle and the 50-case query/sort workloads provide independent result checks.

The largest remaining cumulative query allocation is source parsing and
verification (11.6 GB across a broad Musenalm sorted JSON query in this
fixture), not callback snapshots. `TextIndex::save_parts()` still creates two
large `Vec<u8>` buffers for snapshot saving, leaving Musenalm's 69.72 MB
peak-live growth unchanged after view-column borrowing. `modified_order`
also materializes an ordering array, but is much smaller. Neither was changed
because this task does not redesign the snapshot format.
