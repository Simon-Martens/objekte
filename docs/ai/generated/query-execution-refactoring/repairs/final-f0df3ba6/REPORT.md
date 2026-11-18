# Query execution repair final evidence

Task: `core-query-execution-repair`

- Final implementation revision: `f0df3ba68548a4e487556b3cbfcd7f0fba2db590`
- Original comparison base: `2e18871c0bcfb9cf9335d9e73525c64f91c960cf`
- Integration branch: `integration/query-execution-refactoring`
- Corpus: `/home/simon/source/objekte/examples/musenalm`
- Host: AMD Ryzen 7 5700G, 32 logical CPUs
- Toolchain: Rust/Cargo `1.99.1`
- Workers: 4
- Full matrix repetitions: 5

Raw samples, diagnostics, result digests and manifests are preserved in each
subdirectory. Diagnostic plans are collected after the primary timed samples.

## Validation

```text
cargo check --locked -p objekte-search-core --all-targets
cargo test --locked -p objekte-search-core
cargo build --release --locked -p objekte-search-core --example bench_musenalm
```

The final core run passed 564 library tests, all 28 core integration tests and
the doc test. All result digests and ordinary work counters match original
master.

## Balanced ascii-off

Fresh paired samples are in `baseline-ascii-off/` and `ascii-off/`.

| Case | Base ms | Final ms | Ratio | Delta ms |
| --- | ---: | ---: | ---: | ---: |
| Q001 | 14.922 | 15.456 | 1.0358 | +0.535 |
| Q007 | 360.296 | 343.959 | 0.9547 | -16.337 |
| Q030 | 3.190 | 2.986 | 0.9362 | -0.204 |
| Q035 | 0.081 | 0.058 | 0.7094 | -0.024 |
| Q084 | 0.037 | 0.056 | 1.5208 | +0.019 |
| Q127 | 0.066 | 0.075 | 1.1303 | +0.009 |
| Q141 | 4.569 | 4.500 | 0.9849 | -0.069 |
| Q150 | 2.923 | 2.775 | 0.9492 | -0.149 |
| Q163 | 2.294 | 2.151 | 0.9375 | -0.143 |
| Q164 | 6.807 | 6.141 | 0.9021 | -0.666 |
| Q181 | 35.060 | 34.658 | 0.9885 | -0.403 |

Q001 exceeded 0.2 ms but not 5%. Q084 exceeded 5% but not 0.2 ms. No ascii-off
case crossed both parts of the investigation gate.

## Incremental Unicode

The five-sample matrices are in `baseline-incremental-unicode/`,
`incremental-unicode/` and `incremental-unicode-rerun/`. Their Q030/Q141
medians varied enough to cross the combined gate, so focused 100-repetition
runs were taken in alternating base/final order.

| Case/profile | Base | Final | Result |
| --- | ---: | ---: | --- |
| Q030 unicode-off | 2.639 ms | 2.673 ms | +0.034 ms, no regression |
| Q141 unicode-off, final-first | 4.503 ms | 4.839 ms | +0.336 ms |
| Q141 unicode-off, base-first | 4.397 ms | 4.569 ms | +0.173 ms |

The Q141 direction is slightly slower, but it is not consistent across paired
order and remains near the combined threshold. `q141-base.perf` and
`q141-final.perf` recorded 1.410 s versus 1.429 s elapsed for 100 complete
query iterations, approximately +0.19 ms per query. The final run has about
4.8% more core instructions but no additional scan, file, parse, residual or
candidate work. No result digest differs.

Warm verified-cache samples remain tens of microseconds and perform no source
reads, provider applies or filter work. Q141's diagnostics now show executed
sort-column observations with effective inputs `4928 -> 3821 -> 3308` and the
text provider's `4928 -> 1627` restriction for each physical segment.
