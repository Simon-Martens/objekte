# Plan 4 of 5: the index on disk in a redb store

- Task identifier: `redb-index-store`
- Status: specification. Do not start until the user assigns this task.
- Depends on: plans 1, 2 and 3 merged into `master`.
- Audited base: `master` at `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9` plus plans 1 to 3. Line numbers refer to `c54486f893`; search for the quoted identifier if a line has moved.
- Next plan: `main-shard-vacuum` (plan 5).

## 0. Rules for the implementing agent

1. Read `AGENTS.md` and `docs/ai/README.md` and follow that workflow: branch `integration/redb-index-store`, worktree `../objekte-integration-redb-index-store`, memory `docs/ai/memory/redb-index-store/`.
2. This plan authorises exactly one new dependency: `redb` (licence MIT OR Apache-2.0), version `4`, in `crates/core`. `bincode 1.3` is already a dependency.
3. The repository once had index persistence and removed it on purpose (`docs/ai/done/restore-native-btrees-memory-only.md`). This plan must not bring any of that back by accident:
   - `Index::build`, `Index::build_with_sources`, `Index::start_incremental_build` and everything the CLI, TUI and desktop app call today must never read, create or modify a store file. The existing tests that prove this (`opens_folder_without_index_artifacts`, `manual_close_does_not_write_index_artifacts`, `source_open_ignores_existing_index_artifacts`, `generated_index_namespaces_are_excluded_from_source_discovery`, `removed_storage_flags_are_unsupported_arguments`) must keep passing unchanged.
   - Do not touch `crates/cli` or `crates/desktop`. Wiring the store into a frontend is a separate, later task.
4. The index is derived data. Source files are the truth. Whenever the store is missing, locked, damaged, or made for another configuration, the correct reaction is: build from source and write a new store. A store problem is never a reason to fail `open_persistent`, and never a reason to return wrong results.
5. Never make a test pass by deleting an assertion, loosening an expected value, adding `#[ignore]`, or comparing the implementation with itself.
6. No lint suppressions. Formatting: `rustfmt` and `python3 scripts/format-rust-spacing.py` on files you create; only changed lines elsewhere.
7. Stop and write a blocker if a step needs a design decision that section 3 does not make.

## 1. Outcome

1. `Index::open_persistent(root, definitions, options, sources, store_options)` exists. If a valid store for this configuration is present, the index is loaded from it without parsing any source file. Otherwise the index is built from source as today and a new store is written.
2. `Index::checkpoint()` writes everything that changed since the last checkpoint in one atomic redb transaction:
   - a shard that was never written is written completely, once;
   - a shard that no longer exists is deleted;
   - the main shard of a collection is written incrementally: only the posting and gram chunks that changed, and only the new rows of its tables.
3. After a load, the normal metadata reconciliation runs and picks up every source change that the store does not know about, including changes made after the last checkpoint of a previous session.
4. Queries on a loaded index return exactly what a freshly built index returns.

### What this plan is and is not

The store holds the index as copy-on-write B+-trees on disk and updates them in place, page by page. RAM still holds a complete decoded copy; queries never read the file. Loading therefore costs time proportional to the index size. Reading postings straight from the file on demand is a possible later step and is out of scope. The `(key, chunk)` layout of plan 2 was chosen so that step stays possible.

### Non-goals

- No frontend changes, no automatic checkpoints, no checkpoint on shutdown.
- No demand paging, no memory mapping of index data into query structures.
- No migration of old snapshot files. Files from the removed persistence code are ignored.
- No compatibility promise between store format versions. A version mismatch means rebuild.

## 2. Background (verified at the audited base)

### 2.1 What a published index consists of

`PublishedIndex` (`lifecycle.rs:419`): `instance_id`, `revision`, `repository` (static base; empty in a live index), `configuration_identity: [u8; 32]`, `active` (base masks; all empty), `segments` (the shards), `next_segment_ordinal`, `last_size_maintenance_revision`, `catalog: FileCatalog`, `file_versions: FileVersions`, `reconciliation`, `processed_files`, `processed_memberships`, and, after plan 3, `fold_trigger`.

### 2.2 Existing helpers this plan reuses

| Helper | Location | Use |
| --- | --- | --- |
| `configuration::configuration_hash(&definitions, options, &sources)` | `configuration.rs:38` | the 32-byte identity that decides whether a store fits |
| `repository_configuration::prepare_source_definitions(&root, &definitions)` | called at `lifecycle_open.rs:34` | must run before hashing, as every build does |
| `Repository::build_fresh(root, definitions, options, &sources, false)` | `repository.rs:1248` (private) | builds an empty static base without touching source files |
| `FileCatalog::snapshot_slots()` / `FileCatalog::restored(base, slots)` | `lifecycle_catalog.rs:95`, `:110` | catalogue to rows and back; `restored` sets `construction_complete` |
| `FileVersions::initial(allocated_files)` | used at `lifecycle_open.rs:93` | versions for a freshly opened instance |
| `Index::from_direct_build` | `lifecycle_open.rs:74` | the model for assembling a `PublishedIndex` from an empty base, catalogue rows and shards |
| `Index::start_reconcile()` | `lifecycle.rs:1656` | compares size and modification time of every catalogued file with the file system and applies changes |
| Generated directories: any directory named `.objekte` or `objekte-indexes` | `discovery.rs:67-79` | source discovery never descends into them, so a store below `<root>/.objekte/` is never read as source data |

### 2.3 Serialisation support that already exists

`serde` derives exist on `FileEntry`, `SourceRevision`, `RecordEntry`, `RecordSlot`, `ScalarKey`, `SortKey`, `SortOptions`, `InferenceReport`, `InferredFieldReport`, `PendingReference`. `Number` and `ExactDecimal` implement `Serialize` by derive and `Deserialize` by hand. Bitmaps use `RoaringBitmap::serialize_into` / `deserialize_from`.

