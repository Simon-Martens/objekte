# Incremental index availability: measured results

Task: `incremental-index-availability` (Plan 1). Final acceptance report, 2026-11-13.

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

## Final incremental construction comparison

Seven fresh-process paired warm trials alternated master and candidate order on the same absolute Musenalm root. Both used the same schema and corpus fingerprints, ASCII alphabet, 32 build workers, configured fields, inference disabled, Rust compiler and cache conditions. The measured candidate builder is `526ae413db2acaf87755c3f19b27ab6ede16d9b6`; later commits altered completed-query execution, reports and formatting, not construction. The [campaign](../../../bench/results/incremental-index-availability/2026-11-13-final-paired-warm/README.md) retains every command, manifest, sample and quartile. An earlier paired campaign is superseded by this final one.

| Measure | Master median (IQR) | Candidate median (IQR) | Change / gate |
| --- | ---: | ---: | ---: |
| Full build | 973.344 ms (35.867 ms) | 910.233 ms (34.882 ms) | 6.48% faster; passes ≤5% slower |
| Fresh-process high-water RSS | 536.842 MB (5.325 MB) | 568.316 MB (9.161 MB) | 5.86% higher; passes ≤10% |
| RSS immediately after build | 519.459 MB (5.032 MB) | 558.719 MB (9.216 MB) | 7.56% higher |
| Estimated completed live index | 69.663 MB (0) | 69.966 MB (0) | 0.44% higher; passes ≤10% |
| Construction parses | 148,486 (0) | 148,486 (0) | Equal; no second pass |

The candidate made exactly 148,486 construction source reads and 193,297,230 source bytes in every trial: one read and parse per physical file. It published eight complete immutable segments. Median catalogue availability was 208.079 ms, first searchable batch 286.082 ms, and first nonempty result 286.264 ms. Master discovery median was 190.152 ms and offered no early search. Snapshot size increased from 129,139,458 to 178,287,528 bytes (+38.1%). This persistence cost has no specified acceptance limit and remains a material limitation.

## Complete query comparison

The final candidate at `ad542f6e9ad3ede9fca0e2235df61476ec721749` ran all 194 extended Musenalm cases in four profiles (ASCII/Unicode × cache off/warm) with five timed repetitions. The matched master is `7a6b96684e1fbb849eab7cc20185b942159ed189`. Both used the same absolute corpus root and matching schema/corpus fingerprints, compiler, worker settings and profile conditions. [Candidate raw measurements, plans, digests, manifests and strict comparison](../../../bench/results/incremental-index-availability/2026-11-13-final-query/COMPARISON.md) are retained. The comparator checked the common static collection registration fields, all 776 status/count rows and source-open counts. All passed, with zero candidate source-open increases. Candidate direct and lifecycle normalized result digests agree on all 194 cases. Cross-version hitful digest files cannot be compared bytewise because the older master runner hashes a volatile generation value and the candidate normalizes it to zero; all 13 zero-hit digests agree.

Cache-off sums of per-case medians fell from 1,998.535 to 983.339 ms (ASCII) and 1,985.371 to 980.562 ms (Unicode). Warm sums fell from 1,322.716 to 2.958 ms (ASCII) and 1,316.706 to 2.799 ms (Unicode), largely because complete lifecycle views can use the published index. These sums do not mask individual cases. Two full-suite rows exceeded both the 10% and 0.5 ms investigation thresholds: Q003 ASCII (5.336→5.948 ms) and Q004 Unicode (10.692→14.787 ms). [Three alternating master/candidate pairs with seven repetitions per run](../../../bench/results/incremental-index-availability/2026-11-13-final-focused-pairs/campaign.json) on the same corpus did not reproduce either regression: Q003 ASCII medians 5.202→3.914 ms, Q003 Unicode 5.212→5.546 ms (+0.334 ms, 6.4%); Q004 ASCII 10.414→8.830 ms, Q004 Unicode 10.553→8.411 ms. Both acceptance thresholds pass on the repeated matched comparison.

## Foreground queries and held views

A separate [nine-process instrumented campaign](../../../bench/results/incremental-index-availability/2026-11-13-concurrency-instrumented/README.md) alternated idle, fixed foreground and held-early-view builds at `28a008b2ad`. Later changes only affected completed-query execution. Idle median full build was 895.354 ms; foreground median 936.034 ms (+4.54%) with 885–906 completed Q108/Q116/Q120 queries per run; held-view median 923.650 ms. Foreground per-process median latency was 24.4–29.1 µs and p95 60.7–70.6 µs, with zero query source opens. First nonempty results appeared 281–315 ms after the foreground worker began. Held-view high-water RSS median was 567.898 MB versus idle 572.215 MB; no accumulating full-view copies were observed. Every run made exactly 148,486 construction reads/parses. The 32-worker scheduler reached 32 active jobs. Idle median summed worker execution was 4.471 s across concurrent workers; scheduler receive wait 74.335 ms, admission 361.932 ms, eight publication intervals 235.196 ms, and finalization 12.301 ms. These components overlap in wall time.

## Functional and repository checks

[Native desktop and PTY TUI evidence](../../../bench/results/incremental-index-availability/2026-11-13-client-smoke/README.md) shows a 148,486-file catalogue and provisional desktop results before completion, then an exact 275-match query with record preview. The TUI's 6,200-file one-CPU fixture showed the catalogue before the first batch, provisional matches, query replacement, exact final results, resize, Ctrl+Q exit 0 and Ctrl+C exit 130. The final combined code also launched in native headless Wayland and opened all 148,486 records; [screenshot](../../../bench/results/incremental-index-availability/2026-11-13-client-smoke/final-code-native.png). The deliberately slowed TUI fixture timings are not performance claims.

At `ad542f6e9a`, `cargo test --workspace --all-targets --locked` passed 905 tests, 30 ignored, zero failed. `cargo clippy --workspace --all-targets --locked` passed with no warnings. `git diff --check` passed. Workspace-wide `cargo fmt --all --check` and `python3 scripts/format-rust-spacing.py --check` still fail on broad preexisting style drift (master also fails). In the rustfmt comparison, master had 113 failing files, the candidate 99, and zero candidate-only failing files after scoped formatting. The combined workspace `cargo check --workspace --all-targets --locked` passed earlier during integration; the final full test/clippy builds cover the later changes.

## Remaining limits

The 38.1% larger completed snapshot is the measured storage tradeoff. Provisional results are explicitly labelled and can reorder until complete; complete pagination/export waits for final coverage. The changed benchmark digest normalization prevents bytewise cross-version comparison of hitful digest files, though matched status/count/source counters and candidate direct/lifecycle digests agree. No in-scope correctness failure or repeated performance regression remains open. This branch is not merged into master.
