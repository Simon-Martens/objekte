# Performance of the current query path

The default JSON filter backend builds an in-memory file picker on first use of a collection. It indexes decoded scalar values and distinct Unicode-folded projections with consecutive and skip-one bigrams, and creates typed postings for declared number, integer, boolean, and enum fields. Queries select candidates, scan original file bytes, parse candidates with Sonic, and evaluate the full expression. Views use ordered keys when their requested filter and sort can be indexed. See [implementation.md](implementation.md) for details.

## Current 16-case bigram versus source-grep run

On 30 October 2026, `json-query-grep-compare` ran 16 Musenalm `contents` filters with `--count`, a 50-record page, three timed repetitions per backend, and alternating backend order. The 133,948-file collection was indexed and both backends were checked for complete result parity before timing. Each figure below is the median across the 16 per-case median times; the OS file cache was warm and uncontrolled.

| Backend | Across-case median | Per-case range | Cases faster |
| --- | ---: | ---: | ---: |
| Warm in-memory bigram | 6.82 ms | 3.31–75.55 ms | 16 of 16 |
| Source grep | 386.81 ms | 363.56–411.48 ms | 0 of 16 |

The grep path still inventories all 133,948 source files for each counted request, even when its raw-byte prefilter finds fewer candidates. These measurements exclude first index construction and process startup. Reproduce with the command in the [benchmark index](../crates/bench/README.md).

## Warm JSON filter queries

The historical [43-case Musenalm comparison](../crates/bench/BIGRAM-VS-SEGMENTS-2026-10-29.md) ran complete `Repository::query_json` requests over 133,948 `contents` files. Each request counted all matches, returned the first three records, and validated those returned records. Both backends were warmed before five timed repeats per case; a second process reversed case order. The OS file cache was warm and uncontrolled. These are per-case medians in milliseconds, excluding initial index construction. The run predates `:ignorecase(...)` and Unicode-folded projections, so these are historical timings for the older ASCII-insensitive `~` behavior:

| Measure | In-memory bigram and typed index | Persistent segments |
| --- | ---: | ---: |
| Forward: median across 43 cases | 14.8 ms | 22.6 ms |
| Reverse: median across 43 cases | 15.5 ms | 23.4 ms |
| Cases won, forward / reverse | 35 / 37 | 8 / 6 |

The initial in-memory index build took 525 ms and 520 ms in those two processes. A fresh CLI invocation creates a fresh process and pays that cost again; repeated requests on one `Repository` handle reuse the picker. The segment figures below describe a removed backend and remain only as comparison evidence. Its fresh CLI observations were not controlled startup or memory measurements.

### First-use index build breakdown

The CLI's `bigram_open_ms` includes directory inventory, the ID table, reading and projecting every source file, bigram compression, typed postings, and ordered view keys. Detailed stage timers are compiled only with the opt-in `index-timings` Cargo feature. Normal release builds omit their measurement code and their diagnostic fields; the overall `bigram_open_ms` remains available. On 29 October 2026, a single warm-filesystem-cache `contents` query over 133,948 JSON files gave these stage timings (milliseconds):

| Stage | Release | Debug |
| --- | ---: | ---: |
| Directory inventory and stamps | 139 | 231 |
| ID slots and rebuild table | 90 | 224 |
| Parallel file scan, JSON projection, bigram insertion (wall time) | 162 | 1,044 |
| Bigram compression | 15 | 79 |
| Typed postings finalization | 195 | 1,720 |
| Ordered view keys finalization | 102 | 532 |
| **Total `bigram_open_ms`** | **712** | **3,852** |

The stages leave a small amount of setup and cleanup outside their timers. The scan runs in parallel; `bigram_project_worker_ms` and `bigram_add_worker_ms` in diagnostics sum elapsed time across workers, including any waits, and therefore must not be added to wall timings. In this run, typed postings and ordered view keys together accounted for roughly 42% of release build time and 58% of debug build time. Both structures currently hold scalar-field information independently. The observed time depends on build profile, hardware, file cache, and collection. A fresh CLI process rebuilds these structures even if the same query ran immediately before.

To collect the detailed timings in an optimized build:

```sh
cargo run --release -p objekte-cli --features index-timings -- examples/musenalm query contents 'year = 1800' --count --diagnostics
```

The forward run's median candidate selection took 1.4 ms, and its median source scan and verification took 12.8 ms. The phases are medians over different cases and do not add up to a representative request. For the old `preferred_title ~ 'Liebe' && year = 1800` query (now written with `:ignorecase('Liebe')`), intersecting text and typed postings selected 242 files and the full query took 1.8 ms. Common short strings can be less selective: `preferred_title = 'No. 11.'` opened 16,141 candidate files and took 17.4 ms with bigrams versus 6.4 ms with segments. Broad or negated queries may need to read most files, especially with `--count`.

The published 43-case report predates the typed `OR` union planner. Its `year_or` row shows a full scan of 133,948 files and is **not a current timing** for indexed numeric branches. The current planner can union typed postings when every branch has a safe indexed candidate set; the full filter is still verified against source JSON. Mixed text/number `OR` may still scan broadly. Those cases need a fresh benchmark. The [release acceptance test](../crates/core/tests/musenalm_bigram.rs) compares complete ordered IDs and totals with direct evaluation for all 43 filters. An [independent source-file oracle](../crates/core/tests/json_query_oracle.rs) parses with `serde_json` and uses handwritten predicates to compare complete IDs with bigram results for those same 43 filters; it is an opt-in release test. A smaller oracle fixture runs in the routine suite against bigram, direct, and grep backends. Focused tests cover numeric `OR`, edits, exact decimals, arrays, Unicode, and candidate completeness.

## Measurement limits and next checks

- Warm query measurements exclude process startup, first index build, and a cold OS page cache. They do not predict fresh CLI latency.
- The 43-case report measures JSON filters, not the full configured view workload. View first-use, warm page, count, and sort costs need a current end-to-end comparison across memory and direct modes.
- Candidate counts, file opens, source bytes, parsing, and validation should be reported by phase for broad matches, mixed text/number filters, and edits. A repeated-query benchmark should keep one repository handle open.
- Compare complete outputs and ordering whenever changing the candidate planner; a fast prefilter cannot omit a real match.
- The benchmark uses `--count`, so the in-memory backend must check every candidate to compute totals. Without a count, it now checks candidates in bounded parallel batches and stops after the requested page when global scalar-cardinality checks do not require a full scan. First-use index construction still visits the collection.

Run the current profiler and acceptance checks from the repository root:

```sh
cargo run --release -p objekte-core --example profile_bigram_query
cargo test --release -p objekte-core --test musenalm_bigram -- --ignored --nocapture
cargo test --release -p objekte-core --test json_query_oracle -- --ignored --nocapture
```

## Earlier experiments

The reports under `crates/bench/` preserve measurements of earlier SQLite, NDJSON, qj/jaq, Node, and Rust projection designs. They remain useful historical evidence but their cache recommendations and CLI timings describe removed implementations. Start with the [benchmark index](../crates/bench/README.md), [shared engine results](../crates/bench/RESULTS.md), and [old SQLite index study](../crates/bench/INDEX-COLD-BUILD-FIX-2026-10-29.md) when comparing those designs. The active runtime uses the in-memory picker by default; it has no persistent index or SQLite execution path.
