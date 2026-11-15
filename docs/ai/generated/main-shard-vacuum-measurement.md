# Main-shard vacuum and reuse measurement

- Task: `main-shard-vacuum` (plan 5).
- Date: 2026-11-15.
- Machine: 13th Gen Intel(R) Core(TM) i9-13900K, 32 logical CPUs, Linux,
  rustc 1.99.1 (58cf8f9c3 2026-10-08).
- Fixture: disposable copy of `examples/musenalm`.
- Command:

```sh
cp -a examples/musenalm /tmp/objekte-main-shard-vacuum/fixture
OBJEKTE_FOLD_FIXTURE=/tmp/objekte-main-shard-vacuum/fixture \
  cargo test --release -p objekte-search-core --locked --lib \
  measure_fold_cost_on_real_fixture -- --ignored --nocapture
```

The test first updates 2,000 `contents` files once, then runs 20,000 further
updates cycling over those same files.

## First 2,000 updates

| Measurement | Value |
| --- | --- |
| updates | 2,000 |
| updates without fold: median / p95 / max | 4.46 ms / 15.91 ms / 178.27 ms |
| updates with fold: median / p95 / max | 12.62 ms / 40.68 ms / 234.96 ms |
| folds | 249 |
| failed folds | 0 |
| shards at the end | 74 |
| main shard records / live records | 3,064 / 2,040 |

## 20,000-update churn cycle

| Step | Main records | Main live | Shards | Folds | Vacuums | RSS |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 2,000 | 3,064 | 2,040 | 74 | 499 | 3 | 861,020 kB |
| 4,000 | 3,064 | 2,040 | 74 | 749 | 5 | 861,696 kB |
| 6,000 | 3,064 | 2,040 | 74 | 999 | 7 | 862,248 kB |
| 8,000 | 3,064 | 2,040 | 74 | 1,249 | 9 | 862,312 kB |
| 10,000 | 3,064 | 2,040 | 74 | 1,499 | 11 | 862,340 kB |
| 12,000 | 3,064 | 2,040 | 74 | 1,749 | 13 | 862,344 kB |
| 14,000 | 3,064 | 2,040 | 74 | 1,999 | 15 | 862,344 kB |
| 16,000 | 3,064 | 2,040 | 74 | 2,249 | 17 | 862,520 kB |
| 18,000 | 3,064 | 2,040 | 74 | 2,499 | 19 | 862,640 kB |
| 20,000 | 3,064 | 2,040 | 74 | 2,749 | 20 | 862,816 kB |

- Churn update latency: median 3.50 ms, p95 14.45 ms, max 119.30 ms.
- Folds during the cycle: 2,500; vacuums: 19; migrations: 0; fold failures: 0.
- RSS at the start: 858,520 kB; at the end: 862,816 kB; growth 0.5%.
- Query probe p95: 0.0010 ms at the end, 0.0014 ms at the start.
- Physical slots stayed at 1.5 times the live count (3,064 / 2,040), below the
  4x gate.

The first 2,000 updates' counts include three vacuums that already happened
before the churn cycle. The cycle's own counters are the differences above.

An explicit `Index::compact()` still rewrites everything densely and turns the
main shard into `Built` partitions; vacuum is a separate in-place operation.

Update latency with a fold is dominated by the fixed-bitmap copy documented in
`docs/ai/issues/perf/main-shard-bitmap-clone-per-fold.md`; that issue remains
open.
