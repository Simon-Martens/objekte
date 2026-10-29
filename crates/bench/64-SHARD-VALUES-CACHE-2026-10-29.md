# 64-shard values cache experiment, 29 October 2026

This extends the [values-only cache experiment](VALUES-ONLY-CACHE-2026-10-29.md)
with 64 packed text shards. It is a benchmark prototype, not a production cache.
Each of the 133,948 `contents` IDs is assigned to a shard by a stable FNV-1a hash;
records remain byte-sorted by ID within each shard. The benchmark keeps the
line-to-ID mapping in memory. Each shard has only current scalar values, with
ASCII unit separators between values and one newline per record.

```sh
cargo build --release -p objekte-bench --bin values-cache-compare
target/release/values-cache-compare --repo examples/musenalm --workload crates/bench/workloads/musenalm-grep.json --data-dir target/json-query-grep-cache --work-dir target --runs 3
target/release/values-cache-compare --repo examples/musenalm --workload crates/bench/workloads/musenalm-grep.json --data-dir target/json-query-grep-cache --work-dir target --mode update
```

The runner builds per-record files, one pack, and 64 shards together in a
temporary directory. Building all three took **3.03 s** with warm OS caches.
The 64 shards contain 52,475,524 logical bytes and use **52,600,832 allocated
bytes**; the one pack has the same logical size. The per-record files use
548,651,008 allocated bytes on this filesystem.

## Warm query results

All three values layouts search with ripgrep's `grep-searcher`, parse candidates
from original JSON files with Sonic, apply the compiled full filter, count all
matches, and materialize the first 50 in ID order. Shards are searched in
parallel with Rayon across 32 available workers; the single packed file is
searched by one worker. Every result was checked against SQLite before timing.
Figures are median wall milliseconds from three timed repetitions, with backend
order reversed on alternate runs. The SQLite cache was already warm.

| Filter | Candidates | SQLite | One pack | 64 shards | Per-record files |
| --- | ---: | ---: | ---: | ---: | ---: |
| `preferred_title ~ 'almanach'` | 2,109 | 906.4 | 13.7 | 4.5 | 54.8 |
| `preferred_title ~ 'Liebe'` | 7,716 | 896.8 | 18.3 | 8.5 | 57.2 |
| `preferred_title ~ 'Taschen%'` | 3,610 | 902.4 | 14.1 | 5.6 | 55.2 |
| `preferred_title ~ 'Gedicht'` | 65,996 | 915.0 | 49.2 | 35.0 | 79.6 |
| `preferred_title ~ 'Mond'` | 1,258 | 904.8 | 13.3 | 4.8 | 54.7 |
| `preferred_title ~ 'Schiller'` | 1,146 | 905.4 | 14.3 | 4.1 | 55.4 |
| `preferred_title ~ 'Goethe'` | 446 | 914.9 | 12.1 | 3.4 | 55.5 |
| `preferred_title = 'Der Mond'` | 255 | 0.9 | 18.9 | 3.4 | 53.6 |
| `preferred_title = 'No. 11.'` | 11 | 0.8 | 11.9 | 2.8 | 53.5 |
| `musenalm_type ?= 'Gedicht/Lied'` | 59,586 | 883.3 | 47.1 | 30.9 | 76.3 |
| `year = 1800` | 133,948 | 3.4 | 58.6 | 58.0 | 59.2 |
| `year = 1801` | 133,948 | 3.2 | 58.2 | 58.2 | 59.0 |
| `year = 1802` | 133,948 | 3.5 | 59.2 | 58.2 | 59.4 |
| `pseudonym = true` | 11,675 | 919.8 | 19.4 | 9.9 | 57.7 |
| `pseudonym = false` | 132,702 | 931.3 | 84.9 | 63.0 | 102.6 |
| `preferred_title ~ 'Liebe' && year = 1800` | 7,716 | 22.0 | 17.8 | 8.2 | 57.0 |

Across the seven standalone `preferred_title ~` filters, the median of the
per-filter medians was **4.8 ms** for 64 shards, **14.1 ms** for one pack, and
**55.4 ms** for per-record files. Parallel shard scans account for part of the
gain over one pack. Queries with no safe text needle still parse all records and
gain nothing from sharding. SQLite remains faster for indexed title equality
and year equality. The combined title/year filter took 8.2 ms with shards and
22.0 ms with SQLite on this workload.

## One-record text-shard replacement

A separate 20-repetition update probe alternated one record's title between its
original value and a slightly longer value. It parsed that source file with
Sonic, flattened the record, read its current shard, rewrote the shard to a
temporary file, synced the file, renamed it over the prior shard, and synced the
directory. The selected shard had **2,043 records** and **812,609 bytes**. The
median was **3.09 ms**. After the final repetition, its bytes matched the
original shard exactly. Source JSON was never changed.

This is a text-only update cost. It excludes writing a persisted ID map,
metadata/revision checks, typed indexes, snapshot manifest publication, and
reader-safe reclamation. The query prototype likewise keeps ID maps in memory
and does not implement freshness or schema validation. These costs must be
measured before replacing SQLite.
