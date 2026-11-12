# SIMD focused gram-generation measurements

- Control base B: `54dcac5c3b3e1ca499180061ebcee7ad60c27101`.
- Candidate implementation: `3b155e13` on `task/index-construction-experiment-revision/vector-gram-generation`. The benchmark corpus extension to all short lengths 0–8 is part of this evidence commit; the binary was built from that combined tree.
- Release test binary: `target/release/deps/objekte_search_core-0945fa776efe814b`.
- Binary SHA-256: `49fb7be2ca2d2fc0c9a7f269fe77c6d7024b541c0bbb36e335a04d553b81d2d7`.
- Compiler: rustc 1.99.1, LLVM 23.1.1; locked dependencies; release profile.
- Host: Intel Core i9-13900K, x86_64, AVX2 available, 32 logical CPUs. No task affinity or governor override was applied for these focused runs.

## Commands

Run in the candidate worktree:

```
cargo test -p objekte-search-core --release --locked --lib benchmark_folded_gram_emission -- --ignored --nocapture --test-threads=1
cargo test -p objekte-search-core --release --locked --lib benchmark_real_builder_gram_generation -- --ignored --nocapture --test-threads=1
```

`checksum.txt` and `builder.txt` contain complete stdout/stderr from one serialized run of each command. Each test alternates scalar/native role order by case and asserts equal observable checksums. The builder test includes gram insertion, duplicate handling, chunk flush, completion and conversion to postings. The native backend is AVX2 for ASCII-index segments of at least 32 input bytes, and scalar otherwise. Unicode-index segments always use scalar gram generation.

These focused timings are a screening measurement, not a full-corpus retention decision. The cases include 0–8-byte segments, 16/32/64/128-byte boundaries, repeated and varied ASCII, mixed UTF-8, and predominantly non-ASCII text up to 2 KiB. They do not establish a corpus-weighted speedup. The initial run showed useful ASCII-index builder wins from 32 bytes upward and a loss on predominantly non-ASCII input. Short segments and the Unicode alphabet were near parity; their scalar/native labels execute the same path. Full-build comparisons against B remain the acceptance gate.