Not yet serialisable (add plain `#[derive(Serialize, Deserialize)]` where the plan needs it; do not change any field): `SegmentFile`, `RelationContribution`'s header data, `GlobalInferenceSummary` and what it contains, `CompositeSortPart`, and any type the compiler names while you implement section 3, D6.

### 2.4 redb facts checked against version 4.6.0

```rust
use redb::{Database, Durability, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};

const ROWS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("rows");

let database = Database::create(path)?;                 // or Database::open(path)
let database = Database::builder()
    .create_with_backend(redb::backends::InMemoryBackend::new())?; // tests
let mut write = database.begin_write()?;
write.set_durability(Durability::None)?;                // or Durability::Immediate
{
    let mut rows = write.open_table(ROWS)?;
    rows.insert(key_bytes.as_slice(), value_bytes.as_slice())?;
    rows.remove(key_bytes.as_slice())?;
    rows.retain_in::<&[u8], _>(start.as_slice()..end.as_slice(), |_, _| false)?; // delete a key range
}
write.commit()?;                                        // atomic
let read = database.begin_read()?;                      // consistent snapshot
let rows = read.open_table(ROWS)?;
for row in rows.range::<&[u8]>(start.as_slice()..end.as_slice())? {
    let (key, value) = row?;
    let (key, value): (&[u8], &[u8]) = (key.value(), value.value());
}
```

- `&[u8]` keys sort by plain byte comparison.
- A second `Database::create`/`open` of a file that is already open fails with `DatabaseError::DatabaseAlreadyOpen`.
- A read transaction keeps seeing the data as of its start while later write transactions commit.
- Values of 8 KiB and more are fine.

A scratch measurement (300,000 entries with tiny values, one thread): one bulk write transaction 280 ms; reopening, scanning, decoding and rebuilding an in-memory map 110 ms; an empty durable commit 25 ms.

## 3. Decisions already made (do not reopen)

**D1. Modules.** New private modules in `crates/core/src/`:

| File | Content |
| --- | --- |
| `index_store.rs` | `IndexStore` (open, validate, checkpoint, load), `StoreOptions`, reports |
| `index_store_keys.rs` | table definitions and key encoders/decoders |
| `index_store_shard.rs` | writing, diffing and reading one shard |
| matching `.test.rs` files | tests |

**D2. Public API.** Added to `impl Index` in a new file `lifecycle_store.rs`:

```rust
/// Where and how the index store is kept.
#[derive(Clone, Debug)]
pub struct StoreOptions {
    /// `None` means `<root>/.objekte/.index/index.redb`.
    pub path: Option<PathBuf>,
    /// `false` skips fsync on commit (tests and benchmarks).
    pub durable: bool,
}

/// What `open_persistent` did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreOpenOutcome {
    /// Loaded from the store; no source file was parsed.
    Loaded,
    /// Built from source and wrote a new store. The reason says why.
    Rebuilt(StoreRebuildReason),
    /// Built from source; the store could not be used at all (for example it
    /// is locked by another process). `checkpoint` will return an error.
    WithoutStore(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreRebuildReason {
    Missing,
    FormatVersion,
    Configuration,
    Damaged(String),
}

/// Counters of one checkpoint.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CheckpointReport {
    pub revision: u64,
    pub shards_written: u64,
    pub shards_deleted: u64,
    pub main_shards_updated: u64,
    pub chunks_written: u64,
    pub chunks_deleted: u64,
    pub rows_written: u64,
    pub bytes_written: u64,
    pub duration: Duration,
}

impl Index {
    pub fn open_persistent(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        sources: SourceRegistry,
        store: StoreOptions,
    ) -> Result<(Index, StoreOpenOutcome), IndexError>;

    /// Write everything that changed since the last checkpoint.
    pub fn checkpoint(&self) -> Result<CheckpointReport, IndexError>;

    /// True when this index was opened with a usable store.
    pub fn has_store(&self) -> bool;
}
```

Store failures are reported as `IndexError::Io(format!("index store: {error}"))`. Do not add an `IndexError` variant (the frontends match on that enum). `checkpoint` on an index without a store returns that error with the text `"index store: no store is attached"`.

`IndexInner` (`lifecycle.rs:1055`) gets one field: `store: Mutex<Option<IndexStore>>`. It is `None` for every index not created by `open_persistent`. Holding this mutex serialises checkpoints.

**D3. File location and locking.** Default path `<root>/.objekte/.index/index.redb`; create the directory if needed. Do not add this path to the `owned_namespaces` list in `repository_configuration.rs:36-39`: that list feeds the configuration hash, and the whole `.objekte` directory is already excluded from source discovery. Plan 6 (`index-sync-frontends`) adds the other store locations and derives the file name from the configuration. If redb reports `DatabaseAlreadyOpen`, return `StoreOpenOutcome::WithoutStore(..)` after a normal source build. Never wait, retry or delete in that case.

**D4. Validity.** The `meta` table holds:

| Key | Value |
| --- | --- |
| `format` | `STORE_FORMAT_VERSION: u32 = 1`, little-endian |
| `configuration` | the 32 bytes of `configuration_hash` computed from the prepared definitions |
| `chunk_bits` | `POSTING_CHUNK_BITS: u32` |
| `revision` | `u64` |
| `next_segment_ordinal` | `u64` |
| `next_store_id` | `u64` |
| `complete` | one byte, `1` when the last checkpoint finished |

A store is valid when `format`, `configuration` and `chunk_bits` match and `complete == 1`. Otherwise `open_persistent` rebuilds: it drops the database handle, deletes the file, creates a new one, builds from source, and checkpoints. Any error while reading a valid-looking store (decode error, missing row, inconsistent counts) is `StoreRebuildReason::Damaged` and handled the same way. No partially loaded index is ever published.

**D5. Shard identity on disk.** Each stored shard has a `store_id: u64` taken from `next_store_id`. It is independent of the shard's `ordinal`.

