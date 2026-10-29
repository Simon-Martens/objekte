# Warm SQLite versus source grep JSON queries, 29 October 2026

Command, from the repository root:

```sh
cargo build --release -p objekte-bench --bin json-query-grep-compare
target/release/json-query-grep-compare --runs 3 --repo examples/musenalm --workload crates/bench/workloads/musenalm-grep.json --data-dir target/json-query-grep-cache
```

The `contents` collection has 133,948 source JSON files (524 MiB on disk). The
machine was an Intel Core i9-13900K with 32 logical CPUs. SQLite used an existing,
warm disposable cache; the runner warmed each filter before timing, including any
lazy index creation. Each query requested a count and the first 50 complete records.
Both backends ran through the public `Repository::query_json` API in one process.
The source backend used ripgrep's buffered `grep-searcher` with 32 Rayon workers
for candidate search and Sonic verification. It reused the repository's checked
inventory and descriptor opens. Memory mapping was disabled because the source
files can change concurrently and the crate marks mapping unsafe in that case.
Backend order alternated across three repetitions. The table shows median wall time
in milliseconds. The warmup checked equal counts and serialized page records for
every filter. OS filesystem caches were warm and not cleared. Source files were
unchanged.

| Filter | Matches | SQLite ms | Grep ms | Grep candidates |
| --- | ---: | ---: | ---: | ---: |
| `preferred_title ~ 'almanach'` | 909 | 907.0 | 436.7 | 2,109 |
| `preferred_title ~ 'Liebe'` | 2,718 | 915.7 | 435.7 | 7,716 |
| `preferred_title ~ 'Taschen%'` | 618 | 893.1 | 437.6 | 3,610 |
| `preferred_title ~ 'Gedicht'` | 1,977 | 905.8 | 444.2 | 65,996 |
| `preferred_title ~ 'Mond'` | 419 | 908.3 | 435.7 | 1,258 |
| `preferred_title ~ 'Schiller'` | 307 | 895.1 | 439.2 | 1,146 |
| `preferred_title ~ 'Goethe'` | 59 | 907.9 | 437.5 | 446 |
| `preferred_title = 'Der Mond'` | 12 | 0.9 | 434.9 | 1,258 |
| `preferred_title = 'No. 11.'` | 4 | 0.9 | 451.2 | 133,948 |
| `musenalm_type ?= 'Gedicht/Lied'` | 59,586 | 884.5 | 443.1 | 65,996 |
| `year = 1800` | 2,908 | 3.5 | 453.2 | 133,948 |
| `year = 1801` | 2,608 | 3.2 | 454.1 | 133,948 |
| `year = 1802` | 3,073 | 3.5 | 454.3 | 133,948 |
| `pseudonym = true` | 11,672 | 905.5 | 435.0 | 11,675 |
| `pseudonym = false` | 122,276 | 933.4 | 453.8 | 132,702 |
| `preferred_title ~ 'Liebe' && year = 1800` | 57 | 21.7 | 437.1 | 7,716 |

Parallel source search beat SQLite on all seven standalone title substring filters,
by roughly a factor of two. It also won on the unindexed array and boolean
equality filters in this workload. Its request time stayed around 0.44–0.45 seconds
even when nearly every file was a candidate. SQLite's expression indexes made exact
title and year equality far faster: roughly 1–4 milliseconds. Combining a title
substring with an indexed year also strongly favored SQLite (22 versus 437 ms).
The earlier single-thread matcher-only measurement is superseded by this run.

This compares end-to-end warm requests with count and a 50-record page. It does not
measure cache construction, cold OS caches, all-result output, or concurrent queries.
The grep backend only observes scalar-cardinality errors in files that pass its
byte prefilter; see [filter semantics](../../docs/filters.md).
