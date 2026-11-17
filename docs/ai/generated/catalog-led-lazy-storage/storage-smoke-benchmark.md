# Catalog-led storage smoke benchmark

Date: 2026-11-17  
Revision: `ed789336e6`  
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

The user authorized a smaller benchmark for this task. This run is therefore a bounded
smoke measurement, not the complete required 150-case matrix.

| Operation | Time |
| --- | ---: |
| Native construction | 819,727,250 ns |
| Explicit save | 2,121,991,565 ns |
| Search-ready reopen | 459,725,093 ns |
| Offline audit | 610,882,939 ns |

| Query | Cold | Warm median | Warm min | Warm max | Count |
| --- | ---: | ---: | ---: | ---: | ---: |
| Q001 | 1,078,878,523 ns | 1,108,278,867 ns | 1,101,190,168 ns | 1,120,412,051 ns | 3,269 |
| Q119 | 27,729,484 ns | 4,686,140 ns | 4,643,302 ns | 4,848,414 ns | 133,948 |

Open read only the `CURRENT` record and the bounded metadata manifest during the measured
search-ready interval. It reported zero source reads and zero source parses. The cold
status reported 148,486 eager file rows, 148,486 eager record rows, 418 eager index
descriptors, and 714 eager leaf rows.

The save produced 1,216 objects and 248,524,432 bytes. Audit validated all 1,216 reachable
objects with no errors. The storage remained lazy for Q001 in this run and used the
correct source-scan fallback; therefore the warm Q001 time is not evidence of a working
stored-gram acceleration path. This limitation must be repaired before final acceptance.
