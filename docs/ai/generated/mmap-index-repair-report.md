# mmap-index-v1 repair status

Date: 2026-11-15  
Branch: `integration/mmap-index-v1`  
Reviewed commit: `81ba4ce200`  
Original baseline: `c54486f893`  
Repair head: `23ae6b0384`

## Result

All seven review findings now have an implemented repair path and executable
coverage. The publication path is packed-only: fresh builds, direct builds,
incremental builds, add/update/move/delete, verified replacement, and
compaction all seal completed runs into the canonical packed representation
before the publication becomes searchable. Load opens the packed catalogue and
run objects without rebuilding complete file tables, dictionaries, posting
arenas, or per-record cache cells.

## R1 — visibility persistence: resolved

- Added `ObjectKind::Visibility` and `SectionKind::Visibility`; object and
  manifest format versions are 4.
- Every manifest run carries `RunRef.visibility`, a content-addressed packed
  record set bound to that run's slot universe.
- Save persists the pinned publication's current active mask even when the
  immutable run object is reused.
- Load attaches the visibility reader directly. Catalogue coverage is not used
  as a substitute for visibility, and the active bitmap is materialized on the
  first query rather than scanned during Load.
- Cleanup retains visibility objects and rejects stores without them.

Regression:

```text
cargo test -p objekte-search-core index_storage::index_tests::packed_reuse_preserves
test packed_reuse_preserves_deleted_and_replaced_visibility ... ok
```

## R2 — bounded, fallible page access: resolved

- Posting and bigram reads use checked descriptors and 8 KiB integrity units.
- `RecordSetView` uses paged backing rather than borrowing complete arenas.
- Estimates read stored cardinalities, dictionary keys, and record-set
  descriptors without decoding posting payloads.
- `RecordStore::try_get` is fallible; packed row access never panics for a
  missing row, part, file, or record class.
- Corrupt payload units fail at the operation that touches them, while
  unrelated admissions and lookups continue to work.

## R3 — resident packed publication: resolved

- `lifecycle_compaction::seal_native_segments` seals every native shard before
  `publish_prepared_inner` installs a publication.
- Direct builds seal their staged shards before `from_published`.
- `partition_base` emits resident packed parts rather than native parts.
- Packed execution parity covers derived existence/null/array/object/length
  predicates, declared array steps, relation source fields, regex gram
  candidates, and verified slot-preserving replacement.
- `IndexCatalog::segment` and `BigramAccess` use the segment's packed reader.
- Save no longer constructs dictionaries, record sets, records, or sort
  indexes as its normal path.

Regression:

```text
cargo test -p objekte-search-core index_storage::index_tests::every_published_mutation_family
test every_published_mutation_family_uses_resident_packed_runs ... ok
```

The test inspects the publication after build, update, move, add, and delete and
asserts every run has resident packed backing, no native record vector, and no
native posting dictionary.

## R4 — packed catalogue and bounded record views: resolved

- Added `SectionKind::ActiveFiles`, a persisted active-file bitmap, so Load does
  not reconstruct the active set by reading every catalogue row.
- `FileCatalog` can now use a packed catalogue base. `restore_from_store` opens
  the catalogue object directly; it no longer calls `catalogue_entries`,
  builds a complete `FileInventory`, or rebuilds path maps.
- Path lookup and directory-prefix lookup use the persisted path dictionary.
- Run construction decodes only the file rows referenced by that run. File
  ownership uses compact segment-slot ranges rather than one `RecordSlot` per
  record, and per-run `file_index` HashMaps are not rebuilt at Load.
- Packed record access allocates page cells only when a row is first read; one
  access decodes at most one 64-record page. Load allocates zero record cells.
- Default identity order is read through the packed identity-order section on
  demand instead of collecting a complete slot array at Load.
- Desktop and TUI catalogue startup do not materialize complete file lists.
  Desktop requests 256-file and 256-directory pages and has explicit "load
  more" continuation; TUI file access resolves one active ID at a time.
- Added `SectionKind::Directories`, a persisted directory/collection-membership
  index. `catalogue_directory_page` and `catalogue_page` return bounded pages
  without decoding file rows or unrelated directories.
- Added `SectionKind::CollectionFiles`, a per-collection active file-ID index.
  Collection filtering no longer scans file rows.
- A full mutation overlay is merged into fresh resident packed catalogue
  metadata after 4,096 entries. The merge is explicit in-memory maintenance and
  does not Save or enumerate source files.
- File versions and active IDs are read through the catalogue rather than a
  reconstructed row vector.

Regression:

```text
cargo test -p objekte-search-core load_defers_packed
test load_defers_packed_record_page_allocation_until_a_row_is_read ... ok
```

## R5 — durable object reuse: resolved

- `ObjectInput` distinguishes resident bytes from durable object references.
- Save reuses mapped run and visibility objects without copying or hashing
  their payloads.
- Cross-store transfer streams in bounded chunks and verifies the digest.
- Retained sealing digests avoid rehashing complete resident objects.
- Save planning and transfer observe cancellation.

Regression:

```text
cargo test -p objekte-search-core index_storage::index_tests::same_store_save_reuses
test same_store_save_reuses_mapped_run_and_visibility_objects ... ok
```

## R6 — streaming tiered compaction: resolved

