# Query benchmarks

## Downloadable million-record examples

Two preparation scripts create schema-free Objekte collections under `examples/`.
The generated example directories, including downloaded source archives and JSON
files, are ignored by Git. Each script defaults to one million records and can
be rerun after an interruption.

```sh
python3 crates/bench/prepare_amazon_reviews.py
python3 crates/bench/prepare_nyc311.py
```

The [Amazon Reviews'23 Musical Instruments source](https://amazon-reviews-2023.github.io/main.html)
is downloaded as JSONL gzip and split into `examples/amazon-reviews-2023/data/reviews/`.
The [NYC 311 source](https://data.cityofnewyork.us/resource/erm2-nwe9.json)
is fetched in ordered API pages and split into `examples/nyc311/data/requests/`.
Use `--limit N --root PATH` to build a separate smaller fixture. Amazon also
accepts `--archive FILE.jsonl.gz`; NYC accepts `--page-size N`. NYC's API is
live, so a fresh import can contain changed records.

## Core allocation and copying

The [November 2 allocation report](CORE-ALLOCATION-2026-11-02.md) compares
four core changes on a 20,000-record nested fixture and Musenalm. It records
allocation counts, cumulative bytes, peak live heap, peak RSS, wall time,
query-result checks, and remaining allocation sources. The
[fixture generator](generate_allocation_fixture.py),
[benchmark executable](src/bin/core_allocation.rs), and
[runner](run_core_allocation.py) reproduce the measurements.

## Index-build stage profiles

The [November 2 index-build report](INDEX-BUILD-PROFILE-2026-11-02.md) measures
cache-free construction, source read and parsing, general and field text
projection, three posting-insertion stages, all three compression stages,
secondary serialization, peak RSS, and compressed sizes. It covers Musenalm,
Lenz, Leipzig, and the two materialized million-record examples. The
[runner](run_index_build_profiles.py),
[profile executable](src/bin/index_build_profile.rs), and
[raw 30-run result file](results/index-build-profile-2026-11-02.ndjson) make the
measurements reproducible:

```sh
python3 crates/bench/prepare_amazon_reviews.py
python3 crates/bench/prepare_nyc311.py
python3 crates/bench/run_index_build_profiles.py --runs 3
```

The profiler compiles with `index-timings`; normal builds omit the stage clocks.
Each profile starts a new process and ignores saved index caches. The generated
Amazon and NYC source repositories stay under ignored `examples/` directories;
only the reproducible benchmark code and results are committed.

## Bigram encoding comparison

`bigram-encoding-compare` builds a fresh picker with one encoding per process,
then measures peak resident memory, compressed index size, snapshot save/load,
and candidate-only query latency. Run both modes against the same directory or
the built-in multilingual corpus. The benchmark does not modify the repository.

```sh
cargo build --release -p objekte-bench --bin bigram-encoding-compare
target/release/bigram-encoding-compare ascii examples/musenalm/data/contents
target/release/bigram-encoding-compare utf8_bytes examples/musenalm/data/contents
target/release/bigram-encoding-compare ascii synthetic:24000
target/release/bigram-encoding-compare utf8_bytes synthetic:24000
```

Peak RSS is read from Linux `/proc/self/status`; it is `null` elsewhere. The
[November 2 comparison](BIGRAM-ENCODING-2026-11-02.md) records the measurements
and candidate counts.

The active `json-query-grep-compare` benchmark checks complete result pages and counts, then alternates in-memory bigram and source-grep backend order over a workload. It reports median wall time and candidate counts as NDJSON.

```sh
cargo build --release -p objekte-bench --bin json-query-grep-compare
target/release/json-query-grep-compare --repo examples/musenalm --workload crates/bench/workloads/musenalm-grep.json --runs 3
```

The `json-query-mix` benchmark runs 100 unscoped filters against every
collection with one warm repository handle. The first 50 are the original
JSON-field workload; cases 51–100 exercise source metadata fields, byte-size
units, UTC date periods, status, and combinations with JSON fields. Each filter
counts all matches and returns one record. It warms the indexes and runs each
filter once before timing, then alternates case order across rounds. Output is
NDJSON with a setup row followed by one row per filter.

```sh
cargo build --release -p objekte-bench --bin json-query-mix
target/release/json-query-mix --repo examples/musenalm --runs 7
target/release/json-query-mix --repo examples/musenalm --runs 7 --bigram-encoding utf8_bytes
target/release/json-query-mix --repo examples/musenalm --runs 7 --no-schema
```

Use `--workload crates/bench/workloads/musenalm-mix-50.json` to rerun the
original 50-case workload. Historical reports below refer to that workload.
Modification and creation timestamps depend on the local checkout and
filesystem, so metadata case counts can differ between machines.
The [100-query metadata report](QUERY-MIX-100-METADATA-2026-11-02.md) lists
all new filters and the ASCII/UTF-8 byte timings.

The [November 2 encoding comparison](BIGRAM-50-QUERY-2026-11-02.md) runs all
50 configured-schema queries with both bigram encodings and records each result.

The [400,000-record multilingual comparison](BIGRAM-FOREIGN-50-2026-11-02.md)
uses 50 searches over Leipzig news sentences in eight languages. The
[example corpus](../../examples/leipzig-multilingual/README.md) contains the
JSON files and data attribution. The [preparation script](prepare_leipzig_corpus.py)
downloads pinned source archives and recreates them; the report links the
workload and all six raw benchmark runs.

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
The [single-pass rerun](SORT-MIX-50-SINGLE-PASS-2026-11-01.md) compares all 50
sorted cases before and after reusing parsed matches from candidate verification.

## Full-text and historical measurements

The [30-case full-text report](FTS-MUSENALM-2026-10-31.md) records build time,
resident memory, and warm searches on an earlier Musenalm fixture with a
temporary rendering schema overlay. Reproduce the workload with:

```sh
cargo run --release -p objekte-bench --bin fts-query-mix -- examples/musenalm 3
```

The [historical query performance notes](QUERY-PERFORMANCE-HISTORY.md) collect
older comparisons and build-stage measurements. Some commands there refer to
removed binaries; use the current commands above for new measurements.
