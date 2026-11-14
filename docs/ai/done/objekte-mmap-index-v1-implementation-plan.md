# Objekte mmap index implementation specification

**Task ID:** `mmap-index-v1`  
**Inspected baseline:** `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`  
**Specification date:** 2026-11-14  
**Status:** Implementation specification; implementation has not been performed by the planning agent.

This specification makes the main implementation decisions. The implementing model should follow them rather than spend time comparing storage engines or inventing additional options.

The intended result is:

> Objekte builds directly into a compact, immutable, searchable representation. The same representation works in RAM and through mmap. Saving is an explicit operation. Loading opens a saved index directory without reconstructing its dictionaries, postings, record tables, or sort orders.

All implementation phases below are required for completion.

Before implementing, read and follow the repository's `AGENTS.md` and `docs/ai/README.md`. This document does not override repository authorization, worktree, commit, push, or validation rules.

---

## 1. Product behavior

### 1.1 Three separate operations

Implement these as separate operations:

1. **Build**
   - Reads source files.
   - Constructs an index in RAM.
   - Publishes immutable packed segments.
   - Does not save an index automatically.

2. **Load**
   - Opens a previously saved index directory.
   - Maps its immutable objects.
   - Constructs lightweight readers and publication metadata.
   - Does not rebuild from sources.
   - Does not automatically save changes.

3. **Save**
   - Pins one complete index publication.
   - Writes that publication to a selected directory.
   - Reuses unchanged objects already present there.
   - Atomically publishes the saved generation.

Closing an application must not implicitly save its index.

### 1.2 Loaded indexes remain updateable

Loading must not make an index permanently read-only.

After loading:

- Existing segments remain mmap-backed.
- New or replaced file contributions are constructed in RAM.
- Deletions change the published active-content catalogue.
- Queries merge mapped segments and resident segments.
- An explicit Save persists the resulting publication.

Consequently, a live index can contain both resident and mapped segments.

### 1.3 Source freshness policy

For this first version, make freshness checking explicit:

- **Default Load:** load the saved publication immediately.
- **Check for changes:** run the existing reconciliation machinery.
- **Load and check:** convenience operation combining those two steps.

A load-only operation must perform zero source-record parses and zero recursive source discovery.

The UI must show:

```text
Source state: Not checked since loading
```

After successful reconciliation:

```text
Source state: Checked
```

Reconciliation updates the live index in RAM. Saving those updates remains explicit.

This keeps “open quickly” measurable and avoids hiding a potentially expensive source scan inside Load.

### 1.4 Load failures

Explicit Load must return an error for:

- Missing index.
- Unsupported format.
- Incompatible configuration.
- Missing object.
- Invalid manifest.
- Invalid object header.
- Detected corrupt block.

Do not silently fall back to building.

The GUI and TUI can expose an explicit **Build from sources** action after the error.

---

## 2. Architectural decisions

Use the following architecture:

```text
Index
└── PublishedIndex
    ├── Runtime configuration
    ├── Paged file catalogue
    ├── Path lookup
    ├── Active content generations
    └── Immutable collection runs
        ├── Record metadata
        ├── File-part directory
        ├── Logical-ID dictionary
        ├── Typed dictionaries
        ├── Fixed structural postings
        ├── Bigram dictionaries
        ├── Posting blocks
        ├── Sort keys and orders
        ├── Relations
        └── Coverage and statistics
```

### 2.1 Storage engine

Use **custom immutable binary objects with `memmap2`**.

For this version:

- Implement immutable ordered dictionary pages.
- Implement immutable posting blocks.
- Implement packed arrays and variable-length arenas.
- Use manifest switching for durability.
- Use existing lifecycle publication semantics for live views.

Dependencies selected for this task:

| Crate | Version requirement | Purpose |
|---|---:|---|
| `memmap2` | `0.9.12` | Read-only mappings |
| `crc32fast` | `1.5.4` | Block checksums |
| `uuid` | `1.29.1`, `v4` feature | Store/generation/publication identifiers |
| `dirs` | `6.0.0`, CLI crate | Shared frontend cache-directory resolution |

Reuse existing `sha2`, `serde`, and `serde_json`.

These versions are already present transitively in the inspected lockfile. Add the required direct dependencies.

### 2.2 Shared resident/mapped representation

Implement:

```rust
enum Backing {
    Resident(Arc<[u8]>),
    Mapped(Arc<MappedObject>),
}
```

A reader stores:

- A backing reference.
- Offsets.
- Lengths.
- Small decoded descriptors.

It must not own a reconstructed dictionary or full posting collection.

The resident and mapped implementations must use the same binary readers and algorithms.

### 2.3 Temporary builders

Mutable Rust collections remain appropriate during construction:

- `BTreeMap`.
- `HashMap`.
- `Vec`.
- `RoaringBitmap`.

However, construction must finish by sealing into the packed representation and releasing its builder collections.

A published index must not retain an equivalent native copy alongside its packed representation.

### 2.4 Query intermediates

Continue using `RoaringBitmap` for query-owned intermediate membership where useful.

This deliberately separates:

- Persistent membership representation.
- Temporary query results.

The implementing model must not replace every query algorithm simultaneously merely to remove Roaring.

---

## 3. Concrete first-version simplifications

These are deliberate decisions.

### 3.1 Preserve physical segment-local slots

Current execution relies heavily on `RecordSlot`.

Retain segment-local `u32` slots as **physical execution coordinates**.

Introduce stable record addresses separately:

```rust
struct StableRecordAddress {
    collection: CollectionId,
    file: FileId,
    content_generation: u64,
    local_record: u32,
}
```

Compaction may remap physical slots inside its output run. It must preserve stable addresses.

This avoids spreading a new composite identifier through every bitmap operation.

### 3.2 Rewrite catalogue metadata during explicit Save

Version 1 may write a new compact catalogue object on each changed Save.

That catalogue contains:

- File metadata.
- Active content generations.
- Path dictionary.
- Collection membership.
- Diagnostics.
- Small global summaries.

Large unchanged search segments must be reused.

This makes Save roughly:

```text
O(current catalogue metadata + newly created search objects)
```

It must not become:

```text
O(all dictionary keys + all postings + all sort data)
```

A repeated Save of the identical publication to the same unchanged target must be a no-op.

### 3.3 No general-purpose block compression initially

Implement:

- Empty.
- All.
- Range.
- Inline sorted IDs.
- Sorted arrays.
- Bitmaps.
- Runs.

General compression such as Zstd, PForDelta, or Elias–Fano is outside version 1.

This makes direct mapped execution easier to implement and measure.

### 3.4 One ordered dictionary implementation

Use the same immutable ordered-page implementation for:

- Typed values.
- Logical IDs.
- Paths.
- Unicode bigrams.
- Lengths.

Use fixed postings for booleans and structural states.

Schema enum values remain their existing logical type. Do not introduce a new `LogicalType::Enum` in this task.

---

## 4. Identity and publication invariants

### 4.1 File IDs

- Keep `FileId(u32)`.
- Allocate monotonically.
- Persist the allocation high-water mark.
- Never reuse IDs during an index lineage.
- Preserve deleted slots as tombstones.
- Return an explicit overflow error.

Compaction must not renumber file IDs.

A full rebuild creates a new index lineage and may allocate a fresh ID space.

### 4.2 Separate mutation version from content generation

Persist two different values:

```text
file_version
content_generation
```

`file_version`:

- Supports existing mutation conflict detection.
- Advances after accepted file mutations, including moves.

`content_generation`:

- Identifies the indexed contribution.
- Advances when the extracted contribution changes.
- Stays unchanged for a proven content-preserving move.

“Contribution changes” includes path-dependent identity or collection changes even if source bytes are identical.

### 4.3 Publication identity

Add a persistent publication identifier:

```rust
struct PublicationId(Uuid);
```

Create a new identifier when searchable data or physical index layout changes.

Do not change it merely because:

