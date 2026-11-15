# Appendable shards and fold cost on Musenalm

- Task: `appendable-shards` (plan 3).
- Code: C8 commit based on `516ea6e43c`.
- Date: 2026-11-15.
- Machine: 13th Gen Intel(R) Core(TM) i9-13900K, 32 logical CPUs, Linux,
  rustc 1.99.1 (58cf8f9c3 2026-10-08).
- Fixture: `examples/musenalm`, `contents` collection.
- Command:

The fixture must be a disposable copy because the test rewrites 2,000 files:

```sh
cp -a examples/musenalm /tmp/objekte-appendable-shards-final2/musenalm-fold
OBJEKTE_FOLD_FIXTURE=/tmp/objekte-appendable-shards-final2/musenalm-fold \
  cargo test --release -p objekte-search-core --locked --lib \
  measure_fold_cost_on_real_fixture -- --ignored --nocapture
```

The test opens the fixture with `Index::build`, then updates 2,000 `contents`
files one at a time with byte-identical content plus a trailing newline.

| Measurement | Value |
| --- | --- |
| updates | 2,000 |
| updates without a fold: median | 3.450 ms |
| updates without a fold: p95 | 4.900 ms |
| updates without a fold: max | 24.359 ms |
| updates with a fold: median | 8.568 ms |
| updates with a fold: p95 | 11.100 ms |
| updates with a fold: max | 23.903 ms |
| folds | 239 |
| failed folds | 0 |
| shards at the end | 73 |
| main shard records | 1,912 |
| main shard live records | 1,912 |

An update that triggers a fold costs about 2.5 times a plain update in this
fixture. The fold copies the main shard's fixed bitmaps (`presence`, `nulls`,
`empty_selections`, `logical_records`, `empty_array_records` and the inference
bitmaps) as described in plan 3 D9; the measured main shard stayed small
because updates cycled over the same 2,000 files, so that copy is a modest part
of the fold cost here. The fold count is bounded by the eight-shard trigger.

## Mutation benchmark comparison

`bench_lifecycle_mutations` medians of three runs on fresh fixture copies
(plan 3 C0 at `6e369cda37` versus this plan's final commit):

| Mode | Phase | C0 ms | Final ms | Change |
| --- | --- | ---: | ---: | ---: |
| small | build | 2.965 | 2.985 | +0.7% |
| small | add | 0.176 | 0.162 | -8.0% |
| small | update_append | 0.089 | 0.074 | -16.9% |
| small | update_verified | 0.089 | 0.088 | -1.1% |
| small | move | 0.036 | 0.034 | -5.6% |
| small | delete_verified | 0.021 | 0.020 | -4.8% |
| small | reconcile_no_change | 0.111 | 0.103 | -7.2% |
| stress | build | 1285.320 | 1297.110 | +0.9% |
| stress | update_append | 5.690 | 5.589 | -1.8% |
| stress | update_verified | 24.574 | 23.467 | -4.5% |
| stress | batch_large_128 | 458.807 | 542.400 | +18.2% |
| stress | reconcile_with_query | 374.121 | 361.423 | -3.4% |
| stress | compact | 33.027 | 34.124 | +3.3% |

Stress query-probe p95 moved from about 0.077 ms to 0.088 ms (about +14%),
inside the gate's 25% blocking limit but above its 10% reporting threshold;
the run shares the machine with another long build.
