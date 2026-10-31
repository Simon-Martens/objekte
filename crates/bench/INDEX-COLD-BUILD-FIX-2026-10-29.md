# Cold JSON-index construction fix, 29 October 2026

This supersedes the remaining cold-build regression in
[the earlier audit](INDEX-REGRESSION-AUDIT-2026-10-29.md). It preserves raw JSON
expression indexes, exact source bytes, type-correct filters, and the library API.
No extra field columns or persistent projections are introduced.

## What changed

- One raw-record table per opened collection, inside the same repository database.
  Payloads are stored once. An index build no longer scans other collections' raw data.
  Configuration loading still does not open/enumerate a source collection.
- The schema/UI still defines permitted indexed fields. Only the physical indexes
  required by an actual view/query are materialized. Default order, opposite order,
  compound-covering keys, and scalar query keys remain distinct. There is no frequency
  heuristic and every permitted request is supported.
- Fresh ingestion creates the requested empty indexes and shares one record's
  extracted keys across writes. It no longer retains the entire collection's keys
  merely to open its default view. Later builds use a bounded 128 MiB key cache, with
  correct fallback and cleanup. Multi-pass builds can reuse SHA-verified keys by ID
  while the writer transaction holds the data immutable; a write clears that memo.
- Later extraction validates the complete Sonic JSON DOM, but converts only the
  fields needed by the indexes. Duplicate keys, exact numbers, malformed unselected
  values, non-object roots, and depth limits retain their semantics.
- An already verified open session builds a newly requested index from cached bytes,
  without a second source inventory or manifest load. File changes still follow the
  selected freshness policy and explicit refresh.
- Queries materialize identities before fetching payloads from their collections.
  Fixed join order avoids scanning payloads for small pages. New-index and collection
  primary-key statistics are collected together: otherwise SQLite can underestimate
  a primary-key scan and ignore a selective JSON index.
- Cache format 6 rebuilds older disposable databases once. The persisted configuration
  manifest remains independent of the schema versions applied to records. Source JSON
  is unchanged. [validation.md](../../docs/validation.md) documents the physical schema.

## Repeated performance comparison

The same driver/workload and `afe72d9e56` materialized-column baseline as the previous
audit. Ryzen 7 5700G, 16 logical CPUs, rustc 1.96.0, native release build, four ingestion
workers, manual validation, SQLite 64 MiB page cache, WAL with synchronous=FULL.
Fresh temporary caches on `/tmp`; source-file OS caches are warm, not flushed.
No other builds or tests ran during these benchmark samples.

Three fresh-cache runs per revision, alternating order; 15 cases, three warm repeats
per case, then reopen. **All 450 executions have matching result digests.** All warm
requests enumerate/read zero source files; reopened requests read zero source files.
The raw output also includes plans and separate stage timers. Timings are milliseconds;
first/reopen are medians of three samples, warm medians of nine samples. Each run
shares one cache across the listed cases; these are not fifteen independent cold starts.
Result construction is timed; digest calculation and printing are excluded.