- `IndexedCollectionSegment` gets a field `lineage: u64`. `0` means none. `to_shared()` (plan 3) assigns a new non-zero value from a process-wide `AtomicU64`; `append` copies it from `self`. Every other place that constructs a shard sets `0`.
- `IndexStore` keeps, in memory, for every collection:
  - `immutable: HashMap<u64 /*ordinal*/, StoredShard>` for shards without lineage;
  - `main: HashMap<u64 /*lineage*/, StoredMain>` for main shards.
- `StoredShard { store_id, active: ActiveRecords }`.
- `StoredMain { store_id, snapshot: Arc<IndexedCollectionSegment>, active: ActiveRecords, dense_epoch: u64, reuse_log_len: usize }`. `snapshot` is the shard object as of the last checkpoint. `dense_epoch` and `reuse_log_len` are always `0` in this plan; plan 5 uses them.

**D6. Tables.** All tables are `TableDefinition<&[u8], &[u8]>` except `meta` (`TableDefinition<&str, &[u8]>`). All integers inside keys are big-endian, so rows of one shard sort by position. Every key of a shard table starts with the 12-byte prefix `collection: u32` + `store_id: u64`.

| Table | Key after the shard prefix | Value |
| --- | --- | --- |
| `collections` | (key is `collection: u32` only) | bincode of the collection's `GlobalInferenceSummary` option |
| `catalog` | (key is `file_id: u32` only) | bincode of `(FileEntry, Option<[u8; 32]>, Option<[u8; 32]>)`, one row of `snapshot_slots()`; absent row = `None` |
| `shards` | nothing | bincode of `ShardHeader` |
| `active` | nothing | `RoaringBitmap` of the active slots |
| `files` | `position: u32` | bincode of `SegmentFile` |
| `records` | `slot: u32` | bincode of `RecordEntry` |
| `posting_fields` | `family: u8`, `field` | bincode of `PostingHeader` |
| `postings` | `family: u8`, `field`, `key`, `chunk: u32` | `RoaringBitmap` |
| `bitmaps` | `kind: u8`, `field` | `RoaringBitmap` |
| `sort_columns` | `column` | bincode of `SortColumnHeader` |
| `sort_keys` | `column`, `slot: u32` | bincode of `SortKey` |
| `composites` | `index: u32` | bincode of the composite's `fields` |
| `relations` | `ordinal: u32` | bincode of `RelationHeader` |
| `references` | `ordinal: u32`, `source: u32`, `n: u32` | bincode of `PendingReference`; `source` is the reference's source slot and `n` its position among that slot's references |
| `inference` | nothing | bincode of `InferenceReport` |

`field`, `column` and `key` are variable-length and are encoded as `length: u32` followed by the bytes, so that no encoded key is a prefix of another within a table.

- `field` bytes: bincode of the field path (`Vec<String>`); for custom bindings bincode of `(Vec<String>, String)`.
- `key` bytes: bincode of `ScalarKey` (values), bincode of `Number` (lengths), one byte for `StructuralKey` (its declaration order `0..=6`), 8 bytes big-endian for a gram.
- `family`: `0` declared values, `1` declared lengths, `2` declared structural, `3`-`5` the same for custom postings, `6`-`8` the same for inferred indexes, `9` raw normal grams, `10` raw skip grams, `11` fallback normal, `12` fallback skip, `13` custom text normal, `14` custom text skip. Gram families of `raw` and `fallback_text` use an empty `field`.
- `kind` in `bitmaps`: `0` presence, `1` nulls, `2` empty selections (each per posting family group: add 10 for custom, 20 for inferred), `30` logical records, `31` empty array records, `32` inferred empty selections, `33` inferred short-string coverage, `40`-`43` the four relation bitmaps with the relation ordinal as `field`.

Headers:

```rust
#[derive(Serialize, Deserialize)]
struct ShardHeader {
    ordinal: u64,
    role: u8,                 // 0 Built, 1 Mutation, 2 Main
    shared: bool,             // true when the families use the shared backing
    source_id: String,
    source_version: u32,
    record_count: u32,
    file_count: u32,
    record_parses: u64,
    failed_parses: u64,
    has_inference: bool,
}

#[derive(Serialize, Deserialize)]
struct PostingHeader {
    kind: LogicalType,
    multi: bool,
    values_indexed: bool,
    value_complete: bool,
    length_complete: bool,
    empty_string_complete: bool,
    covered_records: u32,
    has_structural: bool,
    custom_complete: Option<bool>,   // Some(..) for custom postings
}

#[derive(Serialize, Deserialize)]
struct SortColumnHeader {
    path: FieldPath,
    kind: LogicalType,
    locale: String,
    options: SortOptions,
    collation_version: String,
}

#[derive(Serialize, Deserialize)]
struct RelationHeader {
    declaration: RelationDefinition,
    record_count: u32,
    reference_count: u32,
}
```

**D7. What is not stored, and how it is rebuilt on load.**

| Not stored | Rebuilt from |
| --- | --- |
| `file_index` | positions of the `files` rows |
| `id_lookup` | `records` rows in slot order; when an id is already mapped, replace the mapping unless the mapped slot is active. The result is the active slot if the id has one, otherwise the highest slot, which is what `append` leaves behind. |
| `identity_order`, `ascending`, `descending`, composite `order` | sort all slots with the comparators of plan 3, D10 |
| `PostingStats` | `finish_stats()` |
| `accounting` | `recompute_accounting()` |
| `FileVersions` | `FileVersions::initial(..)` |
| `instance_id`, timings, verified-result cache | new instance |

In a main shard an id can be mapped to a retired slot (the record was deleted). That is expected; every reader checks the `active` mask after the lookup.

