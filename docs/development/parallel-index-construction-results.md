# Parallel index construction results

Measured implementation: `d15b689d1ac21709195b79fc382c045d8dec9867` on `fix/parallel-index-construction`. The final report and raw data were committed after this revision. The starting development revision was `156fe5bc440791dd361d82e7f0f7935c50820f46`, after the shared case-folded bigram work had been merged. The direct serial comparison is the committed bulk-bigram builder at `4f2cb8e85ba6b1a6fe6f5b7aecc8f0c7fff1aee8`; this isolates the later batch and worker changes. The earlier case-fold serial build took 11,958 ms ASCII and 12,165 ms Unicode without snapshots, but includes both file-membership and bigram-constructor costs that the direct serial comparison has already removed.

## Protocol and identity

All Musenalm runs parsed 148,486 records in seven collections with zero failed parses. Three fresh processes per configuration used `cargo build --release` outside the timed sample, the default allocator, Rust 1.99.1, a 13th Gen Intel Core i9-13900K, `performance` governor, CPUs 0–31 affinity, 32 query workers, and warm filesystem caches. No sample restored a snapshot. The schema digest was `38f2dc5112146aa6ac0514291c76eaeb741421fa8a4624e50fa83c4c0a449bf9`; the source-metadata fingerprint was `eec50dbfbfd023aad31e9c3609f4a7f14b177ecc1a022c182321ae704c188cf6`. The saved runner verified both plus registration identity, compiler, affinity, governor and build flags against the serial predecessor. Each raw sample records its command, CPU times, policy, phase times, memory, record counts and semantic index signature.

Timing includes discovery, reading/framing, parsing, index construction and publication, and optional snapshot writing. It excludes compilation and the semantic signature computed afterward. Filesystem caches were warm; these are not cold-storage timings. The runner's compatibility and resource gate passed in all configurations.

Raw samples and saved comparison tables are in [the final build artifact](../../bench/results/musenalm/parallel-index-construction-d15b689d1a/), including [printed tables](../../bench/results/musenalm/parallel-index-construction-d15b689d1a/final-tables.md), [comparison.csv](../../bench/results/musenalm/parallel-index-construction-d15b689d1a/comparison.csv), [comparison.json](../../bench/results/musenalm/parallel-index-construction-d15b689d1a/comparison.json), individual `gate.json`/`measurements.jsonl` files under `one`, `auto`, and `w2`–`w16`, and the alternating [paired samples](../../bench/results/musenalm/parallel-index-construction-d15b689d1a/paired-musenalm-samples.jsonl). The immutable schema manifests are saved beside each run.

## Main comparison

Medians are milliseconds. “Baseline” is `4f2cb8e85b`; the one-worker and automatic runs are the measured implementation. The final two columns show the automatic build compared with that baseline.

| Configuration | Baseline ms | New one-worker ms | New automatic ms | Auto change ms | Auto change % | Auto peak RSS MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| ASCII, no snapshot | 5,438.4 | 5,944.7 | 1,689.7 | −3,748.7 | −68.9% | 303.0 |
| Unicode, no snapshot | 6,232.8 | 6,775.5 | 1,792.7 | −4,440.1 | −71.2% | 304.3 |
| ASCII, snapshot rebuild | 5,700.3 | 6,280.3 | 1,977.0 | −3,723.3 | −65.3% | 386.8 |
| Unicode, snapshot rebuild | 6,525.2 | 7,083.5 | 2,178.0 | −4,347.3 | −66.6% | 396.8 |

The new one-worker path is slower than the direct serial predecessor by 506.3 ms ASCII (9.3%) and 542.7 ms Unicode (8.7%) without snapshots. This cost sits mainly in extraction/index time (ASCII 5,497 ms, Unicode 6,337 ms in the new one-worker path) and includes batch contribution and merge machinery. We have not isolated its components further. The 32-worker automatic path saves 4,255.0 ms ASCII and 4,982.8 ms Unicode relative to the new one-worker path. An alternating one/automatic process run reproduced the direction: 6,003.7 versus 1,630.9 ms ASCII and 6,776.7 versus 1,794.7 ms Unicode (three of each, alternating order).

## Worker scaling and resources

All rows used disabled snapshots and three fresh processes. CPU seconds are process user plus system time. “Same” means a matching semantic index signature for that alphabet; snapshot rebuilds match the corresponding disabled build too.