- Progress changes.
- A source check starts.
- An unchanged source check finishes.
- A UI requests statistics.

Save pins this identifier.

If mutations publish a newer identifier during Save:

- The Save succeeds for its pinned publication.
- The UI continues to show unsaved changes.

### 4.4 Reopened process identity

A loaded index gets a new live `instance_id`.

Persisted publication identity must not cause old process-local handles, query caches, or UI cursors to become valid in a new process.

### 4.5 Scoped `All`

Every `All` posting is relative to an owning domain:

- A run.
- A file part.
- A logical-record subset.
- Another explicitly specified coverage domain.

Never interpret an old posting as all records added later.

---

## 5. Module layout

Add a subsystem under:

```text
crates/core/src/index_storage/
```

Use these modules:

| Module | Responsibility |
|---|---|
| `mod.rs` | Public storage types and exports |
| `error.rs` | Typed storage errors |
| `backing.rs` | Resident/mapped bytes, checked access |
| `format.rs` | Format constants and fixed descriptors |
| `scalar.rs` | Borrowed typed-key encoding/comparison |
| `dictionary.rs` | Ordered dictionary reader/cursors |
| `dictionary_builder.rs` | Bulk dictionary construction |
| `record_set.rs` | Borrowed local set representation |
| `posting.rs` | File-grouped postings and operations |
| `posting_builder.rs` | Posting encoding |
| `records.rs` | Record and file-part readers |
| `catalogue.rs` | Paged file metadata and path access |
| `sort.rs` | Sort-key columns and order cursors |
| `relations.rs` | Packed adjacency and target IDs |
| `segment.rs` | Frozen segment/run reader |
| `segment_builder.rs` | Sealing complete contributions |
| `manifest.rs` | Manifest structures and validation |
| `store.rs` | Load/save publication protocol |
| `task.rs` | Progress, cancellation, task handle |
| `diagnostics.rs` | Structural and I/O counters |

Keep tests in separate `*.test.rs` files.

Avoid making `store.rs` responsible for query execution or source parsing.

---

## 6. Immutable object format

### 6.1 Directory layout

A saved index folder has this structure:

```text
chosen-folder/
├── STORE
├── CURRENT
├── writer.lock
├── readers.lock
├── objects/
│   ├── <sha256>.obx
│   ├── <sha256>.obx
│   └── ...
├── generations/
│   ├── <generation-uuid>/
│   │   └── manifest.json
│   └── ...
└── staging/
    └── <operation-uuid>/
```

`STORE` contains:

- Store magic.
- Store format version.
- Store UUID.

`CURRENT` contains:

```text
<generation-uuid> <manifest-sha256>\n
```

Object names are their full SHA-256 digest.

Object contents are immutable once published.

### 6.2 Object header

Each `.obx` object begins with a fixed 128-byte header.

It contains:

- Magic.
- Format version.
- Object kind.
- Header length.
- Total file length.
- Section-directory offset.
- Section-directory count.
- Checksum-table offset.
- Checksum block size.
- Header checksum.
- Reserved zeroed bytes.

All offsets and lengths are `u64`.

All integer fields use little-endian encoding.

All readers must use checked addition and checked slice access.

Do not cast arbitrary mapped bytes to Rust structs.

### 6.3 Section directory

Each section entry is a fixed-size descriptor containing:

```text
section kind
flags
offset
length
entry count
entry width, when fixed
reserved
```

Relevant section kinds include:

- Records.
- File parts.
- Logical IDs.
- Field directory.
- Dictionary pages.
- Posting payloads.
- Sort keys.
- Sort orders.
- Relation offsets.
- Relation edges.
- String arena.
- Coverage.
- Statistics.

Unknown mandatory section kinds are incompatible.

Unknown optional section kinds can be ignored.

### 6.4 Checksums

Use:

- SHA-256 for object identity when sealing/writing.
- CRC32 for independently accessed 64 KiB object regions.
- A separately stored checksum table.
- A header checksum.

On Load:

- Validate header.
- Validate section boundaries.
- Validate manifest references.
- Do not hash the entire object.

On first access to a region:

1. Bounds-check the requested range.
2. Validate the relevant region checksums.
3. Mark successful validation in a compact per-object bitset.

Keep this validation state per object, not per dictionary entry or posting.

Provide a full verification operation for tests and diagnostics.

### 6.5 mmap ownership

`MappedObject` owns:

- Mapping.
- Object metadata.
- Validation state.
- Store reader lease.

Readers borrow bytes through that owner.

Saving or compacting must never modify or truncate an object that may be mapped.

The only required unsafe mapping operation belongs in `backing.rs`, with its ownership invariant documented there.

---

## 7. Dictionary implementation

### 7.1 Structure

Implement immutable B+tree-style pages:

- Target page size: 8 KiB.
- Internal pages contain separator keys and child offsets.
- Leaf pages contain keys and values.
- Leaf values contain compact descriptors.
- Bulk construction proceeds from sorted entries.
- Opening reads the root descriptor.
- Queries descend only into required pages.

Use a cursor stack for forward and reverse traversal.

### 7.2 Key storage

Support:

- Inline fixed-width keys.
- Inline variable-length keys.
- Overflow references for keys too large for a page.

Long paths and string values must work. Do not impose a 512-byte key limit.

Preserve the original exact string bytes.

### 7.3 Comparator

Implement a borrowed encoded-key comparator.

Rules:

- Strings and IDs: existing lexical comparison.
- Numbers: existing `Number::cmp` semantics.
- Date: chronological comparison.
- Datetime: instant comparison.
- Unixtime: signed integer comparison.
- Paths: existing reversible query-key ordering.
- Grams: unsigned integer comparison.

For common numeric cases, decode directly into stack values.

Exact-decimal comparisons may use a small owned temporary initially. They must retain existing correctness.

Do not sort numeric encodings by their variant tag or raw bytes unless that encoding has been proven equivalent to `Number::cmp`.

### 7.4 Required operations

Implement:

```text
get
contains_key
lower_bound
upper_bound
range
forward cursor
reverse cursor
entry_count
```

Dictionary entry counts come from stored metadata.

A cardinality estimate must not require reading posting payloads.

### 7.5 Field directory

Assign each declared/inferred/custom binding a small field ordinal during sealing.

Store the field directory once per run.

The runtime may build a small field-name-to-ordinal map because its size scales with schema fields, not records or terms.

Do not reconstruct value dictionaries into that map.

---

## 8. Record sets and postings

### 8.1 `RecordSetRef`

Use a fixed 32-byte descriptor:

```text
tag:           u8
flags:         u8
inline_count:  u16
reserved:      u32
a:             u64
b:             u64
cardinality:   u64
```

Interpretations:

| Tag | Meaning |
|---|---|
| Empty | No members |
| All | Entire owning domain |
| Range | `a = start`, `b = exclusive end` |
| Inline | Up to four sorted `u32` members packed into `a` and `b` |
| Blocks | `a = block-directory offset`, `b = directory length` |

### 8.2 Block directory

A block covers IDs with a common:

```text
block_key = id >> 16
```

Store a sorted directory of fixed-size block descriptors.

Each descriptor includes:

- Block key.
- Codec.
- Cardinality.
- Payload offset.
- Payload length.

Implement three codecs:

1. **Array**
   - Sorted `u16` offsets.

2. **Bitmap**
   - Little-endian `u64` words.
   - At most 1,024 words.
   - Trailing zero words omitted.

3. **Run**
   - `(start: u16, length_minus_one: u16)` pairs.
   - Full 65,536-member run is representable.

Choose the smallest encoded payload.

Tie-break deterministically:

```text
Array → Run → Bitmap
```

Before choosing blocks, detect Empty, All, Range, and Inline forms.

### 8.3 File parts

A large physical file may be split across several contribution chunks.

Represent each contiguous chunk as a file part:

```rust
struct FilePart {
    file_id: FileId,
    content_generation: u64,
    local_record_start: u32,
    record_count: u32,
    segment_slot_start: u32,
}
```

