# Query execution refactoring validation

Task: `query-execution-refactoring`

- Base: `2e18871c0bcfb9cf9335d9e73525c64f91c960cf`
- Integration branch: `integration/query-execution-refactoring`
- Host: AMD Ryzen 7 5700G, 32 logical CPUs
- Compiler: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`
- Cargo: `cargo 1.99.1 (5f94df478 2026-08-27)`
- Corpus: `/home/simon/source/objekte/examples/musenalm`
- Query workers: 4
- Profile: `ascii-off`
- Repetitions: 5 for the full short set, 10 for alternating rechecks
- Features: package defaults
- Result and work digests: identical for all 11 baseline/final cases

## Full short benchmark

```sh
cargo build --release --locked -p objekte-search-core --example bench_musenalm
./target/release/examples/bench_musenalm \
  --root /home/simon/source/objekte/examples/musenalm --extended \
  --cases Q001,Q007,Q030,Q035,Q084,Q127,Q141,Q150,Q163,Q164,Q181 \
  --profiles ascii-off --query-workers 4 --repetitions 5 \
  --result-dir <revision-specific-directory>
```

| Case | Base median ms | Final median ms | Ratio | Delta ms |
| --- | ---: | ---: | ---: | ---: |
| Q001 | 15.757 | 15.894 | 1.0087 | +0.136 |
| Q007 | 364.666 | 347.315 | 0.9524 | -17.350 |
| Q030 | 2.993 | 3.237 | 1.0815 | +0.244 |
| Q035 | 0.053 | 0.056 | 1.0605 | +0.003 |
| Q084 | 0.037 | 0.041 | 1.1051 | +0.004 |
| Q127 | 0.068 | 0.068 | 0.9951 | -0.000 |
| Q141 | 4.623 | 4.757 | 1.0292 | +0.135 |
| Q150 | 2.701 | 2.936 | 1.0868 | +0.234 |
| Q163 | 2.233 | 2.241 | 1.0036 | +0.008 |
| Q164 | 7.129 | 6.688 | 0.9381 | -0.441 |
| Q181 | 35.182 | 35.332 | 1.0043 | +0.150 |

The initial Q030 and Q150 rows crossed the absolute-plus-relative
investigation gate. Alternating base/final/final/base rechecks with 10
repetitions measured:

| Case | Base A | Final A | Final B | Base B | Paired delta A/B |
| --- | ---: | ---: | ---: | ---: | ---: |
| Q030 | 2.643 ms | 2.802 ms | 2.977 ms | 2.816 ms | +0.159 / +0.161 ms |
| Q150 | 2.522 ms | 2.671 ms | 2.704 ms | 2.579 ms | +0.148 / +0.124 ms |

Both rechecks stayed below the 0.2 ms increase gate. They are treated as normal
small-query timing variation, not a repeatable regression. No changed case
introduced a new source scan, whole-posting conversion, lost early termination,
or differing result/work digest.

## Correctness and focused evidence

- `cargo test -p objekte-search-core`: passed.
- `cargo check --workspace --all-targets`: passed.
- `cargo test --workspace`: passed.
- Provider selection tests reverse registration order and cost, reject an
  ineligible cheaper term, and surface an injected apply failure.
- The independent sorted scalar/gram backing compares scalar and text
  membership against a fixture oracle and demonstrates selective text reducing
  the structured lookup input.
- EXPLAIN tests assert zero provider applications; ANALYZE tests assert the
  same application count as one ordinary search execution.
- Ownership tests cover borrowed/shared membership, zero-materialization
  comparisons, domain mismatches, identity set algebra, compact unavailable
  locators, and shared `Arc` cache publication.

## Formatting

`cargo fmt --all -- --check` reports pre-existing formatting differences under
`crates/desktop`; the changed core files were formatted and `git diff --check`
passes.