| Alphabet | Workers | Median ms | Min–max ms | CPU s | Peak RSS MiB | Same index |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| ASCII | 1 | 5,944.7 | 5,930.8–5,953.6 | 7.11 | 276.0 | yes |
| ASCII | 2 | 3,468.3 | 3,450.1–3,510.1 | 6.58 | 276.6 | yes |
| ASCII | 4 | 1,869.8 | 1,864.6–1,908.7 | 7.08 | 282.9 | yes |
| ASCII | 8 | 1,644.9 | 1,598.5–1,657.3 | 6.78 | 291.8 | yes |
| ASCII | 16 | 1,650.9 | 1,594.0–1,668.6 | 6.84 | 298.8 | yes |
| ASCII | 32 (auto) | 1,689.7 | 1,643.3–1,721.7 | 6.90 | 303.0 | yes |
| Unicode | 1 | 6,775.5 | 6,744.0–6,780.5 | 7.99 | 281.3 | yes |
| Unicode | 2 | 3,894.6 | 3,891.9–3,903.3 | 7.43 | 282.7 | yes |
| Unicode | 4 | 2,084.2 | 2,066.5–2,090.8 | 7.92 | 288.6 | yes |
| Unicode | 8 | 1,800.3 | 1,742.2–1,848.4 | 7.67 | 298.5 | yes |
| Unicode | 16 | 1,804.9 | 1,762.9–1,806.7 | 7.66 | 302.3 | yes |
| Unicode | 32 (auto) | 1,792.7 | 1,759.0–1,805.6 | 7.70 | 304.3 | yes |

Speed levels off around eight workers. In automatic mode the measured average busy CPUs were 4.13 ASCII and 4.32 Unicode, despite 32 available workers. Discovery remains about 307–309 ms and finalization about 119–120 ms, both outside the parallel extraction/index phase. The latter phase fell from 5,497 to 1,250 ms ASCII and 6,337 to 1,336 ms Unicode when switching from one to automatic workers. The phase timings are wall intervals; CPU time is summed across workers. Further scaling is limited at least by the remaining serial intervals and bounded batch publication. The data do not isolate storage bandwidth, allocator contention or individual merge functions sufficiently to claim one sole cause for the plateau.

The bounded admission window peaked at 51.41 MiB of estimated source/batch bytes in automatic mode, against 16.45 MiB with one worker. The automatic no-snapshot peak RSS was 303.0 MiB ASCII and 304.3 MiB Unicode, up 65.4 and 61.2 MiB against the direct serial predecessor. Snapshot rebuild peaks were 386.8 and 396.8 MiB, up 46.1 and 48.9 MiB. The snapshot files were 106.06 and 108.46 MiB. Inventory and bigram heap estimates were 57.14 MiB and 50.96/53.73 MiB. There were 15 bigram stores and 25,477/37,909 distinct ASCII/Unicode gram keys in every worker mode. The accepted index representation did not grow with worker count; transient build memory did.

An earlier automatic scheduler at `c27e68b315` peaked at 334.3/338.7 MiB without snapshots and failed the memory gate ([raw build results](../../bench/results/musenalm/parallel-workers-auto-c27e68b315/)). Compact worker metadata and a smaller admission window at `954b812c33` brought that checkpoint to 287.4/287.8 MiB ([raw build results](../../bench/results/musenalm/parallel-workers-auto-954b812c33/)). The final cross-collection scheduler uses a 52 MiB estimated admission cap and reaches the 303.0/304.3 MiB figures above. This cap bounds admitted batch weights; a single oversized file is handled as a barrier and can exceed the ordinary window while its framed record ranges run concurrently.

## Source layouts and semantic checks

The generated large fixture has one 12 MB JSONL file with 100,000 records; the mixed fixture has one large file plus 512 small files, 90,240 records and 10.8 MB of source bytes. Fixture manifests retain the content hashes. Three alternating one/automatic runs per alphabet verified identical schema, gram count, semantic signature, full parse count and zero failed parses. Raw samples are in [layout-samples.jsonl](../../bench/results/musenalm/parallel-index-construction-d15b689d1a/layout-samples.jsonl) and the matching [CSV](../../bench/results/musenalm/parallel-index-construction-d15b689d1a/layout-results.csv).

