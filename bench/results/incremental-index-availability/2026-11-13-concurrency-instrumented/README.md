# Instrumented construction under foreground queries

This nine-process warm campaign repeated the alternating idle, foreground,
held-view order on integration revision `28a008b2ad` after the local scan
repair. Each process used the same absolute original Musenalm root, configured
schemas, ASCII bigrams and 32 build workers. Foreground mode cycled through
Q108, Q116 and Q120 with a 2 ms pause per three queries; held-view mode pinned
the first published view until completion. `campaign.json` and the nine
individual JSON files retain all measurements.

| Mode | Median full build | Median high-water RSS | Median segment publication | Median finalization |
| --- | ---: | ---: | ---: | ---: |
| Idle | 895.354 ms | 572.215 MB | 235.196 ms | 12.301 ms |
| Foreground | 936.034 ms | 564.703 MB | 244.818 ms | 12.687 ms |
| Held view | 923.650 ms | 567.898 MB | 245.726 ms | 15.581 ms |

Foreground queries raised median full-build time by 4.54% relative to idle.
Each foreground run completed 885–906 queries. Per-process median query
latency was 24.4–29.1 µs, p95 60.7–70.6 µs, and source opens zero. The first
nonempty result appeared 281–315 ms after the query worker began. The early
view remained at revision 2 through subsequent publications. Its median
high-water RSS was 4.317 MB below the idle median in these samples, so there
is no observed accumulation of full-view copies. RSS sampling after dropping
the view is allocator-dependent and is retained in the raw files.

Every process used exactly 148,486 construction reads and parses and read
193,297,230 source bytes. The 32-worker scheduler reached 32 active jobs in
each run. Idle median summed worker execution time was 4.471 s; this is a sum
across concurrent workers, not a wall-clock phase. Median idle scheduler
receive wait was 74.335 ms, admission 361.932 ms, and extraction/index
timeline 434.831 ms. Publication time aggregates eight seal, index, publish
and callback intervals; it lies within the extraction/index timeline.
Finalization covers the metadata/fingerprint summary before the final
completion publication.

Command for each run:

```text
target/release/examples/bench_initial_concurrency --root /home/simon/source/objekte/examples/musenalm --mode MODE --output bench/results/incremental-index-availability/2026-11-13-concurrency-instrumented/NN-MODE.json
```
