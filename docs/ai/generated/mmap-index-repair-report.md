# mmap-index-v1 repair status

Date: 2026-11-15  
Branch: `integration/mmap-index-v1`  
Reviewed commit: `81ba4ce200`  
Original baseline: `c54486f893`  
Repair head: `73b5f0338f`

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
  manifest format versions are 2.
- Every manifest run carries `RunRef.visibility`, a content-addressed packed
  record set bound to that run's slot universe.
- Save persists the pinned publication's current active mask even when the
  immutable run object is reused.
- Load attaches the visibility reader directly. Catalogue coverage is not used
  as a substitute for visibility.
- Cleanup retains visibility objects and rejects stores without them.

Regression:

```text
cargo test -p objekte-search-core index_storage::index_tests::packed_reuse_preserves
test packed_reuse_preserves_deleted_and_replaced_visibility ... ok
```

## R2 — bounded, fallible page access: resolved

- Posting and bigram reads use checked descriptors and 8 KiB integrity units.
- `RecordSetView` uses paged backing rather than borrowing complete arenas.
- Estimates read stored cardinalities and bounded dictionaries, not full
  posting payloads.
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
- Run construction decodes only the file rows referenced by that run.
- Packed record access allocates page cells only when a row is first read; one
  access decodes at most one 64-record page. Load allocates zero record cells.
- Default identity order is read through the packed identity-order section on
  demand instead of collecting a complete slot array at Load.
- Desktop catalogue startup materializes active file IDs only. Collection and
  directory filtering reads rows/path prefixes on demand, with a 256-ID visible
  page bound.
- File versions and active IDs are read through the catalogue rather than a
  reconstructed row vector.

Regression:

```text
cargo test -p objekte-search-core load_defers_packed
test load_defers_packed_record_page_allocation_until_a_row_is_read ... ok
```

Remaining limitation: the desktop directory tree still derives directory names
from the publication paths when the tree is refreshed. It does not decode file
rows or diagnostics, but a full directory list is an explicit enumeration rather
than a persisted child-directory page. No correctness path depends on it.

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
- `index_catalog` and executor convert storage errors at the planning boundary
  instead of erasing them with `ok()` or empty candidate sets.
- Packed row access returns typed failures; no packed record path contains a
  persisted-row `expect`.
- Record-set, posting, dictionary, sort, relation, and catalogue readers return
  storage errors with object/section context.
- Frontend and CLI consumers receive operation errors without discarding the
  previous usable publication.

## Updated contracts

- Object format: 2.
- Manifest format: 2.
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
934 passed; 0 failed
```

Core-only suite:

```text
cargo test -p objekte-search-core --lib --offline
605 passed; 0 failed; 15 ignored
```

Desktop suite:

```text
cargo test -p objekte-desktop --offline
166 passed; 0 failed
```

## Benchmarks and remaining verification

A release-mode Musenalm quick query checkpoint was run at `73b5f0338f`:

```text
python3 bench/run_query_regressions.py quick   --comparison repair   --baseline bench/results/musenalm/controlled-foundation-performance-92cdb92b19   --result-dir bench/results/musenalm/repair-final-quick-a2bf950f78
```

The aggregate ratio against the reviewed foundation was `0.685` (current/foundation)
across 150 ASCII uncached samples. All result counts agree with the independent
`jq` oracle, including Q140, Q147, Q149, and Q150 where the old foundation
baseline itself disagreed with the oracle. One individual case, Q085
(`language:each = "ger"`), measured 10.980 ms against a 4.844 ms baseline
(+6.136 ms, +126.7%). Candidate and parsed-record counts are identical
(4,928/4,928); the absolute regression is packed page decode overhead on a
universal array predicate. The quick gate exits non-zero because the foundation
baseline schema manifest is missing and because of Q085.

Fresh in-memory build wall times at the same revision were 0.924 s (ASCII) and
0.998 s (Unicode), one process each. The repository build-regression script
could not compare these against the available foundation `gate.json` because
that historical file uses the older `ascii-disabled` summary schema.

Raw benchmark artifacts are under
`bench/results/musenalm/repair-final-quick-a2bf950f78/` and
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
