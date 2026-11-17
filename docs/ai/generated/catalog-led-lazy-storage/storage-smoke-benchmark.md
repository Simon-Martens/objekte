# Catalog-led storage smoke benchmark

Date: 2026-11-17  
Revisions: `ed789336e6` and `456184cd38`  
Worktree: `/home/simon/source/objekte-integration-catalog-led-lazy-storage`  
Dataset: `examples/musenalm`  
Profile: release, 32 query workers, verified cache disabled  
Queries: Q001 and Q119, five warm repetitions

Command:

```sh
cargo run --release -p objekte-search-core --example bench_storage -- \
  --cases Q001,Q119 --repetitions 5 --workers 32 \
  --storage-root target/storage-benchmark
```

The user authorized a smaller benchmark for this task. These runs are therefore bounded
smoke measurement, not the complete required 150-case matrix.

| Operation | Provisional binary metadata at `ed789336e6` | rkyv-framed metadata chunks at `456184cd38` |
| --- | ---: | ---: |
| Native construction | 819,727,250 ns | 794,299,689 ns |
| Explicit save | 2,121,991,565 ns | 2,656,014,596 ns |
| Search-ready reopen | 459,725,093 ns | 1,231,176,136 ns |
| Offline audit | 610,882,939 ns | 2,111,890,203 ns |
| Stored bytes | 248,524,432 | 419,651,448 |

| Query | Revision | Cold | Warm median | Warm min | Warm max | Count |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Q001 | `ed789336e6` | 1,078,878,523 ns | 1,108,278,867 ns | 1,101,190,168 ns | 1,120,412,051 ns | 3,269 |
| Q001 | `456184cd38` | 1,062,678,261 ns | 1,048,728,827 ns | 1,034,168,748 ns | 1,064,426,739 ns | 3,269 |
| Q119 | `ed789336e6` | 27,729,484 ns | 4,686,140 ns | 4,643,302 ns | 4,848,414 ns | 133,948 |
| Q119 | `456184cd38` | 25,067,725 ns | 4,714,343 ns | 4,687,405 ns | 4,796,561 ns | 133,948 |

Open read only the `CURRENT` record and the bounded metadata manifest during the measured
search-ready interval. It reported zero source reads and zero source parses. The cold
status reported 148,486 eager file rows, 148,486 eager record rows, 418 eager index
descriptors, and 714 eager leaf rows.

The latest save produced 1,236 objects and 419,651,448 bytes. Audit validated all 1,236
reachable objects with no errors. The storage remained lazy for Q001 and used the correct
source-scan fallback; therefore the warm Q001 time is not evidence of a working stored-gram
acceleration path. This limitation must be repaired before final acceptance.

The latest format removes bincode and frames metadata chunks with rkyv, but the chunk payloads
still contain a serialized eager snapshot rather than the required independently archived
catalog rows and maps. That schema and its size/open-time overhead remain blocking work.
