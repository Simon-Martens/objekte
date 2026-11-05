# Verified predicate and ordered-result cache — 3 November 2026

## Decision

Retain the repository-local cache. It stores schema-bound query expressions, evaluated and matching slot bitsets, and complete or proven-prefix ordered slot lists. It does not retain source bytes, parsed JSON, rendered text, or returned records. The default budget is 64 MiB; `Options::query_cache_bytes = 0` disables it. Cache hits still read and verify the records returned on the page.

The implementation reuses `SlotSelection`, `ModifiedOrder`, and declared sort indexes. Complete ordered lists serve later offsets directly. Safe AND subsets reuse verified conjunctions and partial term truth; a complete cached conjunction combined with exact typed postings can prove a larger conjunction without another candidate scan. Complete OR branches can form a complete union. The cache is bypassed for scalar-cardinality risk, source metadata predicates, unchecked restored pickers, and `HashOnRequest`. An unindexed sort continues through the bounded fallback. A collection refresh, index replacement, or effective schema reload invalidates its entries. `MetadataOnRequest` refreshes before lookup. Under `MetadataOnOpen`, changes to non-page source files remain unknown until reconciliation, as with the existing index policy.

## Method

Starting revision `9f1ad89faf`; local changes in this report. Rust 1.99.1, Linux 7.2.13, Intel Core i9-13900K, local Btrfs. Release builds, warm filesystem caches, seven alternating cache-off/cache-on process pairs for each workload. The runner checks the complete page ID sequence, count, and truncation flag for every sequence and selective pair. The 100-case and 30-case runners check their existing result signatures. The [raw measurements](results/query-cache-2026-11-03.ndjson) contain all 2,926 individual setup and query rows; [the runner](run_query_cache.py) reproduces the paired protocol. The sample count in this report is per variant. Timings are medians of seven paired runs, not a comparison with historical measurements. Cache-off and cache-on use the same index and source data. Source scans count candidate files opened by the JSON scanner; parses count parsed documents, including returned-page materialization.

The sequence uses `contents` in Musenalm. It adds, replaces, removes, and reorders predicates, then pages `created < updated` in default modification order and changes to indexed `preferred_title` ascending and descending order. Counted queries exercise complete lists; the no-count and 10,000-cap prefix behavior is tested separately. The 100 JSON cases and 30 FTS cases perform an untimed expected-result pass before their timed pass, so their timings represent **repeated use**. The sequence and large-corpus first query timings measure first use on a warm index. Index construction and snapshot restoration are excluded.

## Interactive sequence

At the 64 MiB budget, selected median wall times were:

| Step | Cache off | Cache on | Reuse |
|---|---:|---:|---|
| Add `annotation ~ 'Mond'` to `year = 1800` | 3.22 ms | 3.59 ms | First verification |
| Add `place_statement ~ 'Berlin'` | 3.26 ms | 0.89 ms | Complete conjunction |
| Replace place with `preferred_title ~ 'a'` | 3.09 ms | 0.57 ms | Complete conjunction |
| Remove `year = 1800` | 6.34 ms | 6.55 ms | 16 known term results; remaining candidates verified |
| Repeat the year + annotation query | 3.42 ms | 0.59 ms | Complete order and membership |
| First counted `created < updated` page | 129.89 ms | 117.83 ms | First verification |
| Page at offset 25 | 117.40 ms | 1.25 ms | Ordered-list slice |
| Page at offset 2,500 | 109.36 ms | 0.59 ms | Ordered-list slice |
| Indexed title ascending | 122.37 ms | 1.81 ms | Membership reused; order built |
| Indexed title descending | 137.94 ms | 1.38 ms | Membership reused; order built |
| Repeat indexed title ascending | 128.88 ms | 1.03 ms | Ordered-list slice |

The first new conjunction was 0.37 ms slower on this run. Removing a constraint still has to verify previously unknown slots; its 0.21 ms difference is small relative to run variation. Unrelated first-use terms (`annotation ~ 'Schiller'` and `preferred_title ~ 'Taschen'`) took 3.95 → 3.48 ms and 8.53 → 8.39 ms, respectively. None of the 100 JSON or 30 FTS cases had a repeat-run slowdown greater than 30% and 1 ms in absolute terms.

The 16, 32, and 64 MiB budgets produced the same result signatures and no evictions on this sequence. The largest estimated cache footprint was 0.37 MB, below every tested budget. A unit test exercises eviction at a deliberately tiny budget. The cache does not scan all sources to fill a page: uncounted early pages store only a proven leading prefix, and capped indexed sorts store a capped prefix with distinct truncation semantics.

## Existing workloads and large corpora

| Workload | Cache-off summed case medians | Cache-on summed case medians | Candidate scans off → on | Result parity |
|---|---:|---:|---:|---|
| Musenalm 100 JSON cases | 4,431 ms | 2,925 ms | 3,430,874 → 2,003,621 | 100/100 |
| Musenalm 30 FTS cases | 585 ms | 15.0 ms | 572,590 → 0 | 30/30 |

