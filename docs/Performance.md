# Performance and benchmarks

Objekte builds or restores an in-memory index on first use of a collection.
Saved snapshots live under `.objekte/.cache`; query results are not cached.
Warm queries reuse the index but still verify selected source files. Query time
depends on collection scope, candidate selectivity, `--count`, sort coverage,
and whether the operating system has cached source files. See the
[index lifecycle](implementation.md) for the current execution paths.

The [benchmark index](../crates/bench/README.md) has commands and workload
definitions. Recent measurements include:

| Workload | What it measures |
| --- | --- |
| [100 Musenalm filters](../crates/bench/QUERY-MIX-100-METADATA-2026-11-02.md) | 50 JSON-field and 50 metadata filters, ASCII and UTF-8 byte indexes |
| [Core allocation](../crates/bench/CORE-ALLOCATION-2026-11-02.md) | Cold build, views, sorted queries, and snapshot save/restore |
| [Bigram encoding](../crates/bench/BIGRAM-ENCODING-2026-11-02.md) | Construction, memory, snapshot size, and query candidates for both encodings |
| [Sorted queries](../crates/bench/SORT-MIX-50-SINGLE-PASS-2026-11-01.md) | Indexed and unindexed sorts over the original 50 JSON filters |
| [Full-text search](../crates/bench/FTS-MUSENALM-2026-10-31.md) | An earlier Musenalm fixture with 30 FTS queries and a schema overlay |

Run the current 120-case query mix, including 20 sorted cases, from the repository root:

```sh
cargo build --release -p objekte-bench --bin json-query-mix
target/release/json-query-mix --repo examples/musenalm --runs 3
target/release/json-query-mix --repo examples/musenalm --runs 3 --bigram-encoding utf8_bytes
```

Pass `--workload crates/bench/workloads/musenalm-mix-50.json` to rerun the
original 50 cases. The runner warms indexes and source pages before timing,
alternates case order, and checks result stability across rounds. Its totals
are capped by the repository's 10,000-result limit. File modification and
creation dates depend on the local checkout and filesystem, so metadata
filter counts may vary between machines.

Compare matching results, candidate scans, allocation, and memory as well as
latency. Index warmup, snapshot restoration, and warm queries are separate
operations. The [historical performance notes](../crates/bench/QUERY-PERFORMANCE-HISTORY.md)
retain older measurements, including retired engine experiments; their
timings are not current baselines.
