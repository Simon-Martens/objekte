# Performance and benchmarks

Objekte builds or restores in-memory indexes on first use of a collection.
Saved snapshots live under `.objekte/.cache`. A separate, bounded process-local
query cache can retain verified predicate membership and complete ordered
result slots. Compatible page changes can reuse the ordered slots without
rechecking non-page records; returned page records are still read from source.
Set `Options.query_cache_bytes` to zero to disable this cache. Streaming reuse
requires the separate `Options.stream_query_cache` opt-in. Query time depends
on collection scope, candidate selectivity, `--count`, sort coverage, cache
state, and whether the operating system has cached source files. See the
[index lifecycle](implementation.md) for the current execution paths.

The [benchmark index](../../crates/bench/README.md) has commands and workload
definitions. These dated reports record earlier, revision-specific measurements:

| Workload | What it measures |
| --- | --- |
| [100 Musenalm filters](../../crates/bench/QUERY-MIX-100-METADATA-2026-11-02.md) | 50 JSON-field and 50 metadata filters, ASCII and UTF-8 byte indexes |
| [Core allocation](../../crates/bench/CORE-ALLOCATION-2026-11-02.md) | Cold build, views, sorted queries, and snapshot save/restore |
| [Bigram encoding](../../crates/bench/BIGRAM-ENCODING-2026-11-02.md) | Construction, memory, snapshot size, and query candidates for both encodings |
| [Sorted queries](../../crates/bench/SORT-MIX-50-SINGLE-PASS-2026-11-01.md) | Indexed and unindexed sorts over the original 50 JSON filters |
| [Full-text search](../../crates/bench/FTS-MUSENALM-2026-10-31.md) | An earlier Musenalm fixture with 30 FTS queries and a schema overlay |

The default `musenalm-mix-100.json` workload currently loads **152 cases**;
its filename reflects an earlier size. The runner requests a count and one
returned record per case. Run the current mix from the repository root with
both query-cache budgets:

```sh
cargo build --release -p objekte-bench --bin json-query-mix
target/release/json-query-mix --repo examples/musenalm --runs 7 --bigram-encoding ascii --query-cache-mib 0
target/release/json-query-mix --repo examples/musenalm --runs 7 --bigram-encoding ascii --query-cache-mib 64
```

Pass `--workload crates/bench/workloads/musenalm-mix-50.json` to rerun the
original 50 cases. `--bigram-encoding utf8_bytes` and `--no-schema` select
other supported configurations. The runner warms indexes before timing,
alternates case order, and checks result stability across rounds. Its totals
are capped by the repository's 10,000-result limit. File modification and
creation dates depend on the local checkout and filesystem, so metadata filter
counts may vary between machines. Use the same source paths and schema for a
reference/candidate comparison, and preserve the reference binary separately.

Compare per-case results, candidate scans, source reads and parses as well as
latency; use the allocation runner for allocation and memory questions. Cache
hits can still read returned records. Index warmup, snapshot restoration,
cache-off warm-index queries, and repeated cache hits are separate operations.
The runners generally use warm filesystem caches, not cold physical storage.
The [historical performance notes](../../crates/bench/QUERY-PERFORMANCE-HISTORY.md)
retain older measurements, including retired engine experiments; their timings
are not current baselines.
