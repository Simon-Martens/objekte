# Segment cache timings — 29 October 2026

Release builds on `examples/musenalm`, collection `contents` (133,948 JSON files),
with a warm OS file cache and an existing segment cache. The filter is
`preferred_title ~ "almanach" && year >= 1800`, with count and limit 3. It
finds 569 matches. Nine repetitions were used for each current series.
These are single-machine measurements, not a controlled SQLite comparison.

| Measurement scope | Warm median |
| --- | ---: |
| Fresh CLI process, external wall clock, current build | **338–348 ms** across two series |
| Fresh CLI process, timed command body before repository teardown | **259–269 ms** |
| Same repository handle, in-process query | **31.5 ms** |

The earlier **699 ms** number was also a fresh-process CLI measurement. That
build decoded the 50.7 MB of shard metadata twice: once during freshness
checking and again when opening the query snapshot. Reusing the first decode
brought the median to 519 ms. Parallel metadata decode brought it to 395 ms;
parallel source-file metadata checks brought it to 338–348 ms in two later
series. None of these changes altered the cache format or freshness rule. The
fresh-process figure should not be compared directly with the values-only
prototype's in-process query timings.

## Fresh-process phase timings

The current CLI exposes phase timings with `--diagnostics`. Medians from the
latest nine fresh processes, in milliseconds:

| Phase | Median |
| --- | ---: |
| Repository configuration load | 2.5 |
| Decode 64 persisted metadata shards (50.7 MB) in parallel | 75.7 |
| Inventory 133,948 source files for freshness in parallel | 119.0 |
| Other synchronization work, including fingerprint and locking | 28.1 |
| Build/intersect the year candidate set from ordered indexes | 21.5 |
| Ripgrep scan of 64 values-only text shard files | 3.2 |
| Verify 1,101 candidate JSON source files in parallel | 1.4 |
| Materialize results | 0.2 |
| Drop the repository and its metadata at process exit | 62.0 |

The synchronization total is about 223 ms, including metadata decode,
inventory, and its other work. Command setup, output, diagnostics, process
startup, and exit account for the remaining small difference to the external
wall time. The ripgrep search uses `grep-searcher::search_path` on the shard
files; it does not scan the in-memory metadata. With a warm OS file cache,
those file pages are usually served from RAM. The ID maps and ordered indexes
are deserialized into process memory for each fresh CLI invocation.

The text search produces 2,109 candidates. Intersecting with the year index
leads to 1,101 source reads, including three returned records read again for
validation. The cold initial build took 3,436 ms in the previous check.

## Long-lived process comparison

The public `Repository::query_json` API was measured with one warm repository
handle. Each backend result was checked against raw source grep. Medians of
five timed runs, count enabled:

| Filter | Limit | Segment cache | Raw source grep |
| --- | ---: | ---: | ---: |
| `preferred_title ~ 'Liebe' && year = 1800` | 50 | 7.9 ms | 438 ms |
| `preferred_title ~ 'almanach' && year >= 1800` | 3 | 31.5 ms | 437 ms |
| `preferred_title ~ 'almanach'` | 50 | 6.9 ms | 438 ms |

The earlier 64-shard prototype measured 8.2 ms for the same `Liebe` combined
filter at limit 50. The current production path measured 7.9 ms in the same
long-lived-process scope. The 31.5 ms `almanach`/year query is a different
filter with many more candidates surviving its numeric condition.

To reproduce the CLI measurement:

```sh
cargo build --release -p objekte-cli --bin objekte
target/release/objekte examples/musenalm query contents 'preferred_title ~ "almanach" && year >= 1800' --count --limit 3 --data-dir target/segment-accept-final --diagnostics
```

To reproduce the in-process measurement, build
`-p objekte-bench --bin json-query-grep-compare` and run it with `--repo`,
`--workload`, `--data-dir`, `--runs 5`, and `--limit`.
