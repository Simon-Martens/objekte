# Library improvement execution record

## Starting state and baseline

The integration branch starts from `81788174898c27b2c2c367319bad20ecee450562` on `master` (7 November 2026). The original checkout remains on `master`; its untracked implementation plan is not part of the baseline. Toolchain: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, `cargo 1.99.1`. Machine: 32 available CPUs, Linux `performance` governor. All commands below ran in `/home/simon/source/objekte-library-integration-20261107` with a separate Cargo target directory and repository fixture in that worktree.

Baseline validation: `cargo test --workspace` passed; `cargo clippy --workspace --all-targets` exited successfully with pre-existing warnings. Complete logs are in [the baseline evidence](../../bench/results/musenalm/library-improvement-baseline-8178817489/).

The release examples were compiled before timing:

```sh
cargo build --release -p objekte-search-core --example bench_build_musenalm --example bench_musenalm
./target/release/examples/bench_build_musenalm --root examples/musenalm --alphabet ascii --snapshot-mode disabled --build-workers auto --schema-manifest-path .task-results/baseline/build-schema.json
./target/release/examples/bench_musenalm --root examples/musenalm --extended --cases Q035,Q045,Q084,Q127,Q189,Q190,Q194 --profiles ascii-off --repetitions 6 --result-dir .task-results/baseline/queries
```

The raw result files were moved into the linked evidence directory after measurement. Their manifests retain the exact invocation. The fresh build used 32 resolved workers, parsed 148,486 records with zero parse failures, and had a 1,233.968 ms repository-opening wall interval: 249.414 ms discovery, 717.936 ms extraction/indexing, and 254.137 ms finalization. Peak RSS was 490.215 MiB. Index signature: `a6149d2d78f5f4c14970dae09ebd146e206a59c4a717d224afb84aa42fd2cc5a`.

The query checkpoint used the extended schema and fixed ASCII alphabet, no verified-result cache, zero source/decoded cache, 32 query workers, and six repetitions. All seven cases passed. Median times and actual matches were:

| Case | Query category | Median ms | Matches | Source parses |
| --- | --- | ---: | ---: | ---: |
| Q035 | numeric range | 0.037 | 124 | 0 |
| Q045 | numeric range | 0.176 | 1,107 | 0 |
| Q084 | array equality | 0.209 | 4,922 | 0 |
| Q127 | stored composite sort | 0.061 | 203 | 0 |
| Q189 | text filter with inline sort | 6.326 | 7 | 203 |
| Q190 | numeric range with computed sort | 114.186 | 3,821 | 3,821 |
| Q194 | text filter with composite sort | 0.361 | 7 | 203 |

These are first measurements, not speedup claims. Query construction in the harness also saved and restored a snapshot outside the primary timed query samples; its separate output appears in the raw report. Future comparisons must match corpus, schema, profile, workers, and cache conditions. The chosen cases provide a compact checkpoint; they do not establish the requested 1/4/16-term planning scaling, which needs a focused harness or fixture after Q1 implementation.

## Integrated work

| Finding | Task head | Integration merge | Combined validation |
| --- | --- | --- | --- |
| C1 prepared-cache accounting | `3811445dba` | `7c64ddd22d` | Five focused cache tests and workspace tests passed. |
| C2 JSON parser acceptance | `a9309e6c54` | `e15918792b` | Two focused reader differential tests and workspace tests passed. |
| C4 distinct-byte inference summary | `2c7da67595` | `aee8ef0bcc` | Six focused allocation/boundary tests and workspace tests passed. |
| C5 public numeric invariants | `376bf2bcca` | `bc36dfaf08` | Three focused numeric/admission tests and workspace tests passed. |

The C4 task's second commit moved its allocation tests into a separate test file to match the repository code convention. No integration conflicts occurred in these merges. C3 composite-status ordering and C6 path/binding contract remain pending in Phase 1.
