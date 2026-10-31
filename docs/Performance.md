# Performance of the current query path

The default JSON filter backend builds an in-memory file picker on first use of a collection. It indexes decoded scalar values and distinct Unicode-folded projections with consecutive and skip-one bigrams, creates presence bits for declared top-level fields, and creates B-tree typed postings only for top-level scalar schema fields with `"index": true`, including strings. Covered scalar and presence queries count matches from the index and read only their returned page; other queries scan source candidates and verify them with Sonic. Views use ordered keys when their requested filter and sort can be indexed. The Musenalm example explicitly opts its former numeric and boolean fields into the ordered indexes, preserving those query paths. See [implementation.md](implementation.md) for details.

JSON queries now accept stored paths outside the schema. An unscoped query searches every collection, and collections without a usable index scan source files. In a seven-run release rerun of the 50-query Musenalm workload on 31 October 2026, all match counts stayed the same, while the sum of per-query median times rose from 1.63 to 3.38 seconds and total scan counts from 1.46 to 3.46 million. These sums describe the workload, not one end-to-end run. Most added work came from fields that were previously queried only in schema-compatible collections. Specify a collection when the intended scope is known. The raw [before](../crates/bench/results/musenalm-mix-50-worker-exit-2026-10-31.ndjson) and [after](../crates/bench/results/musenalm-mix-50-schema-free-2026-10-31.ndjson) runs have all 50 cases.

With explicit schema indexes enabled for the Musenalm example's 15 numeric and boolean fields, a seven-pass release run of the same 50 queries had a 719 ms all-collection index warmup and 3.37 seconds in summed query medians. Temporarily removing all 15 `index` declarations shortened warmup to 488 ms but raised summed query medians to 4.56 seconds. The 50 match counts were identical; total scans rose from 3.46 to 4.76 million and parses from 2.80 to 4.10 million. This compares one index warmup per configuration, so the build-time difference is illustrative. The schemas were restored after the run. Raw results: [explicit indexes](../crates/bench/results/musenalm-mix-50-opt-in-indexes-release-2026-10-31.ndjson) and [no declared indexes](../crates/bench/results/musenalm-mix-50-no-declared-indexes-release-2026-10-31.ndjson).

## Current 16-case bigram versus source-grep run

On 30 October 2026, after field-presence bits and exact scalar counts were added, `json-query-grep-compare` ran 16 Musenalm `contents` filters with `--count`, a 50-record page, three timed repetitions per backend, and alternating backend order. The 133,948-file collection was indexed and both backends were checked for complete result parity before timing. Each figure below is the median across the 16 per-case median times; the OS file cache was warm and uncontrolled.

| Backend | Across-case median | Per-case range | Cases faster |
| --- | ---: | ---: | ---: |
| Warm in-memory bigram | 6.98 ms | 3.48–125.22 ms | 16 of 16 |
| Source grep | 386.09 ms | 360.65–395.04 ms | 0 of 16 |

The grep path still inventories all 133,948 source files for each counted request, even when its raw-byte prefilter finds fewer candidates. The broad `pseudonym = false` case took 125 ms in memory because it checked metadata for 122,276 indexed matches; it remained faster than source grep's 384 ms for that case. These measurements exclude first index construction and process startup. Reproduce with the command in the [benchmark index](../crates/bench/README.md).

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

On 30 October 2026, after adding field-presence bits and exact scalar counts, three more release runs of `year = 1800 --count --limit 1` on the same 133,948-file collection gave a median first build of **724 ms**. Median stage times were inventory 131 ms, ID slots 89 ms, parallel scan/projection/add 174 ms, compression 17 ms, typed finalization 203 ms, and view ordering 106 ms. Presence bits are collected during the existing parse and are included in the scan stage. This small uncontrolled comparison does not establish a build-time regression or improvement over the earlier 712 ms run. Typed finalization and view ordering still account for about **43%** of first build time.

After grouping repeated typed scalar keys, five release runs under the same workload gave median **633 ms** first build and **105 ms** typed finalization. The earlier three-run medians were 724 ms and 203 ms respectively. The new path samples each field and keeps the original ordered-map insertion for mostly unique values; numerically equal spellings are still merged exactly. These are small warm-filesystem-cache samples, so the difference is evidence for this workload rather than a cross-device guarantee.

After bulk-loading the sorted inventory into final ID slots, five more release runs gave median **538 ms** first build and **12.6 ms** in the ID-slot stage. The previous five-run medians were 633 ms and 92.7 ms respectively. The fresh picker no longer fills a temporary changed-file set or copies its already compact table; later rebuilds still compact after additions or deletions. These warm-filesystem-cache samples indicate a useful improvement for this workload, not a cross-device guarantee.

The index projection now walks Sonic's parsed tree directly and converts only selected top-level scalars for typed and view keys, avoiding a second full JSON tree. Five release runs after this change gave median **543 ms** first build and **174 ms** in parallel scan/projection/add, versus approximately 538 ms and 173 ms before it. There is no measured wall-time improvement on this workload; the change removes redundant full-tree materialization, but the index still parses every file once.

Ordered view keys are required for current view behavior, so their approximately 103 ms finalization remains in the first build. Inventory is another roughly 130 ms; its path, metadata, and sorted-map construction are the next places to inspect. The values-only layout experiment is excluded because its approximately five-second build cost misses the startup target.

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
- The current 16-case benchmark includes exact scalar counts and field-presence bits. Counted queries check indexed matches by metadata for covered predicates; other expressions still verify each selected source candidate. Without a count, source scanning starts with a bounded parallel batch, grows batches after misses, and stops after the requested page when global scalar-cardinality checks do not require a full scan. First-use index construction still visits the collection.

Run the current profiler and acceptance checks from the repository root:

```sh
cargo run --release -p objekte-core --example profile_bigram_query
cargo test --release -p objekte-core --test musenalm_bigram -- --ignored --nocapture
cargo test --release -p objekte-core --test json_query_oracle -- --ignored --nocapture
```

## Earlier experiments

The reports under `crates/bench/` preserve measurements of earlier SQLite, NDJSON, qj/jaq, Node, and Rust projection designs. They remain useful historical evidence but their cache recommendations and CLI timings describe removed implementations. Start with the [benchmark index](../crates/bench/README.md), [shared engine results](../crates/bench/RESULTS.md), and [old SQLite index study](../crates/bench/INDEX-COLD-BUILD-FIX-2026-10-29.md) when comparing those designs. The active runtime uses the in-memory picker by default; it has no persistent index or SQLite execution path.
