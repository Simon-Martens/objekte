# Catalog-led storage smoke benchmark

Date: 2026-11-17

Revision: `f711919fd8`

Worktree: `/home/simon/source/objekte-integration-catalog-led-lazy-storage`

Dataset: `examples/musenalm`

Profile: release, 32 query workers, verified cache disabled

Queries: Q001 and Q119, five warm repetitions

Command:

```sh
cargo run --release -p objekte-search-core --example bench_storage -- \
  --cases Q001,Q119 --repetitions 5 --workers 32 \
  --storage-root target/storage-benchmark-final-smoke
```

The user authorized a smaller benchmark for this task. These runs are bounded smoke
measurements, not the complete required 150-case matrix.

| Operation | Result |
| --- | ---: |
| Native construction | 768,065,742 ns |
| Explicit save | 2,507,818,228 ns |
| Search-ready reopen | 1,229,775,918 ns |
| Offline audit | 2,181,328,811 ns |
| Stored bytes | 420,710,352 |
| Stored objects | 1,062 |

| Query | Cold | Warm median | Warm min | Warm max | Count |
| --- | ---: | ---: | ---: | ---: | ---: |
| Q001 | 58,037,369 ns | 6,121,324 ns | 5,933,534 ns | 6,597,855 ns | 3,269 |
| Q119 | 5,461,149 ns | 4,802,463 ns | 4,652,454 ns | 4,946,908 ns | 133,948 |

The previous scanner-routed Q001 path measured about 1.06 s both cold and warm.
Stored FTS now enters the normal segment planner, loads dictionaries only for selected
segments, and reads only the gram leaves needed by the query. After loading, the five
warm repetitions produced 976 cache hits with no additional index reads. Q001 retained
208 decoded cache entries totalling 24,225,910 bytes in the measured process. That is
down from 28 whole binding payloads and 107,614,944 retained bytes before per-gram leaf
splitting.

The open snapshot still reported zero application-cache payload reads and zero source
parses. It read 35 eager metadata objects totalling 282,241,928 bytes and restored
148,486 file rows, 148,486 record rows, 418 eager index descriptors and 241,546 declared
leaf rows. Offline audit verified all 1,062 reachable objects without error.

## Remaining limitations

- Posting values and state bitmaps are still loaded as one field-level frame. Independent
  posting leaves and a fully eager posting-leaf catalog remain unfinished.
- FTS dictionaries are loaded per binding, while their normal/skip Roaring leaves are now
  independent and coalesced. A complete eager catalog row per leaf is still absent.
- Sort collections still use one collection/segment-level frame rather than one selected
  field/configuration frame per query.
- Base relation CSR adjacency is lazy, but segment-local relation contributions remain
  eager metadata and relations are not split into independent endpoint payloads.
- The snapshot body remains JSON split across rkyv-framed metadata chunks. Search-ready
  open is therefore still dominated by eager metadata decode and is a blocking schema
  and performance gap.
- FTS remains a conservative filter. Source verification for exact literal or regex
  evidence is retained where the query semantics require it.
- The complete 150-query matrix, worker/config sweeps, budget sequence, mutation matrix,
  S01-S80 execution evidence and final acceptance report are not yet complete.
