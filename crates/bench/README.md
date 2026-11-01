# Query benchmarks

The active `json-query-grep-compare` benchmark checks complete result pages and counts, then alternates in-memory bigram and source-grep backend order over a workload. It reports median wall time and candidate counts as NDJSON.

```sh
cargo build --release -p objekte-bench --bin json-query-grep-compare
target/release/json-query-grep-compare --repo examples/musenalm --workload crates/bench/workloads/musenalm-grep.json --runs 3
```

The `json-query-mix` benchmark runs 50 unscoped filters against every
collection with one warm repository handle. Each filter
counts all matches and returns one record. It warms the indexes and runs each
filter once before timing, then alternates case order across rounds. Output is
NDJSON with a setup row followed by one row per filter.

```sh
cargo build --release -p objekte-bench --bin json-query-mix
target/release/json-query-mix --repo examples/musenalm --runs 7
target/release/json-query-mix --repo examples/musenalm --runs 7 --no-schema
```

`--no-schema` discovers the same data folders without loading their default
schemas or ordered schema postings. The example repository is not modified.
Cases 34–36 now query `entries.items` and the `contents.entry` reference.
The [50-query schema comparison](QUERY-SCHEMA-COMPARISON-2026-10-31.md)
records the earlier fixture, when `entries.contents` was embedded; its saved
numbers are historical and differ from a run of the current workload.

See [the October 31 baseline](QUERY-MIX-50-2026-10-31.md) for the full table
before snapshot counts and null/inequality postings. The
[after-change report](QUERY-INDEXED-COUNTS-AFTER-2026-10-31.md) compares the
affected cases and links the full raw rerun.

The dated reports in this directory record earlier architectures and experiments. The [43-case bigram versus segment report](BIGRAM-VS-SEGMENTS-2026-10-29.md) is historical evidence for the in-memory decision; its segment backend has been removed. The [FFF comparison](FFF-VS-SEGMENTS-2026-10-29.md) and [values-only layout study](VALUES-ONLY-CACHE-2026-10-29.md) likewise describe retired benchmark executables. Commands in those reports are archival and are not expected to run against the current workspace.

## Sorting the 50-query workload

`json-query-sort-mix` runs the same 50 filters with and without a sort clause.
The [sort workload](workloads/musenalm-sort-mix-50.json) assigns a relevant
collection and one of 33 sort orders to each filter, covering 25 first fields.
`--scope original` keeps the 50-query benchmark's unscoped searches;
`--scope relevant` searches the assigned collection so declared sort indexes
can be used. Each query counts up to the repository's 10,000-result cap and
returns 25 records. An untimed pass warms source pages and checks count parity;
the timed rounds check count and page stability.

```sh
cargo build --release -p objekte-bench --bin json-query-sort-mix
target/release/json-query-sort-mix --repo examples/musenalm --scope original --runs 3
target/release/json-query-sort-mix --repo examples/musenalm --scope relevant --runs 3
```

The [November 1 report](SORT-MIX-50-2026-11-01.md) includes every case and
links the raw NDJSON results for both scopes.