**D8. Round-trip tests come first.** Before writing any table code, add `index_store_keys.test.rs` with `bincode` round-trip tests for every value type in D6: every `ScalarKey` variant (including `Number::Integer` at `i128::MIN` and `MAX`, `Number::Float` for `0.0`, `-0.0`, a subnormal, infinity, and `Number::ExactDecimal` with a long fraction), every `SortKey` variant, `RecordEntry`, `FileEntry` with diagnostics, `SegmentFile`, `PendingReference`, `InferenceReport`, and the four headers. Equality is checked with `==` where the type has it, otherwise through `Debug` output. If a type does not round-trip through `bincode` (for example because its `Deserialize` needs a self-describing format), write a small hand-made encoder for that type in `index_store_keys.rs`, cover it with the same test, and note it in the report. Do not change the type's existing `serde` behaviour.

**D9. Writing a shard completely.** `write_shard(&mut tables, collection, store_id, &shard, &active)` writes every row of every table in D6 for that shard. For posting and gram maps it writes one `postings` row per stored chunk. For maps in the `Building` backing it splits each bitmap with `posting_map::split_chunks`, so the on-disk layout is the same for both backings.

It uses `for_each_chunk` (Appendix A), which visits `(key, chunk, bitmap)` in key order and then chunk order. For the `Building` backing it splits each posting on the fly; for `Shared` it walks the stored entries. A key with an empty posting is visited once with chunk 0 and an empty bitmap, and must be written as such so the key survives a round trip.

**D10. Diffing a main shard.** `diff_main(&mut tables, collection, store_id, old: &IndexedCollectionSegment, new: &IndexedCollectionSegment, ..)` writes only differences:

| Family | Rule |
| --- | --- |
| each posting or gram map | `new_map.diff_chunks(old_map, ..)` (below). If either side is not shared, or the field exists on one side only: delete the field's key range and write it completely. |
| each fixed bitmap (`bitmaps` table) | rewrite when `old != new` |
| `posting_fields`, `sort_columns`, `composites`, `relations` headers, `inference`, `shards` header | rewrite when the encoded bytes differ |
| `records` | write rows `old.record_count()..new.record_count()` |
| `sort_keys` | per column, write the same slot range |
| `references` | per relation, write the references of every source slot in `old.record_count()..new.record_count()` (`references_for_source`) |
| `files` | rewrite all rows of the shard when `new` and `old` are different objects (`!Arc::ptr_eq`); delete rows at positions `>= new.file_count()` |
| `active` | rewrite when the mask changed |

`PostingMap::diff_chunks` and `PostingMap::for_each_chunk` are given in Appendix A, with tests in Appendix B. Both were compiled and run with both chunk sizes before this plan was written. `diff_chunks` uses `imbl::OrdMap::diff`, which skips subtrees that both maps share, so its cost follows the amount of change and not the size of the map. Add forwarding methods with the same names to `GramMap`: for `Shared` forward to the inner `PostingMap<u64>`; for `Building`, `for_each_chunk` goes through a temporary sorted `PostingMap::from_btree_map(self.to_btree_map())` and `diff_chunks` returns `Ok(false)`.

`references` rows are keyed by source slot because plan 5 inserts references for reused slots in the middle of the list. Rows keyed by list position would shift.

**D11. Checkpoint algorithm.**

1. Lock `IndexInner.store`. If `None`, return the "no store" error.
2. `let view = self.view()?;` This pins one published root for the whole checkpoint.
3. Begin a write transaction; set durability from `StoreOptions.durable`.
4. Set `meta.complete = 0`.
5. For each collection, for each shard of the view:
   - `lineage != 0` and `store.main` has that lineage: `diff_main` against `StoredMain.snapshot`.
   - `lineage == 0` and `store.immutable` has that ordinal: rewrite the `active` row if the mask differs from `StoredShard.active` (compare `bitmap()` results only if `len()` or `slot_count()` differ or the masks are not the same allocation; add a cheap `ActiveRecords::same_as(&self, other) -> bool` that compares chunk pointers).
   - otherwise: take a new `store_id` and `write_shard`.
6. For each stored shard (immutable or main) that the view no longer has: delete its 12-byte key prefix range from every shard table with `retain_in`.
7. Catalogue: compare `view` catalogue rows (`snapshot_slots()`) with the rows written at the last checkpoint (kept in `IndexStore`); write changed rows, remove rows that became `None`.
8. `collections` rows: rewrite when changed.
9. Write `revision`, `next_segment_ordinal`, `next_store_id`, then `complete = 1`.
10. Commit. Only after a successful commit, replace the in-memory `immutable`/`main` maps and the catalogue copy with the new state.
11. If any step fails: abort the transaction (drop it), clear the in-memory maps and set a flag `rewrite_all`. The next checkpoint then deletes every row of every table first and writes everything. Return the error.

The transaction is atomic: a crash at any point leaves the store exactly as the previous successful checkpoint left it.

**D12. Load algorithm.**

1. Prepare definitions (`prepare_source_definitions`), compute `configuration_hash`.
2. Open the database and validate `meta` (D4).
3. Build the empty static base: add `pub(crate) fn Repository::empty_for_configuration(root, definitions, options, sources: &SourceRegistry) -> Result<Self, String>` that calls `build_fresh(.., false)`; then set each collection's `global_inference` from the `collections` table.
4. Read the catalogue rows into `Vec<Option<(FileEntry, Option<[u8; 32]>, Option<[u8; 32]>)>>` (index = file id; length = highest id + 1, at least the base inventory length).
5. Read every shard: header, tables, bitmaps. Build the families in the `Building` backing first (insert rows in key order; assemble each posting from its chunks with a union). For a header with `shared == true`, call `freeze_all()` afterwards and set `role` and a new `lineage`. Rebuild what D7 lists. Read shards of different collections in parallel with the existing `rayon` dependency, each worker with its own read transaction.
6. Assemble the `PublishedIndex` like `from_direct_build` does: `PublishedIndex::from_repository(instance_id, IndexRevision(stored_revision), base)`, `catalog = FileCatalog::restored(base, slots)`, `file_versions = FileVersions::initial(catalog.allocated_files())`, `next_segment_ordinal` from `meta`.
7. Add each shard with a new method `PublishedIndex::with_restored_segment(self, index, active, ordinal) -> Result<Self, IndexError>`. It checks: the collection exists; `source_id`/`source_version` match the collection's source registration (as `with_segment` does); `active.slot_count() == index.record_count()`. It pushes the `PublishedSegment` as given. It does not run maintenance, does not renumber, and does not reset the mask.
8. Consistency checks before publishing; any failure is `Damaged`: number of rows read equals the counts in each header; every file id referenced by a shard has a catalogue row or the shard has no active record for it; no record id is active in two shards of one collection.
9. `Index::from_published(published)`, attach the `IndexStore` with its in-memory maps filled from what was read, then call `start_reconcile()`.

