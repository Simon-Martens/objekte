# Default build timing after the profiling feature gate

This comparison checks the ordinary release binary against the pre-change profiling implementation. The baseline binary was built from `40808d893e`; `git diff 40808d893e 9c6c9e1af3` is empty, so its source tree matches the starting `master` tree. The candidate binaries were built from the modified `master` working tree with the default features. Binary SHA-256 hashes and every measured process are in [construction.json](construction.json) and [queries.json](queries.json).

Both builds used Rust 1.99.1, Linux 7.2.15-200.fc44.x86_64, an Intel Core i9-13900K, CPU affinity 0–31, and the `performance` governor. The corpus was `examples/musenalm`: 148,486 files and records in the incremental construction samples. Every run used the same corpus and schema. Processes ran sequentially, with one warmup per binary and alternating measured order.

## Incremental index construction

Build both binaries with `cargo build --release --locked -p objekte-search-core --example bench_index_construction`. For each process, run the resulting example with `--root examples/musenalm --result-dir <trial-directory> --incremental --workers 8`. The benchmark's fresh-process `build_ns` spans construction through full availability. Four measured processes per binary followed the warmups.

| Milestone | Baseline median | Default feature median |
| --- | ---: | ---: |
| Metadata observed | 209.936 ms | 212.307 ms |
| First searchable batch | 281.288 ms | 276.626 ms |
| Full build | 940.538 ms | 939.177 ms |

The full-build median difference was −0.15%. Observed full-build ranges were 933.367–948.066 ms before and 923.736–947.019 ms after. These samples do not establish a material construction speed change.

## Query checkpoint

Build both binaries with `cargo build --release --locked -p objekte-search-core --example bench_musenalm`. Each process ran `--root examples/musenalm --extended --cases Q035,Q045,Q084,Q127,Q189,Q190,Q194 --profiles ascii-off --repetitions 5 --incremental --result-dir <trial-directory>`. One warmup and two measured processes ran per binary. All case statuses were `ok`, and result digests matched between the binaries.

| Case | Baseline median | Default feature median |
| --- | ---: | ---: |
| Q035 | 0.019 ms | 0.019 ms |
| Q045 | 0.042 ms | 0.042 ms |
| Q084 | 0.017 ms | 0.017 ms |
| Q127 | 0.062 ms | 0.063 ms |
| Q189 | 0.462 ms | 0.420 ms |
| Q190 | 5.709 ms | 4.619 ms |
| Q194 | 0.265 ms | 0.266 ms |

The sum of the seven case medians was 6.577 ms before and 5.446 ms after. The two measured query processes per side are a regression checkpoint, not enough to claim a stable speedup; Q190 accounts for most of the difference. The default build showed no material regression in this sample.
