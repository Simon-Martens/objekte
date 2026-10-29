# Values-only text cache experiment, 29 October 2026

This is a disposable benchmark prototype, not a replacement for the repository
cache. It uses the 133,948 JSON records in Musenalm `contents` and the same 16
filters as [the source grep benchmark](GREP-QUERY-2026-10-29.md).

```sh
cargo build --release -p objekte-bench --bin values-cache-compare
target/release/values-cache-compare --repo examples/musenalm --workload crates/bench/workloads/musenalm-grep.json --data-dir target/json-query-grep-cache --work-dir target --runs 3
```

## Cache representation and cost

The builder uses Sonic to read each source record and emits every scalar value as
decoded text. It omits property names, JSON braces, brackets, commas, and syntax
quotes. Punctuation that belongs to a string value remains. ASCII unit separators
separate values; one newline separates records. The per-record layout has one text
file per JSON source. The packed layout concatenates the same record lines into one
file and maps line numbers to IDs using an in-memory sorted path list. The benchmark
creates both layouts together in a temporary directory and deletes them on exit.

| Artifact | Logical bytes | Allocated bytes |
| --- | ---: | ---: |
| Source JSON files | 145,870,586 | 548,651,008 |
| Values-only per-record files | 52,475,524 | 548,651,008 |
| Values-only packed file | 52,475,524 | about 52.5 million |
| Existing SQLite cache directory | 261,533,696 | 261,533,696 |

Building both values layouts took **3.02 s** with a warm OS file cache. A separate
fresh SQLite query using a new temporary cache directory took **6.24 s**; that
measurement also includes SQLite ingestion, index construction, validation, and
the first query, so it is not a build-only comparison. No source file was changed.

## Warm query results

All four source search paths use ripgrep's `grep-searcher` and parse candidates
with Sonic before applying the same compiled `JsonFilter`. `Raw control` searches
the original JSON paths using the same benchmark loop as `Values files`, without
repository inventory checks. `Raw query` is the production API's source backend,
which includes its inventory and freshness checks. The packed search scans one
text file, then verifies candidates against original JSON in parallel. SQLite is
the warm production backend. All queries requested a count and the first 50
records. Warmup checked equal counts, IDs, and page JSON against SQLite for every
case. Three timed repetitions alternated backend order; numbers are median wall
milliseconds on an Intel Core i9-13900K with 32 workers and warm OS caches.

| Filter | Candidates | SQLite | Raw query | Raw control | Values files | Packed values |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `preferred_title ~ 'almanach'` | 2,109 | 908.2 | 442.9 | 63.4 | 47.1 | 13.8 |
| `preferred_title ~ 'Liebe'` | 7,716 | 909.9 | 443.9 | 64.9 | 49.4 | 17.7 |
| `preferred_title ~ 'Taschen%'` | 3,610 | 910.6 | 442.7 | 64.4 | 47.7 | 14.1 |
| `preferred_title ~ 'Gedicht'` | 65,996 | 916.1 | 446.1 | 82.0 | 68.8 | 48.9 |
| `preferred_title ~ 'Mond'` | 1,258 | 912.6 | 444.1 | 63.4 | 46.6 | 12.8 |
| `preferred_title ~ 'Schiller'` | 1,146 | 921.1 | 445.8 | 63.3 | 47.4 | 13.7 |
| `preferred_title ~ 'Goethe'` | 446 | 919.5 | 442.6 | 63.5 | 47.9 | 11.8 |
| `preferred_title = 'Der Mond'` | 255 | 0.9 | 441.3 | 63.0 | 45.6 | 18.8 |
| `preferred_title = 'No. 11.'` | 11 | 0.8 | 456.4 | 62.4 | 45.6 | 11.8 |
| `musenalm_type ?= 'Gedicht/Lied'` | 59,586 | 885.9 | 445.9 | 80.3 | 66.6 | 46.5 |
| `year = 1800` | 133,948 | 3.4 | 461.3 | 57.3 | 57.5 | 55.8 |
| `year = 1801` | 133,948 | 3.2 | 460.8 | 57.2 | 58.1 | 56.4 |
| `year = 1802` | 133,948 | 3.5 | 460.4 | 58.2 | 57.3 | 56.3 |
| `pseudonym = true` | 11,675 | 909.2 | 445.0 | 65.8 | 50.5 | 19.4 |
| `pseudonym = false` | 132,702 | 935.2 | 462.3 | 101.3 | 91.2 | 82.3 |
| `preferred_title ~ 'Liebe' && year = 1800` | 7,716 | 22.5 | 442.9 | 64.6 | 48.7 | 17.1 |

For seven standalone title substring filters, the medians were **63.5 ms** for
raw control, **47.7 ms** for per-record values, and **13.8 ms** for packed values.
Thus the text format itself helped, and packing helped more by removing most
per-file opens. The production source query's roughly 444 ms median includes
inventory/freshness work absent from the cache prototype. Candidate counts for
the controlled raw and values searches were equal on these filters, so their
timing difference came from the searched representation rather than fewer hits.

SQLite remained much faster for indexed exact title and year equality. Numeric
equality had no safe literal in this prototype, because equal JSON numbers can
have different source lexemes; all 133,948 source records were parsed. The packed
cache offered no help there. The combined title/year filter was close (17 versus
22 ms), but only SQLite used the year index.

## What the prototype leaves out

The workload supplies manually chosen, necessary literal needles. A production
backend would need a general extractor for the entire filter syntax, including
escapes, Boolean branches, and numeric normalization. The values cache does not
track source revisions, reconcile edits, validate schemas, or persist its packed
line-to-ID map. Its query timings are therefore a lower bound for a correct cache
replacement. Updating one per-record text file is straightforward; a packed file
needs an updateable index or periodic rebuilds. These costs should be included
before deciding to replace SQLite.