**D13. Reconciliation is what makes a stale store safe.** After a load, `open_persistent` always starts reconciliation. A file changed, added or removed since the last checkpoint differs from the catalogue in size or modification time and goes through the normal mutation path. This is the same guarantee the index gives today for edits made while it is running; its known limit (an edit that restores both size and modification time is not seen) is unchanged and must be stated in the documentation.

**D14. Tests use two kinds of store.** Unit tests of tables and diffs use `IndexStore::open_in_memory()` (`#[cfg(test)]`, redb `InMemoryBackend`). Reopen tests use a real file in a temporary directory created with the helpers the lifecycle tests already use.

## 4. Checkpoints

### C0. Baseline

Record `START`. Run and save, as in plan 3 C0: both workspace test runs, Clippy, the detailed-profiling check, both acceptance runs, the build gate, the quick query gate.

### C1. Dependency, keys and codecs

1. Add `redb = "4"` to `crates/core/Cargo.toml`; update `Cargo.lock`.
2. `index_store_keys.rs`: the table definitions, `STORE_FORMAT_VERSION`, encoders and decoders for every key shape of D6, `fn prefix_end(prefix: &[u8]) -> Option<Vec<u8>>` (the smallest byte string greater than every string with that prefix: drop trailing `0xFF` bytes, then increment the last byte), and the header structs.
3. Add the missing `serde` derives (section 2.3).
4. Tests per D8, plus:
   - `shard_keys_sort_by_position`: for random `(collection, store_id, slot)` triples, byte order of the encoded keys equals tuple order.
   - `posting_keys_group_chunks_of_one_key`: for keys `"a"`, `"ab"`, `"b"` with chunks `0, 1, 300`, all chunks of one key are adjacent and ascending, and no key's rows fall inside another key's range.
   - `prefix_end_bounds_exactly_the_prefix`.

Commit: `Add index store key and value codecs`.

### C2. Write and read one shard

1. Append the code of Appendix A to `posting_map.rs` (above the `#[cfg(test)]` module line) and the tests of Appendix B to `posting_map.test.rs`. Add the `GramMap` forwarding methods (D10). Run `cargo test -p objekte-search-core --locked --lib posting_map` with and without `--features force-shared-backing`; three more tests pass.
2. Add the `lineage` field (D5).
3. `index_store_shard.rs`: `write_shard` and `read_shard`.
4. Tests in `index_store_shard.test.rs`, using the shard-building helpers and `assert_shards_equal` from `mutation_segment.test.rs` (move the helper to a shared `#[cfg(test)]` module if needed):
   - `shard_round_trip_native`: a `Built` shard with all families populated, written and read through an in-memory store, equals the original, and `accounting` is equal after `recompute_accounting()`.
   - `shard_round_trip_shared`: the same for `to_shared()` of that shard with three more shards appended (so the main shard contains a replaced file and retired slots). The read shard uses the shared backing and equals the original.
   - `shard_round_trip_keeps_the_active_mask`.
   - `deleting_a_shard_prefix_removes_every_row`: write two shards, delete one prefix, assert every table has rows for the other shard only.

Commit: `Write and read one shard in the index store`.

### C3. Diff a main shard

1. `diff_main` in `index_store_shard.rs`, built on `diff_chunks` (already added in C2).
2. Tests:
   - `diff_then_read_equals_full_write`: write main shard `m1` completely; append deltas to get `m2`; apply `diff_main(m1, m2)`; `read_shard` equals `m2`. Repeat with a delta that replaces a file and with one that causes an inferred path to be dropped.
   - `diff_writes_are_proportional_to_the_delta`: with `m1` of 5,000 records and a 1-record delta, the report counts `chunks_written` at most `4 * (number of distinct posting and gram keys of the delta)` and `rows_written` at most `10 + file_count(m2)`.

Commit: `Write main shard changes incrementally`.

### C4. Checkpoint and load

1. `index_store.rs`: `IndexStore` with open/validate (D3, D4), the checkpoint algorithm (D11) and the load algorithm (D12).
2. `Repository::empty_for_configuration`, `PublishedIndex::with_restored_segment`, `ActiveRecords::same_as`.
3. `lifecycle_store.rs`: `open_persistent`, `checkpoint`, `has_store` (D2). `open_persistent` flow: prepare definitions; try to open and load; on `Loaded` return; otherwise build with `Self::build_with_sources(..)`, attach a fresh store, `checkpoint()`, return `Rebuilt(reason)`. When the file is locked: build, attach nothing, return `WithoutStore`.
4. Export `StoreOptions`, `StoreOpenOutcome`, `StoreRebuildReason`, `CheckpointReport` from the crate the same way other lifecycle types are exported.

Commit: `Checkpoint and load the index through redb`.

### C5. End-to-end tests

In `lifecycle_store.test.rs`, with temporary fixture directories (at least 60 files, two collections, a relation, sort fields, inference enabled) and the query list of plan 3's `folding_matches_an_index_that_never_folds`. "Control" always means a second index built with `Index::build` from the same directory at that moment.

