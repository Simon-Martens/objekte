# Query benchmarks

The active `json-query-grep-compare` benchmark checks complete result pages and counts, then alternates in-memory bigram and source-grep backend order over a workload. It reports median wall time and candidate counts as NDJSON.

```sh
cargo build --release -p objekte-bench --bin json-query-grep-compare
target/release/json-query-grep-compare --repo examples/musenalm --workload crates/bench/workloads/musenalm-grep.json --runs 3
```

The dated reports in this directory record earlier architectures and experiments. The [43-case bigram versus segment report](BIGRAM-VS-SEGMENTS-2026-10-29.md) is historical evidence for the in-memory decision; its segment backend has been removed. The [FFF comparison](FFF-VS-SEGMENTS-2026-10-29.md) and [values-only layout study](VALUES-ONLY-CACHE-2026-10-29.md) likewise describe retired benchmark executables. Commands in those reports are archival and are not expected to run against the current workspace.