- Run levels persist in the manifest and survive Save/Load.
- Four oldest runs at the same level are merged and promoted one level.
- `compact_packed_merge` performs one k-way pass over selected input
  dictionaries, records, bigrams, sorts, relations, coverage, and metadata.
  Output is one packed run with bounded pages; 2,048-slot output fragmentation
  no longer rescans the complete input.
- The 64-run admission bound is enforced for every collection before a mutation
  publishes another run. Full base partitions no longer receive an extra
  allowance, and consolidation loops until the bound holds.
- Cancellation is checked between merge inputs and while walking large value,
  length, gram, and relation postings.
- Record slot translation is built once per selected merge.
- Fully active runs participate, so fan-out is bounded without tombstones.
- Compaction remains source-free and reuses the existing compare-and-swap
  publication path.

Regression:

```text
cargo test -p objekte-search-core four_same_level_runs
test four_same_level_runs_consolidate_and_promote ... ok
```

The test creates four level-0 runs, compacts, asserts a promoted run and a
reduced run count, and verifies the complete result set.

## R7 — typed error propagation: resolved

- Packed planner estimates, exact execution, length predicates, derived state
  predicates, and bigram candidate/estimate paths return `StorageResult`.
- Catalogue row and directory-page access has fallible `try_*` entry points;
  corrupt packed catalogue pages surface as operation errors.
- Load admits immutable object headers and directories without validating
  unrelated payloads. A corrupted, unread arena is reported only by the
  operation that reads it (`corrupt_unused_arena_is_reported_only_when_read`).
- Sorting-column eligibility uses a fallible packed presence/null accessor.
- `index_catalog` and executor convert storage errors at the planning boundary
  instead of erasing them with `ok()` or empty candidate sets.
- Packed row access returns typed failures; no packed record path contains a
  persisted-row `expect`.
- Record-set, posting, dictionary, sort, relation, and catalogue readers return
  storage errors with object/section context.
- Frontend and CLI consumers receive operation errors without discarding the
  previous usable publication.

## Updated contracts

- Object format: 4.
- Manifest format: 4.
- Segment statistics: 2 (adds `max_record_range_len` for bounded read
  scheduling).
- Catalogue section: `ActiveFiles` (25).
- Older experimental stores without visibility, active-file, or version-2
  segment statistics are rejected explicitly. No implicit source rebuild is
  attempted.

## Validation

```text
cargo fmt --all
python3 scripts/format-rust-spacing.py
python3 scripts/format-rust-spacing.py --check   # passed
cargo clippy --workspace --all-targets --offline  # exit 0, warning-level diagnostics only
cargo test --workspace --locked --offline
936 passed; 0 failed
```

Core-only suite:

```text
cargo test -p objekte-search-core --lib --offline
607 passed; 0 failed; 15 ignored
```

Desktop suite:

```text
cargo test -p objekte-desktop --offline
166 passed; 0 failed
```

## Benchmarks and remaining verification

A release-mode Musenalm quick query checkpoint was run at `6d956c70ea`:

```text
python3 bench/run_query_regressions.py quick   --comparison repair   --baseline bench/results/musenalm/controlled-foundation-performance-92cdb92b19   --result-dir bench/results/musenalm/repair-final-quick-6d956c70ea
```

The aggregate ratio against the reviewed foundation was `0.424` (current/foundation)
across 150 ASCII uncached samples. All result counts agree with the independent
`jq` oracle, including Q140, Q147, Q149, and Q150 where the old foundation
baseline itself disagreed with the oracle. Q085 (`language:each = "ger"`) now
measures 4.682 ms against a 4.844 ms baseline (-3.3%). The quick gate's three
remaining diagnostics are baseline metadata incompatibilities: corpus
fingerprint, a missing baseline `schema-manifest.json`, and semantic
registration metadata. No query count or individual timing fails.

Fresh in-memory build wall times at the same revision were 0.924 s (ASCII) and
0.998 s (Unicode), one process each. The repository build-regression script
could not compare these against the available foundation `gate.json` because
that historical file uses the older `ascii-disabled` summary schema.

Raw benchmark artifacts are under
`bench/results/musenalm/repair-final-quick-6d956c70ea/` and
`bench/results/musenalm/repair-final-build-a2bf950f78/`. The full 3,000-sample
gate and a same-schema build baseline were not available in this session. The
earlier GUI validation evidence remains under
`docs/ai/generated/mmap-index-storage-gui/`.

## Commits

- `163a8e9afd` — publish every resident run through packed readers.
- `141c6627c6` — merge packed runs by tier and open packed catalogue pages.
- `8a08af99d8` — page packed metadata and bound packed record caches.
- `a2bf950f78` — read default identity order through packed pages.
- `73b5f0338f` — repair report and validation record.
- `430d54a8c0` — packed file ranges, persisted directory index, bounded
  overlay merge, and strict run admission.
- `1575b9c675` — deferred run-visibility and catalogue-page materialization.
- `6d956c70ea` — descriptor-only packed estimates and format version 3.
- `e4e1ca5d66` — packed collection-file membership index.
- `81b1995867` — collection index completion and corruption regression.
- `aa2019b28b` — desktop file-catalogue paging.
- `23ae6b0384` — desktop and TUI bounded metadata browsing.