- `first_open_builds_and_writes_a_store`: outcome `Rebuilt(Missing)`; the file exists; queries equal control.
- `second_open_loads_without_parsing`: after dropping the first index, outcome `Loaded`; `index.status()?.build_parses == 0`; queries equal control; `status()` reports initial construction complete.
- `loaded_index_equals_checkpointed_shards`: mutate until a main shard exists with retired slots, checkpoint, keep the view; reopen; for every collection the loaded shards, ordered by ordinal, satisfy `assert_shards_equal` against the kept view's shards, with equal active masks.
- `incremental_checkpoint_is_small`: after the first checkpoint, one single-record update that folds (`set_fold_trigger_for_tests(1)`), then `checkpoint()`: `shards_written <= 1`, `main_shards_updated == 1`, `chunks_written` bounded as in C3, and the reopened index equals control.
- `mutations_after_the_last_checkpoint_are_recovered`: checkpoint; edit a source file, call `update_file`; add a file, call `add_file`; delete a file, call `delete_file`; drop the index without checkpoint; reopen (`Loaded`); `wait_for_reconciliation()`; queries equal control.
- `source_edits_while_closed_are_picked_up`: checkpoint, drop; edit, add and delete source files on disk; reopen; wait for reconciliation; queries equal control.
- `changed_configuration_rebuilds`: reopen with a definition that adds a sort field: `Rebuilt(Configuration)`; queries equal control; a further reopen is `Loaded`.
- `format_version_mismatch_rebuilds`: overwrite `meta.format` through a raw redb handle: `Rebuilt(FormatVersion)`.
- `interrupted_checkpoint_rebuilds`: set `meta.complete = 0` through a raw handle: `Rebuilt(Damaged(_))`.
- `damaged_store_rebuilds`: three sub-cases: file truncated to half; file replaced by 1 KiB of zero bytes; one `records` row replaced by garbage through a raw handle. Each: no panic, a `Rebuilt(..)` outcome, queries equal control.
- `locked_store_opens_without_store`: open twice in one process: the second outcome is `WithoutStore(_)`, its queries equal control, its `checkpoint()` returns the "no store" error, and the first index can still checkpoint.
- `checkpoint_failure_keeps_the_index_usable`: make the next commit fail (`#[cfg(test)]` failure switch inside `IndexStore`, set through a method on the store, not a global); `checkpoint()` returns `Err`; queries still work; the next `checkpoint()` succeeds and a reopen equals control.
- `pinned_views_are_unaffected_by_checkpoints`.
- `build_does_not_touch_an_existing_store`: create a store with `open_persistent`, drop it, record the file's length and modification time, run `Index::build` on the same root and query it: the file is unchanged.
- `store_survives_main_shard_dissolve`: force a dissolve (plan 3), checkpoint, reopen: equals control, and the old main shard's rows are gone (count rows by prefix).

Run both workspace test commands; the store tests must also pass with `force-shared-backing`.

Commit: `Test index store round trips and recovery`.

### C6. Musenalm acceptance through the store

Add an example `crates/core/examples/accept_musenalm_store.rs` modelled on `accept_musenalm.rs`: copy nothing; open the fixture with `open_persistent` and a store path given by `--store PATH` (required, outside the repository), run the acceptance cases, then drop the index, reopen (must be `Loaded`), and run the cases again. Exit non-zero on any difference or if the second open is not `Loaded`.

```sh
cargo run --release -p objekte-search-core --example accept_musenalm_store -- --alphabet ascii --store /tmp/objekte-redb-index-store/accept-ascii.redb
cargo run --release -p objekte-search-core --example accept_musenalm_store -- --alphabet unicode --store /tmp/objekte-redb-index-store/accept-unicode.redb
```

Then verify that the fixture directory was not modified: `git status --short examples/musenalm` prints nothing.

Commit: `Accept Musenalm through the index store`.

### C7. Measure, validate, report

1. Add a manual measurement test `measure_index_store_on_real_fixture` (`#[ignore]`, fixture root from an environment variable, store in a temporary directory). Report medians of 3 runs: source build time; first checkpoint time and file size; load time (open to queries possible) with `durable: true`; time and counters of a checkpoint after one folded update; time and counters of a checkpoint after 1,000 updates; total time spent in `diff_chunks` during that checkpoint; process peak RSS during load. Save as `docs/ai/generated/redb-index-store-measurement.md`.
2. Re-run every C0 command; build gate with `--baseline`.
3. Write `docs/development/index-store.md`: purpose, the explicit API, file location, validity rules, table layout (D6), checkpoint and load algorithms, the reconciliation guarantee and its limit (D13), and the statement that frontends do not use the store yet. Replace the text of `docs/development/hybrid_index_storage.md` only by adding one line that points to the new document. Update the sentence in `docs/search-core.md` that says there is no index load or save API.
4. Complete the task per `docs/ai/README.md` section 7.

## 5. Gates

| Gate | Requirement |
| --- | --- |
| G1 | Workspace tests pass (default features). The persistence-absence tests named in section 0 are unchanged and pass. |
| G2 | Workspace tests pass with `force-shared-backing`. |
| G3 | Index signatures equal C0; build gate reports no failure; build time and peak RSS within 5% of C0 (report; investigate 5% to 15%; block above 15%). `Index::build` must not have become slower. |
| G4 | `accept_musenalm` (default and forced) and `accept_musenalm_store` pass for ASCII and Unicode. |
| G5 | Quick query gate passes. |
| G6 | Clippy warnings not higher than C0. |
| G7 | `git diff --stat START -- crates/cli crates/desktop` is empty. |
| G8 | Plan 3's mutation benchmarks (`small`, `stress`) within 10% of this plan's C0 values: an index without a store must not pay for the store. |

There is no gate on load time. Report it next to the build time. If loading is not clearly faster than building on the test machine, say so plainly in the report; that is information the user needs, not a failure of this plan.

## 6. Pitfalls

