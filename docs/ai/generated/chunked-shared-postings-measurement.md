# Chunked shared posting backing on Musenalm

- Task: `chunked-shared-postings` (plan 2).
- Code: commit `32f2dd1d29` plus the measurement test in the C4 commit.
- Date: 2026-11-15.
- Machine: 13th Gen Intel(R) Core(TM) i9-13900K, 32 logical CPUs, Linux,
  rustc 1.99.1 (58cf8f9c3 2026-10-08).
- Fixture: `examples/musenalm`, `contents` collection, 133,948 records.
- Command:

```sh
OBJEKTE_SHARED_BACKING_FIXTURE="$PWD/examples/musenalm" \
  cargo test --release -p objekte-search-core --locked --lib \
  measure_shared_backing_on_real_fixture -- --ignored --nocapture
```

All times are medians of 5 repetitions. `native` is the
`Building`-backed shard; `shared` is the same shard with every posting and
gram family frozen to the chunked `Shared` backing.

| Label | Value |
| --- | --- |
| `freeze_ms` | 57.079 |
| `keys` (posting dictionaries) | 140,781 |
| `chunks` (posting storage) | 148,687 |
| `gram_keys` | 5,264 |
| `gram_chunks` | 14,396 |
| `estimate_native_bytes` | 52,585,116 |
| `estimate_shared_bytes` | 52,720,940 |
| `lookup_native_ns` | 69.7 |
| `lookup_shared_ns` | 193.1 |
| `scan_native_ms` | 2.292 |
| `scan_shared_ms` | 6.211 |
| `fts_native_us` | 2,246.7 |
| `fts_shared_us` | 5,141.6 |
| `clone_shared_ns` | 75,606 |
| `cow_value_us` | 0.938 |
| `cow_gram_us` | 0.520 |

The measured field is `["musenalm_id"]` with 133,948 distinct values; it is
the posting field with the most distinct values. `fts_*` is the total time for
100 literals of 5 to 10 characters taken from record ids. `clone_shared_ns` is
one clone of the shard's posting, gram, custom and inference families (the
dense record/file/sort tables are not cloned). `cow_*_us` is the average cost
of one copy-on-write insert on a clone of the frozen map.

Reading through the shared backing costs roughly 2 to 3 times the native
`BTreeMap` per lookup, scan or full-text candidate intersection. The byte
estimate grows by 0.26% because per-chunk roaring headers replace one header
per key. Cloning the whole shared family set is cheap (about 76 µs), and a
single copy-on-write update copies only one chunk (about 1 µs for a value,
0.5 µs for a gram).

These numbers are input for plan 3 (`appendable-shards`): they show that
freezing a 134k-record shard is affordable, but also that shared-backed
lookups are slower and should stay limited to shards that benefit from cheap
clones and incremental updates.
