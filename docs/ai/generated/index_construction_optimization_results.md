# Index construction optimization results

Task: `index-construction-optimization` · 12 November 2026 · baseline B `737cfb7745ff15915804553ab583af260ac0f2e4` · measured combined code `609474ee42e26c568cd43f89893482e0a0886bbe` on `integration/index-construction-optimization`.

Historical status: the [selected merge results](index_construction_selected_merge_results.md)
supersede this report's decision to retain A1. The original A1-inclusive
measurements and reasoning below remain as experiment evidence.

## Result and decisions

The selected combination builds the configured 148,486-record Musenalm corpus in a median **2,107.469 ms ASCII** and **2,255.591 ms Unicode** at 16 workers. Five alternating, warm-cache pairs per alphabet give paired median changes of **−422.229 ms (−16.691%) ASCII** and **−417.635 ms (−15.655%) Unicode** against B. All ten final pairs improved. The same corpus and schema fingerprints, 148,486 parses, zero failed parses, 300 posting fields, eight sort fields, 228,503 relation edges, gram-key counts, and estimated index sizes were observed. These are combined measurements; individual savings are not added together.

| Variant and control | ASCII median build, ms | ASCII paired change | Unicode median build, ms | Unicode paired change | Decision |
| --- | ---: | ---: | ---: | ---: | --- |
| B control in final pairs | 2,529.698 | — | 2,667.664 | — | Baseline |
| M 64-path metadata vs B | 2,194.548 | −341.758 ms (−13.475%) | 2,355.602 | −331.932 ms (−12.351%) | Keep |
| E1 file buffer vs B | 2,529.424 | +1.513 ms (+0.060%) | 2,664.400 | −15.133 ms (−0.571%) | Keep for bounded buffer/read-call and memory benefit; speed neutral |
| E2 bigram scratch vs B | 2,569.551 | +46.931 ms (+1.861%) | 2,725.038 | +33.816 ms (+1.254%) | Reject |
| E3 inline field selection vs B | 2,515.118 | +2.742 ms (+0.110%) | 2,674.313 | +7.264 ms (+0.272%) | Reject; full-corpus benefit not established |
| E4 direct logical JSON vs B | 2,485.963 | −67.785 ms (−2.657%) | 2,634.970 | −55.818 ms (−2.097%) | Keep |
| A0 admission tracing vs B, disabled | 2,521.988 | +10.881 ms (+0.433%) | 2,672.990 | −5.682 ms (−0.212%) | Keep opt-in diagnostics; disabled overhead inconclusive |
| A1 dispatch refill vs A0 | 2,514.377 | −22.095 ms (−0.873%) | 2,658.082 | −14.081 ms (−0.526%) | Keep, with bounded memory tradeoff |
| **Combined vs B** | **2,107.469** | **−422.229 ms (−16.691%)** | **2,255.591** | **−417.635 ms (−15.655%)** | **Ready for user merge** |

Every row uses its own paired B or A0 control and the candidate median from that pair set. The displayed candidate and control medians need not subtract to the paired median. ASCII measurements used five pairs and Unicode at least three; the combined row used five of each. Full raw durations, pair order, IQR, binary hashes, manifests, and profile samples are in [the evidence archive](../../../bench/results/index-construction-optimization/2026-11-12-master-737cfb77/README.md). The main metric is `build_ns`, not whole-process time.

Final median peak high-water RSS was **543.6 → 542.9 MiB ASCII** and **559.2 → 568.6 MiB Unicode** (B → combined). The Unicode increase is **9.4 MiB**, about 1.7%, while the unchanged `estimated_index_bytes` shows no retained index-content growth. Metadata parallelism and earlier dispatch can overlap more resident work. Post-build RSS increased from 476.4 to 492.8 MiB ASCII and from 539.8 to 548.9 MiB Unicode; allocator-retained pages are not necessarily live index data. No task byte/job allowance or source-batch-size limit was raised.

## Measurement method and provenance

The host was an AMD Ryzen 7 5700G (16 logical CPUs), Fedora kernel 7.2.13, Rust 1.99.1, `powersave` governor. Runs used the unchanged `examples/musenalm` configured corpus (193,297,230 source bytes), corpus fingerprint `db869a76f26e1a3275a5162e5a207f47fb50fa8be02c4bdcfe3f1db50cfb747c`, schema fingerprint `4115070eb30b0af98b109429d0883b493f857cdf671ec55b943d91b73c5ebfe3`, 16 resolved workers, no inference, no snapshot, and a fresh process per trial. Each binary was built with `cargo build --release --locked -p objekte-search-core --example bench_index_construction` in an isolated `CARGO_TARGET_DIR`; control/candidate SHA-256 hashes differ and are recorded in `config.json`. Filesystem caches were warmed by one run per role. No compilation, tests, or other benchmarks overlapped timed pairs. Profile, query, and snapshot checks were separate.