| Fixture / alphabet | Workers | Median ms | Min–max ms | CPU s | Peak RSS MiB | Equivalent index |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| One large file / ASCII | 1 | 494.0 | 491.3–535.4 | 0.71 | 77.9 | yes |
| One large file / ASCII | 32 | 265.3 | 255.1–271.5 | 0.89 | 101.9 | yes |
| One large file / Unicode | 1 | 576.7 | 575.9–609.0 | 0.80 | 77.6 | yes |
| One large file / Unicode | 32 | 272.4 | 269.2–274.0 | 1.05 | 96.5 | yes |
| Mixed / ASCII | 1 | 457.7 | 445.7–473.3 | 0.64 | 65.4 | yes |
| Mixed / ASCII | 32 | 266.3 | 263.7–270.0 | 0.81 | 88.2 | yes |
| Mixed / Unicode | 1 | 542.3 | 523.3–543.1 | 0.72 | 65.3 | yes |
| Mixed / Unicode | 32 | 264.9 | 262.5–268.2 | 0.87 | 84.0 | yes |

The single-file improvement confirms parallel work within one physical file; the mixed fixture confirms it also coexists with smaller file jobs. A reader that cannot expose independent record ranges still uses its file as the serial unit; other files can run concurrently. File-to-record mappings, record ordering, diagnostics and snapshot semantics remain in the final publication path.

The deterministic semantic signatures are `521402437efd766e5e964cfe5f70443de57d6ad8b5e6e1e61425b9b6dfaa0688` for ASCII and `5182e515803bc9334e3e3bf4401ac0dee5bc43e072010324298f661a4389411f` for Unicode. They cover inventory, generations, coverage, typed and custom postings, both gram stores and sort columns. They match across worker counts and disabled/rebuild modes. They are semantic comparisons, not byte-for-byte snapshot hashes.

## Attribution and verification

The separate [file-membership attribution](../../bench/results/musenalm/parallel-membership-3835931a7d/REPORT.md) changed a 16,000-file construction case from 48.730 to 1.429 ms (−47.301 ms) by removing growing-vector membership searches. The separate [bulk-bigram attribution](../../bench/results/musenalm/parallel-bigram-attribution-4f2cb8e85b/REPORT.md) changed matched serial Musenalm builds from 8,242 to 5,456 ms ASCII and 8,313 to 6,208 ms Unicode; controlled 10,000-record constructors cut whole-process allocation counts from roughly 135,000/144,000 to roughly 71,000. These are separate stages, so the total improvement must not be credited to worker scheduling alone.

Focused tests cover one and many workers, reverse and delayed completion, unaligned batch slots, same-file ranges, cross-collection overlap, malformed and invalid UTF-8 siblings, custom readers, deterministic diagnostics, error shutdown, semantic postings/grams/sorts and restore parity. The final workspace run passed 109 core tests, 25 TUI tests and two other tests (one core allocation-profile test remained ignored). Four benchmark-tooling tests passed; Clippy exited successfully with warnings. Both alphabets passed all 150 workload acceptance cases with restored snapshots. The independent jq oracle passed 300/300 complete result ID sets and does not use the core parser, binder, scanner, index or cache. Logs are in the final artifact. `cargo fmt --all --check` still fails on widespread style drift across core and TUI files; the touched build files pass focused `rustfmt --check`. The full check output is saved as `fmt-workspace.log.gz`.

The final implementation was published as focused commits: `34262d34d3` (evidence truth), `23074150d4` (benchmark schema identity), `3835931a7d` (file membership), `4f2cb8e85b` (bulk bigrams), `a47d78acfb` (batch contributions), `c27e68b315` (bounded workers), `954b812c33` (memory bound), `36b48ded2c` (large files), `d3acb306f3` (integration tests), `e042b4c183` (record-ID transfer), `7f785ec746` (semantic signatures), `08aa8332eb` (cross-collection scheduling), and `d15b689d1a` (formatting). Baseline and intermediate attribution documentation were committed separately. Each implementation commit was pushed to `fix/parallel-index-construction`.

The remaining measured costs are the one-worker batch overhead, serial discovery/finalization, and higher transient memory under concurrency. Neither a new index representation nor query behavior was introduced by this build work.
