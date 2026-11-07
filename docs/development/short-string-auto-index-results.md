# Short-string AutoIndex: implementation and measured result

Measured code: `b5e00011ae70f0f5722cb5b569c2b0f3be1e4725` on `integration/short-string-auto-index`, 2026-11-07. The result files are in [`bench/results/short-string-auto-index-b5e0001/`](../../bench/results/short-string-auto-index-b5e0001/). The exact commands, output, peak-RSS samples, source fingerprints and full per-query counts are in `invocations.json`, `*.ndjson`, `*.time`, `environment.json`, `queries.csv`, `builds.csv` and `snapshots.csv`. The deterministic fixture generator is [`bench/generate_auto_index_fixture.py`](../../bench/generate_auto_index_fixture.py). The snapshot byte sizes are recorded in `snapshot_sizes.json`; generated snapshot binaries were removed after successful restore checks.

## Result and limits

AutoIndex now admits decoded string values up to and including 64 UTF-8 bytes without the old small-dictionary gate. It retains partial coverage and scans unresolved records, including values over 64 bytes. Its B-tree keys use the existing scalar comparison order. Explicit schema postings take precedence; AutoIndex can still index other paths. Inference remains disabled by default. The implementation keeps the existing source reader, query and snapshot APIs; `obj --auto-index`, `--schema FILE` and `--tui` configure the same repository and query engine. The TUI uses the existing screen components with query-derived columns, bounded visible pages and no link navigation in the single-folder mode.

The final policy defaults are 4 MiB per inference batch, 64 MiB total inferred state, 128 MiB repository work budget and 24 MiB per short-string path; policy version 3 is in snapshot compatibility. The initial 256 KiB batch allowance admitted zero paths on the medium and Musenalm fixtures. Those raw runs are in `short-string-auto-index-567f39b/`. The corrected policy admits 8 paths on medium (6 short-string, 1 partial) and 58 on Musenalm (48 short-string, 37 partial). The old run therefore is a useful budget failure observation, not a performance baseline for an admitted short-string index. A separate intermediate run found combined manual/automatic snapshot restore rejection; its raw evidence is in `short-string-auto-index-7c9ac08/`. Commit `b5e0001` fixed the report/coverage invariant and the final restore run succeeded.

## Procedure and correctness

Release `obj` was compiled before timing with rustc 1.99.1, no `RUSTFLAGS`, ASCII bigrams, automatic build-worker selection, a fixed source tree per dataset, and verified-result cache disabled by the repository default. The 512-record medium fixture has 63-, 64- and 65-byte strings, a mostly-short array with 68-byte outliers, high-cardinality UUIDs and long free text. Musenalm uses the 4,928 JSON files under `examples/musenalm/data/entries` (content hash in `environment.json`). The 100,000-record UUID fixture has distinct 36-byte strings. The 100-record path fixture exposes 10,000 unique property paths. Each four-mode run was interleaved for three repetitions. `search_ns` excludes NDJSON output work. These are local warm-filesystem-cache observations, not cold-disk measurements; one-off UUID and snapshot timings have no variance estimate.

The 9 medium queries and 6 Musenalm queries gave identical exact counts and ordered page IDs in all 12 fresh runs per dataset and all 8 build/restore runs per dataset. Snapshot restore reported zero build parses for all four modes. The normal fresh build parsed each source once: 512, 4,928 or 100,000 records, with no second source parse pass. `cargo test --workspace` passed, including core inference/coverage, query, snapshot, schema adapter, CLI process and TUI tests. The combined explicit-schema plus AutoIndex restore regression test now passes. `cargo clippy --workspace --all-targets` completed with existing warnings. `cargo fmt --all -- --check` still reports extensive pre-existing formatting differences (570 diff hunks across unrelated files); edited Rust files were formatted individually.

The many-unique-path fixture did not publish partial indexes: `complete=false`, `omitted_paths=1`, `admitted_paths=0`. Its known-field equality still returned the same one record as Disabled, scanning 100 records in both modes. This demonstrates visible fallback when the observation budget is exhausted.

## Fresh builds and query batches

Median of three independent process runs. Build is the reported repository construction interval; RSS is process peak from `/usr/bin/time`. Search is the sum of the fixed batch's core `search_ns` values, not terminal printing. A single process reused its built repository for its batch.

| Dataset | Mode | Build ms | Peak RSS MiB | Batch search ms | Admitted paths |
| --- | --- | ---: | ---: | ---: | ---: |
| Medium 512 | Disabled | 11.13 | 7.97 | 14.47 | — |
| Medium 512 | Schema | 10.92 | 7.92 | 14.77 | — |
| Medium 512 | AutoIndex | 15.81 | 8.41 | 15.74 | 8 |
| Medium 512 | Both | 12.68 | 8.88 | 12.69 | 5 |
| Musenalm 4,928 | Disabled | 53.64 | 32.33 | 194.80 | — |
| Musenalm 4,928 | Schema | 52.27 | 33.94 | 112.76 | — |
| Musenalm 4,928 | AutoIndex | 98.14 | 61.07 | 137.07 | 58 |
| Musenalm 4,928 | Both | 104.09 | 61.93 | 114.19 | 55 |