| Case | Old first ms | New first ms | Old warm ms | New warm ms | Old reopen ms | New reopen ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `contents_default` | 4091.431 | 3504.571 | 1.103 | 1.106 | 345.606 | 346.727 |
| `entries_default` | 1367.258 | 1373.596 | 3.996 | 3.943 | 17.408 | 16.978 |
| `agents_default` | 188.800 | 184.946 | 1.097 | 1.172 | 11.848 | 11.637 |
| `items_default` | 81.378 | 67.155 | 0.864 | 0.939 | 11.500 | 11.568 |
| `pages_default` | 1.477 | 5.150 | 0.628 | 0.677 | 0.760 | 0.815 |
| `places_default` | 4.542 | 4.772 | 0.774 | 0.848 | 1.403 | 1.453 |
| `series_default` | 35.341 | 34.676 | 0.812 | 0.877 | 4.063 | 4.094 |
| `contents_year_desc` | 1.464 | 668.948 | 0.948 | 1.024 | 1.523 | 1.487 |
| `contents_year_filter` | 53.671 | 1476.021 | 47.383 | 20.969 | 55.422 | 24.493 |
| `contents_compound` | 27.333 | 19.391 | 26.843 | 18.895 | 26.857 | 19.395 |
| `contents_offset` | 3.051 | 4.979 | 1.073 | 4.533 | 2.889 | 6.511 |
| `year` | 1154.822 | 613.411 | 1163.930 | 91.614 | 1152.415 | 96.226 |
| `all_year` | 2142.783 | 674.191 | 2143.169 | 382.276 | 2093.114 | 377.701 |
| `text` | 1091.282 | 1686.724 | 1088.116 | 1024.071 | 1097.098 | 1027.030 |
| `nested` | 1968.099 | 1924.806 | 1984.647 | 1942.930 | 1933.555 | 1851.048 |

Cold contents opening is **3504.571 ms**,
versus **4091.431 ms** for the old
column-based design and **7475.293 ms** in the earlier JSON-index audit. Entries
are back near their original cold cost. The eight-record pages collection is
**5.150 ms**, versus 524.529 ms in that earlier audit;
its small remaining absolute overhead is included above.

The median sum of all 15 first-use operations is **12211.762 → 12256.176 ms**:
roughly unchanged from the old column-based design, including later index construction.
The earlier JSON-index implementation measured **20379.752 ms** for that sequence.
Across all measured operations (first uses, warm repeats and reopen), the per-run
sum is **38383.409 → 26653.453 ms**, about **30.6% less**.
These are sums of measured library operations, not subprocess wall-clock times.

**First use of an unused sort/filter still pays its index-build cost.** In this
sequence, year descending costs 668.948 ms the first time,
and the filtered view/count costs 1476.021 ms to build its covering/field
indexes. Their warm costs are 1.024 and 20.969 ms. This is explicit lazy construction,
not a claim that every first request is faster. The offset/count request also has a
small absolute warm increase, shown above. The scoped year query still seeks the
JSON index and measures 91.614 ms; global year is 382.276 ms.

## Verification and reproduction

**Verification passed:** 163 routine workspace tests, Clippy with the SQLite benchmark
feature, formatting/whitespace checks, and both real Musenalm acceptance tests. The
acceptance tests cover all 148,486 source records, complete direct/indexed view results,
reference occurrences, validation, strict/metadata reopening, and scoped/global JSON
filters. The independent query-first history also seeks the JSON index (93.807 ms warm
for the scoped year page/count, zero source reads).

The routine workspace suite includes regression tests for lazy table/index creation,
no source rescan for a new index, reuse across restart, invalidation and later rebuilding,
true format-5 shared-table cleanup, guarded key reuse, zero-budget fallback, and
malformed/unselected JSON. Existing change detection, rollback, schema/configuration,
reference, validation, numeric/operator/modifier and CLI tests remain in place.

```sh
cargo test --workspace --features objekte-bench/sqlite-bench
cargo clippy --workspace --all-targets --features objekte-bench/sqlite-bench -- -D warnings
cargo fmt --all -- --check
cargo test --release -p objekte-core --test musenalm --test musenalm_json_filter -- --ignored --nocapture
python3 crates/bench/compare_index_revisions.py BEFORE_PROFILE AFTER_PROFILE \
  examples/musenalm crates/bench/index-workload.json target/index-comparison \
  --samples 3 --repeats 3
```

Both executables must use the same historical `profile_indexes` driver (removed).
The [runner](compare_index_revisions.py) removes its temporary databases and writes
raw JSONL plus medians. This run's raw measurements are in ignored
`target/index-comparison-cold-fix-2026-10-29/`. Hardware timings are reported rather
than asserted as brittle thresholds; result equality and index use are regression tests.
