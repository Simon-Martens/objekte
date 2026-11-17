# Catalog-led storage smoke benchmark

Date: 2026-11-17

Implementation revision: `ee52c8239c`

Worktree: `/home/simon/source/objekte-integration-catalog-led-lazy-storage`

Dataset: `examples/musenalm` (148,498 files, approximately 603 MiB)

Profile: release, ASCII alphabet, 32 query workers, verified-result cache disabled

Queries: canonical Q001 and Q119, five warm repetitions, two independent opens

Command:

```sh
cargo run --release -p objekte-search-core --example bench_storage -- \
  --cases Q001,Q119 --repetitions 5 --workers 32 --open-samples 2 \
  --out docs/ai/generated/catalog-led-lazy-storage \
  --storage-root target/storage-benchmark-final
```

The user authorized this bounded smoke run instead of the complete 150-case matrix.
This is a final-revision measurement, not a substitute for every worker, alphabet,
mutation, memory and query-distribution sweep in the specification. Raw samples are in
`construction-samples.tsv`, `save-samples.tsv`, `open-samples.tsv`,
`query-samples.tsv`, `cache-sequence.tsv`, `storage-io-counters.tsv`,
`memory-summary.tsv` and `object-size-summary.tsv`.

## Construction, save, and open

| Operation | Result |
| --- | ---: |
| Native completed build | 805,910,122 ns |
| Explicit memory-only archive | 314,554,575 ns |
| First `archive_to` total | 2,759,753,145 ns |
| First save encode | 1,170,000,125 ns |
| First save object write | 1,530,668,953 ns |
| First save synchronization | 1,645,629 ns |
| Stored objects | 1,077 new plus 1 reused |
| Referenced storage bytes | 516,659,480 |
| Metadata object bytes | 84,998,904 in 5 objects |
| Payload-pack bytes | 431,660,576 in 1,072 objects |
| Search-ready open, two samples | 1,292,858,011 ns; 1,402,072,473 ns |
| Open metadata reads | 39 calls, 367,452,992 bytes on each sample |
| Open source reads/parses | 0 / 0 |
| Offline audit | 2,607,810,237 ns; all 1,077 objects verified |

Eager open restored 148,486 file rows, 148,486 record rows, 1,256 index
descriptors and 238,150 leaf rows. Application payload cache residency remained
zero until the first query. A sequential open grew peak RSS to approximately
2.18 GiB and current RSS to approximately 1.79 GiB.

## Query results

| Query | State | Latency | Count | Index reads |
| --- | --- | ---: | ---: | ---: |
| Q001 | Native | 7,094,361 ns | 3,269 | 0 |
| Q001 | Resident archived, no disk | 45,991,209 ns | 3,269 | 0 |
| Q001 | Reopened, first selected query | 14,360,213 ns | 3,269 | 253 |
| Q001 | Reopened warm median | 9,098,890 ns | 3,269 | 253 |
| Q119 | Native | 6,193,881 ns | 133,948 | 0 |
| Q119 | Resident archived, no disk | 7,009,558 ns | 133,948 | 0 |
| Q119 | Reopened, first selected query | 6,967,487 ns | 133,948 | 253 |
| Q119 | Reopened warm median | 6,808,177 ns | 133,948 | 253 |

Every storage result digest matched the native result exactly. The two warm query
sequences added no index reads after first use. They retained 1,070,230 accounted
payload bytes across 184 cache entries, with 920 cache hits and 184 misses. Fifteen
pack handles remained open at the end of the query sequence.

The former scanner-routed Q001 path measured about 1.06 s. This final smoke run's
first selected Q001 cost about 14 ms and its warm median about 9 ms. The resident
archived Q001 one-shot was 46 ms and is reported as measured rather than omitted.

## Cache sequence

The deterministic two-case sequence was run independently with 64 MiB and 256 MiB
budgets. Both sessions loaded the same 184 payload units (1,070,230 accounted bytes),
so neither small sequence reached either budget and no eviction occurred. The
separate finite-budget integration test uses a 64-byte budget, confirms eviction,
and verifies that selected posting leaves reload after query pins are released.

## Memory floor

| Checkpoint | RSS | Peak RSS |
| --- | ---: | ---: |
| Completed native build | 714,432,512 | 746,618,880 |
| After explicit memory archive | 942,747,648 | 947,482,624 |
| After first save | 1,374,822,400 | 2,013,229,056 |
| After eager open | 1,785,040,896 | 2,181,570,560 |

The eager metadata floor is substantial for this corpus. The storage payload cache is
small in this query slice, but eager snapshot/catalog materialization dominates RSS.
The cache budget does not include eager catalogs, unsaved native owners, query pins or
query scratch, and therefore is not a total-process memory limit.

## Remaining limitations

- Only Q001 and Q119 were measured. The complete 150-query ASCII/Unicode matrix,
  worker sweeps, mutation/compaction matrix and seven-process native comparison were
  not run for this bounded smoke authorization.
- Eager open still decodes a JSON snapshot carried in checked rkyv-framed metadata
  objects. It performs no lazy payload reads, but is slower and materially larger in
  memory than a final independently archived metadata schema would be.
- Presence/null/empty-selection state remains eager rather than loading as independent
  query-state leaves. Segment-local relation metadata is also coarser than the target
  endpoint-level layout.
- FTS remains a conservative filter and retains source verification where exact text,
  regex or evidence semantics require it.
- These measurements use an unloaded application payload cache. OS page-cache state was
  not controlled. No physical power-loss or non-Linux storage experiment was performed.