The JSON workload reduced parsed source records from 3,400,004 to 1,999,756 in its timed pass and recorded 112 cache hits. Its final cache footprint was approximately 7.5 MB. The FTS workload parsed 572,607 → 26 documents and recorded 30 hits. The 30-case gain is specifically from repeating the same queries after the expected-result pass; it is not a first-query gain.

The larger corpus check used one selective standalone text query per repository, with seven pairs each. Counts and returned IDs matched in every pair.

| Corpus and query | Matches or cap | First use off → on | Repeat off → on | Repeat scans off → on | Cache footprint |
|---|---:|---:|---:|---:|---:|
| Leipzig, UTF-8 `ريتشارد` | 13 | 18.00 → 18.30 ms | 14.62 → 1.91 ms | 12,986 → 0 | 0.10 MB |
| Amazon, `headphones` | 10,000 cap | 40.39 → 41.56 ms | 57.76 → 3.61 ms | 16,874 → 0 | 0.32 MB |
| NYC 311, `Park` | 10,000 cap | 436.12 → 436.61 ms | 420.53 → 3.32 ms | 338,931 → 0 | 1.30 MB |

The Amazon and NYC repeated queries parsed only the 25 returned page records. The capped result sets are cached only as leading prefixes and are not treated as complete universes when predicates change. The first-use Leipzig difference is 0.30 ms; the seven-run ranges overlap. Amazon first use was 1.17 ms slower (2.9%), while NYC was 0.49 ms slower (0.1%).

## Allocations

The `allocation-profile` feature uses a process-wide counting allocator around each query. These are separate seven-pair samples because atomic allocation accounting changes wall time. Numbers are medians. Cumulative allocated bytes count all allocations across parallel workers; peak live delta is relative to the start of the query.

| Query step | Allocations off → on | Allocated bytes off → on | Peak live delta off → on |
|---|---:|---:|---:|
| Add place after cached conjunction | 69,919 → 3,119 | 196 MB → 0.80 MB | 5.19 → 0.19 MB |
| Repeat year + annotation | 68,510 → 2,887 | 195 MB → 0.52 MB | 5.15 → 0.09 MB |
| Page at offset 2,500 | 11,057,758 → 2,762 | 9.36 GB → 0.54 MB | 6.40 → 0.12 MB |
| New unrelated annotation term | 122,548 → 122,615 | 112.1 → 112.5 MB | 4.06 → 4.12 MB |

The first broad query still allocates substantially because it verifies 133,948 source files. The cache saves work on later pages; it is not a source-content cache. Ordered lists are capped by the configured memory budget before insertion. The primary wall-time runs use the normal allocator.

## Correctness and limits

The complete workspace test suite passes, including new tests for canonical reordered AND terms, complete OR unions, exact-posting completion, partial evaluation, default and indexed ordering, `@id` sorting, pagination, capped prefixes, refresh, insert/delete, schema reload, `MetadataOnRequest`, `HashOnRequest`, text modes, field-path materialization, and error-sensitive cardinality. Existing callback and source-change tests pass. The ignored, release-mode Musenalm independent Serde oracle also passed all 43 benchmark filters against complete source-derived ID sets. The final seven-pair run was made after the exact-posting reuse change. Workspace Clippy with the allocation feature and `-D warnings` passes. All changed Rust files pass rustfmt and the repository blank-line convention. The whole-workspace format check still fails on pre-existing formatting in unrelated TUI, bigram, and profile-test files; those files were left untouched.

The cache does not yet promote frequently used partial predicates in the background. Unsupported unindexed sorts still use `SortBuffer`. `@created`, `@status`, and other source metadata predicates are deliberately excluded from verified-result reuse in this first version. Sort-comparison counts were not instrumented; the indexed-sort cache-hit path uses the existing order and does not project JSON sort keys or enter `SortBuffer`. Cache memory is estimated from vector capacities and map-entry overhead; it is not a process RSS measurement. The benchmark uses warm filesystem caches and does not claim cold-storage performance.

## Streaming TUI follow-up

The benchmark measurements above were taken before TUI streaming searches used the cache. The TUI keeps one repository handle but normally uses `query_json_stream`, `search_text_stream`, and their sorted page-update variants. It now opts those calls into the same verified predicate and ordered-result entries as collected queries, while continuing to emit provisional rows. Ordinary library streaming calls retain full candidate verification by default; a real-data source-change test depends on detecting a file modified late in a stream. TUI cache reuse follows the `metadata-on-open` policy for non-page files, with reconciliation invalidating entries. A TUI worker test confirms repeated text and JSON-filter searches hit complete ordered lists. Core tests confirm streamed default and indexed-sorted searches reuse membership and ordered pages when opted in, and that a callback failure during verification does not publish partial results. The recorded benchmark timings above remain measurements of collected queries; no streaming latency claim is inferred from them.