The exact final ASCII command is recorded in the [archive README](../../../bench/results/index-construction-optimization/2026-11-12-master-737cfb77/README.md); the Unicode command changes `--alphabet` and output directory. All other exact checkout, binary, alphabet, worker, and output arguments are in each `config.json`; the complete executable argument vector and resolved revision are in every `run-manifest.json`. Final B release binary SHA-256: `3cdb0e44734d084d47b602a21a4646ae524827e81622b5823dde2084bc5428e8`; combined binary: `758732a5359bf6678756a0791c303cf55b6102af74f04abba8193a7daa0a5249`.

| Track | Branch | Measured implementation commit | Integration commit |
| --- | --- | --- | --- |
| M | `task/index-construction-optimization/metadata-parallelism` | `1b0d4676`, `d5965bd1` | `9cacbe39`, `41b3a68a` |
| E1 | `task/index-construction-optimization/file-buffer-reuse` | `90057337` | `74584174` |
| E2 | `task/index-construction-optimization/bigram-scratch-reuse` | `c6f60bff` | Rejected, branch retained |
| E3 | `task/index-construction-optimization/field-selection-allocation` | `e41b7f21` | Rejected, branch retained |
| E4 | `task/index-construction-optimization/direct-logical-deserialization` | `66f42cc3` | `ed38a27c` |
| A0 | `task/index-construction-optimization/batch-admission-analysis` | `82f7c1a8` | `d91214ed` |
| A1 | `task/index-construction-optimization/batch-admission-optimization` | `ce3bd850` | `11dd9f5d` |
| Warning cleanup | Integration branch | — | `609474ee` |

### Metadata M

Metadata is prepared in indexed Rayon windows of at most 64 paths per worker, then inserted into the inventory in original ordinal order. This preserves deterministic file IDs, membership, and first-error selection. The pool is initialized before discovery and its setup time is recorded separately. A profiled 16-worker run had 327.430 ms discovery: 100.765 ms enumeration, 53.849 ms parallel metadata preparation, 170.712 ms ordered insertion, and 0.618 ms pool setup; peak active metadata workers was 16. In the same profile mode, B discovery was 661.641 ms. The 64-path window beat the independently committed 32-path window in all three direct ASCII pairs by a 27.090 ms median.

| ASCII workers | Pairs | B median, ms | M median, ms | Paired change |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 3 | 7,648.692 | 7,612.892 | −35.800 ms (−0.468%) |
| 4 | 3 | 2,918.764 | 2,642.986 | −276.507 ms (−9.502%) |
| 16 | 5 | 2,522.763 | 2,194.548 | −341.758 ms (−13.475%) |

The one-worker difference is within ordinary noise; the improvement comes from parallel metadata reads at four and 16 workers. Focused checks covered overlapping collections, original order and error choice; the core test suite passed. Existing discovery tests cover empty collections, symlinks, and non-UTF-8 paths.

### Allocation experiments

E1 reused one input `Vec<u8>` per 512-file build batch, reserves to the discovered size up to 1 MiB, reads to EOF, and releases an oversized capacity. Malformed whole-file fallback retains byte ownership through `mem::take`. In the profiled real build, 148,486 files were read, only 719 files grew the retained buffer, the largest retained capacity was 86,760 bytes, and batch-end capacities summed to 1,345,616 bytes. Sampled read calls fell from 17,804 to 4,672; sampled bytes, complete-file digests, queries, and index counters matched. Five paired ASCII builds were neutral, three paired Unicode builds slightly faster, and median ASCII peak RSS fell 3.1 MiB. The bounded retention and lower read-call traffic justify keeping it; it does not claim an independently established wall-time speedup.

E2 retained up to eight lazy bigram scratch instances per worker. A focused test confirmed no stale gram bits across empty, 1/63/64/65-record batches and both alphabets; the full core suite and query checks passed. The profiled build created 40, reused 556, and grew 42 scratch instances, yet the paired build regressed by 46.9 ms ASCII and 33.8 ms Unicode. Median ASCII peak RSS rose **14.6 MiB**. The existing allocator likely already recycles much of the batch scratch, while thread-local retention keeps more tables resident; the evidence does not justify the added complexity or memory. E2 remains an independent rejected branch and is absent from integration.

E3 replaced construction-only `Vec` selections and intermediate mapped/reference vectors with inline `SmallVec` storage. Existing tests cover path selection, missing/null/empty values, multi-value traversal, and custom operations; full core tests and query checks passed. The profiled full corpus recorded only 63 sampled custom-text single selections and no custom-posting selections in sampled files. Valgrind DHAT on the same 776-record configured subset recorded **50,943,456 bytes / 284,002 blocks** for B and **50,943,382 bytes / 283,875 blocks** for E3: 74 bytes and 127 blocks fewer over the whole process. The full-corpus timing was neutral in both alphabets. E3 is rejected because its exercised-path and allocation evidence does not establish a useful configured-build benefit; it does not claim to remove the separate owned `map_text` string.

