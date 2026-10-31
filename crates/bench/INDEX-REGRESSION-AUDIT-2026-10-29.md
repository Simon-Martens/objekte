# Index regression audit, 29 October 2026

Historical measurements. The [cold-build fix and final comparison](INDEX-COLD-BUILD-FIX-2026-10-29.md) supersede the remaining cold-cache regression described here.

This follow-up checks the JSON-index change against `afe72d9e56`, the last
materialized-column implementation, using the same benchmark driver, source files,
settings and workload on both revisions. It also fixes regressions found in the
initial JSON-index implementation (`2ea9cd9b2f`).

## Changes driven by the audit

- A covering expression index holds ID and all configured view keys. Compound
  ordering and filters read these keys instead of reparsing entire JSON records.
  This adds no ordinary or generated field columns to any table.
- Counts select a matching field index, or the covering index for multiple fields.
  Scalar-cardinality checks explicitly seek the multi-value marker in the JSON index.
- JSON query pages materialize their selected identities before payload joins.
  Both view and query joins explicitly iterate the page first, preventing a full
  raw-table scan caused by inaccurate estimates after a different cache-build history.
  Collection identity is a known SQL constant while collecting matches, avoiding
  unnecessary raw-record lookups just to retrieve the collection name.
- Additional actual-JSON scalar indexes are lazy. A view builds its view indexes;
  a general query creates only the configured scalar indexes needed by its scalar
  comparisons/cardinality checks. `exists`, `?` and modifier-only paths do not create
  unused scalar-query indexes. Built indexes survive restart and follow record edits.
- Bulk index construction uses compact keys seeded from already-parsed ingestion
  records. Its transient retained-key budget is 128 MiB, separate from SQLite's page
  cache. If full, extraction falls back to the cached JSON. The transient cache is
  released after success or failure. Incremental writes reuse one parsed document.
- Exact decimal comparison has an allocation-free signed-integer path; other numeric
  representations retain the existing arbitrary-precision comparison.

Index-definition version 2 rebuilds existing format-5 indexes from cached records.
It does not require rereading unchanged source files or replacing the raw record cache.

## Repeated before/after comparison

AMD Ryzen 7 5700G, 16 logical CPUs; rustc 1.96.0; release with native CPU targeting.
Both builds use four ingestion workers, manual validation, a 64 MiB SQLite page cache,
and fresh temporary databases on `/tmp`. OS source-file caches are warm and were not
flushed. No competing builds/tests ran during these timing samples. The new transient
build-key cache can use additional memory as described above; this is not an equal-RSS
comparison.

Three fresh-cache samples per revision, with revision order alternating. Each sample
runs 15 cases, repeats each case three times in the same session, then reopens the
repository and runs every case again. Every result digest matches across both revisions,
repetitions and reopening: **450 matching executions**. Warm requests enumerate/read
zero source files; reopening reads zero source files while applying metadata freshness.
The separate real-data acceptance tests compare complete results with direct execution.

All numbers below are **milliseconds**. First/reopen columns are medians of three
samples; warm columns are medians of nine measurements. Timing includes the library
operation and construction of the comparison result, but excludes digest calculation
and printing. The first seven cases create their collection caches in the listed order,
sharing one database. Later cases reuse those caches; their first call may create a
lazy scalar-query index. This is not fifteen independent cold starts.

| Case | Old first ms | New first ms | Old warm ms | New warm ms | Old reopen ms | New reopen ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `contents_default` | 4100.597 | 7475.293 | 1.088 | 1.201 | 342.110 | 342.058 |
| `entries_default` | 1367.532 | 3657.228 | 4.044 | 4.161 | 16.859 | 17.538 |
| `agents_default` | 186.789 | 1034.796 | 1.118 | 1.255 | 11.517 | 11.691 |
| `items_default` | 81.121 | 1014.381 | 0.887 | 1.014 | 11.841 | 11.525 |
| `pages_default` | 1.478 | 524.529 | 0.620 | 0.789 | 0.830 | 0.892 |
| `places_default` | 4.860 | 718.614 | 0.764 | 0.945 | 1.427 | 1.515 |
| `series_default` | 36.647 | 772.612 | 0.817 | 1.025 | 4.044 | 4.086 |
| `contents_year_desc` | 1.496 | 1.478 | 0.960 | 1.282 | 1.493 | 1.610 |
| `contents_year_filter` | 52.932 | 24.885 | 48.624 | 20.779 | 58.263 | 24.421 |
| `contents_compound` | 27.055 | 19.031 | 27.637 | 19.018 | 27.932 | 18.511 |
| `contents_offset` | 3.151 | 2.833 | 1.021 | 2.379 | 3.127 | 2.619 |
| `year` | 1170.277 | 632.561 | 1168.453 | 91.811 | 1160.956 | 95.124 |
| `all_year` | 2143.609 | 742.314 | 2146.395 | 382.096 | 2108.155 | 374.121 |
| `text` | 1095.891 | 1780.132 | 1099.985 | 1091.762 | 1094.111 | 1101.628 |
| `nested` | 1993.033 | 1999.494 | 1991.290 | 2017.327 | 1924.502 | 1964.885 |

