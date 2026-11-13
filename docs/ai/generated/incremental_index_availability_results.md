# Incremental index availability: measured results

Task: `incremental-index-availability` (Plan 1). This report is in progress until the implementation, validation, and paired comparison are complete.

## Master baseline

- Revision: `7a6b96684e1fbb849eab7cc20185b942159ed189` (`master` and `origin/master` after fetch on 2026-11-13).
- Host: Intel Core i9-13900K, 32 logical CPUs available, Rust 1.99.1, Linux x86-64. Musenalm fixture: seven collections, 148,486 physical files and records. Normal build worker setting: 32; ASCII alphabet; configured schemas and default inference policy. Fresh-process smoke used a warm filesystem cache and `--profile --snapshot --check-queries`.
- Command: `target/release/examples/bench_index_construction --root examples/musenalm --result-dir /tmp/objekte-plan1-master-baseline --alphabet ascii --workers 32 --profile --snapshot --check-queries`. This was one smoke run, not the required seven paired warm trials.
- Build: 1,002.569 ms; discovery: 191.940 ms; construction parses: 148,486; failed parses: 0; process high-water RSS: 530,898,944 bytes; RSS after build: 514,269,184 bytes; estimated live index: 69,662,968 bytes; snapshot: 129,139,458 bytes. The high-water RSS is from `/proc/self/status` in a fresh process, not a post-build sample.
- The existing direct lifecycle `Index::build` path is *not* the task baseline: a Q001-only `bench_lifecycle_musenalm` check built ASCII in 1,180.118 ms with 838,804 KiB high-water RSS. This warns against simply publishing many existing lifecycle segments without controlling retained metadata and per-segment overhead.

The [construction sample](../../../bench/results/incremental-index-availability/2026-11-13-master-7a6b966/construction-smoke/sample.json) and its [run manifest](../../../bench/results/incremental-index-availability/2026-11-13-master-7a6b966/construction-smoke/run-manifest.json) preserve the measurements and source/schema identity.

A separate Unicode fresh-process smoke used the same command with `--alphabet unicode`: build 1,214.222 ms, discovery 335.355 ms, 148,486 parses, high-water RSS 552,534,016 bytes, after-build RSS 535,965,696 bytes, estimated live index 72,571,368 bytes, snapshot 131,650,034 bytes. Its [sample](../../../bench/results/incremental-index-availability/2026-11-13-master-7a6b966/construction-smoke-unicode/sample.json) and [manifest](../../../bench/results/incremental-index-availability/2026-11-13-master-7a6b966/construction-smoke-unicode/run-manifest.json) are retained. The larger discovery time illustrates why a single smoke run cannot establish a build-time regression or improvement.

## Master query baseline

Command: `target/release/examples/bench_musenalm --root examples/musenalm --extended --profiles ascii-off,ascii-warm,unicode-off,unicode-warm --repetitions 5 --result-dir /tmp/objekte-plan1-master-query-extended`. All 194 resolved cases passed in all four profiles, with five timed repetitions per profile. The retained [report](../../../bench/results/incremental-index-availability/2026-11-13-master-7a6b966/extended/REPORT.md), [manifest](../../../bench/results/incremental-index-availability/2026-11-13-master-7a6b966/extended/run-manifest.json), [per-query CSV](../../../bench/results/incremental-index-availability/2026-11-13-master-7a6b966/extended/query-results.csv), measurements, plans and result digests permit matched comparisons. The standard 150-case set also passed, but the extended 194-case set is the full comparison baseline.

The extended runner builds ASCII and Unicode sequentially in one process; its Unicode high-water RSS includes memory retained from the earlier ASCII phase. Fresh-process construction measurements are required for the peak-RSS acceptance gate.

## Incremental construction comparison

The measured implementation binary was built from `bad06b4007476c7f861fe9f1090e9d7be744218e`. Seven paired warm trials alternated run order and used fresh processes, the same absolute Musenalm root, ASCII alphabet, 32 workers, configured schemas, inference disabled, profiling, snapshot writing and five query probes. All fourteen manifests have the same corpus and schema fingerprints, 148,486 files/records, and 193,297,230 source bytes. [Commands, samples, manifests, quartiles and the runner](../../../bench/results/incremental-index-availability/2026-11-13-paired-warm/README.md) are retained. The run used no foreground query workload; that experiment is separate.

| Measure | Master median (IQR) | Incremental median (IQR) | Change |
| --- | ---: | ---: | ---: |
| Full build | 967.277 ms (38.717 ms) | 896.591 ms (46.393 ms) | 7.31% faster |
| Fresh-process build high-water RSS | 535.683 MB (9.255 MB) | 573.788 MB (3.826 MB) | 7.11% higher |
| RSS immediately after build | 519.188 MB (9.562 MB) | 564.118 MB (2.988 MB) | 8.65% higher |
| Estimated live index families | 69.663 MB (0) | 69.966 MB (0) | 0.43% higher |
| Construction parses | 148,486 (0) | 148,486 (0) | Equal |

The candidate recorded exactly 148,486 construction source reads and 193,297,230 bytes read in every trial: one read and parse per file. It published eight completed segments, with median first searchable batch at 334.747 ms and first result at 334.904 ms. Its median observed catalogue time was 254.769 ms. The comparable master discovery median was 189.377 ms; master has no early publication. These data pass the stated no-query full-build, peak-RSS and live-index budgets. A snapshot was 178,287,528 bytes versus 129,139,458 bytes on master (38.1% larger), a material persistence cost even though no snapshot-size limit was specified.

The first trial campaign used the original checkout as the process working directory for both binaries, causing the candidate manifest to report the master revision. Its measurements were discarded for the formal comparison. The retained campaign runs each binary from its own worktree and names the same absolute corpus root.

## Interactive client checks

The [desktop and TUI evidence](../../../bench/results/incremental-index-availability/2026-11-13-client-smoke/README.md) captures real interaction. The desktop showed the complete physical-file catalogue and provisional query results while indexing Musenalm, then an exact 275-match result with record preview. A 6,200-file PTY TUI check showed catalogue availability before the first batch, provisional results, query replacement during construction, final exact results, resize and cancellation. The one-CPU TUI fixture deliberately lengthened observable intermediate states; its timings are not performance claims.

## Query comparison

The full 194-case lifecycle query suite and matched latency/source-read analysis are in progress. Results must retain existing acceleration as well as result equality; preliminary matched counters found relation and metadata cases that require repair before Plan 1 completion.
