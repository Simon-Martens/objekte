# Native segment construction baseline

Task: `native-segment-construction`

Base revision: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`

Recorded: 2026-11-16

## Environment

- Linux `7.3.3-200.fc44.x86_64`
- Intel Core i9-13900K, 32 logical CPUs, 24 physical cores, one NUMA node
- `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, LLVM 23.1.1
- `cargo 1.99.1 (5f94df478 2026-08-27)`
- Release examples compiled with `--locked`, default features, and no Rayon, allocator, OpenMP, or loader overrides
- Musenalm fixture: `examples/musenalm`, 148,486 JSON files
- Query workload SHA-256 `56f2ba99e3a889b95975a6b460c4ef894732ad769f99e6f69086a3f0d3ac91c0`

The preserved baseline executables and raw measurements are in
`/tmp/objekte-native-segment-baseline-c54486f8/`.

## Blocking Repository construction

Command:

```text
bench_build_musenalm --root examples/musenalm --alphabet ascii --build-workers 8
```

Three complete runs produced a median `build_ns` of 910,984,168 ns and median
process peak RSS of 410,136,576 bytes. Every run parsed 148,486 records with
zero failed parses. The semantic index signature was identical in all runs:

```text
2233b7760c4cbe19c27d0475ac2b2e561a89720defff7396977fdbbd525c6172
```

## Progressive Index construction

Command, once per alphabet:

```text
bench_musenalm --root examples/musenalm --incremental \
  --profiles ascii-off,unicode-off --repetitions 3 \
  --query-workers 8 --verified-cache-bytes 0
```

| Alphabet | Open/build | First batch | Peak RSS | Searchable batches |
| --- | ---: | ---: | ---: | ---: |
| ASCII | 917,645,145 ns | 294,015,068 ns | 541,511,680 bytes | 8 |
| Unicode | 1,004,707,910 ns | 334,063,478 ns | 868,315,136 bytes | 8 |

Both modes parsed all 148,486 records and reported zero failed parses.

## Query baseline

The 150-case workload ran under ASCII and Unicode with caching disabled, three
repetitions, and eight query workers. Both the direct Repository and progressive
Index reports contain 900 completed measurements with no benchmark failures.

## Interface migration checklist

| Surface | Current ownership | Required migration |
| --- | --- | --- |
| `RepositoryOptions` | Build workers, inference, alphabet and profiling | Add one `SegmentPolicy` shared by every ordinary construction path |
| `PendingCollection` | Collection-wide posting, bigram, sort and relation aggregation | Remove from ordinary construction; retain only compact metadata/routing summaries |
| `SegmentAdmission` | Optional eager collection/segment merge followed by a second merge | Replace with deterministic draft routing and ownership transfer |
| `CollectionSegment::into_indexed_profiled` | Sequential component merge and validation | Split reduction/finalization into worker jobs with independent component ownership |
| `Repository` | Flat `CollectionState` payloads plus no segment list | Own the adopted native segment snapshot and active routing state |
| `FileCatalog` | Retains `Arc<Repository>` | Retain shared inventory/configuration data without a base-to-catalog cycle |
| `PublishedIndex::from_repository` | Creates an empty segment list over a flat base | Adopt the Repository's completed segments without copying or repartitioning |
| `Index::from_repository` | Calls the 2,048-record base partitioner | Adopt the existing segment lists directly |
| Repository query methods | Flat scan/materialization implementation | Route through the same segmented evaluation used by `IndexView` |
| `IndexView` query methods | Segmented production path already present | Share catalog/routing/bounds with Repository and remove flat fallback |
| Mutation and compaction | Separate segment-size constants and 2,048-record projections | Use `SegmentPolicy`, deterministic `SegmentId` updates, and atomic routing publication |
| `bench_build_musenalm` | Reads and hashes collection-wide fields | Add `--segment-records`, report segment distribution, and hash logical segment content |

## Phase order

The implementation will proceed through the specified phases without treating
unit-test success as sufficient evidence while a production entry point still
uses a flat collection-wide builder.
