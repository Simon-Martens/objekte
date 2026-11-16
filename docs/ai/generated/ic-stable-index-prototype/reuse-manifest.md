# Stable-index prototype reuse manifest

Task: `ic-stable-index-prototype`  
Base commit: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`  
Integration branch: `integration/ic-stable-index-prototype`

## Source revisions inspected

| Source | Revision | Use in the prototype |
| --- | --- | --- |
| `master` | `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9` | Native `PostingIndex`, `BigramIndex`, `SortColumn`, `RelationIndex`, record/file inventory types, `ScalarKey`/`Number` comparison and exact sort comparison |
| `origin/abandoned/redb-primary-index` | `09445853ccbce1cdc690a6f1c62bfe765a72e774` | Reviewed metadata and exact scalar fixture ideas only |
| `origin/abandoned/rocksdb-bulk-construction-repair` | `bd8b29e90b3bc698e69dcdb1644e704f406b5c21` | Reviewed ordered key-codec ideas, validation/reporting structure and semantic fixtures |
| `origin/task/hybrid-index-storage/catalog-stable` | inspected | Confirmed no reusable `ic-stable-structures` catalog implementation existed at the pinned base |

## Reused responsibilities

The implementation reuses master types directly rather than copying donor backend code:

- `crate::posting::{PostingIndex, ScalarKey, StructuralKey}` for exact typed semantics,
  coverage flags and structural states.
- `crate::bigram::BigramIndex` for normal/skip gram keys and compressed postings.
- `crate::sorting::{SortColumn, SortKey, SortOptions}` for native collation and
  placement comparison.
- `crate::relation::{RelationIndex, RelationTarget}` for occurrence multiplicity,
  dangling targets and relation states.
- `crate::inventory::{FileEntry, FileInventory, RecordEntry}` and
  `crate::model::{CollectionId, FileId, RecordSlot, RecordId}` for identity and
  metadata semantics.
- `crate::repository::{Repository, CollectionState}` as the native oracle;
- `crate::discovery`, `crate::build` batch extraction and
  `CollectionAdmission` accumulation for the direct source-build path, without
  constructing a production `Repository` in the benchmark.

No donor admission coordinator, transaction layer, write receipt, database
environment, namespace dispatcher, reverse-gram subsystem or database key layout
was copied. The new module has no redb or RocksDB dependency and no persistence
during construction.

## Deliberately excluded donor mechanisms

- TransactionDB/redb/rocksdb environments and writers.
- Global admission coordinators and write receipts.
- Per-record reverse gram retention.
- Contribution compression or LZ4.
- One global tree containing every field and metadata namespace.
- Database-backed pending-file rows.

## New code ownership

All new storage, envelope, manifest, snapshot, mutation and query-primitive code
is in `crates/core/src/stable_index/` and the feature-gated benchmark in
`crates/core/examples/bench_ic_stable_index.rs`. The production CLI, desktop
application and default native backend paths are unchanged.
