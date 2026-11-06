# General JSON filter acceptance, 28 October 2026

Command:

```sh
cargo test --release -p objekte-core --test musenalm_json_filter -- --ignored --nocapture
```

All seven registered collections, 148,486 records. Temporary application directory,
4 ingestion workers, 64 MiB SQLite page-cache budget, manual background validation.
Release build with the workspace's native CPU setting. The first SQL query creates
the entire raw-record cache; later queries reuse it. OS filesystem caches were not
flushed. Direct execution validates inline, whereas SQL manual mode leaves validation
pending, so total times describe these execution modes, not a pure engine benchmark.

Every query requests all matching records and a count, and compares source/SQLite
counts and complete ordered record identities, paths, and JSON values. Timers exclude
terminal serialization. Warm SQL repetitions assert zero source reads/enumerations.

| Filter | Matches | Direct total ms | SQL total ms | SQL query phase ms | Warm SQL total ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| `year >= 1800 && year < 1805` | 14,196 | 5783.252 | 7418.846 | 2567.637 | 2549.513 |
| `preferred_title ~ 'liebe'` | 2,797 | 5687.875 | 2181.554 | 2180.594 | 2057.498 |
| `contributors.agents ?= '1500' \|\| exists(contents[0])` | 2,394 | 5621.188 | 3442.335 | 3441.328 | 3295.745 |

These are individual acceptance timings, not statistical latency distributions. A
verification repeat also passed all comparisons and explicit returned-row-count
assertions; its warm SQL totals were 2712.600, 2269.388, and 3588.511 ms respectively.
The first
SQL run read 148,486 source files; every later SQL run read zero. All comparisons
passed. The compiler scans the scoped cached JSON with strict typed semantics and
does not use UI-derived field indexes or cache query results yet.
