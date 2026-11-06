# 152-query speed check on current master

## What was measured

Current `master` at `ec2ae919bf19c80798e2916f7fb671309ede5b7f` was compared with two preserved release binaries: old master `2d6d876ee7f308890ee19868d18ad26ebea31cc3` and the pre-text-integration library revision `90b8db3c7e67d5102355e7525da90a8733ce890b`. The repository working tree had no production edits. The loaded workload contains **152** filters, despite its `musenalm-mix-100.json` filename.

All binaries used the same Musenalm files and metadata. The current fingerprint matches the prior master comparison exactly: 148,486 records, 193,297,230 bytes, and source-content/stat SHA256 `90e36ad63dcb6386e9521c583fdcd36451fd3d6182187dced542fb8292f730ce`. The workload SHA256 is `5cfb37386221a6f2237bc1755e12c7a89f1e1f4fdeff4dd57f1b00370c4eb99e`. Rust/Cargo 1.99.1, native CPU release flags, ASCII bigrams, configured schemas, 16 Rayon threads, CPUs 0–15, and warm filesystem cache match the earlier run. Each process restored seven base and seven secondary snapshots, warmed the query set once, then measured seven rounds. The current binaries were built with:

```sh
RUSTFLAGS='-C target-cpu=native' cargo build --locked --release -p objekte-bench --bin json-query-mix -j 2
```

The two exact paired runner commands, source and binary hashes, and all settings are in [provenance.json](provenance.json). The existing runner was invoked with `--pairs 1 --rounds 7 --only json-schema-ascii-cache0,json-schema-ascii-cache64`. Its per-process command, exit status, stderr, setup row, and 152 rows containing every measured sample are retained under [old-master pair](raw/) and [pre-integration pair](raw-preintegration/). Every process exited successfully. The [full 152-row table](QUERY-TABLE.md) compares old master and current by filter, result count, first ID, scans, and both cache-budget timings.

## Result

All 152 reported counts and first IDs match both preserved binaries at both budgets. Scans, source reads, parses, predicate-cache hits, and ordered-result hits also match **case by case**. Counts can cap at 10,000, and this benchmark requests a one-record page; it does not establish complete-page equality.

| Comparison | Cache | Reference sum of case medians | Current sum of case medians | Difference |
| - | -: | -: | -: | -: |
| Old master → current | 0 MiB | 11,012.517 ms | 10,909.952 ms | −102.564 ms (−0.93%) |
| Old master → current | 64 MiB | 10,083.481 ms | 9,941.357 ms | −142.124 ms (−1.41%) |
| Pre-integration → current | 0 MiB | 10,824.662 ms | 10,919.895 ms | +95.233 ms (+0.88%) |
| Pre-integration → current | 64 MiB | 9,867.907 ms | 9,911.750 ms | +43.843 ms (+0.44%) |

These sums summarize per-case medians; they are **not** an end-to-end timed workload. One paired process per budget is enough to rule out a large general speedup in this workload, but small percentage differences need more repeats before attribution. The 0 and 64 MiB pairs ran at different times, so their small aggregate timing difference includes session variance.

## Why some queries feel much faster

The measurable large speedup is **reuse of verified and ordered results when the 64 MiB query cache is warm**. On the current binary, the same 152 filters require 6,019,350 source candidate scans and 6,148,732 reads at 0 MiB, versus 4,721,364 scans and 4,877,531 reads at 64 MiB. At 64 MiB, 26 cases report 65 predicate and 65 ordered-result cache hits in the case-median rows; at 0 MiB there are none. The sum of current case medians falls by 968.596 ms (8.88%) between these configurations.

| Filter | Current 0 MiB | Current 64 MiB | Scans 0 → 64 MiB | Reads 0 → 64 MiB |
| - | -: | -: | -: | -: |
| `annotation !~ :ignorecase('Mond')` | 123.797 ms | 7.011 ms | 148,478 → 0 | 148,479 → 1 |
| `created < updated` | 114.087 ms | 4.044 ms | 148,486 → 0 | 148,487 → 1 |
| `annotation !~ 'Mond'` | 115.965 ms | 7.024 ms | 148,478 → 0 | 148,479 → 1 |
| `contributors:length > 0` | 112.849 ms | 9.445 ms | 133,948 → 0 | 133,949 → 1 |
| `preferred_title !~ 'Mond'` | 102.123 ms | 4.842 ms | 138,876 → 0 | 138,877 → 1 |

The remaining read on those cache hits materializes the requested page. The cache stores generation-bound verified membership and ordered slots, so a warm compatible request can select the page without repeating all predicate verification. The benchmark also restores existing index snapshots (about 530 ms to warm all collections); it does not time a fresh index build or cold physical storage.

The recent text-definition integration does **not** explain a large acceleration of these 152 native JSON filters. Current-versus-pre-integration work counts are identical, and current timings are within about 1% of that preserved binary, slightly slower in this one pair. The older master comparison already found the pre-integration library roughly 2% faster than old master. A noticeably faster interactive session is therefore consistent with warmed indexes, result-cache reuse, and filesystem cache state; this benchmark cannot identify which of those conditions changed in the user's session.

## Limits

The benchmark uses a fixed query order, warmed repository, `Bigram` backend, exact reported count, and a page limit of one. It omits end-to-end cold startup, full ordered pages, custom text-definition queries, and TUI rendering. Per-query percentage changes at sub-millisecond scale should not be treated as stable from one pair. The raw data and [full table](QUERY-TABLE.md) are retained so suspicious cases can be repeated selectively.
