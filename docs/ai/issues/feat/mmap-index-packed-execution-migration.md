# Packed readers are not yet the execution path

**Category:** feat  
**Originating task:** `mmap-index-v1`  
**Branch:** `integration/mmap-index-v1`  
**Locations:** `crates/core/src/index_storage/`, `crates/core/src/lifecycle.rs`, `crates/core/src/run.rs`

## Observed behavior

Saving an index writes immutable packed catalogue and segment objects. The
same objects can be reopened through `FrozenSegment`, `CatalogueView`,
`FieldPostingsView`, `BigramSectionView`, and sort/relation readers. The
`Index` load path also writes and reads a source-free native query-state object;
the existing query engine executes against that native representation.

## Expected behavior

The first-version specification asks published indexes to query packed readers
directly, without reconstructing native posting maps, bigram maps, sort vectors,
or relation targets during load.

## Impact

Persistence is complete and query semantics are preserved. Loaded publications
now attach their mapped `FrozenSegment` readers and `IndexCatalog`/executor
typed and bigram candidate access uses the packed `PostingAccess`/`BigramAccess`
views. A test removes the loaded native postings and source files and confirms
the packed typed query still answers.

Load now reconstructs the queryable repository from packed catalogue/run
objects without a native snapshot. Typed postings and raw/fallback/custom
bigrams stay packed: no native value or gram dictionaries are rebuilt.
`IndexCatalog`, posting-value enumeration, relation reverse probes, and bigram
candidates read mapped packed sections, and loaded re-saves reuse the original
packed run object bytes.

Sort columns/orders now retain packed descriptors and read keys/order slots
directly from mapped arrays (`SortColumn`/`SortOrder`). Relation adjacency now
retains packaged relation descriptors and reads edges/state probes from mapped
sections (`PackedRelation`, `RelationIndex`, `RelationContribution`).

Record rows and logical IDs are now lazy packed views: `RecordStore` materializes
individual `RecordEntry` rows on access from `RecordTableView`, and
`RecordIdLookup::Packed` resolves IDs through the packed logical-ID dictionary.
Load performs zero eager record-row or ID-map reconstruction. Warm/cold mmap
loads are measured; GUI automation remains blocked on a display server in this
environment.

## Why not resolved here

Migrating `IndexCatalog`, exact posting execution, bigram candidate execution,
posting-value enumeration, sorting, and relations requires changing shared
query interfaces across `run.rs`, `executor.rs`, `reference.rs`, and
`index_catalog.rs`. That is a separate, large interface migration.

## Possible approaches

1. Introduce a `PostingAccess` trait over resident/packed views and migrate
   query planning and execution one family at a time.
2. Make `IndexCatalog` return borrowed packed views for typed and bigram
   accesses, retaining native maps only for the current publication.
3. Add differential tests that execute every query family against both the
   native and packed repositories before switching the default.
