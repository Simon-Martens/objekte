# Catalog-led storage reduced-scope benchmark

Date: 2026-11-17

Task: `catalog-led-storage`

This summary reuses the completed representative run. It does not restart the
benchmark campaign or rebuild historical revisions.

## Configuration

- Dataset: `examples/musenalm`
- Files: 148,486
- Records: 148,486
- Collections: 7
- Alphabet: ASCII
- Workers: 32
- Query workers: 32
- Verified-result cache: disabled
- Storage cache: `KeepLoaded`
- OS page-cache state: observed/unspecified; application payload cache empty after
  reopen

Command:

```sh
cargo run --release -p objekte-search-core --example bench_storage -- \
  --all-cases --repetitions 5 --workers 32 --alphabet ascii \
  --open-samples 7 \
  --out docs/ai/generated/catalog-led-lazy-storage/final/ascii \
  --storage-root target/storage-benchmark-final/ascii
```

The completed run contains the full canonical 150-query suite. The reduced acceptance
scope below uses only Q001, Q119 and metadata `RecordId` query Q034.

## Build, archive and save

| Operation | Single sample |
| --- | ---: |
| Native `Index::build` | 832.574 ms |
| Explicit in-memory archive | 311.232 ms |
| First synchronized `archive_to` | 2,663.628 ms |

The first save wrote 434,041,128 bytes across 1,077 new objects and one reused object,
with four synchronization calls. The stored dependency graph is 516,659,480 bytes:
84,998,904 bytes of metadata objects and 431,660,576 bytes of payload packs.

## Eager open

Seven search-ready opens were measured.

| Statistic | Time | Metadata reads | Metadata bytes |
| --- | ---: | ---: | ---: |
| Median | 1,417.951 ms | 39 | 367,452,992 |
| p95 | 1,442.062 ms | 39 | 367,452,992 |
| Maximum | 1,442.062 ms | 39 | 367,452,992 |

Every sample restored 148,486 file rows, 148,486 record rows, 1,256 index descriptors
and 238,150 leaf rows. Eager open reported zero source reads, zero source parses and
zero lazy payload reads.

## Native versus archived state

`resident_archived` means explicit in-memory archive. `reopened_first` means the first
execution after storage open with an empty application payload cache. Result equalities
include count, returned records and the ordered hit sequence represented by the result
digest.

| Query | State | Time | Count | Returned | Reads added after open | Ordered digest |
| --- | --- | ---: | ---: | ---: | ---: | --- |
| Q001 `Almanach` | native | 6.765 ms | 3,269 | 50 | 0 | `4324ae91...5756` |
| Q001 `Almanach` | resident archived | 44.540 ms | 3,269 | 50 | 0 | `4324ae91...5756` |
| Q001 `Almanach` | reopened first | 12.784 ms | 3,269 | 50 | 214 | `4324ae91...5756` |
| Q119 | native | 5.260 ms | 133,948 | 50 | 0 | `41bc8798...1a5d` |
| Q119 | resident archived | 5.449 ms | 133,948 | 50 | 0 | `41bc8798...1a5d` |
| Q119 | reopened first | 5.437 ms | 133,948 | 50 | 6,000 | `41bc8798...1a5d` |
| Q034 `@record.id = "d7kbujq7399g22j"` | native | 14.990 us | 1 | 1 | 0 | `3e8e284b...763c` |
| Q034 `@record.id = "d7kbujq7399g22j"` | resident archived | 16.916 us | 1 | 1 | 0 | `3e8e284b...763c` |
| Q034 `@record.id = "d7kbujq7399g22j"` | reopened first | 72.534 us | 1 | 1 | 1,072 | `3e8e284b...763c` |

No comparable storage-backend baseline was available for this revision. The values
above are absolute timings; this report makes no storage speedup claim.

## Memory checkpoints

| Checkpoint | RSS | Process peak RSS |
| --- | ---: | ---: |
| After native build | 745,848,832 B | 757,260,288 B |
| After explicit archive | 977,776,640 B | 979,603,456 B |
| After first save | 1,419,239,424 B | 2,032,672,768 B |
| After eager open | 2,034,503,680 B | 2,348,408,832 B |

These are single-process checkpoints. The process peak includes earlier native and save
allocations and is not an open-only allocation measurement.

## Raw evidence

- `final/ascii/construction-samples.tsv`
- `final/ascii/save-samples.tsv`
- `final/ascii/open-samples.tsv`
- `final/ascii/query-samples.tsv`
- `final/ascii/reduced-scope-query-results.tsv`
- `final/ascii/cache-sequence.tsv`
- `final/ascii/storage-io-counters.tsv`
- `final/ascii/memory-summary.tsv`
- `final/ascii/object-size-summary.tsv`

The root `construction-*.tsv`, `save-*.tsv`, `open-*.tsv`, `query-*.tsv`,
`cache-sequence.tsv`, `storage-io-counters.tsv`, `memory-summary.tsv` and
`object-size-summary.tsv` are generated from these samples by
`generate_summaries.py`.

## Scope limitation

Per the user's reduced scope, worker-count and alphabet matrices, mutation/compaction
benchmark matrices and repeated cold/warm series were not run. Correctness coverage for
those behaviors remains the already-passed test suite recorded in `test-matrix.tsv`.