A part belongs to one collection.

A run may contain multiple parts from one file.

This is a physical subdivision; the file’s new contribution is published atomically.

### 8.4 File-grouped posting

Use this persistent representation for ordinary term postings:

```text
FileGroupedPosting
├── all_parts: RecordSetRef over part ordinals
└── partial_parts: sorted array
      part ordinal -> RecordSetRef over part-local record offsets
```

A part appearing in `all_parts` must not also appear in `partial_parts`.

The set of matching parts is:

```text
all_parts ∪ keys(partial_parts)
```

Do not store a redundant third matching-part set.

For one-record-per-file collections, common terms naturally become compact part sets.

### 8.5 Query operations

Implement these directly on borrowed sets:

```text
contains
iterate
cardinality
intersection_count
union_into
intersect_into
difference_into
```

Required block combinations:

- Array/array.
- Array/bitmap.
- Bitmap/bitmap.
- Run/run.
- Array/run.
- Bitmap/run.

For operations requiring a result, write into query-owned scratch.

For count-only operations, avoid constructing a result set.

### 8.6 Compatibility with existing execution slots

Provide adapters that translate:

```text
part-local offset
    → segment_slot_start + offset
```

This allows existing execution paths to continue using segment-local `RoaringBitmap` intermediates.

The adapter must process borrowed blocks. It must not deserialize and cache every persisted posting as an owned Roaring bitmap.

### 8.7 Fixed field postings

Store fixed descriptors for:

- Presence.
- Null.
- Empty selection.
- Empty string where currently indexed.
- Boolean true.
- Boolean false.
- Structural facts.
- Logical eligibility.
- Partial-coverage domains.

Missing can be represented by complement within an explicit complete domain where valid.

Preserve current distinctions between:

- Missing path.
- Empty selection.
- Explicit null.
- Empty array.
- Empty string.
- Raw fallback record.

---

## 9. Record metadata and file catalogue

### 9.1 Record rows

Use fixed-width record rows.

A practical 48-byte row contains:

```text
part ordinal:          u32
local record ID:       u32
byte start:            u64
byte end:              u64
logical-ID offset:     u64
logical-ID length:     u32
diagnostic reference:  u32
record class:          u8
reserved:              7 bytes
```

Collection and content generation are obtained from the owning run/part.

Logical-ID strings reside in the run’s arena.

### 9.2 Logical-ID dictionary

Persist:

```text
logical record ID -> physical segment slot
```

Use the ordered dictionary implementation.

Also persist identity order for default ordering.

Do not rebuild an `id_lookup` hash table when opening.

Duplicate-ID checks during mutation probe existing run dictionaries and apply active-content filtering.

### 9.3 File catalogue

Use paged metadata with:

- 64 file rows per logical page.
- Fixed-width row headers.
- Separate variable-length payloads.
- Tombstones.
- Allocation high-water mark.

Each file row includes:

- File ID.
- Status.
- File version.
- Content generation.
- Size.
- Modified time.
- Optional created time.
- Native relative path.
- Query path key.
- Collection membership.
- Diagnostics.
- Fingerprint, where available.

Resident mutations copy the affected metadata page and its changed payloads.

Loaded catalogue pages remain mapped until replaced in a later publication.

### 9.4 Path access

The saved catalogue contains an immutable ordered path dictionary.

For live modifications, use a bounded metadata overlay:

```text
path -> new FileId or tombstone
```

Decision for version 1:

- Overlay maximum: 4,096 changed path keys.
- Publication shares the unchanged base.
- Exact lookup consults overlay before base.
- Range lookup merges overlay and base, suppressing shadowed entries.
- At the limit, bulk-merge into a new packed resident path dictionary.
- That operation reads metadata only.

This overlay is an explicitly bounded mutation structure, not a reconstructed full path index.

### 9.5 Native paths

Persist native paths losslessly:

- Unix: raw `OsStr` bytes.
- Windows: UTF-16 code units.

Record the platform in the manifest.

Keep the existing reversible query-path representation for comparisons and display.

Reject loading a store with an incompatible native-path encoding.

---

## 10. Sorting, relations, inference, and diagnostics

These are mandatory parts of persistence.

### 10.1 Sort storage

For every configured sort column, persist:

1. Key lookup by physical record slot.
2. Ascending ordered slots.
3. Descending ordered slots where current tie/null semantics require a distinct sequence.
4. Locale.
5. Sort options.
6. Collation version.

Keep lexical typed string dictionaries separate from ICU sort keys.

For variable-length sort keys:

```text
offset array + byte arena
```

For fixed-width keys:

```text
fixed-width array
```

Persist composite orders currently supported by the engine.

### 10.2 Sort execution

Replace direct uses of:

```rust
column.keys[slot]
&column.ascending
&column.descending
```

with:

```text
key_at(slot)
order_cursor(direction)
```

Broad top-k queries must be able to merge run cursors without materializing complete order vectors.

Selective queries must fetch keys for selected records.

### 10.3 Inferred orders

Current query-created inferred sort orders remain bounded query-derived caches.

Loading must not eagerly build them.

Persist inference summaries and coverage so the decision to create an inferred order does not require a source scan.

### 10.4 Relations

Persist:

- Forward adjacency offsets.
- Edge arrays.
- Target logical IDs.
- Array-position information.
- Presence/null/empty/unavailable domains.

Resolve relation targets against the current pinned publication.

Do not persist a bare target `RecordSlot` as the sole identity of a cross-run target.

Reverse relation access continues to use ID postings, now through mapped readers.

### 10.5 Diagnostics

Persist bounded source diagnostics and omitted-diagnostic counts.

Reading one diagnostic must not decode the complete catalogue.

### 10.6 Statistics

Persist the values needed for:

- Query planning.
- Index status.
- File counts.
- Record counts.
- Fallback counts.
- Estimated source bytes.
- Dictionary counts.
- Posting cardinalities.
- Storage breakdown.

Status polling must not walk all records, postings, or dictionary entries.

---

## 11. Replace the current published data structures

The main migration targets are:

```text
crates/core/src/posting.rs
crates/core/src/bigram.rs
crates/core/src/inventory.rs
crates/core/src/mutation_segment.rs
crates/core/src/index_catalog.rs
crates/core/src/sorting.rs
crates/core/src/relation.rs
crates/core/src/repository.rs
crates/core/src/run.rs
crates/core/src/posting_values.rs
crates/core/src/relation_query.rs
crates/core/src/lifecycle_caller.rs
```

### 11.1 Separate builder and reader types

Introduce clearly distinct names:

```text
PostingBuilder        / PostingView
BigramBuilder         / BigramView
SegmentBuilder        / FrozenSegment
RecordTableBuilder    / RecordTableView
SortBuilder           / SortColumnView
```

Do not keep a published `PostingIndex` that secretly decodes its entire saved state on first access.

### 11.2 `IndexCatalog`

Change `IndexCatalog` to expose reader handles.

It should retain:

- Schema interpretation.
- Binding selection.
- Coverage rules.
- Cost estimation.

It should no longer require `&RoaringBitmap` or `&BTreeMap` from persistent structures.

### 11.3 Error propagation

Mapped access can fail because of corruption.

Reader methods performing checked access must return a fallible result.

A failed posting read must not be converted into:

- An empty posting.
- A missing dictionary key.
- A successful zero count.

Propagate a typed storage error into the existing query error surface.

### 11.4 Repository compatibility

`IndexedRepository` currently dereferences into a resident `Repository`.

Replace frontend dependencies on complete resident structures with explicit inspection APIs.

Examples:

```text
collection_definitions()
collection_summaries()
file_catalogue_page()
directory_children()
diagnostic()
posting_values()
```

Keep schema/runtime definitions resident. They are small.

Do not create fake full vectors merely to preserve old indexing syntax.

Update examples and tests that directly inspect fields to use readers or explicit diagnostic enumeration.

---

## 12. Construction and updates

### 12.1 Fresh construction

