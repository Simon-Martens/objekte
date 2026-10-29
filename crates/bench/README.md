# Query benchmarks

The current benchmarks exercise the repository's JSON query API against original source files and the optional segment cache. The [in-memory index comparison](BIGRAM-VS-SEGMENTS-2026-10-29.md) covers 43 warm filters with complete result parity. The [FFF comparison](FFF-VS-SEGMENTS-2026-10-29.md) covers 33 filters and candidate stages. Its standalone source is in [fff-compare](fff-compare/).

## Segment cache and source grep

`json-query-grep-compare` checks complete result pages and counts, then alternates backend order over the supplied workload. It reports median wall time and candidate counts as NDJSON.

```sh
cargo build --release -p objekte-bench --bin json-query-grep-compare
target/release/json-query-grep-compare --repo examples/musenalm --workload crates/bench/workloads/musenalm-grep.json --data-dir target/json-query-grep-cache --runs 3
```

## Values-only layout experiment

`values-cache-compare` builds disposable per-record, single-pack, and 64-shard text files. It checks results against the current segment backend before timing. The [values-only report](VALUES-ONLY-CACHE-2026-10-29.md) and [shard follow-up](64-SHARD-VALUES-CACHE-2026-10-29.md) record the original measurements and limitations.

```sh
cargo build --release -p objekte-bench --bin values-cache-compare
target/release/values-cache-compare --repo examples/musenalm --workload crates/bench/workloads/musenalm-grep.json --data-dir target/json-query-grep-cache --work-dir target --runs 3
```

The older reports in this directory are retained as historical measurements. Their retired benchmark executables are no longer part of the workspace.
