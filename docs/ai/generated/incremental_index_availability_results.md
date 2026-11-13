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

## Implementation comparison

Pending implementation and seven alternating paired warm trials against this fixed master revision. Do not infer a passing performance gate from the smoke run or the extended runner's sequential-process memory samples.