Use the existing source preparation machinery.

Change finalization so it produces frozen segments directly.

Initial contribution targets:

- 2,048 files.
- 32,768 records.
- 16 MiB estimated encoded payload.

Seal when the first target is reached.

An individual oversized record is allowed to exceed the target. Account for it explicitly.

Large JSONL files must use the existing bounded framing path and preserve file-local record numbering across chunks.

### 12.2 Publication

A file replacement may produce several frozen segments.

Stage all of them, then publish one root containing:

- Updated file metadata.
- New content generation.
- New segments.
- Retirement of old content.
- Updated logical-ID visibility.

Cancellation before publication must leave the old publication intact.

### 12.3 Updates must not touch old term dictionaries

Normal replacement:

1. Validate handle.
2. Prepare changed file.
3. Build new contribution.
4. Validate logical IDs.
5. Update metadata and content generation.
6. Publish.

Do not delete old terms individually.

### 12.4 Deletion

Deletion:

- Tombstones the catalogue entry.
- Advances file version.
- Retires its content generation.
- Removes/shadows its path entry.

Existing mapped posting bytes remain untouched.

### 12.5 Move

Reuse the existing path-independence checks.

For a proven content-preserving move:

- Change path metadata.
- Change path lookup.
- Advance file version.
- Preserve content generation and search contribution.

Otherwise perform a normal replacement.

### 12.6 Verified-update APIs

Preserve public behavior and correctness of existing verified-update options.

For version 1, packed immutable segments may use append replacement when a previous slot-preserving optimization cannot operate on the new representation.

Return the actual strategy and fallback reason.

Do not retain the old native index representation solely to support that optimization.

---

## 13. Compaction

### 13.1 Algorithm

Implement source-free streaming compaction:

1. Select input runs.
2. Pin their publication.
3. Iterate their dictionaries in key order.
4. Merge equal keys.
5. Filter inactive content generations.
6. Translate physical slots into the output layout.
7. Write output packed structures.
8. Publish replacement runs atomically.

Do not parse source files during compaction.

### 13.2 Selection policy

Use size-tiered merging:

- Fresh runs are level 0.
- Four runs at the same level trigger a merge.
- Merge the four oldest eligible runs at that level.
- Output is promoted to the next level.
- Remove completely inactive runs without rewriting them.

Background scheduling:

- One compaction worker per index.
- At most one compaction publication at a time.
- Check cancellation between keys and output blocks.

### 13.3 Limits

Use:

- 64 searchable runs per collection as the foreground admission threshold.
- 128 MiB temporary builder/scratch budget per compaction.
- Immutable output storage accounted separately.
- Explicit accounting for resident input/output overlap and pinned prior views.

If admission reaches the threshold:

- Finish or perform required consolidation before admitting more runs.
- If the resource limit prevents progress, return a typed resource error while keeping the existing index usable.

Do not silently permit unbounded run growth.

### 13.4 RAM and persistence

Background compaction produces resident packed output.

It does not save automatically.

An explicit Save can write that output and reuse already durable objects.

Saving must not implicitly start a full compaction. Users should see and control the cost of those operations separately.

---

## 14. Core public API

Add:

```rust
impl Index {
    pub fn load_from(
        directory: &Path,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        sources: SourceRegistry,
    ) -> Result<Self, IndexError>;

    pub fn save_to(
        &self,
        directory: &Path,
        options: SaveOptions,
    ) -> Result<SaveReport, IndexError>;

    pub fn start_save(
        &self,
        directory: PathBuf,
        options: SaveOptions,
    ) -> Result<StorageTask, IndexError>;

    pub fn storage_status(&self) -> Result<StorageStatus, IndexError>;
}
```

Also provide a manifest inspection API that does not open source files:

```rust
pub fn inspect_store(directory: &Path)
    -> Result<StoredIndexInfo, IndexError>;
```

### 14.1 Save options

```rust
struct SaveOptions {
    frontend_metadata: Option<serde_json::Value>,
}
```

Limit frontend metadata to 1 MiB.

The core treats it as opaque data.

### 14.2 Progress

Use a typed phase enum:

```text
Preparing
WritingObjects
WritingCatalogue
WritingManifest
Committing
CleaningUp
Complete
Cancelled
Failed
```

Progress includes:

- Pinned publication ID.
- Objects completed/total.
- Bytes written.
- Bytes reused.
- Current phase.
- Elapsed time.

### 14.3 Save report

Include:

```text
directory
store ID
generation ID
publication ID
objects written
objects reused
bytes written
bytes reused
catalogue bytes
duration
unchanged/no-op
whether a newer live publication exists
cleanup outcome
```

### 14.4 Storage status

Include:

```text
origin: Built | Loaded
loaded directory, if any
resident packed bytes
mapped virtual bytes
searchable run count
current publication ID
last explicitly saved publication and destination
unsaved changes
freshness state
active save task
```

“Mapped bytes” must not be labeled RSS.

---

## 15. Save protocol and crash behavior

### 15.1 Save preparation

1. Pin the current complete view.
2. Reject saving an incomplete initial build.
3. Acquire the destination writer lock.
4. Validate or initialize the store marker.
5. Read the current generation, if present.
6. Create a uniquely named staging directory.

A store directory must be dedicated to an Objekte index.

Do not overwrite unrelated files.

### 15.2 Object writing

For each required object:

- Reuse it when the destination already contains the correct object.
- Otherwise stream/copy its exact packed bytes into staging.
- Verify bytes written against expected length.
- Flush and synchronize the object.
- Move it into `objects/<sha256>.obx`.

Existing published objects must never be overwritten.

Mapped objects copied to another store must be copied in bounded chunks, not with `fs::read` into one giant allocation.

### 15.3 Catalogue writing

Serialize the pinned current catalogue into a new packed object.

Bulk-build its path dictionary.

This operation may enumerate file metadata, but must not enumerate every term/posting/sort key.

### 15.4 Manifest

The generation manifest includes:

- Format/model versions.
- Platform/path encoding.
- Store and lineage IDs.
- Publication ID.
- Source root.
- Configuration identity.
- Source registration identities/versions.
- Collation and bigram representation versions.
- File-ID high-water mark.
- Catalogue object reference.
- Run object references and levels.
- Collection summaries.
- Frontend metadata.

The manifest must not contain an entry per record or term.

### 15.5 Commit order

Required order:

1. Synchronize new objects.
2. Synchronize object directory changes.
3. Write manifest.
4. Synchronize manifest and generation directory.
5. Write `CURRENT.tmp`.
6. Synchronize `CURRENT.tmp`.
7. Atomically replace `CURRENT`.
8. Synchronize store directory.

Use platform-appropriate atomic replacement. Never emulate replacement by deleting the valid `CURRENT` first.

Before the commit point, cancellation leaves the previous `CURRENT` intact.

After the commit point, report successful commitment even if cancellation arrives during cleanup.

### 15.6 Reader leases and cleanup

- A loader holds a shared `readers.lock` lease through its mapped-object owners.
- Saving uses a separate exclusive writer lock.
- Cleanup attempts an exclusive reader lease.
- If readers exist, skip cleanup and report retained data.
- Cleanup retains current and previous committed generations.
- Delete only unreferenced objects owned by this store.

This conservative first version avoids deleting files still used by another process.

### 15.7 Repeated save

For the same publication and same target generation:

- Return an unchanged report.
- Write zero object bytes.
- Do not rewrite the catalogue or manifest.

If another process changed the destination since the previous save, re-evaluate the target rather than assuming the old no-op result remains valid.

---

## 16. Load protocol

Implement this exact order:

1. Open store reader lease.
2. Read `STORE`.
3. Read `CURRENT`.
4. Read and checksum the referenced manifest.
5. Validate format/platform/configuration.
6. Open/map referenced objects.
7. Validate object headers and section bounds.
8. Create packed catalogue and run readers.
9. Create a new live `Index` instance.
10. Publish storage status with freshness unchecked.

