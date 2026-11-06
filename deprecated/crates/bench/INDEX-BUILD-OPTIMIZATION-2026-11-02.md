# Index construction optimization — 2 November 2026

Four independent changes were measured against `4fa333199fe1`. Each stage ran three instrumented release builds of the same materialized example data, with a fresh index and largely warm filesystem cache. The runner alternated profile order. All reported times are medians. The 32-worker machine and corpus preparation are described in the [baseline profile](INDEX-BUILD-PROFILE-2026-11-02.md). Raw build results for every stage are [here](results/index-build-optimization-2026-11-02.ndjson); the [final restore runs](results/index-build-restore-2026-11-02.ndjson) use the same runner with an added snapshot-load measurement.

## Build time and memory

| Corpus | Baseline build | After 1 | After 2 | After 3 | After 4 | Baseline peak RSS | Final peak RSS |
|---|---:|---:|---:|---:|---:|---:|---:|
| Musenalm, 133,948 files | 1,480 ms | 1,437 ms | 1,476 ms | 1,303 ms | 1,305 ms | 779 MiB | 779 MiB |
| Leipzig, 400,000 files | 4,033 ms | 4,048 ms | 3,391 ms | 3,443 ms | 3,438 ms | 2,554 MiB | 2,951 MiB |
| Amazon, 1,000,000 files | 4,535 ms | 4,065 ms | 3,877 ms | 3,919 ms | 3,539 ms | 3,981 MiB | 3,963 MiB |
| NYC 311, 1,000,000 files | 4,382 ms | 3,538 ms | 3,534 ms | 3,363 ms | 3,351 ms | 1,951 MiB | 1,942 MiB |

These are wall times for the build only. Snapshot saving and restoration are separate. The compressed bigram sizes stayed exactly the same at the byte-count level across all five variants: Musenalm 52.7 MB, Leipzig 109.6 MB, Amazon 430.5 MB, and NYC 379.6 MB. The timing feature adds stage clocks; it is absent from normal builds.

The small Lenz FTS profile remained at 35 ms median build time, matching the earlier baseline's 35 ms. The two-job compression path is disabled below 8,192 files.

## Contribution of each change

| Change | Measured effect | Peak-memory effect |
|---|---|---|
| 1. Reuse projection buffers and skip ASCII folded copies (`6e6c00fd90`) | Amazon build 4,535 → 4,065 ms; NYC 4,382 → 3,538 ms. Field-text worker CPU fell 31.1 → 7.0 s and 34.4 → 9.4 s respectively. Musenalm improved 43 ms; Leipzig was essentially flat. | No material increase; all four peaks fell 5–11 MiB. |
| 2. Compress independent filters in two jobs (`dd5ca55e9d`) | Leipzig complete text-compression wall time 2,481 → 1,909 ms, and build 4,048 → 3,391 ms. Amazon compression 706 → 498 ms and build 4,065 → 3,877 ms. Small Musenalm/NYC build changes varied with run noise. | Leipzig rose 2,546 → 2,935 MiB as filter slabs coexist during compression. Other peaks were effectively unchanged. |
| 3. Sort indexed-view fields concurrently (`c1e5a5e37e`) | Musenalm view-order wall time 519 → 411 ms; build 1,476 → 1,303 ms. The other profiles have no comparable view-order stage. | Musenalm 771 → 774 MiB. |
| 4. Route text through compiled field IDs (`6a697d8db6`) | Amazon field-text worker CPU 6.96 → 6.52 s and build 3,919 → 3,539 ms. NYC field-text CPU 9.34 → 7.98 s; its build was effectively flat. Path construction and registered-path binary searches are removed from index-time leaves. | Amazon 3,985 → 3,963 MiB; NYC unchanged. |

The two-job compressor retains Rayon parallelism inside each filter. `text_compress_nanos` now measures the actual complete phase wall time; individual filter measurements can overlap and must not be summed as a phase duration. Small collections stay on the serial path. `SortIndex::ordered_flat()` still has one declared composite plan on the measured Musenalm collection, so parallelizing composite plans was not included.

## Query regression checks

After **each** change, the first 50 and all 100 Musenalm JSON filters and the 30-case full-text workload were run with three timed rounds. Raw [JSON](results/json-query-optimization-2026-11-02.ndjson) and [FTS](results/fts-query-optimization-2026-11-02.ndjson) results retain every case. Every stage had identical match counts, first IDs, scan and parse counts, and full-text candidate counts.

A separate five-round baseline/final A/B run gives the more stable query comparison below. Values sum each case's median rather than timing the entire suite as one operation. The [A/B raw data](results/query-final-ab-2026-11-02.ndjson) records each case.

| Workload | Baseline | Final | Change |
|---|---:|---:|---:|
| First 50 JSON filters | 2,819 ms | 2,830 ms | +0.4% |
| All 100 JSON filters | 5,556 ms | 5,639 ms | +1.5% |
| 30 full-text cases | 582 ms | 562 ms | −3.4% |

Two metadata queries had medians over 30% slower in the five-round run, although their minimum times and source-read counts were nearly equal. Repeating only those queries for ten rounds in reverse binary order gave `@created = null` 146.1 → 133.0 ms and `year = 1800 && @size > 1KB` 21.7 → 19.8 ms. This [isolation check](results/query-spike-check-2026-11-02.ndjson) supports run noise rather than a new query path. The implementation changes index construction; query evaluation code was unchanged.

## Snapshot and correctness checks

The profiler saves the base and secondary snapshots, then fully restores them through the unchecked-cache path and compares restored index sizes. Its final median snapshot times are:

| Corpus | Baseline save | Final save | Final restore |
|---|---:|---:|---:|
| Musenalm | 412 ms | 369 ms | 361 ms |
| Leipzig | 777 ms | 756 ms | 380 ms |
| Amazon | 2,159 ms | 2,067 ms | 1,079 ms |
| NYC 311 | 2,034 ms | 1,995 ms | 955 ms |

The complete core and workspace test suites passed, as did workspace Clippy with `-D warnings` and the changed-file rustfmt check. The field-routing parity test compares projected bytes against the old path visitor for markup, Unicode, arrays, duplicate keys, exclusions, and malformed containers; the view-order test compares 9,000 parallel slots against serial sorting. The opt-in Musenalm bigram and JSON-filter oracles passed complete source-versus-index match sets. The older opt-in `musenalm.rs` view acceptance test could not begin because its default request has no columns; the current view API rejects empty columns before index evaluation. A whole-workspace format check still reports only three pre-existing CLI TUI files outside this change.

One follow-up experiment kept general projected values as a `String` to avoid UTF-8 revalidation after buffer reuse. Its three-run build medians did not improve, so that change was discarded. Its local measurements are not part of the committed variants.

The largest remaining measured build stages are Leipzig text compression (about 1.9 s), Amazon and NYC parallel scan (about 1.5–1.8 s), and Musenalm view ordering (about 0.4 s). The two-way compressor's additional Leipzig peak memory is the main tradeoff to watch on smaller machines.
