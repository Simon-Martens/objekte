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
| Warm-query overhead | Partial | The final smoke run adds no index reads after warm-up for Q001/Q119. The complete 150-query/warm-native comparison matrix was not run. |
| Cold-query latency | Pass in the bounded smoke | Q001 first selected storage query was 14.36 ms and Q119 was 6.97 ms after a 1.29 s search-ready open. This is not a full cold-query matrix. |
| Memory | Measured limitation | Eager open reached approximately 1.79 GiB RSS and 2.18 GiB peak in the smoke run. The 64-byte payload-budget test proves eviction/reload behavior, but eager metadata dominates this corpus. |
| Disk amplification | Measured, partial | The smoke snapshot used 516.66 MB referenced storage: 85.00 MB metadata and 431.66 MB payload packs. Deletion-only and unchanged-object reuse pass; the full object-amplification matrix was not run. |

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

The authorized final smoke command and all raw samples are recorded in
`storage-smoke-benchmark.md`. Summary files are committed beside the raw TSV samples.

Key results:

- Completed native build: 805.91 ms.
- Explicit memory-only archive: 314.55 ms.
- First synchronized save: 2.76 s; 1,077 new objects and 1 object reused.
- Search-ready open: 1.29 s and 1.40 s for two samples, with 39 metadata reads,
  zero lazy payload reads and zero source reads/parses.
- Q001: native 7.09 ms, reopened first 14.36 ms, warm median 9.10 ms, count 3,269.
- Q119: native 6.19 ms, reopened first 6.97 ms, warm median 6.81 ms, count 133,948.
- Every storage result digest matched its native result.
- Offline audit verified all 1,077 reachable objects with no error.

## Approved deviations and limitations

- The user authorized the smaller Q001/Q119 smoke benchmark. The full 150-query,
  ASCII/Unicode, worker, mutation, compaction and seven-process acceptance matrices
  are unmeasured for this task revision.
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