- **Never publish a partially loaded index.** Load everything, check everything, then publish.
- **Never fail `open_persistent` because of the store.** Only a source-build failure may fail it.
- **Delete by prefix with care.** `prefix_end` must be exact; an off-by-one deletes a neighbouring shard. The C1 and C2 tests cover it.
- **Variable-length key parts need a length prefix.** Without it, key `"a"` chunk 97 can collide with key `"aa"`.
- **Use `store_id`, not `ordinal`, in keys.** A main shard changes its ordinal at every fold.
- **Update the in-memory `StoredMain.snapshot` only after a successful commit.** Otherwise a failed checkpoint makes the next diff skip real changes.
- **`diff_chunks` is fast only for maps that share structure.** Never rebuild a shared map from scratch when a clone would do; a rebuilt map shares nothing, so the diff has to compare every chunk (correct but slow).
- **Loaded shards must not go through `with_segment`.** It renumbers, resets the active mask and may run maintenance.
- **Do not checkpoint from inside the publication lock or the writer mutex.** `checkpoint` pins a view and works outside both.
- **bincode is not self-describing.** Never reorder or remove fields of a stored struct without raising `STORE_FORMAT_VERSION`.
- **The forced feature changes `chunk_bits`.** A store written with one chunk size is rejected by the other; tests must not share store files between the two runs.

## 7. The completion report must include

- `START`, branch, commits per checkpoint.
- C0 and final values for every gate.
- The measurement table of C7, with an explicit comparison of load time and build time.
- Types that needed a hand-made encoder (D8), or "none".
- `serde` derives added.
- Deviations and their explanation.

## Appendix A. Code to append to `crates/core/src/posting_map.rs`

Insert above the final `#[cfg(test)]` module declaration.

```rust
/// One difference between two shared posting maps, at chunk granularity.
pub enum ChunkChange<'a, K> {
    Put(&'a K, ChunkId, &'a RoaringBitmap),
    Delete(&'a K, ChunkId),
}

impl<K: Ord + Clone> PostingMap<K> {
    /// Visit every stored chunk in key order, then chunk order. A key whose
    /// posting is empty is visited once with chunk 0 and an empty bitmap.
    pub fn for_each_chunk(
        &self,
        mut visit: impl FnMut(&K, ChunkId, &RoaringBitmap) -> Result<(), String>,
    ) -> Result<(), String> {
        match &self.backing {
            Backing::Building(entries) => {
                for (key, posting) in entries {
                    let parts = split_chunks(posting.clone());
                    if parts.is_empty() {
                        visit(key, 0, posting)?;
                    }
                    for (chunk, part) in &parts {
                        visit(key, *chunk, part)?;
                    }
                }
            }
            Backing::Shared(shared) => {
                for ((key, chunk), part) in shared.chunks.iter() {
                    visit(key, *chunk, part)?;
                }
            }
        }
        Ok(())
    }

    /// Report what must be written to turn the stored form of `older` into
    /// the stored form of `self`. Subtrees shared by both maps are skipped,
    /// so the cost follows the amount of change. Both maps must use the
    /// shared backing; otherwise nothing is visited and `false` is returned.
    pub fn diff_chunks(
        &self,
        older: &Self,
        mut visit: impl FnMut(ChunkChange<'_, K>) -> Result<(), String>,
    ) -> Result<bool, String> {
        let (Backing::Shared(new), Backing::Shared(old)) = (&self.backing, &older.backing) else {
            return Ok(false);
        };
        for change in old.chunks.diff(&new.chunks) {
            match change {
                imbl::ordmap::DiffItem::Add(address, part) => {
                    visit(ChunkChange::Put(&address.0, address.1, part))?;
                }
                imbl::ordmap::DiffItem::Update {
                    new: (address, part),
                    ..
                } => {
                    visit(ChunkChange::Put(&address.0, address.1, part))?;
                }
                imbl::ordmap::DiffItem::Remove(address, _) => {
                    visit(ChunkChange::Delete(&address.0, address.1))?;
                }
            }
        }
        Ok(true)
    }
}
```

## Appendix B. Tests to append to `crates/core/src/posting_map.test.rs`

```rust
/// Both backings expose the same chunks, and the chunks rebuild the map.
#[test]
fn for_each_chunk_agrees_across_backings() {
    let chunk = 1u32 << POSTING_CHUNK_BITS;
    let mut native: PostingMap<u16> = PostingMap::new();
    for (key, slot) in [(1u16, 3u32), (1, chunk + 4), (1, 5 * chunk), (2, 9), (7, 2 * chunk + 1)] {
        native.insert_slot(key, slot);
    }
    native.replace(4, RoaringBitmap::new());
    let mut shared = native.clone();
    shared.freeze();
    let collect = |map: &PostingMap<u16>| {
        let mut seen = Vec::new();
        map.for_each_chunk(|key, chunk, part| {
            seen.push((*key, chunk, part.clone()));
            Ok(())
        })
        .unwrap();
        seen
    };
    let from_native = collect(&native);
    assert_eq!(from_native, collect(&shared));
    assert_eq!(from_native.len(), 6);
    let mut rebuilt: BTreeMap<u16, RoaringBitmap> = BTreeMap::new();
    for (key, _, part) in from_native {
        *rebuilt.entry(key).or_default() |= part;
    }
    assert_eq!(rebuilt, native.to_btree_map());
}

fn stored_chunks(map: &PostingMap<u16>) -> BTreeMap<(u16, ChunkId), RoaringBitmap> {
    let mut stored = BTreeMap::new();
    map.for_each_chunk(|key, chunk, part| {
        stored.insert((*key, chunk), part.clone());
        Ok(())
    })
    .unwrap();
    stored
}

/// Applying the reported changes to the old stored chunks gives the new
/// stored chunks, and unchanged chunks are not reported.
#[test]
fn diff_chunks_reports_exactly_the_changes() {
    let chunk = 1u32 << POSTING_CHUNK_BITS;
    let mut random = Lcg(77);
    let mut old: PostingMap<u16> = PostingMap::new();
    for _ in 0..4_000 {
        old.insert_slot(random.next(600) as u16, random.next(4 * chunk));
    }
    old.freeze();
    let mut new = old.clone();
    new.insert_slot(5, 3 * chunk + 17);
    new.insert_slot(900, 1);
    new.replace(6, RoaringBitmap::new());
    new.retain_keys(|key| *key != 7);
    let mut stored = stored_chunks(&old);
    let mut puts = 0usize;
    let mut deletes = 0usize;
    let compared = new
        .diff_chunks(&old, |change| {
            match change {
                ChunkChange::Put(key, chunk, part) => {
                    puts += 1;
                    stored.insert((*key, chunk), part.clone());
                }
                ChunkChange::Delete(key, chunk) => {
                    deletes += 1;
                    assert!(stored.remove(&(*key, chunk)).is_some());
                }
            }
            Ok(())
        })
        .unwrap();
    assert!(compared);
    assert_eq!(stored, stored_chunks(&new));
    assert!(puts <= 3, "{puts} puts");
    assert!(deletes >= 1);
    let mut none = 0usize;
    assert!(new.diff_chunks(&new.clone(), |_| {
        none += 1;
        Ok(())
    })
    .unwrap());
    assert_eq!(none, 0);
}

#[test]
fn diff_chunks_refuses_native_maps() {
    let mut native: PostingMap<u16> = PostingMap::new();
    native.insert_slot(1, 1);
    let mut shared = native.clone();
    shared.freeze();
    assert!(!shared.diff_chunks(&native, |_| Ok(())).unwrap());
    assert!(!native.diff_chunks(&shared, |_| Ok(())).unwrap());
}
```