E4 uses a construction-only serde visitor to build the existing logical `Value` and `ArrayValue` directly. It keeps the ordinary reader distinct from `JsonDecodedScalars`, and the old parser runs only on an error path to preserve diagnostic category and coordinates. A focused comparison covers exact number boundaries and exponents, duplicate keys, Unicode escapes, malformed bytes, nested/empty arrays, and depth limits. The visitor follows serde_json's arbitrary-precision number-map token; this private representation is the main maintenance risk and is covered by exact-number tests at the locked dependency version. DHAT on the 776-record subset recorded **36,383,847 bytes / 258,973 blocks**, **14,559,609 bytes (28.6%) and 25,029 blocks (8.8%) fewer than B** across the process. Full-corpus builds improved in every ASCII and Unicode pair, median peak RSS fell about 5 MiB in each alphabet, and all query/index counters matched. E4 is retained.

DHAT totals include setup and post-build reporting as well as construction; the small fixture establishes allocation direction, while only the full configured-corpus paired runs establish elapsed build changes. No full-corpus Valgrind run was needed.

### Admission A0 and A1

A0 adds opt-in component timers around ordered batch admission and worker dispatch/receipt timestamps. A representative profiled run had **1,143.963 ms** of callback plus admission work across collections, of which `contents` accounted for **1,077.58 ms**. The leading components were about **475 ms bigram contribution merges**, **456 ms structured posting merges**, and **166 ms record/inventory admission**. Inference was disabled. The component sums reconcile with each collection's timed total; remainder includes destruction and timer boundaries. The coordinator hit its outstanding-job limit 264 times, the byte limit zero times, and had a conservative 79.8 ms of possible idle-worker capacity during admissions. Two first jobs from later collections were intentionally prefetched. Across jobs, completion-to-receipt and receipt-to-admission sums overlap heavily, so they are not estimates of reducible wall time or CPU idle. Five paired unprofiled ASCII and three Unicode A0 controls showed no material disabled-mode overhead.

Before A1 implementation, this trace supported one bounded dispatch candidate: submit available jobs after each successful ordered admission, while retaining the same job/byte allowance, ordered slots, prefetch policy, source batch sizes, and error draining. A1's independent core suite passed; five ASCII and three Unicode pairs against **A0**, with equivalent diagnostics disabled, gave the modest improvements in the main table. Paired median peak RSS increased **8.7 MiB ASCII** and **7.2 MiB Unicode**, consistent with more worker overlap within the unchanged bound. One profiled pair reduced possible-idle-during-admission time from 118.5 to 51.1 ms and dispatch-to-worker-start aggregate from 1,123.6 to 675.8 ms; these overlapping intervals are diagnostic, not elapsed savings. A1 is retained because both alphabets improved, semantics remained deterministic, and the memory cost stayed bounded. The remaining admission bottleneck is serial bigram/posting merge work, with some scheduling restriction; this task does not claim to solve it entirely.

A single representative **profiled** ASCII run of each variant is available in the archive. Its candidate discovery/source-work/admission times (ms) were: M 327.4/1,347.4/1,164.8; E1 649.7/1,268.9/1,111.6; E2 660.7/1,291.7/1,146.5; E3 648.4/1,316.5/1,173.6; E4 662.8/1,267.5/1,129.0; A0 655.4/1,283.2/1,144.3; A1 652.7/1,300.6/1,173.0; final 327.9/1,206.6/1,076.6. These isolated diagnostics vary with scheduling and are not substitutes for paired unprofiled medians.

## Correctness, persistence, and limits

The final combined code passed `cargo test --workspace --locked` twice, including a rerun after the warning cleanup: **811 passed, 0 failed, 17 ignored** across workspace test groups. `cargo clippy --workspace --all-targets --locked` passed with existing warnings. An exploratory `-D warnings` variant failed on 165 lint findings across the workspace, mostly pre-existing style/API warnings; the new direct-parser helper also inherits the existing large-error-result pattern. This strict variant was not a repository gate and is archived. Touched Rust files were formatted using rustfmt followed by the repository spacing script; The implementation-code diff passed `git diff --check`; the preserved task source has intentional Markdown hard-break spaces, so the full staged document check used `git -c core.whitespace=-blank-at-eol diff --cached --check`. Branch-specific core suites passed (M/E1/E2/E4: 507 tests; E3/A0/A1: 506 tests, excluding ignored tests). Relevant CLI and desktop packages compiled and tested through the workspace command.

For final fresh builds, Q001, Q108, Q116, Q120, and Q194 digests matched B. A separate ASCII snapshot save produced the same **129,139,458-byte** size on B and the combined code. Using a temporary verifier built against the combined code, **both** snapshots restored in `SnapshotMode::Required` with zero build parses and identical five query digests. Snapshot byte hashes differ; this serializer does not promise byte-identical HashMap layout. The 129 MB snapshot payloads are omitted from the archive; samples, manifests, hashes, verifier source, and restore outputs remain.

The measurements are warm-cache on this host only; they do not predict cold-cache or other-machine speed. The final ASCII high-water RSS was essentially unchanged, while Unicode rose 9.4 MiB. E2 is a measured regression and excluded, E3 is a measured neutral result and excluded, and A0's small unprofiled deltas are inconclusive. These choices preserve the user's requirement to explain regressions without carrying an erroneous optimization into the final branch. No known correctness regression or query mismatch remains. Master has not been changed or merged by this task.
