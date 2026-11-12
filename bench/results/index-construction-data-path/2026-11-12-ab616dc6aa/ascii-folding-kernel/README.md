# Folded-gram kernel experiment

Task: `index-construction-data-path`, Experiment 1. Base B: `ab616dc6aabb9b9fee4657deb6752a6512bec9e5`. SIMD candidate commit: `d938fa56` on `task/index-construction-data-path/ascii-folding`.

Host: x86-64, Intel Core i9-13900K, 32 logical CPUs; AVX2 reported by runtime feature detection. Compiler: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`. Dependencies: locked workspace `Cargo.lock`. Release test binary SHA-256: `9776c3fc82f2ecf468eec4ca29cb25d3fd6cc42905a30fc640cff0a96ff22bed`. The AArch64 NEON backend was written but could not be compiled or run locally because only `x86_64-unknown-linux-gnu` is installed.

Build/warm-up command:

```bash
cargo test -p objekte-search-core --release --locked benchmark_folded_gram_emission --lib -- --ignored --nocapture
```

Three fresh timed invocations of the resulting test binary, sequentially:

```bash
target/release/deps/objekte_search_core-0945fa776efe814b benchmark_folded_gram_emission --ignored --nocapture
```

Each trial's complete stdout is in `trial-01.txt` through `trial-03.txt`. The test uses `black_box` on each borrowed segment and emits all adjacent and skip-one grams into a checksum. Each role uses the same input and iteration count; all role checksums matched in all 48 rows of each trial. Input cases are 19, 31, 32, 64, 128, 2688, 1760, and 2176 bytes. The 1760-byte case repeats ASCII-heavy mixed UTF-8 text; the 2176-byte case is predominantly non-ASCII. Iterations are `max(2_000_000 / input_bytes, 2_000)`. Times include folding, classification, pair construction, and callback emission, but exclude index map insertion and full-build scheduling.

Medians of three elapsed times per role, nanoseconds. Negative native-versus-scalar values favor AVX2.

| Alphabet | Case | Reference | Scalar | AVX2 | AVX2 vs scalar |
| --- | --- | ---: | ---: | ---: | ---: |
| ASCII | short-ascii | 32,692,994 | 4,699,897 | 4,577,919 | -2.6% |
| ASCII | ascii-31 | 21,079,587 | 4,702,915 | 4,683,073 | -0.4% |
| ASCII | ascii-32 | 21,091,730 | 4,759,556 | 5,148,167 | +8.2% |
| ASCII | ascii-64 | 20,603,085 | 4,828,736 | 5,317,360 | +10.1% |
| ASCII | ascii-128 | 20,398,948 | 5,048,926 | 5,385,167 | +6.7% |
| ASCII | long-ascii | 82,415,021 | 14,300,642 | 14,386,958 | +0.6% |
| ASCII | mixed | 46,611,023 | 12,306,864 | 11,100,234 | -9.8% |
| ASCII | unicode | 27,203,604 | 26,932,259 | 28,455,461 | +5.7% |
| Unicode | short-ascii | 28,367,677 | 4,967,794 | 4,950,497 | -0.3% |
| Unicode | ascii-31 | 20,069,257 | 5,062,374 | 4,862,957 | -3.9% |
| Unicode | ascii-32 | 19,969,627 | 4,987,179 | 5,185,342 | +4.0% |
| Unicode | ascii-64 | 19,563,731 | 5,164,472 | 5,308,774 | +2.8% |
| Unicode | ascii-128 | 19,592,653 | 5,273,481 | 5,333,963 | +1.1% |
| Unicode | long-ascii | 78,588,008 | 14,514,017 | 14,450,552 | -0.4% |
| Unicode | mixed | 47,027,825 | 10,644,901 | 10,772,164 | +1.2% |
| Unicode | unicode | 25,127,921 | 25,672,123 | 27,357,173 | +6.6% |

The scalar path beats the reference by large margins on ASCII and ASCII-heavy mixed input. AVX2 improves only one representative mixed case materially and regresses common 32–128-byte ASCII and predominantly non-ASCII cases. The measured SIMD gains do not justify dispatch, unsafe intrinsics, and a second backend in production. This is a kernel retention decision; the supervisor will compare the scalar production candidate against B on the full corpus.