## Implementation report

- Task identifier: `redb-index-store`
- Completion date: 2026-11-15
- Base commit: `9fd8838db9` (plan 3 tip)
- Integration branch: `integration/redb-index-store`
- Integration worktree:
  `/home/simon/source/objekte-second-base-integration-redb-index-store`
- Implementation commits:
  - `a713b4ddc8` Add index store key and value codecs
  - `5c369c71a5` Prepare posting chunks and shard lineage for the index store
  - `990a4103d9` Write and read one shard in the index store
  - `ae4f1c07a5` Write main shard changes incrementally
  - `dbf37e03e6` Checkpoint and load the index through redb
  - `da4b7f6250` Accept Musenalm through the index store
  - `066adf379a` Keep the index store lint count at the baseline
  - final documentation/report commit (this commit)

### Summary

`redb = "4"` is the only new dependency. `index_store_keys.rs` defines the
table set, shard prefix, key encoders, `prefix_end`, headers and hand-written
ScalarKey/SortKey codecs (bincode cannot round-trip non-finite floats through
`Number`'s custom deserializer). `index_store_shard.rs` writes and reads every
shard family, writes main-shard diffs through `PostingMap::diff_chunks`, and
deletes a shard by key prefix. `index_store.rs` owns the database, metadata,
checkpoint transaction, catalog and collection rows. `lifecycle_store.rs`
exposes `StoreOptions`, `StoreOpenOutcome`, `StoreRebuildReason`,
`CheckpointReport`, `Index::open_persistent`, `Index::checkpoint` and
`Index::has_store`. `IndexInner` carries an optional store.

A checkpoint pins one view and writes in one atomic transaction. Immutable
shards written once are only updated when their active mask changes. A main
shard is diffed against its stored snapshot. Load reads all shards, rebuilds
derived tables, starts reconciliation and attaches the store without running
maintenance. Store failures never fail `open_persistent`; they fall back to a
source build with or without a store.

### Validation

- Base results (plan 3 final at the same commit): default/forced 853 passed /
  19 ignored, Clippy 236, build gate ASCII 1.003 s / 492.5 MB, Unicode
  1.048 s / 506.7 MB.
- Final (`docs/ai/memory/redb-index-store/final/`): default and forced tests
  878 passed / 20 ignored; Clippy 236 warning lines; detailed-profiling check
  passes; `accept_musenalm` passes for ASCII/Unicode with default and forced
  features; `accept_musenalm_store` passes for ASCII and Unicode.
- Index signatures unchanged; build gate has no failure: ASCII 0.940 s
  (-6.3%, RSS -1.2%), Unicode 0.998 s (-4.7%, RSS +1.1%).
- Quick query gate has the same path-sensitive baseline compatibility failures
  as plans 1-3; aggregate ratio 0.724 with no latency confirmations.
- Manual measurement (`docs/ai/generated/redb-index-store-measurement.md`):
  build plus first checkpoint 2,691 ms; later checkpoint 61 ms; store 570 MB;
  1,000 updates plus checkpoint 88 ms for new shards; load 875 ms.

### Complete/incomplete items

- Implemented and tested: key codecs, shard round trips in both backings,
  active-mask round trip, prefix deletion, incremental main diff, checkpoint
  and load, locked/damaged/format/configuration recovery, source edits while
  closed, checkpoint failure and retry, pinned views, build not touching a
  store, incremental checkpoint after a fold, and the store acceptance example.
- The acceptance example runs six representative queries twice; it does not
  replay all 150 `accept_musenalm` cases.
- `CheckpointReport.bytes_written` remains zero; the other counters are
  populated.
- Not added: `loaded_index_equals_checkpointed_shards` (the round-trip and
  incremental tests cover the same paths more narrowly),
  `store_survives_main_shard_dissolve`, and the raw corrupted-record-row
  sub-case of `damaged_store_rebuilds`.

### Deviations

- The plan's `ShardHeader` did not carry the bigram alphabet, which is needed
  to rebuild `BigramIndex`; the header stores it.
- Sort keys are stored with the hand-written `SortKey` codec instead of
  bincode, for the non-finite float reason above.
- Branches are stacked because agents may not merge into `master`.