Load must not:

- Call `Repository::open_with_sources`.
- Call initial-build APIs.
- Rebuild native posting maps.
- Rebuild native bigram maps.
- Rebuild complete logical-ID hash maps.
- Reconstruct sort vectors.
- Resolve all relation targets eagerly.
- Walk every record for statistics.
- Validate every payload block eagerly.

Small schema/binding maps and run descriptors are allowed.

---

## 17. Shared frontend location and opening layer

Add:

```text
crates/cli/src/index_location.rs
crates/cli/src/index_open.rs
```

The desktop already depends on the CLI crate, so put shared frontend policy there.

### 17.1 Location type

```rust
enum IndexLocation {
    SystemCache,
    Repository,
    Custom(PathBuf),
}
```

### 17.2 Resolved locations

| Selection | Directory |
|---|---|
| System cache | `dirs::cache_dir()/objekte/indexes/<source-key>/` |
| Repository | `<resolved-root>/.objekte/.index/` |
| Custom | Exactly the selected directory |

Default selection in GUI/TUI: **System cache**.

For multiple selected folders, repository location means the existing resolved common root.

### 17.3 Source key

Compute a SHA-256 key from a stable, length-prefixed representation of:

- Canonical source root.
- Source selection mode.
- Ordered selected collection paths.
- Explicit schema path, if any.
- Recursion choice.
- Alphabet.
- Auto-index choice.

Keep actual schema/model compatibility in the manifest configuration identity.

### 17.4 Saved frontend opening recipe

Save a versioned opening recipe in `frontend_metadata`:

```text
recipe version
source mode
canonical root
selected folder paths
explicit schema path
recursion
alphabet
auto-index
```

Encode native paths losslessly.

Do not save:

- Current query.
- Pagination.
- Search result state.
- UI theme.
- Verified-result cache contents.

### 17.5 Opening from only a custom index directory

This must work:

```sh
obj --load-index /some/index-folder --fts Goethe
```

The frontend:

1. Inspects the manifest.
2. Reads the saved opening recipe.
3. Resolves current schema/source registrations.
4. Builds runtime collection definitions.
5. Checks configuration identity.
6. Calls core Load.

Reading schema/configuration files is allowed. Reading source records or recursively discovering sources is not part of Load.

Programmatic core clients supply their own definitions and registry.

### 17.6 Generated-index exclusions

Exclude selected index directories from source discovery when they are inside the source root.

Apply this consistently to:

- Build.
- Reconciliation.
- GUI.
- CLI.
- TUI.

Keep generated-storage exclusions separate from semantic source configuration hashing.

Reject using the source root itself as the custom index directory.

---

## 18. CLI contract

### 18.1 Flags

Add:

```text
--load-index <cache|repo|PATH>
--save-index <cache|repo|PATH>
--check-index
```

Interpret exactly `cache` and `repo` as presets.

All other values are paths.

A literal relative directory named `cache` can be written as `./cache`.

Parse the argument as a native `PathBuf` so custom paths need not be UTF-8.

### 18.2 Semantics

| Invocation | Behavior |
|---|---|
| No storage flags | Existing source build in RAM |
| `--load-index X` | Load X |
| `--save-index X` | Build, then explicitly save X |
| Both flags | Load, optionally check, then save |
| `--check-index` | Reconcile before queries; requires Load |

Save occurs after opening/checking and before executing the query batch.

A Save failure returns operation failure and does not proceed as though persistence succeeded.

### 18.3 No-query invocations

Permit:

```sh
obj --repo ./data --save-index repo
```

This builds and saves without running a query.

Also permit:

```sh
obj --load-index ./saved-index
```

This loads and emits index status without requiring a query.

Keep the existing missing-query error for ordinary invocations without TUI or a storage operation.

### 18.4 Examples

```sh
# Build and save into the repository.
obj --repo ./archive --save-index repo

# Load from the repository index.
obj --repo ./archive --load-index repo --fts Goethe

# Build and save in the system cache.
obj --path ./documents --save-index cache

# Load from the matching cache location.
obj --path ./documents --load-index cache --fts Goethe

# Load a custom directory using its stored opening recipe.
obj --load-index /mnt/indexes/archive --fts Goethe

# Reconcile a loaded index and explicitly save the changes.
obj --load-index /mnt/indexes/archive \
    --check-index \
    --save-index /mnt/indexes/archive \
    --fts Goethe

# Start TUI from a saved index.
obj --load-index /mnt/indexes/archive --tui
```

### 18.5 Configuration overrides

When loading:

- Saved opening recipe supplies semantic index options.
- Explicit source paths may identify/confirm the source selection.
- Conflicting alphabet/schema/inference options return a clear argument error.
- Do not silently override a saved index’s interpretation.

Change `Arguments.index_alphabet` to an optional explicit value so omission can be distinguished from `--index-alphabet ascii`.

Fresh builds resolve omission to ASCII.

Perform validation before hydrating an effective opening specification from the saved recipe.

### 18.6 Protocol

Bump CLI protocol version from 4 to 5.

Add events:

```text
index_load_start
index_loaded
index_save_start
index_save_progress
index_saved
index_storage_error
```

For storage-only invocations, emit normal run start/end events around the storage events.

Include:

- Resolved directory.
- Publication/generation IDs.
- Timings.
- Written/reused bytes.
- Freshness state.

Keep diagnostics on stderr according to repository conventions.

### 18.7 Files to modify

```text
crates/cli/src/arguments.rs
crates/cli/src/loader.rs
crates/cli/src/execution.rs
crates/cli/src/output.rs
crates/cli/src/lib.rs
```

Use one shared open resolver for CLI, TUI, and desktop.

---

## 19. GUI contract

### 19.1 Open dialog

Add an **Index source** section:

```text
○ Build from source files
○ Load saved index
```

When loading, show:

```text
Location:
○ System cache
○ Repository: .objekte/.index
○ Custom folder [path] [Browse]

[ ] Check source files after loading
```

Default:

- Build from sources.
- System cache selected as the storage-location preference.
- Check-after-load unchecked.

A custom saved index can supply its source selection from the saved recipe.

Display the resolved directory before opening.

### 19.2 Index panel

Add a Storage subsection:

```text
Origin: Built in memory / Loaded from disk
Location: ...
Resident packed bytes: ...
Mapped bytes: ...
Runs: ...
Saved generation: ...
Changes: Saved / Unsaved
Source state: Unchecked / Checked / Checking
```

Actions:

- **Save index…**
- **Load saved index…**
- **Check for changes**
- Existing Rebuild.
- Existing Compact.

### 19.3 Save dialog

Use the same three location choices.

Show:

- Resolved path.
- Current publication.
- Whether an existing store is present.

The Save button starts a background task immediately after the user chooses the destination.

Display progress and cancellation.

Do not add an automatic-save checkbox.

### 19.4 Concurrency

All disk work runs on existing controller workers.

Use operation-generation checks so completion from an abandoned load/save cannot update a newer session.

Saving pins a view and must allow searches to continue.

Loading creates a replacement session; install it only after successful load. On failure, retain the currently open session.

### 19.5 Avoid rebuilding GUI catalogues on open

Current desktop `FileCatalogue` retains all file IDs, and `DirectoryCatalog` builds complete lists.

Replace eager opening-time enumeration with paged/lazy access:

```text
file_catalogue_page(filters, cursor, limit)
directory_children(prefix, cursor, limit)
```

Use:

- 256 file rows per UI page.
- Bounded recent-page cache.
- Path-index cursors for directory filtering.
- Stored summary counts for status.

Initial GUI opening must not enumerate every file merely to populate the sidebar.

Directory children can be fetched when the corresponding node is expanded.

### 19.6 Preferences

Persist:

- Preferred storage location.
- Last custom directory.
- Preferred build/load choice.
- Check-after-load choice.

Keep these in existing desktop preferences with serde defaults for older files.

Do not persist “save requested” as a future automatic action.

