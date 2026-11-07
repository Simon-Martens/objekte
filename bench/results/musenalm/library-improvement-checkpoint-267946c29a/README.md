# Phase 1 plus Q1 Musenalm checkpoint

Measured production code: integration merge `267946c29aeba1a2e3cf5976c843dce61bdec76e`, plus documentation-only commit `d78e6ef0f0`. Starting comparison: `81788174898c27b2c2c367319bad20ecee450562`; its raw evidence is in `../library-improvement-baseline-8178817489/`. The original `master` checkout was not used for the run.

Environment: 7 November 2026, Intel Core i9-13900K, Linux performance governor, Rust/Cargo 1.99.1, release profile, 32 resolved build and query workers. The integration worktree was `/home/simon/source/objekte-library-integration-20261107`. Other agents paused Cargo and benchmarking during these commands. The benchmark uses the unchanged worktree copy of `examples/musenalm` and no source or decoded cache.

Commands, run sequentially from that worktree:

```sh
cargo build --release -p objekte-search-core --example bench_build_musenalm --example bench_musenalm
./target/release/examples/bench_build_musenalm --root examples/musenalm --alphabet ascii --snapshot-mode disabled --build-workers auto --schema-manifest-path bench/results/musenalm/library-improvement-checkpoint-267946c29a/build-schema.json > bench/results/musenalm/library-improvement-checkpoint-267946c29a/build.json 2> bench/results/musenalm/library-improvement-checkpoint-267946c29a/build.stderr.log
./target/release/examples/bench_musenalm --root examples/musenalm --extended --cases Q035,Q045,Q084,Q127,Q189,Q190,Q194 --profiles ascii-off --repetitions 6 --result-dir bench/results/musenalm/library-improvement-checkpoint-267946c29a/queries > bench/results/musenalm/library-improvement-checkpoint-267946c29a/queries.stdout.log 2> bench/results/musenalm/library-improvement-checkpoint-267946c29a/queries.stderr.log
```

The build JSON's semantic signature is `a6149d2d78f5f4c14970dae09ebd146e206a59c4a717d224afb84aa42fd2cc5a`, identical to baseline. Its schema digest is `38f2dc5112146aa6ac0514291c76eaeb741421fa8a4624e50fa83c4c0a449bf9`, also identical. The query run manifest matches baseline corpus, schema and workload fingerprints. All seven query statuses are `ok`; the result CSV records all six samples, counts, candidates and source access.

The fresh-build opening interval was 1,191.029 ms versus 1,233.968 ms at baseline, a 42.940 ms lower single sample. Peak RSS was 516.641 MiB versus 490.215 MiB. A single build sample cannot establish a stable speed or memory change. See `docs/development/library-improvement-results.md` for the per-case table and interpretation.
