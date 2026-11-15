# mmap-index-v1 repair status

Date: 2026-11-15  
Branch: `integration/mmap-index-v1`  
Reviewed commit: `81ba4ce200`  
Repair head: `1a052f0209`

## R1 — visibility persistence: resolved

- Added `ObjectKind::Visibility` and `SectionKind::Visibility`; bumped object
  format and manifest versions to 2.
- Each manifest run now carries a content-addressed visibility object encoded
  with the packed record-set representation.
- Save persists the pinned publication's current active mask even when the
  immutable segment object is reused.
- Load opens the visibility object and uses it for the segment's active domain;
  it no longer trusts the run's embedded coverage.
- Compaction and mutation publication retain their own active masks; pinned
  views keep their old visibility.
- Cleanup retains visibility objects and rejects stores without them.

Regression test:

```text
cargo test -p objekte-search-core index_storage::index_tests::packed_reuse_preserves
test packed_reuse_preserves_deleted_and_replaced_visibility ... ok
```

The test deletes one file and replaces another, saves without compaction,
reloads, and verifies the retired values are absent and replacement values are
present.

## R2 — bounded, fallible access: substantially implemented

- `RecordSetView` now supports a paged backing that reads only the block
  directory and requested block payloads. Posting and bigram query paths use
  paged views instead of borrowing whole arenas.
- Object checksum units changed from 64 KiB to 8 KiB, aligned with dictionary
  pages.
- `RecordStore::try_get` returns storage errors instead of panicking on packed
  row failure; `get` no longer contains `expect` calls for persisted rows.
- Remaining: some explicit diagnostic/verification paths still read full
  arenas, and not every query error is propagated as a distinct typed error.

## R3 — resident packed publication: not completed

Fresh builds and mutations still publish native query owners; Save is still the
point that creates packed bytes. A prototype that packed `partition_base` parts
resident was reverted because it exposed unresolved packed execution parity gaps
(array-element structural predicates, mixed-type ranges, relation membership,
and verified slot-preserving update). The branch therefore keeps the previous
native publication path until those gaps are repaired.

## R4 — packed catalogue and bounded record views: not completed

Load still reconstructs catalogue entries, file lookup maps, and
publication-level record metadata. `RecordStore` still allocates one lazy cell
per packed record. Frontend catalogue helpers still enumerate complete
catalogues. This remains the largest outstanding repair.

## R5 — durable object reuse: substantially implemented

- `ObjectInput` now distinguishes resident bytes from durable object
  references carrying identity, length, and path.
- Packed mapped segments expose durable save sources; Save reuses existing
  object paths without copying or hashing the complete payload.
- Cross-store saves stream durable objects in 1 MiB chunks and verify the
  content digest.
- Save manifest construction uses retained digests instead of rehashing
  complete resident objects.
- Regression test:

```text
cargo test -p objekte-search-core index_storage::index_tests::same_store_save_reuses
test same_store_save_reuses_mapped_run_and_visibility_objects ... ok
```

The test updates one file while another mapped run remains untouched and
asserts `objects_reused >= 2` and `bytes_reused > 0`.

## R6 — tiered streaming compaction: partial

- Run level is persisted in the manifest and restored into published segments.
- Compaction output promotes the source level instead of always writing level 0.
- The 64-run admission limit remains enforced.
- Not implemented: four-run same-level consolidation, a k-way dictionary merge,
  and once-per-merge translation structures. Current compaction remains bounded
  per 2,048-slot output part and repeats input field/bigram scans per part.

## R7 — error propagation: partial

- Packed row access is fallible (`try_get`) and no longer panics for missing
  rows, parts, files, or invalid record classes.
- Record-set and posting readers return typed storage errors.
- Remaining: several query/planner paths still convert storage errors into
  `None`/empty candidates, and catalogue/front-end handlers still need the
  fallible interface.

## Validation

```text
cargo test --workspace --locked --offline
931 passed; 0 failed
```

Required repairs R3, R4, the four-run merge in R6, and complete R7 propagation
remain before this task can be marked complete.