### 19.7 Integration files

```text
crates/desktop/src/backend/open.rs
crates/desktop/src/backend/index_lifecycle.rs
crates/desktop/src/controller/runtime.rs
crates/desktop/src/controller/mod.rs
crates/desktop/src/ui/model.rs
crates/desktop/src/ui/open_dialog.rs
crates/desktop/src/ui/panels.rs
crates/desktop/src/ui/preferences.rs
crates/desktop/src/ui/state.rs
```

Extend typed UI actions and updates for storage operations rather than encoding commands in strings.

---

## 20. CLI-TUI contract

### 20.1 Initial opening

Support all CLI storage flags with `--tui`.

Return:

- Existing progressive-build opening type for Build.
- A ready mapped repository for Load.

Loading should show:

```text
Loading saved index…
```

Checking, if selected, is a distinct subsequent phase.

### 20.2 Storage overlay

Add an **Index storage** action to the action picker.

Assign **F6** to open it.

Keep `Ctrl+S` assigned to saving the current query.

Overlay contents:

```text
Index storage

Origin: ...
Changes: ...
Source state: ...
Mapped: ...
Resident: ...

Location:
( ) System cache
( ) Repository
( ) Custom: [...]

[Save] [Load] [Check sources] [Close]
```

### 20.3 Worker integration

Add a dedicated storage worker result channel.

Storage messages include:

- Operation generation.
- Progress.
- Completion.
- Failure.
- Cancellation.

The terminal event loop must remain responsive.

Loading successfully replaces the repository and invalidates:

- Cached file pages.
- Search-result publication.
- Detail locators.
- Relation navigation state.
- Posting-value completion state.

Retain the query draft; if it is invalid in the new source, show its normal validation error.

### 20.4 File browser

Replace opening-time complete ID caching with the same paged catalogue API used by the GUI.

Do not materialize all paths to display the first screen.

### 20.5 Preferences

Store location and custom-directory preferences in existing TUI settings with backward-compatible defaults.

### 20.6 Integration files

```text
crates/cli/src/tui/mod.rs
crates/cli/src/tui/app.rs
crates/cli/src/tui/settings.rs
crates/cli/src/tui/view/mod.rs
```

Add focused storage-overlay tests alongside existing TUI input/state tests.

---

## 21. Implementation sequence

Follow this order. Each phase should compile and have focused tests before continuing.

### Phase 0 — Baseline and worktree

1. Read repository instructions.
2. Create integration branch/worktree for `mmap-index-v1`.
3. Record the actual base revision.
4. Run baseline workspace tests.
5. Capture baseline build/query benchmark results.
6. Record any pre-existing failures separately.

Use the repository’s required branch, worktree, commit, and push workflow.

### Phase 1 — Packed primitives

Implement:

- Backing.
- Checked byte access.
- Object header.
- Section directory.
- Checksums.
- Scalar codec/comparator.
- Record sets.
- Ordered dictionaries.

Acceptance:

- Resident and mapped readers produce identical results.
- Corrupt/truncated inputs return errors.
- Large string keys work.
- Numeric comparison matches existing `Number`.

### Phase 2 — One complete frozen segment

Implement:

- File parts.
- Record rows.
- Logical IDs.
- Typed postings.
- Bigram postings.
- Structural/coverage descriptors.
- Segment sealing.

Build a small fixture and query it entirely through readers.

Acceptance:

- Builder collections are dropped after sealing.
- Same bytes can be saved, mapped, and queried.
- No source parse occurs during reopen.

### Phase 3 — Replace typed and bigram query access

Migrate:

- `IndexCatalog`.
- Exact posting execution.
- Bigram candidate execution.
- Posting-value enumeration.
- Count-only paths.
- Active-domain handling.

Acceptance:

- Existing filter/FTS semantics pass.
- Input postings are consumed without full owning deserialization.
- Dictionary estimates read stored cardinalities.

### Phase 4 — Remaining index families

Migrate:

- Record access.
- Sorting.
- Identity order.
- Relations.
- Inference.
- Diagnostics.
- Status accounting.

Acceptance:

- All existing query families work on frozen resident segments.
- No retained duplicate native representation remains.

### Phase 5 — Lifecycle and compaction

Migrate:

- Initial build.
- Progressive build.
- Add/update/delete/move.
- Stable content generations.
- Pinned views.
- Compaction.

Acceptance:

- Updates do not edit old term dictionaries.
- Large-file replacement is atomic.
- Compaction performs zero source parses.
- Old pinned views remain valid.

### Phase 6 — Durable store

Implement:

- Manifest.
- Save protocol.
- Load.
- Reader/writer leases.
- No-op save.
- Cleanup.
- Storage diagnostics.

Acceptance:

- Crash/failure tests retain a valid prior committed generation.
- Load does no full reconstruction.
- Mapped base plus resident updates can be saved and reopened.

### Phase 7 — Shared frontend opening and CLI

Implement location resolution, recipes, CLI flags, protocol events, and standalone load.

Acceptance:

- All documented examples work.
- All three locations resolve identically across frontend callers.
- Explicit load failure never triggers a build.

### Phase 8 — TUI and GUI

Implement controls, workers, progress, preferences, and paged catalogues.

Acceptance:

- Save/load/check are usable through each interface.
- Opening does not eagerly rebuild complete frontend catalogues.
- Search remains usable during Save.

### Phase 9 — Performance and final validation

Run the full required matrix below.

Fix material regressions before declaring completion.

---

## 22. Required test matrix

### 22.1 Codec tests

Cover:

- Empty sets.
- Full sets.
- Singletons.
- Four-member inline sets.
- Ranges.
- Block boundaries around 65,535/65,536.
- Full-block runs.
- Dense and sparse sets.
- Every block-operation pairing.
- Reverse dictionary traversal.
- Long keys.
- Non-UTF-8 native paths.
- Integer/float/decimal comparison equivalence.
- Corrupt offsets, lengths, tags, and checksums.

### 22.2 Semantic differential tests

Compare three modes:

1. Fresh resident packed index.
2. Saved and mmap-loaded index.
3. Independent reference evaluator where available.

Cover:

- Exact values.
- Ranges.
- Boolean combinations.
- Negation.
- Missing/null/empty distinctions.
- Universal and existential arrays.
- Structural predicates.
- ASCII and Unicode FTS.
- Normal and skip bigrams.
- Custom operations.
- Raw fallback records.
- Sorting.
- Composite sorting.
- Relations.
- Posting-value counts.
- Inferred fields.
- Source bytes/logical/locator return modes.

### 22.3 Lifecycle tests

Cover:

- Add.
- Replace.
- Delete.
- Metadata-only move.
- Path-dependent move.
- Delete and re-add at the same path.
- Multiple collections sharing one file.
- Repeated replacement of one large JSONL file.
- Source check after loading.
- Save during concurrent publication.
- Pinned readers across update and compaction.
- Save to a second directory.
- Reopen after compaction.
- Repeated unchanged Save.

### 22.4 Fault injection

Inject failure after each save stage:

- Partial object write.
- Object synchronization failure.
- Catalogue write failure.
- Manifest write failure.
- Before `CURRENT` replacement.
- After `CURRENT` replacement.
- Cleanup failure.

Verify:

- Before commitment, old generation loads.
- After commitment, new generation loads.
- No manifest points to an incomplete object.
- Cleanup failure does not invalidate commitment.
- Cancellation never produces a falsely successful partial save.

### 22.5 Fast-load structural assertions

Expose counters and assert:

```text
source records parsed during load = 0
source records read during load = 0
recursive discovery calls during load = 0
native value dictionaries reconstructed = 0
native bigram dictionaries reconstructed = 0
full posting deserializations during load = 0
sort orders reconstructed during load = 0
```

These are correctness requirements, not merely performance goals.

### 22.6 Frontend tests

Cover:

