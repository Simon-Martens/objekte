# Musenalm simple timing benchmark

**Revision:** `f202069980a04cc47d6558a7f627f74727939ba6`
**Date:** 2026-11-19
**Purpose:** quick fresh-build and fixed-workload query timing, without a
matched baseline comparison.

## Environment and workload

- CPU: 13th Gen Intel Core i9-13900K, 32 logical CPUs
- CPU governor: `performance`
- CPU affinity: `0-31`
- Compiler: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`
- Dataset: `examples/musenalm`, 148,486 records in seven collections
- Release examples were compiled before timing.
- Build binary SHA-256: `c50b61cd363417938d4c7782264ae6a61d17abaf514be026b89cc7eecfcc20e6`
- Query binary SHA-256: `ba737cdbeca771f2a1e226d0a6a3532972f082c807c2d20b47886feedf2290dc`
- Build workers: auto
- Query workers: 4
- Query workload: all 150 cases from `musenalm_queries.json`
- Query profiles: `ascii-off` and `ascii-warm`
- Warm verified-result cache: 64 MiB

## Commands

```sh
cargo build --release --locked -p objekte-search-core \
  --example bench_build_musenalm --example bench_musenalm

python3 bench/run_build_regressions.py \
  --repetitions 1 \
  --result-dir bench/results/musenalm/simple-20261119-f202069980/build

target/release/examples/bench_musenalm \
  --profiles ascii-off,ascii-warm \
  --repetitions 1 \
  --query-workers 4 \
  --verified-cache-bytes 67108864 \
  --result-dir bench/results/musenalm/simple-20261119-f202069980/queries
```

## Fresh build timing

Each alphabet used one fresh process.

| Alphabet | Wall time | Peak RSS |
| --- | ---: | ---: |
| ASCII | 0.778 s | 620.8 MiB |
| Unicode | 0.734 s | 653.8 MiB |

Both builds returned exactly 148,486 records and 148,486 parses. Failed parses
and benchmark failures were zero. The query benchmark also performed its own
initial ASCII build in 0.633 s.

These single samples measure this machine and checkout only; their small
ASCII/Unicode difference is within ordinary run-to-run variability.

## Query timing

Each row below summarizes the 150 query cases for one profile. `Sum` is the sum
of the 150 per-query medians, `Mean` is their arithmetic mean, and the percentile
columns summarize the distribution of per-query medians.

| Profile | Samples | OK | Sum ms | Mean ms | Median ms | P95 ms | P99 ms | Slowest ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ASCII cache off | 150 | 150 | 1,442.252 | 9.615 | 0.355 | 28.324 | 254.499 | 277.124 |
| ASCII 64 MiB warm | 150 | 150 | 21.797 | 0.145 | 0.043 | 0.788 | 1.597 | 1.832 |

The cache-warm pass immediately followed the cache-off pass and all 150 cases
hit the verified-result cache. Its reduced timings therefore describe reuse of
those completed results, not a generally warm filesystem or a cold-cache
throughput claim.

Slowest cache-off cases:

| Case | Median | Matches | Files/parses |
| --- | ---: | ---: | ---: |
| Q007 | 277.124 ms | 147,798 | 148,486 |
| Q006 | 254.499 ms | 65,548 | 148,486 |
| Q026 | 139.819 ms | 18,825 | 80,994 |
| Q025 | 122.842 ms | 60,319 | 67,748 |
| Q145 | 109.431 ms | 40,947 | 44,373 |

Q006 and Q007 dominate the cache-off result because they scan and parse the full
148,486-record corpus. Warm-cache Q007 remained the slowest case at 1.832 ms.

## Result files

- [Build report](../../../bench/results/musenalm/simple-20261119-f202069980/build/REPORT.md)
- [Build gate](../../../bench/results/musenalm/simple-20261119-f202069980/build/gate.json)
- [Build measurements](../../../bench/results/musenalm/simple-20261119-f202069980/build/measurements.jsonl)
- [Query report](../../../bench/results/musenalm/simple-20261119-f202069980/queries/REPORT.md)
- [Query results CSV](../../../bench/results/musenalm/simple-20261119-f202069980/queries/query-results.csv)
- [Query measurements](../../../bench/results/musenalm/simple-20261119-f202069980/queries/measurements.jsonl)
- [Run manifest](../../../bench/results/musenalm/simple-20261119-f202069980/queries/run-manifest.json)

No query failed, every case returned `status=ok`, and the benchmark manifest
reported zero failures.