Default views return 50 rows without a count. `contents_year_desc` sorts by year
descending; `contents_year_filter` filters `year >= 1800`, returns the default page
and counts matches; `contents_compound` sorts by year ascending then numbering
descending; `contents_offset` requests offset 500 with a count. JSON query cases
return 50 records and a total: `year` is scoped to contents, `all_year` spans all
collections, both using `year >= 1800 && year < 1805`; `text` is the contents title
substring `liebe`; `nested` combines an any-agent match with `exists(contents[0])`.
The exact requests are in [index-workload.json](index-workload.json).

**Warm behavior is preserved or improved for the important view/filter workloads.**
The scalar year query is about 12.7 times faster, the global year query about 5.6 times,
and the compound view about 1.4 times. Small absolute increases remain, including the
offset/count case (about 1.3 ms). Wildcard/nested scans remain around their former cost;
these are not accelerated by scalar indexes.

**Cold creation still regresses against the old materialized-column design.** Contents
rises from 4100.598 ms to 7475.293 ms; entries rises from 1367.532 ms to 3657.228 ms.
Later tiny collections also incur construction cost because their partial indexes
are created on the shared records table after the large collections are present.
The fixes reduce first-view cost versus the initial JSON-index implementation, but
it would be incorrect to call this a regression-free ingestion change. Additional
JSON indexes also have a measurable first-query cost. Do not hide this cost by
reporting only warm views. Keeping JSON-only index storage currently makes this an
explicit tradeoff; further cold-build work remains warranted.

## Tests and reproduction

```sh
cargo test --workspace --features objekte-bench/sqlite-bench
cargo clippy --workspace --all-targets --features objekte-bench/sqlite-bench -- -D warnings
cargo fmt --all -- --check
cargo test --release -p objekte-core --test musenalm --test musenalm_json_filter -- --ignored --nocapture
```

The routine suite has **159 passing tests**. Both full-data acceptance tests also
pass across all **148,486 Musenalm records**, including completed validation and
173,427 relationship occurrences. A query-first cache history separately returns
the contents year page/count in 94.173 ms warm, with zero source reads. It includes the new lazy-index/reopen
regression, actual covering-index reads with JSON extraction functions disabled,
legacy-table cleanup, seeded-key and zero-budget fallback checks, duplicate input
accounting, and cleanup after SQL failure. Existing tests cover field comparisons,
all supported operators/modifiers, exact JSON types/numbers, malformed records,
validation, configuration changes, dependency tracking, references, cache refresh,
crash recovery, and CLI behavior. Workspace Clippy now passes with the SQLite
benchmark feature too; small type aliases and a range tuple resolve pre-existing
benchmark lint failures without changing benchmark semantics.

Build the same historical `profile_indexes` driver (removed) against
both revisions (copy it into an older checkout before building), save each executable,
then run:

```sh
python3 crates/bench/compare_index_revisions.py BEFORE_PROFILE AFTER_PROFILE \
  examples/musenalm crates/bench/index-workload.json target/index-comparison \
  --samples 3 --repeats 3
```

The runner creates/removes temporary databases, alternates revision order, checks every
digest, and writes raw JSON lines plus median summaries. This audit's raw measurements
are in ignored `target/index-comparison-verified-2026-10-29/`. Hardware-dependent timings
are reported, not asserted as brittle CI thresholds. Query plans and equality remain
regression assertions.
