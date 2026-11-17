# Catalog-led lazy storage implementation report

Date: 2026-11-17

Task: `catalog-led-lazy-storage`

Integration branch: `integration/catalog-led-lazy-storage`

Final implementation revision: `ee52c8239c56fbe82b7c67a0e9b368dc4d9b8d8d`

Merged base: `15f45465ed0b7455577fce5431de50dfeea1ee16`

Worktree: `/home/simon/source/objekte-integration-catalog-led-lazy-storage`

## Verdicts

| Area | Verdict | Evidence and boundary |
| --- | --- | --- |
| Correctness | Pass for implemented behavior | Native, reopened, resident-archived, mixed, mutation, compaction, relation, FTS and sort semantic comparisons pass. The full workspace suite passes. |
| Eagerness/laziness | Partial | Eager open has zero lazy payload/source reads. Posting, FTS, sort and base relation payloads are selected lazily. Presence/null state remains eager, segment-local relation metadata is coarse, and snapshot decode still uses JSON inside checked rkyv frames. |
| Durability | Pass for the supported prototype environment | Dependency ordering, file/directory synchronization, failure stages, `CommitUncertain`, two-writer fencing and child-process abort barriers are tested on local Linux. No physical power-loss claim is made. |
| Lifecycle/mutations | Pass for exercised cases | Add, replace, delete, move, multi-generation save/reopen, mixed native/cold state, source-free full compaction and a deterministic 128-step sequence pass. |
| Native construction | Pass structurally | Native build and memory-only archive create no storage files and perform no implicit storage serialization. A full paired P/S construction benchmark was not run. |
| Warm-query overhead | Partial | The completed representative run contains warm samples, but the reduced acceptance scope focuses on first archived execution and provides no comparable storage baseline for a speedup verdict. |
| Cold-query latency | Pass for the reduced scope | Q001 first archived execution was 12.784 ms, Q119 was 5.437 ms and metadata Q034 was 72.534 us after eager open, with identical ordered results. |
| Memory | Measured limitation | Eager open reached 2.035 GiB RSS with a 2.348 GiB process peak after the completed run. The cache-budget tests prove eviction/reload behavior, but eager metadata remains a measured memory floor. |
| Disk amplification | Measured, partial | The representative snapshot used 516.66 MB referenced storage: 85.00 MB metadata and 431.66 MB payload packs. Deletion-only and unchanged-object reuse pass; no broader amplification matrix was authorized. |

## Implementation

The storage backend now implements:

- Explicit `archive_to`, `open_storage`, status, cache trim, audit and prune APIs
  without changing ordinary native build or memory-only archive behavior.
- A fixed-envelope object store with content-addressed metadata/payload objects,
  `CURRENT`, writer/readers locks, durable `CURRENT` replacement and commit-uncertain
  classification.
- Separate checked rkyv file, record, index and leaf catalogs validated before a
  searchable root is published. The leaf catalog records pack identity, range,
  cardinality, decoded-size estimate and SHA-256 for independent Roaring leaves.
- Per-field, inferred and custom posting dictionaries with independently selected
  value, length and structural Roaring leaves.
- Per-binding normal/skip FTS dictionaries with independent gram leaves, bounded
  coalescing, cardinality ordering, early empty-intersection termination and checked
  decode.
- Independent sort key/order frames per configured column and composite order.
- Lazy base relation payload loading with source-free relation/backlink round trips.
- Query-level cache pins, race-safe trim enforcement, post-unpin budget enforcement,
  weak stored-leaf ownership, oversize bypass, decoded cache accounting and a bounded
  64-entry pack-handle cache.
- Source-free reopen and compaction, mutation persistence, unchanged-object reuse,
  deletion-only payload reuse, cold export and offline whole-object pruning.
- Deterministic fault injection for object write/sync/install and `CURRENT`
  write/sync/rename, a sync trace and a child-process abort barrier.

The component-by-component classification is in
`component-coverage.md`. S01-S80 evidence is in `test-matrix.tsv`.

## Validation

Commands completed at the final code revision:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo test --release -p objekte-search-core --test storage_roundtrip
```

Outcomes:

- Formatting: passed.
- Workspace all-target check: passed.
- Workspace tests: passed. Core: 617 passed, 17 ignored. CLI, desktop, integration,
  process and doc tests also passed.
- Release storage round trips: 59 passed.
- The cache-pin unit test and the finite-budget posting-leaf reload test passed.
- An initial workspace run hit one timing-sensitive archive-cancellation test under
  full parallel load. The test passed immediately in isolation and in the final full
  workspace rerun; no functional failure remained.

## Benchmark evidence

The final user-authorized reduced-scope benchmark is recorded in
`reduced-scope-benchmark.md`. It reuses the completed ASCII Musenalm run with the normal
32-worker configuration and does not restart the benchmark campaign.

- Native build: 832.574 ms.
- Explicit memory-only archive: 311.232 ms.
- First synchronized save: 2.664 s; 1,077 new objects and one reused object.
- Seven eager opens: 1.418 s median, 39 metadata reads, zero lazy/source reads.
- Q001: native 6.765 ms; resident archived 44.540 ms; reopened first 12.784 ms.
- Q119: native 5.260 ms; resident archived 5.449 ms; reopened first 5.437 ms.
- Q034 (`@record.id = "d7kbujq7399g22j"`): native 14.990 us; resident archived
  16.916 us; reopened first 72.534 us.
- All three queries retained identical counts and ordered-result digests across native,
  resident-archived and reopened states.
- Offline audit verified all 1,077 reachable objects with no error.

No comparable storage-backend baseline was available for this revision, so the report
contains absolute timing values and makes no speedup claim. The existing full ASCII
150-query raw run is retained under `final/ascii`, but the reduced acceptance summary
only depends on the three required queries above.

## Approved deviations and limitations

- Per the user's final reduced scope, worker-count and alphabet matrices,
  mutation/compaction benchmark matrices and repeated cold/warm series were not run.
  Their correctness remains covered by the passing test suite.
- Presence/null/empty-selection masks are still eager. Independent query-state leaves
  are not implemented.
- Segment-local relation contributions remain coarser than independent endpoint
  payloads.
- Eager metadata is archived as four checked rkyv catalogs, but runtime snapshot
  detail is still JSON carried in bounded rkyv frames. Search-ready open therefore
  performs a substantial eager decode and its measured memory floor is high.
- FTS remains conservative and retains source verification for exact text, regex or
  evidence requirements.
- The supported persistence claim is Linux local-filesystem, 64-bit only. NFS,
  distributed filesystems and Windows crash durability are not claimed.
- The measurement uses an empty application payload cache; OS page-cache state was not
  controlled. No physical machine-crash experiment was performed.

These limitations are correctness-preserving implementation and measurement gaps.
They are not represented as passed performance or schema requirements.

## Integration state

All work is committed and pushed on the integration branch. The integration worktree
and branch are retained for user review. No merge into `master` was performed.
