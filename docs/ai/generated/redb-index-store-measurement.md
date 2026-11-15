# Redb index store measurement on Musenalm

- Task: `redb-index-store` (plan 4).
- Date: 2026-11-15.
- Machine: 13th Gen Intel(R) Core(TM) i9-13900K, 32 logical CPUs, Linux,
  rustc 1.99.1 (58cf8f9c3 2026-10-08).
- Fixture: a disposable copy of `examples/musenalm`.
- Command:

```sh
cp -a examples/musenalm /tmp/objekte-redb-index-store/fixture
OBJEKTE_INDEX_STORE_FIXTURE=/tmp/objekte-redb-index-store/fixture \
  cargo test --release -p objekte-search-core --locked --lib \
  measure_index_store_on_real_fixture -- --ignored --nocapture
```

| Measurement | Value |
| --- | --- |
| build plus first checkpoint | 2,690.9 ms |
| explicit checkpoint after build | 61.4 ms |
| store file size | 570,429,440 bytes |
| 1,000 source updates | 10.6 s total test time |
| checkpoint after 1,000 updates | 87.5 ms |
| main shard chunks written by that checkpoint | 0 |
| rows written by that checkpoint | 0 |
| main shards updated by that checkpoint | 0 |
| load from store | 875.4 ms |

The first checkpoint writes the whole index once. Later checkpoints of
immutable build shards write only changed active masks; the 1,000 small file
updates produced mutation shards, so the post-update checkpoint wrote those
new shards rather than diffing a main shard. Loading (875 ms) is clearly
faster than building and writing the store (2,691 ms) on this machine.

Loading still decodes the complete index into RAM; queries never read the
file. The store is not wired into CLI, TUI or desktop yet.