Musenalm AutoIndex adds 44.50 ms build time (+83%) and 28.74 MiB peak RSS relative to Disabled, while this query batch saves 57.73 ms of core search. Both adds 51.82 ms build time relative to Schema, with roughly equal batch search (1.43 ms slower). Medium AutoIndex adds 4.68 ms build time and 0.44 MiB peak RSS relative to Disabled; the small sample varies enough that its query-time difference is inconclusive. Musenalm Disabled construction is 1.18 ms faster than the previous policy run (54.82 ms); there is no repeatable large Disabled regression on that corpus. Medium Disabled changed from 8.59 to 11.13 ms across revisions, an absolute 2.54 ms on a small build with visible run variation; no causality or repeatable regression is established. These are cumulative optional-index costs, not an allocation count.

Representative median core query time and median source parses (milliseconds/parses):

| Query | Disabled | Schema | AutoIndex | Both |
| --- | ---: | ---: | ---: | ---: |
| Medium `code = UUID` | 0.16 / 13 | 0.08 / 0 | 0.11 / 1 | 0.10 / 0 |
| Medium bounded `code` range, 10 matches | 1.39 / 512 | 0.27 / 0 | 0.71 / 110 | 0.19 / 0 |
| Medium `year` range, 10 matches | 1.14 / 512 | 0.21 / 0 | 0.51 / 110 | 0.13 / 0 |
| Medium `mixed ?= 'm3'`, 64 matches | 0.18 / 64 | 0.22 / 64 | 0.19 / 64 | 0.20 / 64 |
| Musenalm `year >= 1800 && year < 1810` | 20.69 / 4,928 | 0.93 / 0 | 9.88 / 1,848 | 1.35 / 0 |
| Musenalm `pseudonym = true` | 19.87 / 4,928 | 0.29 / 0 | 1.34 / 226 | 0.34 / 0 |
| Musenalm `preferred_title ~ 'Goethe'` | 21.17 / 4,928 | 1.33 / 203 | 20.80 / 4,928 | 2.28 / 203 |

The 110/1,848 candidate range counts reflect an index restriction plus residual verification, not a fully proved answer. `preferred_title ~ 'Goethe'` stays a scan in AutoIndex on this corpus; its schema bigram path is more useful. Broad sort queries still pay for sorting and materialization. The full expressions, counts, source opens, candidates, parses and returned page sizes for every repetition are in `queries.csv`.

## High-cardinality UUIDs and snapshots

The 100,000 distinct UUIDs exceeded the old 64-value dictionary gate. AutoIndex admitted one field and estimated 18,216,684 retained bytes. The one fresh build took 271.41 ms versus 231.42 ms Disabled (+39.99 ms); peak RSS was 159.11 versus 99.72 MiB (+59.39 MiB). This memory cost includes temporary build state and cannot be interpreted as retained index size. AutoIndex's snapshot was 27,766,319 bytes, took 86.92 ms to write, and restored in 120.30 ms with zero build parses. The selective final-range query returned 10 records in both modes. AutoIndex used 10 candidates and 10 source parses in 2.94 ms core search; Disabled used 100,000 candidates/parses in 200.51 ms. The absolute search difference was 197.57 ms for this one query run. Output size is 10, and range work remains proportional to the visited keys/postings and results. A selective equality on the same UUID corpus used one candidate/parse in both modes because the shared bigram index was already selective; the medium UUID equality was 13 parses Disabled versus 1 AutoIndex.

Snapshot sizes for medium were 300,320 bytes Disabled, 358,337 Schema, 482,734 AutoIndex and 481,522 Both. Musenalm sizes were 6,963,092, 7,024,767, 10,826,640 and 10,818,248 bytes respectively. Musenalm AutoIndex restored in 61.26 ms versus 13.45 ms Disabled; medium AutoIndex restored in 3.83 ms versus 1.15 ms Disabled. These are single runs. Full write/restore, stage, count and RSS values are in `snapshots.csv`; full source snapshot files can be regenerated from `invocations.json` and the committed fixture generator.

## Terminal check and remaining limits

A pseudoterminal smoke test of release `obj --tui --auto-index` on medium observed the building label first, indexes-ready output at 102–103 ms and first visible matches at 132–133 ms across three launches. A quiet-screen arrow-key redraw produced its first terminal bytes in 0.18–4.08 ms; this is terminal-output latency, not a CPU rendering benchmark. The UI reports separate search and display/projection durations, and tests assert redraw of an unchanged viewport does not reparse source records. Cancellation during a 100,000-record build exited in about 1 ms after Ctrl+Q and restored the terminal. Raw observations are in `tui-redraw.json` and `tui-probe.json`.

The source-level inference timer is folded into extraction/index work; there is no standalone inference allocation counter. The high-cardinality memory and snapshot cost is material. This run does not prove a 16-byte versus 64-byte policy speed ratio because the available earlier 16-byte/batch-budget state admitted no comparable index. The single Musenalm/UUID snapshot samples and short TUI probe are diagnostic, not stable distribution estimates.