- All three locations.
- Custom index-only opening.
- Saved recipe restoration.
- Configuration mismatch.
- No-query Save.
- No-query Load.
- CLI cancellation.
- TUI overlay navigation.
- GUI stale-worker completion rejection.
- Preference migration.
- No automatic save on close.
- Source/index directory exclusion.

---

## 23. Benchmarks and acceptance targets

Use the existing Musenalm benchmark suite and extend its loading mode. Do not invent a separate query workload.

Required measurements:

| Measurement | Modes |
|---|---|
| Complete fresh build | Baseline / packed resident |
| First searchable publication | Baseline / packed resident |
| Save | First / changed / unchanged |
| Load | Warm / cold index pages |
| First query | Warm / cold |
| Existing query suite | Resident / warm mmap |
| Mutation latency | Resident / mapped base |
| Compaction | Resident / mapped input |
| Memory | After build / after load / after queries |

Run both alphabets.

Disable verified-result caching for the main comparison.

### 23.1 Targets

Use these initial targets:

- Warm mmap Load, without source checking: **at least 3× faster than fresh build** on the representative fixture.
- Fresh packed build: **no more than 20% slower** than baseline.
- Warm query median geometric mean: **no more than 10% slower** than baseline.
- Investigate individual representative queries exceeding **25% regression**.
- Unchanged Save: **zero object bytes written**.
- Small changed Save: unchanged segment objects reused.
- Loaded heap must not scale with total term/posting count.
- Large-file replacement must not write or mutate old global term dictionaries.

These are acceptance targets, not results to assume.

If a target is missed, report measured values and the identified cause. Do not hide it with a cache or silently remove the affected benchmark.

### 23.2 Additional diagnostics

Record:

```text
mapped object count
mapped virtual bytes
resident packed bytes
builder peak estimate
query scratch peak estimate
bytes validated during load
bytes validated during queries
dictionary pages visited
posting blocks visited
objects written/reused
catalogue bytes written
compaction bytes read/written
run count by level
```

Cold-load tests must distinguish cold index pages from cold source files.

---

## 24. Final validation and completion

Required commands include:

```sh
cargo fmt --all
python3 scripts/format-rust-spacing.py
python3 scripts/format-rust-spacing.py --check
cargo test --workspace --locked
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked
```

Run the existing acceptance examples for ASCII and Unicode and the relevant build/query/lifecycle benchmarks.

For GUI validation, follow the repository’s maintained GUI environment guide and capture evidence of:

- Build mode.
- Load mode.
- Three location choices.
- Save progress.
- Successful reload.
- Unsaved changes after reconciliation.
- Error handling without losing the current session.

### Completion requires all of the following

- Published indexes use packed readers.
- Resident and mapped indexes share the same format.
- Loading avoids full reconstruction.
- Every current index family is supported.
- File updates operate through new contributions.
- Explicit Save works.
- Explicit Load works.
- All three location choices work.
- GUI, CLI, and TUI expose the operations.
- Query semantics match.
- Durability/failure tests pass.
- Benchmarks and regressions are documented.
- Integration branch and implementation report satisfy repository workflow.

**The implementing model’s central rule should be: build once into the final searchable layout, then query those bytes directly—whether they reside in RAM or in a mapped file.**

---

## Implementation report

**Task identifier:** `mmap-index-v1`  
**Completion date:** 2026-11-14  
**Base revision:** `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`  
**Integration branch:** `integration/mmap-index-v1`  
**Integration worktree:** `/home/simon/source/objekte-integration-mmap-index-v1`

### Summary

A new `crates/core/src/index_storage/` subsystem implements immutable packed
objects, checked resident/mapped access, lazy CRC validation, paged B+tree
dictionaries, record sets with array/bitmap/run blocks, typed and structural
postings, bigram dictionaries and arenas, record/file-part/logical-ID tables,
sort columns and order cursors, relation adjacency, coverage, paged file
catalogues, manifests, and the durable store protocol.

The core index now exposes explicit persistence:

```rust
Index::load_from(directory, definitions, options, sources)
Index::save_to(&self, directory, options)
Index::save_to_with_control(...)
Index::start_save(directory, options)
Index::storage_status()
inspect_store(directory)
```

The store uses `STORE`, `CURRENT`, immutable `objects/<sha256>.obx` files,
`generations/<uuid>/manifest.json`, staging directories, separate writer and
reader leases, reused content-addressed objects, atomic `CURRENT` replacement,
and conservative cleanup that retains the current and previous generations and
skips object deletion while readers are present. Repeated saves of the same
publication and target write zero object bytes.

Saved indexes reopen without source discovery or source-record parsing. Load
reconstructs the queryable repository directly from packed catalogue and run
objects, including file metadata, records/file parts, declared and observed
postings, inference metadata, custom bindings, bigrams, sort keys/orders,
relations, coverage, diagnostics, and statistics. No native snapshot object is
written or read. Packed readers are attached to loaded segments and used by
`IndexCatalog`/`BigramAccess` for typed and bigram candidate execution.

The desktop Open dialog now exposes Build vs Load, System cache/Repository/
Custom locations, a custom index path, and check-after-load. The desktop Index
panel exposes storage status and a Save action with progress and stale-worker
rejection. The TUI F6 overlay supports Save, Load, and Check; Load runs on a
worker, installs the replacement repository only after success, and ignores
stale completions by operation generation.

Each frontend save records a versioned `opening_recipe` in
`frontend_metadata`: source mode, canonical root, selected folders, explicit
schema path, recursion, alphabet, and auto-index. Native paths are hex-encoded.
`obj --load-index <custom-folder>` inspects the manifest, decodes the recipe,
rebuilds definitions and options, and calls core Load without source arguments.
`--index-alphabet` is optional so omission can be distinguished from an
explicit ASCII choice; conflicting explicit source interpretation returns an
argument error. Reading schema/configuration files is allowed; source records
are not discovered or parsed.

The CLI adds `--load-index`, `--save-index`, and `--check-index`, shared
`IndexLocation`/`index_open` helpers, protocol version 5 storage events, and
end-to-end tests for build/save/load/check/resave and failed-save behavior. The
TUI adds an F6 index-storage overlay with status, save progress, save, and
source-check actions.

### Commits

- `061856b9` — packed object format, backing/mmap checksums, scalar codecs,
  record sets, paged B+tree dictionaries, and their tests.
- `b90cf95d` — complete frozen segment builder/readers for records, file parts,
  logical IDs, typed postings, bigrams, sort columns, relations, coverage, and
  resident/mapped parity tests.
- `8dfa9e83` — paged catalogue, manifest, durable store, typed/bigram query
  surface, native source-free capture/restore, and core save/load APIs.
- `75ea09c2` — CLI storage flags, locations, opening helpers, protocol events,
  and end-to-end CLI storage tests.
- `b1ddae5c` — storage documentation, formatting, and the storage benchmark.
- `447b659e` — repository formatting.
- `6b86aa67` — TUI storage overlay and scope issues.
- `5fb81e64` — catalogue high-water preservation and desktop storage opening.
- `b9cebca6`/`065106fc` — borrowed native/packed posting and bigram access plus
  the packed catalogue bridge and differential tests.
- `9da6ce78`/`6e2230c0` — `IndexCatalog`/executor routing through packed access,
  with loaded-index execution proven after native postings and sources are
  removed.
- `0df7f5bf` — packed-only repository reconstruction from catalogue and runs;
  deletion of the native snapshot object.
- `17f5ace9` — independent content generations across replace, move, delete,
  and reload.
- `a1f6ddad` — desktop Open/Save storage controls and TUI Load with session
  replacement and operation-generation checks.
- `d06af8d2` — versioned saved opening recipe, custom-only `--load-index`
  resolution, optional explicit alphabet, and CLI/desktop recipe tests.
- `92de4202` — typed postings and bigrams remain mapped during Load; packed
  posting-value/relation-probe access and loaded re-save object reuse.
- `a5ddfc99` — packed-backed sort columns/orders and relation adjacency at
  Load, plus the warm/cold mmap load benchmark.
- `83e68e9e` — lazy packed record rows and logical-ID lookup, with zero eager
  reconstruction at Load.
- `eb3ebff0`/`d9ea138d` — source-free compaction of loaded packed runs,
  sealing packed output and covering add/replace/move/delete plus pinned views.

### Packed access abstraction

`crates/core/src/index_storage/access.rs` adds borrowed `PostingAccess` and
`BigramAccess` enums over native `PostingIndex`/`BigramIndex` and packed
`FieldPostingsView`/`BigramSectionView`. The abstraction exposes presence,
nulls, empty selections, structural facts, exact/not-equal/range queries,
lengths, value enumeration, distinct counts, and normal/skip bigram
union/intersection/count operations. It returns query-owned `RoaringBitmap`
results without caching decoded postings.

`PackedIndexCatalog` provides borrowed field/bigram/logical access over one
`FrozenSegment` without owning dictionaries. Differential tests in
`access.test.rs` compare native, resident packed, and mmap packed access for
string and numeric postings and for bigram candidates.

Loaded publications attach their mapped `FrozenSegment` readers to the
corresponding `CollectionState` and `PublishedSegment`. `IndexCatalog` now
selects `PostingAccess::Packed` for declared fields on those publications and
`BigramAccess::Packed` for their raw/custom text bindings. Executor typed,
FTS candidate, count-only, and value access calls route through the borrowed
access enums. A test removes the loaded native posting maps and source files;
the packed typed reader still answers the exact typed query. Differential
tests compare built-native and loaded-packed query hit identities and FTS
candidates.

`Index::load_from` now reconstructs the queryable repository directly from the
packed catalogue and run objects. It rebuilds file inventory/catalogue slots, coverage, diagnostics, and
statistics. The native snapshot object is no longer written
or read; a test asserts that the store has no state object.

Packed typed postings and raw/fallback/custom bigrams are no longer decoded
into native dictionaries during Load. Only small field/bigram descriptors and
inference coverage metadata are retained. Declared, observed, and named field
access resolves through `PostingAccess::Packed`; posting-value enumeration and
relation reverse probes use the same mapped reads; bigram candidates and
estimates use `BigramAccess::Packed`.

Stored sort columns and relation adjacency are also packed-backed. Load retains
column descriptors with empty native key/order vectors and reads keys or order
slots from mapped arrays through `SortColumn`/`SortOrder`. Relation declarations
retain empty native edge/bitmap storage and read edges or state probes from
mapped relation sections. Record rows and logical-ID lookups are also lazy:
`RecordStore` starts with the mapped record table and materializes individual
rows only when a locator or query touches them; `RecordIdLookup` resolves IDs
through the packed logical-ID dictionary. Load performs zero eager record-row
or logical-ID-map reconstruction. Re-saving a loaded index reuses the original
packed run object bytes instead of re-packing from native state.

### Validation

Commands run in the integration worktree:

```text
cargo test --workspace --locked --offline
cargo test --workspace --all-targets --locked --offline
cargo test -p objekte-search-core index_storage --offline
cargo test -p objekte-cli --offline
cargo clippy --workspace --all-targets --locked --offline
```

Outcomes:

- Workspace tests pass with no failures (924 passed). The core library suite
  reports 595 passed, 15 ignored; the CLI library reports 94 passed; the desktop
  library reports 166 passed, 1 ignored. The all-targets run compiles and tests
  examples and integration targets as well.
- The storage subsystem suite reports 79 passed, including dictionary,
  record-set, scalar, catalogue, store, fault-injection, mapped-parity, real
  index save/load, posting-value enumeration over packed dictionaries, loaded
  re-save object reuse, loaded packed compaction after add/replace/move/delete,
  pinned prior views, compaction/save, concurrent-publication, and
  background-task tests.
- CLI tests include the end-to-end storage protocol, TUI storage overlay/Load,
  and six custom-index recipe tests: custom-only query and storage-only opening,
  schema/inference/alphabet restoration, conflicting interpretation, explicit
  source confirmation, and no-op re-save. The desktop backend and Open spec have
  recipe-only load tests.

Storage benchmark, `cargo run -p objekte-search-core --example
bench_index_storage --release`:

```text
records=20000  build=37.77 ms  save=12.55 ms  load=1.59 ms  first_query=0.536 ms  noop_save=0.020 ms
objects_written=11  noop_bytes_written=0  load_speedup_over_build=23.80
records=100000 build=170.92 ms save=59.38 ms load=6.95 ms  first_query=0.843 ms  noop_save=0.054 ms
objects_written=50  noop_bytes_written=0  load_speedup_over_build=24.58
```

The first measurement is a 20,000-record JSONL corpus; the second uses
`OBJEKTE_STORAGE_RECORDS=100000`. Load includes opening the store and restoring
the query owners. Unchanged saves write zero bytes.

Warm/cold mapped-load benchmark, `cargo run -p
objekte-search-core --example bench_mmap_load --release` with `TMPDIR` on a
disk-backed filesystem so `fadvise(DONTNEED)` can evict pages:

```text
records=20000  warm mean=1.79 ms  cold mean=4.59 ms  cold/warm=2.56
records=100000 warm mean=9.13 ms  cold mean=19.89 ms cold/warm=2.18
```

Each entry is the mean of three loads. Cold runs evict every store file before
Load. The automated GUI matrix could not be run in this environment because no
display server or `xvfb-run` is available; desktop controller/UI tests pass
headlessly.

### Deviations and remaining limitations

1. **Packed owners are lazy after Load.** Typed postings, bigrams, sort
   columns/orders, relation adjacency, record rows, and logical-ID lookups are
   backed by mapped packed sections. No complete native dictionary, order
   vector, edge array, record vector, or ID map is rebuilt during Load; record
   rows materialize one at a time on access. Remaining work is broader
   fast-load/GUI validation, recorded in
   `docs/ai/issues/feat/mmap-index-packed-execution-migration.md`.
2. **Desktop/TUI storage controls and custom-only opening are implemented.**
   The desktop Open dialog exposes Build/Load, three locations, custom path, and
   check-after-load; the Index panel exposes storage status and Save with
   progress/stale checks. The TUI F6 overlay supports Save/Load/Check, with Load
   installed only after success and operation-generation rejection of stale
   completions. Saves include a versioned opening recipe in manifest frontend
   metadata; custom directories can open without `--path` or `--repo`, and
   conflicting explicit interpretation options are rejected. Resolved in
   `docs/ai/done/resolved-issues/mmap-index-frontend-storage-controls.md` and
   `docs/ai/done/resolved-issues/mmap-index-saved-opening-recipe.md`.
3. **Independent content generations are implemented.** Catalogue rows persist
   `file_version` and `content_generation` separately; replacement advances the
   latter, content-preserving moves preserve it, and delete tombstones retain
   it. The resolved report is in
   `docs/ai/done/resolved-issues/mmap-index-content-generations.md`.
4. **Compaction has a source-free packed path.** Loaded packed runs are
   materialized into bounded projection state, compacted without opening source
   files, then sealed back into packed runs (records, postings, bigrams, sort,
   relations, coverage, inference metadata). The temporary projection state is
   dropped after sealing, and subsequent Save reuses the packed output. A
   differential test covers loaded add/replace/move/delete plus pinned prior
   views and save/reload. The projection is currently materialized rather than
   consumed as a fully streaming cursor, so peak temporary memory during
   compaction still scales with the bounded batch.
5. **Formatting check caveat.** `cargo fmt --all` and
   `python3 scripts/format-rust-spacing.py` were applied. The repository's
   `format-rust-spacing.py --check` first runs `cargo fmt --check`, which
   conflicts with the two-blank-line convention it adds; the two-line style is
   retained in the committed files.

### Cleanup

The integration worktree and branch are retained for the user. Temporary task
notes were maintained under `docs/ai/memory/mmap-index-v1/` in the original
checkout and are removed after this report is pushed. No worktrees outside this
task were modified.
