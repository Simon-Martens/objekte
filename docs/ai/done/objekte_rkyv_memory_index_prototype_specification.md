# Objekte rkyv in-memory index prototype specification

## 1. Assignment and acceptance contract

Implement a working prototype in which completed structured posting dictionaries and the current file/record lookup catalog use `rkyv` archived collections. Make this the default representation on the prototype branch. Keep construction parallel and retain native mutable containers only while constructing or rebuilding components. Implement and validate addition, replacement and deletion of source files, and expose explicit index compaction through the existing lifecycle API.

All archive buffers remain in process memory. This assignment does not implement an index file format, saving, loading, memory mapping, filesystem synchronization, automatic spilling, eviction, or crash recovery. Reading source files to build or update the index remains necessary. Tests may create and modify disposable source fixtures. Index operations must not write the user's source files.

This is an implementation assignment for the local agent, not a request for another proposal. Follow the decisions below. Complete the specified implementation, tests, benchmark comparison and committed report; then stop. Do not merge into `master`.

Repository: `Simon-Martens/objekte`.

Reviewed `master`: **`2e18871c0bcfb9cf9335d9e73525c64f91c960cf`** on 2026-11-16. This includes the native segmentation work and the corrected scheduler-window assertion. Start from current `master` after verifying that this commit is an ancestor. If `master` has advanced, inspect the intervening changes and record the actual starting commit. Do not reset it to this reference revision.

The essential completion conditions are:

1. Both `Repository` construction/querying and lifecycle `Index` construction/querying use archived dictionaries by default, including initial progressive publications.
2. Completed declared, inferred and custom structured posting dictionaries use `ArchivedBTreeMap`; current file lookup and record identity lookup use `ArchivedHashMap` as specified below.
3. A query does not deserialize an archived dictionary into a native map, and no complete native dictionary remains alongside the archive as its real query implementation.
4. The catalog contains archived metadata, not merely a serialized copy that queries ignore.
5. Add, update and delete are correct for single-record and multi-record files, across collections and segments, including all affected query paths.
6. Compaction works from existing indexed contributions without source reads, removes retired records, and optionally combines small segments.
7. A deterministic mutation test suite compares incremental results with fresh reconstruction after every operation. It also has independent expected results and native dictionary comparison tests.
8. Total build timing includes archive construction, validation and publication. No performance report may hide that work in an untimed phase.

Memory optimization is not an acceptance gate. Large slowdowns, repeated full-catalog reconstruction and accidental serialization of parallel work are material findings and must be measured and explained. Correctness failures cannot be deferred as memory work.

## 2. Repository workflow and worktrees

Read `AGENTS.md` and all of `docs/ai/README.md` before implementation. This assignment explicitly authorizes adding `rkyv` and the internal adapters described here. Do not ask for separate approval for that dependency or these routine implementation decisions.

Use task identifier **`rkyv-memory-index-prototype`**.

- Integration branch: `integration/rkyv-memory-index-prototype`.
- Integration worktree: `../objekte-integration-rkyv-memory-index-prototype`, relative to the original checkout.
- For `/home/simon/source/objekte`, the integration worktree is `/home/simon/source/objekte-integration-rkyv-memory-index-prototype`.
- Baseline worktree, if needed: sibling `../objekte-baseline-rkyv-memory-index-prototype`, detached at the recorded starting commit.
- Keep the original checkout on `master`. Make no code changes, commits, or merges there.
- Never place task worktrees in `/tmp`.
- Do not reuse the completed native-segmentation repair branch for this task.

Inspect the configured remote, working-tree status and existing worktrees first. Fetch the configured remote, verify the intended base, and create the new branch/worktree without modifying unrelated files. If the task branch already exists, inspect and resume it rather than resetting or overwriting it. All tests, builds and implementation commits belong to the integration worktree. Benchmark the baseline in its own worktree and target directory so binaries cannot be confused.

Use one supervising agent. If the local orchestration supports subagents, delegate the independent test fixtures after the storage interface contract is committed; give each delegate its own `task/rkyv-memory-index-prototype/<subtask>` branch and sibling worktree. Only the supervisor writes to the integration worktree. Do not let concurrent agents edit the same lifecycle or storage interface. A single agent can execute the ordered phases if delegation is unavailable.

Push immediately after every commit, including integration and report commits. Preserve user edits, never force-push, and never commit temporary notes. Follow the repository's existing handling of an initially untracked task specification. At completion, preserve the full specification and append the implementation report in `docs/ai/done/`; commit and push it. Keep the integration worktree for the user's inspection and merge.

Do not start unrelated tasks discovered in `docs/ai/tasks` or `docs/ai/issues`.

## 3. Existing implementation to extend

The following are confirmed integration points at the reviewed revision. Read the complete functions before changing them; line numbers may move.

| File | Existing responsibility | Required work |
| --- | --- | --- |
| `crates/core/src/posting.rs` | `PostingIndex`, `ScalarKey`, value/length/structural dictionaries and `PostingValueCursor` | Separate mutable building from archived dictionary reads; retain predicate semantics |
| `crates/core/src/posting_values.rs` | Ordered value enumeration, counts and cursors | Support archived forward and descending traversal, exact liveness and pagination |
| `crates/core/src/index_catalog.rs` | Shared `IndexCatalog` query access for collections and segments | Extend the existing read interface; do not create a competing query engine |
| `crates/core/src/native_segment.rs` | `IndexedCollectionSegment`, `PublishedSegment`, `SegmentRecordAddress`, component descriptors and `SegmentPolicy` | Own archived components and lookup tables; retain qualified record addresses |
| `crates/core/src/mutation_segment.rs` | Native aggregation, finalization and partition projection | Freeze completed dictionaries and adapt source-free projection to archive readers |
| `crates/core/src/repository_incremental.rs` | Parallel segment jobs, ordered completion, private framed-file groups | Include freezing in worker jobs; preserve bounded outstanding work and atomic file publication |
| `crates/core/src/lifecycle_catalog.rs` | Sharded current file catalog, path lookup, file liveness and metadata | Replace native published catalog lookup maps with immutable archived shards |
| `crates/core/src/inventory.rs` | `FileEntry`, `RecordEntry`, `FileVersions`, `ActiveRecords`, lossless paths | Add storage conversions and retain current file-handle and source-revision semantics |
| `crates/core/src/lifecycle.rs` | `PublishedIndex`, mutation staging, file retirement, publication and maintenance | Publish archived catalogs and replacement segments as one revision |
| `crates/core/src/lifecycle_mutation.rs` | Existing `add_file`, `update_file`, `delete_file` integrations and mutation results | Reuse public operations; implement the replacement-segment policy below |
| `crates/core/src/lifecycle_compaction.rs` | Existing bounded `Index::compact()` and report | Preserve bounded behavior and add explicit full-compaction options |
| `crates/core/src/lifecycle_partition.rs` | Source-free selection, projection and dense slot remapping | Reuse it for replacement segments and compaction |
| `crates/core/src/source_access.rs` | Source access through inventory or current catalog | Continue resolving lifecycle reads through the pinned current catalog |
| `crates/core/src/model.rs`, `decimal.rs` | Exact numeric equality, ordering and hashing | Preserve these semantics in archived key comparisons |
| `crates/core/src/sorting.rs`, `relation.rs`, `relation_semijoin.rs`, `run.rs` | Sorting, relation resolution and specialized query paths | Migrate direct dictionary/lookup access without stale-snapshot fallbacks |

`Index::compact()` already exists. It currently performs bounded, source-free removal of retired contributions. Do not introduce a second independent compaction system or describe compaction as entirely absent.

The current default segment policy is 16,384 records and 16,384 files. Keep this policy and its existing oversized-indivisible-contribution rule. Test smaller targets to force segmentation; do not lower production defaults to make tests convenient.

## 4. Fixed scope of the storage change

### 4.1 What must be archived

Archive the completed, data-sized structures listed here:

- `PostingIndex.values`: scalar key to posting ordinal.
- `PostingIndex.lengths`: numeric key to posting ordinal.
- `StructuralFacts.postings`: structural key to posting ordinal.
- These dictionaries wherever they occur: declared, inferred, custom/named and stored reverse-relation postings that use the same representation.
- Current file catalog: file-ID lookup, exact path lookup, ordered path lookup and file metadata rows.
- Each segment's record-ID lookup and file-ID lookup.
- Each segment's record metadata rows and per-file record ownership information needed for locating, scanning and returning records.

Use `ArchivedBTreeMap` for ordered dictionaries and `ArchivedHashMap` for equality-only ID/path lookup. Store ordinal-indexed metadata in archived vectors. These are real query representations, not unused export copies.

Do not mechanically replace every `BTreeMap` in the repository. Parsed `Value::Object`, schema/configuration maps, temporary candidate sets such as `SegmentRecordSet`, build profiling reports, mutable inference observations and temporary aggregation maps can remain native. Record the disposition of every retained data-sized map in the completion report so a hidden native query dictionary is not mistaken for a builder.

### 4.2 Bitmap payloads and other existing components

**Retain the existing native `RoaringBitmap` representation and operations for this prototype.** Move each completed dictionary's bitmap values into an immutable ordinal-indexed vector, owned together with its archived dictionary. Archived dictionary values are checked `u32` ordinals into that vector.

This decision avoids inventing a bitmap archive adapter and avoids serializing/decompressing every bitmap merely to test archived tree lookup. It does not expand postings into one entry per record. It is explicitly an in-memory prototype: the complete component is the archive plus its owned native bitmap payload vector. The archive alone is not yet a complete on-disk component. State this in the report; do not claim persistence readiness or zero-copy bitmap loading.

Keep bigram `HashMap`/Roaring structures, normal and skip-1 gram generation, sorting key arrays and relation adjacency arrays in their existing representation. They must still be correctly projected and remapped when a segment is rebuilt. Their future archival is outside this assignment. Small field-to-component dispatch maps may remain native metadata.

The catalog differs from the bitmap exception: its lookup maps and authoritative stored metadata must be archived. Small decoded compatibility views of metadata may be cached as specified in section 8. They must not reconstruct lookup maps.

### 4.3 Default behavior

On the integration branch, archived dictionaries are the default completed representation. Do not hide them behind an opt-in backend flag, leave ordinary builds native, or change only a benchmark example. Native containers remain permitted as private builders and unit-test oracles.

Do not add a public backend-selection framework. Compare performance against the baseline commit rather than maintaining a second production engine solely for comparison. Keep public query syntax and result semantics unchanged. Existing CLI/TUI/desktop users should require no new storage argument; adjust internal call sites where needed and compile all workspace targets.

## 5. Dependency, ownership and checked access

### 5.1 Dependency

Pin `rkyv` to `=0.8.18` for this experiment and commit the lockfile. Use its `std`, `bytecheck`, `aligned`, `little_endian` and `pointer_width_64` features explicitly with default features disabled. The 64-bit archived pointer setting avoids introducing a small relative-offset limit as an unrelated prototype constraint. This is not a durable file-format commitment.

Use the 0.8 API, including `rkyv::to_bytes`, `rkyv::access` and `rkyv::util::AlignedVec`. Do not copy 0.7 examples using obsolete API names. No database, mmap, allocator replacement, or bitmap dependency is authorized by this task. Transitive dependencies required by `rkyv` are included in the authorization.

### 5.2 Archive owner

Add one small private storage module, for example `archive_storage.rs`, with a typed owner for an immutable aligned byte buffer. Use an `Arc` around that owner for sharing. Keep the owning `AlignedVec` allocation intact after freezing.

Required invariants:

1. Construction serializes a known storage type into aligned owned bytes.
2. Construction performs checked `rkyv::access` once and the applicable semantic checks before the owner becomes publishable.
3. Successful validation is associated with this exact owner and storage type.
4. No public or internal mutation API exposes the bytes after validation. No append, resize, seal mutation, `get_mut` escape or writable slice is allowed.
5. A root accessor derives its reference from `&self` and the owned bytes. The reference cannot outlive the owner.
6. Repeated accesses do not repeat whole-component validation.
7. Sharing and cloning duplicate ownership references, not archive bytes or bitmap vectors.

After successful checked construction, a single audited private accessor may use `access_unchecked` against the unchanged aligned buffer. Its safety comment must establish type, root position, alignment, validation, immutability and lifetime. Do not scatter unsafe access across query code. Do not store an invented `'static` reference, use `transmute` to extend a lifetime, or make a self-referential struct with a raw root pointer.

Do not convert `AlignedVec` into `Vec<u8>` or `Box<[u8]>` for storage: those conversions do not preserve the required alignment guarantee. Do not cast native `BTreeMap` memory into archived memory.

Validate at finalization for this prototype, rather than on first query. Everything is already in memory; this keeps validation failures out of query execution and makes construction timing honest. Components stay owned and ready until their publication and all readers release them. No LRU/eviction design is required.

### 5.3 Semantic validation beyond byte validity

Byte validation alone does not establish Objekte invariants. During freezing verify:

- Dictionary iteration is strictly increasing under the actual logical comparator.
- Every posting ordinal is in bounds, and the expected entries/payloads correspond exactly.
- Every posting contains only slots in the segment's record domain.
- Archived record rows have the expected dense local slots and valid file/collection ownership.
- ID lookup resolves to the matching row and preserves duplicate-ID rejection rules.
- File ownership routes reference the correct segment, collection, local slots and file incarnation.
- Archived paths round-trip losslessly; exact and ordered path lookups agree with the row.
- Structural flags, completeness and inference coverage metadata remain internally consistent.

Validate inputs and return structured errors before publication. Do not turn archive failures into panics or a silent fallback to the native query representation.

## 6. Posting dictionary representation and access

### 6.1 Builder and reader separation

Introduce explicit private builder and completed-reader types. Existing native `PostingIndex` construction can be adapted into `PostingIndexBuilder`; the completed `PostingIndex` should expose read operations backed by archived components. Keep conversion localized instead of wrapping every public method in a broad generic storage framework.

A completed field component conceptually owns:

```rust
struct ArchivedPostingComponent {
    archive: Arc<ValidatedPostingArchive>,
    bitmaps: Arc<Vec<RoaringBitmap>>,
    // Existing immutable field/capability metadata, if not inside the archive.
}
```

The storage root contains `BTreeMap<StoredScalarKey, u32>`, `BTreeMap<StoredNumber, u32>` and the required structural dictionary before serialization; their archived fields are `ArchivedBTreeMap`. Native storage structs are temporary serialization inputs, not retained query objects.

Use a single deterministic bitmap ordinal space per component. Consume the native value map in key order, move each bitmap into the payload vector, and assign an ordinal. Do the same for lengths and structural facts. Presence/null/empty-selection bitmaps can remain direct immutable native fields. Do not clone all bitmaps while converting the dictionaries.

A conversion can construct a temporary map of stored keys and ordinals and serialize it normally. This is the first implementation, because it is straightforward to validate. Once correct, use `serialize_from_ordered_iter` if profiling shows that constructing this second temporary tree materially affects build time. That optimization must preserve the same representation and tests; do not replace the required implementation with an untested custom serializer overnight.

### 6.2 Required read surface

Preserve or adapt the existing operations in `posting.rs`: `value_posting`, `value_cardinality`, `value_range`, `for_each_value`, `for_each_length`, restricted unions, estimates, exact-set evaluation and domain validation. `IndexCatalog` must continue to be the common query entry point.

Equality lookup performs an archived tree lookup and indexes the owned bitmap vector. It must not construct a native map, serialize the query key, clone a bitmap just to borrow it, or validate the component again.

Ranges use the archive's comparator-aware access where needed. Convert query bounds once per operation. Returning an owned public key for value enumeration is acceptable where the current API already returns owned keys; cloning keys for every comparison is not.

Read-only maintenance iterators must expose keys and bitmap references from the same component owner. Temporary builders used by projection/merge must be explicit and short-lived. Code must not mutate a published archived dictionary through a compatibility method.

### 6.3 Descending traversal is not a mechanical `.rev()` conversion

The inspected rkyv 0.8.18 B-tree iterator implementation is forward-only; it has no `DoubleEndedIterator`/`next_back` implementation. Current Objekte `PostingValueCursor` uses a reversed native range. Do not assume that replacing the type name will compile or preserve descending-page complexity.

For this prototype, store an additional **archived ascending entry vector** for each ordered dictionary that needs reverse traversal. Each entry contains the same stored key and posting ordinal. Populate it in the same ordered pass as the tree.

- Equality and ordinary forward tree ranges must actually use `ArchivedBTreeMap`.
- Descending traversal binary-searches the archived ascending vector for the requested lower/upper bounds, then visits the resulting index interval backwards.
- Apply inclusive/exclusive bound semantics exactly; support empty, unbounded and reversed-empty ranges.
- Do not collect an entire dictionary or range into a temporary vector for each descending query.
- Keep cursor tie-breaking and exact counts unchanged.
- Verify once during freezing that the vector and tree contain identical keys and ordinals.

This intentionally duplicates key storage in the archived representation to keep the implementation safe and descending access efficient. Account for its bytes and construction time separately. Do not conceal it as part of a native-map fallback. Do not fork rkyv or traverse its private node layouts.

## 7. Exact key semantics

Incorrect ordering produces silent missed ranges, so implement and test this before migrating query execution.

### 7.1 Scalar keys

Define private stored equivalents of the existing scalar variants, with the existing variant ordering: string, ID, number, boolean, date, datetime and Unix time. String and ID must remain distinct typed values even when their text matches.

Store strings as archived strings, dates as the existing date's Julian-day integer, datetimes as an exact UTC instant in nanoseconds plus the original offset in seconds, and Unix time as `i64`. Datetime comparison uses the instant, not the presentation offset; retaining the offset preserves the selected key's existing returned representation. Conversion back to public values must preserve the existing observable semantics; do not introduce locale collation into scalar equality/range dictionaries. Sorting continues using the existing ICU-derived sort keys and options.

Use borrowed query-to-archived comparisons. Do not allocate a `String` from an archived string to compare it with `&str`.

### 7.2 Numbers

`Number` currently contains `Integer(i128)`, `Float(f64)` and `ExactDecimal { negative, digits, exponent }`. Its `Ord` deliberately compares across representations. Deriving `Ord` for a stored enum would instead order by variant tag and is wrong.

Store the same three numeric cases. Implement native-stored, archived-archived and query-archived ordering through one shared comparison routine. Factor a borrowed numeric view with primitive integer/float fields and a borrowed exact-decimal digit string. Keep the common integer/integer, float/float and integer/float cases allocation-free. For decimal comparisons, share the existing exact logic and preserve its resource limits; do not add archived-string cloning or parsing once per tree node. Existing exceptional cross-representation conversion costs can remain for this prototype, but measure them separately and prepare the query-side conversion once where it is reused. Do not redesign numeric semantics as part of the storage experiment.

Required properties:

- `1`, `1.0` and an exactly equal decimal compare equal under the existing policy.
- Negative zero and positive zero follow current `Number` semantics.
- Values around and beyond `2^53` do not collapse through `f64` conversion.
- `i128` boundaries, huge exact integers, fractional decimals, large positive/negative exponents and negative values retain their order.
- Nonfinite floats remain rejected by index admission; do not create a new accepted numeric domain.
- Query-bound comparisons and tree key comparisons agree in both operand directions.

Do not encode all numbers as JSON text and compare lexicographically. Do not convert every numeric key to decimal strings merely to avoid implementing the existing fast numeric cases. Do not change `Number` equality or hashing to make an archived derive compile.

## 8. Archived catalog and record location

### 8.1 Preserve existing identity contracts

Keep the existing public `FileHandle { instance_id, file_id, version }`, monotonically advancing `FileVersion`, `IndexRevision`, and segment-qualified `SegmentRecordAddress` model.

Do not introduce a second global record-numbering system. The internal indexed-record address is `(collection, segment, local_slot)`. A replacement segment has a new `SegmentId`, so a reused local slot cannot be confused with an old record. The logical `RecordId` may remain the same across a file update. File version tokens must advance even if modification time and length happen to remain equal.

Preserve duplicate logical-ID rules within a collection. Allow the same logical ID in different collections where current behavior allows it. Never put unqualified local slots from different segments in one bitmap.

### 8.2 Catalog shards

Replace the current published native map overlays with immutable archived shards and shallow shared directories. Do not add an unbounded chain of base-plus-delta overlays.

Use these fixed layouts:

1. **File metadata shards:** contiguous ranges of 1,024 `FileId` values, selected by `file_id / 1024`. A shard contains an archived file-ID hash map to row ordinal and an archived row vector. Store active/deleted state, lossless relative path, `SourceRevision`, created time, collections, diagnostics, omitted-diagnostic count, fingerprint and configuration identity. Keep the existing shared `FileVersions` chunks as the authority for incarnation tokens; do not duplicate mutable version authority in the archived metadata rows. Primitive version arrays are an intentional native control-metadata exception, not a retained lookup map.
2. **Exact/ordered path shards:** 1,024 hash-selected shards using a consistent existing-style shard function over the canonical query-path string. Each shard contains an archived hash map from path string to active `FileId`, and an archived B-tree for ordered path comparisons. Exact lookup visits one shard. Ordered path comparisons visit the shards' relevant ranges and merge according to existing ordering/limit semantics. A small path-shard directory is native publication metadata.
3. **File ownership shards:** contiguous ranges of 1,024 `FileId` values, independently replaceable from file metadata. Each row records the current file incarnation's segment contributions, grouped by collection and `SegmentId`, with sorted local slots. Archive the lookup and row vectors. Keep this separate so rebuilding a segment does not require rewriting unchanged path strings and diagnostics.

Empty shard instances can be shared. File IDs are not reused after deletion; keep the next-ID counter and version history needed to reject stale handles. Retired rows may be reduced to tombstone metadata, but compaction must not make an old handle address a later file.

Ordered path lookup must retain the existing bounded-optimization contract: if its examination budget is exceeded, report that the optimization is unavailable and let the normal predicate path finish. Never return a truncated set as though it were complete. Merge path results across shards with the current ordering rules; hash-shard iteration order is not result order.

For a mutation, materialize only affected shard contents into temporary native builders, apply all changes for that shard as a batch, archive/validate it once, and replace its `Arc` in a new directory. Untouched shards retain pointer identity. No per-record archive rebuild and no full catalog reconstruction for one changed file.

For initial construction, aggregate each shard once and finalize independent shards through the existing worker pool. Do not insert one file at a time into a published archive or rearchive a whole shard after every discovered file. Preserve the existing distinction between discovered files, processed membership and completed initial construction.

The discovery `FileInventory` may use native maps while building. After the current archived catalog becomes authoritative, release its obsolete native path dictionaries or make the inventory a facade over archived lookup. Do not keep a second completed native catalog for hidden query fallbacks. A retained file-row compatibility vector is permitted under section 8.3.

### 8.3 Lossless paths and metadata access

Use the existing `path_repr::query_path` representation for query ordering and a lossless native representation for opening files. On Unix, store native path bytes; on Windows, store native wide units under the appropriate compilation target. Preserve existing normalization and root-relative path validation. Do not use `to_string_lossy`, unescape a query path by guessing, or silently reject valid non-UTF-8 filenames.

If existing public/read APIs need `&FileEntry` or `&RecordEntry`, a shard/component may have a lazy `OnceLock<Vec<...>>` compatibility cache built from its archived rows. Archive lookups still locate row ordinals, and the cache only supplies native metadata objects. It must not contain a native hash map or B-tree. Cache initialization must be synchronized, performed at most once per owner, and accounted for when measuring first-use cost. Prefer borrowed accessors for hot primitive fields. Do not perform full-row deserialization for every candidate.

### 8.4 Segment record catalog

Store each segment's record rows in an archived vector indexed by local slot. Include logical ID, file ID, source byte range, source revision, record class and diagnostic reference. Archive `id_lookup` and `file_index` as hash maps to local row ordinals; store per-file slot lists in archived vectors.

Use dense local indexing for a candidate already identified by `(segment, slot)`. Do not hash the logical ID again for every candidate. External-ID and relation resolution use the archived identifier lookup and the pinned liveness rules.

The current file catalog resolves active paths and revisions. Segment record metadata retains the source incarnation/range that was indexed. Those must agree before reading current source bytes; do not reopen a record with a different file version's offsets.

### 8.5 Catalog authority and liveness

The catalog determines whether a file incarnation remains current. Retain the existing per-segment `ActiveRecords` masks as an eagerly maintained execution index of record validity. They are publication metadata outside the immutable payload.

Updating/deleting a file looks up its ownership row and visits only those segment descriptors to retire their local slots. It does not open or rebuild those segments' posting components. The catalog row, ownership row and liveness masks change in one staged root. Before publishing, require agreement between catalog membership and masks for touched files.

Build a small publication-level `SegmentId -> position` directory for direct routing if needed; this is control metadata, not an alternative record catalog. Never assume vector position equals segment ID. A segment containing records from several files can be partially live.

When replacing/merging segments, update ownership rows for **all surviving files whose addresses moved**, not only the file that triggered the operation. Batch those ownership-shard updates. This unavoidable work must be included in mutation and compaction timings. Omitting it would make the next update/delete operate on stale addresses.

Remove membership entries for retired segment IDs from the new publication. Old views keep their own catalog, ownership and masks. Keep a dirty-segment set for cleanup; deduplicate it and remove stale entries when a segment is replaced.

## 9. Construction and publication

### 9.1 Freezing boundary

The normal pipeline remains: parse/extract, aggregate native segment contributions, finalize component semantics, freeze dictionaries/record metadata, validate, return a completed segment, publish.

Archive work is part of each existing segment-finalization job. Do not return native completed segments to the coordinator and then archive them serially. Independent field components may freeze in parallel through the existing Rayon pool. Do not create an additional worker pool per component or per segment.

Keep native builders uniquely owned and move data where possible. Drop temporary maps after the archived dictionaries and moved bitmap vectors are constructed. Sharing a published component must not deep-clone its payload.

Global inference admission can finish later than a local segment. Preserve the existing aggregate inference observations and policy. If final admission changes a component, replace only that component/capability metadata before its final publication; do not silently retain an admitted dictionary that was rejected or rearchive unrelated text/sort components. Never infer completeness merely from the presence of some short values.

### 9.2 Parallelism and framed files

Preserve `SegmentBuildScheduler` and its existing bound on submitted completions not yet consumed: `workers.saturating_mul(2).max(1)`. The entered-but-not-returned job counter is not a worker-thread count and can exceed the configured worker count with nested Rayon work. Do not reintroduce the incorrect assertion that it must be `<= workers`.

Support 1, 3, 8 and 32 configured workers where permitted by the existing worker resolver. Do not impose a new cap of eight, change CPU detection, serialize nested work or change segment sizes to produce favorable benchmark numbers.

For JSONL/framed files spanning segments, seal and submit components while reading continues, keep completed segments private until whole-file validation completes, then publish the group atomically. On extraction, archive or worker failure, retain the first meaningful error, drain every submitted completion and publish none of the failed file's contributions.

Initial progressive publications must contain only sealed archived dictionaries. Catalog metadata for discovery may be staged incrementally, but a reader must never observe a mutable builder or a segment whose catalog routes are missing.

### 9.3 Adoption and ownership

`Repository` and `Index` must use the same archived component owners. Adopting a completed repository into an index transfers or shares the archived payloads; it must not deserialize/rearchive them.

The retained repository configuration/runtime object must not permanently pin obsolete segment payloads or the initial native query dictionaries after lifecycle ownership transfer. Preserve the completed repair's ownership guarantees. Pinned user views legitimately retain older archives; an unrelated permanent base reference must not.

## 10. File mutation algorithms

### 10.1 API contract

Reuse these existing operations and error conventions:

```rust
Index::add_file(&self, path: &Path) -> Result<FileMutationResult, IndexError>
Index::update_file(&self, handle: FileHandle, path: &Path)
    -> Result<FileMutationResult, IndexError>
Index::delete_file(&self, handle: FileHandle) -> Result<DeleteResult, IndexError>
```

The application has already created or changed the source file before add/update. Delete invalidates indexed state; it does not remove the source file. Preserve `update_file_from_bytes`, moves and verified mutation modes already exposed by the library; route their completed contributions through the same storage/publication logic. Do not silently disable existing modes to make the prototype pass.

Use current prepare/stage/publish revision checks and file-handle validation. The writer publishes one root containing all changed collections, catalog shards, ownership routes, masks, caches' generation identity and replacement segments. Readers see an old or new publication, never an intermediate mixture.

If the existing `FileMutationStrategy` variants no longer accurately describe a destination rebuild, add an explicit `RebuildSmallestSegment` variant and update workspace consumers. Keep `AppendReplacement` for operations that only create new segments. Report real work; do not continue labeling every operation as a cheap append. This additive enum change may require exhaustive-match updates, which belong to this task.

### 10.2 Smallest-segment selection

For each affected collection, use prospective liveness after retiring the old incarnation. A candidate is an existing segment with at least one live record and capacity for at least one incoming indivisible contribution under the current record/file targets. Select the candidate with the smallest live-record count; break ties by `SegmentId`.

Use retained live file membership when evaluating the file-count limit. Do not select by historical allocated slot count. Empty retired segments are removed as metadata; treat an empty collection as having no destination.

If every existing segment is full or the next contribution cannot fit, construct a new segment. Do not rebuild a full segment merely to add one record and split off another. Oversized indivisible contributions follow `SegmentPolicy`'s existing explicit exception, and splittable multi-record inputs continue to be split.

Rebuild at most one existing destination segment per affected collection per public file mutation. Fill its available capacity and place the remaining incoming contributions into new bounded segments. Do not repeatedly choose and rebuild the same newly produced segment for each record in one file.

### 10.3 Add

1. Pin the current root and normalize/validate the path using current rules. Reject an active path collision.
2. Allocate a never-used `FileId` and initial version in private staging; preserve collection selection, adapter validation, diagnostics and identity policy.
3. Read/prepare the new file once. For multiple collections reuse the existing extraction/preparation workflow; do not reopen it separately for each field index.
4. For each affected collection, choose the smallest eligible segment as above.
5. If selected, project that segment's live contributions into a temporary builder using stored postings/metadata; append the new prepared contributions and partition as necessary. If none is eligible, construct new segments directly.
6. Freeze the replacement/new components, assign new segment IDs, construct the updated file and record catalogs, and update all moved survivors' ownership routes.
7. Perform existing duplicate-ID and publication checks. On any error discard staging and leave the published root unchanged.
8. Publish once and return the accepted new file handle and accurate mutation counts.

### 10.4 Update/change

1. Validate the caller's instance, current file handle and expected publication. Capture the old file incarnation's ownership entry.
2. Read and fully prepare the replacement file before making old records invisible to any published reader.
3. In the private next root, retire all local slots listed for the old incarnation across every collection/segment. Mark affected original segments dirty. Remove old path membership if the update is also a move.
4. Advance the file version even when source size and timestamps match. Use the same stable `FileId` where current update semantics do so.
5. Add the new contributions using the smallest-segment algorithm against this prospective live state. Old logical IDs are no longer live duplicate candidates; unrelated live duplicate IDs still are.
6. If the selected destination is one of the old file's segments, its projection must exclude the retiring old records. Never project them and then accidentally append both versions.
7. Update the catalog metadata, active path index, ownership directory and masks; atomically publish one revision.

A replacement can add/remove logical records, change their IDs, change byte offsets/lengths, produce no records, become broken/fallback data, or change collection membership. Apply existing source-adapter validation behavior exactly. Malformed input that the existing contract retains as searchable fallback remains an accepted replacement with appropriate status. A genuine preparation/publication error must leave the old index publication intact. These are different cases and need different tests.

Do not require caller-supplied old bytes for the normal append/rebuild policy. Old indexed contributions are sufficient for survivor projection; old source bytes may already be gone.

### 10.5 Delete

1. Validate the handle and locate current catalog ownership. No source open is needed; the file may already be absent.
2. Privately retire its slots in all owning segment masks, remove the active exact/ordered path entries, retire the catalog row, advance/retain incarnation information as required for stale-handle rejection and remove current ownership routes.
3. Add affected mixed-live/dead segment IDs to the dirty set. Drop all-dead segment references from the new root as a metadata operation, remove their now-obsolete dirty entries, and do not rebuild mixed live/dead segment payloads during ordinary deletion. Count retired records before removing those references.
4. Publish once and report the exact retired-record count.

Repeated delete and an update through a stale handle must follow existing errors, not mutate another file or silently report fabricated work. Re-adding the same path creates a new file identity according to the existing add contract.

### 10.6 Source-free destination reconstruction

Extend `PartitionRouting` and the existing `project_*_for_partitions`/`compact_partition_with_postings` machinery. Read archived keys and the owned native bitmap payloads through the new read interface; do not reconstruct a whole collection-wide dictionary.

Prepare explicit mappings from each input `(SegmentId, local_slot)` to an output `(new SegmentId, local_slot)` for surviving records. Remap every slot-bearing structure: declared/inferred/custom postings, lengths, structural facts, presence/null domains, raw/fallback/custom text bigrams, normal/skip grams, record rows, file slot lists, identity order, sorting orders/keys and relation source positions. Preserve logical target IDs for cross-segment relation resolution.

When merging keys from multiple inputs, combine remapped bitmaps for equal logical keys and remove empty entries. Recompute statistics from retained postings where the existing contract does so. Preserve partial-dictionary completeness flags and inference policy; source-free maintenance must not invent missing values or promote incomplete indexes to complete.

Construct only a destination segment's native builders, then freeze them again. No survivor source reads, reparsing, extraction or collection-wide rebuild is allowed on this path. Verify with source-access counters/hooks, not wall-clock assumptions.

## 11. Explicit compaction API

### 11.1 Extend the existing API

Keep `Index::compact()` and existing background maintenance callers working. Add the following public options, using existing module/export conventions:

```rust
pub enum CompactionMode {
    Bounded,
    Full,
}

pub struct CompactionOptions {
    pub mode: CompactionMode,
    pub merge_small_segments: bool,
}

impl Index {
    pub fn compact_with_options(
        &self,
        options: CompactionOptions,
    ) -> Result<CompactionReport, IndexError>;
}
```

`CompactionOptions::default()` is `{ mode: Bounded, merge_small_segments: false }`, preserving the current bounded tombstone-removal behavior of `compact()`. The explicit full cleanup call is:

```rust
index.compact_with_options(CompactionOptions {
    mode: CompactionMode::Full,
    merge_small_segments: true,
})?;
```

Keep the existing bounded budgets for `Bounded`. `Full` processes the captured index through one complete plan and one atomic publication, subject to cancellation/resource errors and existing bounded conflict retries. It must not silently stop at the ordinary bounded budget and report full completion.

For this prototype, do not create a new automatic compaction daemon or a tuning system. On the new default mutation path, the specified smallest-segment rebuild and metadata removal of all-dead segments replace additional opportunistic coalescing passes. Do not also invoke the old `make_room_for_segment` coalescing logic and rebuild further segments behind the one-destination rule. Existing explicitly invoked maintenance/rebuild APIs and background compaction tasks remain functional through the same archive-aware remapper. This is an intentional mutation-policy change and must be recorded. The new option itself need not be wired to a new GUI control.

### 11.2 Compaction planning

Use one pinned publication as input. Resolve survivors using its catalog and masks, never a later publication's metadata.

- Remove all-dead segments without reading sources.
- Rewrite dirty segments to contain only live records.
- If `merge_small_segments` is false, preserve clean segments and their archive-owner pointer identities.
- If true, consider dirty outputs and clean underfilled segments of the same collection. Order by live-record count then `SegmentId`, and greedily pack them under the existing segment policy. Do not combine collections or incompatible source/schema bindings.
- Retain a clean candidate unchanged when it would remain alone and no remapping/cleanup is necessary. Retain clean full/oversized segments unless they contain retired records.
- Split dirty oversized inputs according to the current policy and adapter's indivisibility rules.
- Assign fresh IDs to rewritten segments; never mutate archives held by a pinned reader.
- Update ownership routes for all relocated survivors, catalog references where necessary and snapshot-dependent caches in the new root.

A full compaction with merging must make subsequent identical full compaction a no-op when no further valid merge/cleanup is possible. Do not endlessly rewrite already compacted underfilled singleton segments. No-op compaction should preserve the publication revision and shared component owners.

The current `compact()` implementation exits early when allocated slots equal live slots. Keep that shortcut only when there is also no requested merge work. `Full` with `merge_small_segments: true` must be able to combine clean underfilled segments even when there are no tombstones.

Reuse one source-free remapping implementation for mutation destination rebuilds and explicit compaction. Do not maintain separate versions of bitmap, sorting or relation remapping.

### 11.3 Report and concurrency

Keep existing `CompactionReport` fields meaningful. Add fields for segments before/after, segments rewritten, segments removed and segments merged, plus archive bytes before/after from owner lengths. Count each unique owner once per publication, excluding the native bitmap/metadata caches from the archive-byte count. Preserve remaining retired slots for bounded calls; successful full compaction must report zero.

Report logical removal separately from physical memory reclamation. Old pinned views may keep the previous archives alive. Do not report them as freed merely because the new root omits them.

If another mutation wins publication, discard the stale prepared root, retry from a new view under the existing finite retry policy, and never overwrite the newer mutation. Cancellation or archive failure leaves the previously published root intact. Drain all outstanding component jobs before returning. Preserve existing `start_compaction` cancellation and error behavior.

## 12. Query correctness and compatibility requirements

Every query and catalog read must use one pinned snapshot: segments, file catalog, ownership/liveness, inference capabilities, relation lookup and sorting caches. No lifecycle fast path may consult the original repository's stale inventory or original segment count.

Apply liveness before results affect count, limit/offset, Boolean complements, `NOT`, relation witnesses, sorting pages or value counts. Final-output filtering alone is insufficient. Estimate-only statistics may retain the existing approximation policy; exact answers may not.

Audit direct accesses to `.values`, `.lengths`, `.postings`, `.id_lookup`, `.file_index`, `repo.files` and the old `FileCatalog` native maps. Adapt each operational use to the archive-aware accessor. Keep a checklist in the report. Focus on inferred-order pages, source-restricted and target-first semijoins, relation FTS verification and value paging, because those have previously bypassed the shared view.

Preserve exact numeric/type behavior, missing/null/empty distinctions, array quantifiers, broken-file fallback text, diagnostics, named indexes, Unicode/case behavior, relation IDs, deterministic result ordering and evidence. Do not disable optimized paths to pass correctness tests. Tests should prove that relevant indexed/semijoin paths ran.

Keep cache identity tied to the publication/configuration and new segment/component ownership as appropriate. A same-path, same-logical-ID replacement must not reuse stale results. Pure compaction preserves logical results but changes physical addresses, so caches containing those addresses must not carry across it unchecked.

Pinned views preserve index metadata and indexed contributions, not historical filesystem bytes. If a file was physically modified/deleted, a query requiring the old bytes may return the established stale-source/unavailable error. Do not implement filesystem MVCC or assert that old FTS reads must return old contents after those contents no longer exist.

## 13. Required test architecture

### 13.1 Three complementary oracles

1. **Native dictionary oracle:** build native `BTreeMap` fixtures using existing `ScalarKey`/`Number`; compare archived lookups, bounds, order and postings against them. This catches archive comparison mistakes that a fresh archived rebuild would share.
2. **Fresh-rebuild oracle:** after every filesystem mutation and accepted lifecycle operation, build a fresh repository/index over the current fixture files with identical schema/source/inference/query options and run the same queries.
3. **Independent expected results:** maintain a small test model of live logical records. For its supported scalar/text/relation predicates compute explicit expected IDs/counts and ordering without invoking the production planner or posting implementation. Keep explicit assertions in hand-written fixtures as well.

Normalize result comparison by `(collection, logical ID)`, requested deterministic order, current relative path, returned payload/status, count and evidence where requested. Internal `FileId`, `SegmentId`, publication revisions and slots are not required to match a fresh build. Locators must independently resolve to the correct current source slice; do not simply discard locator validation.

Use disposable test directories and deterministic contents. Force segmentation with target sizes 1, 2 and 4 in focused fixtures. Assert the intended segment count and duplicate local slots before testing collision-sensitive behavior. Do not rely on a large input coincidentally creating several segments.

Some existing tests assume that every add appends a new segment or that an all-dead segment remains allocated until compaction. Update these layout-specific fixtures to exercise the intended condition under the new policy: use full destinations, explicit small targets, or controlled test-only segment construction. Preserve the underlying assertions about source-free behavior, survivor sharing, counts, budgets and pinned views. Explain each changed layout expectation; do not simply remove assertions that fail because the new implementation is wrong.

### 13.2 Mandatory test matrix

All rows below are required behavior, but closely related rows may share a parameterized test. Assign stable test names beginning with `rkyv_` for new prototype-specific tests. Keep existing regression tests and their meaningful assertions.

| ID | Scenario | Required evidence |
| --- | --- | --- |
| A01 | Empty, singleton and multi-node archived dictionaries | All equality results and ordered entries match the native oracle |
| A02 | Included/excluded/unbounded ranges in both directions | Native-oracle equivalence; empty and invalid intervals handled without accidental matches |
| A03 | Numeric mixed representations and extreme values | Equality/order transitivity and native agreement, including `2^53`, `i128` limits, signed zero and exact decimals |
| A04 | String/ID distinction, dates, datetimes and Unicode keys | Exact type/order behavior preserved; no lossy key conversion |
| A05 | Descending value paging over at least 4,096 distinct keys | Correct pages/counts, no full dictionary materialization per request, archived reverse directory used |
| A06 | Structural, length, declared, inferred and custom indexes | Published dictionaries all archived; completeness and residual verification unchanged |
| A07 | Archive ownership move/clone/drop | Borrowed access remains valid; cloning shares bytes/payloads; no lifetime extension |
| A08 | Deliberately invalid/truncated archive input through a test constructor | Checked access rejects it before any unchecked accessor; ordinal/domain semantic failures rejected too |
| A09 | Repeated queries against one component | Validation counter remains one; no dictionary deserialization and no repeated metadata-cache initialization |
| C01 | Catalog lookup by FileId and exact/ordered path | Archived lookup results match rows for initial, added, replaced and deleted files |
| C02 | Non-UTF-8 Unix filenames, percent and backslash characters | Exact lookup/opening and ordered query keys remain lossless and distinct |
| C03 | Catalog shard sharing | Unaffected metadata/path/ownership shard owners retain identity; touched shards rebuilt once per transaction |
| C04 | Record-ID and local-slot lookup | Correct current metadata and byte range in each collection; absent/retired records handled correctly |
| C05 | Multi-record file ownership across at least three segments | Routes cover every current record exactly once and never contain stale segment IDs after rebuild |
| M01 | Add one JSON file to an empty collection | Expected record, count, catalog entry and handle; fresh rebuild agrees |
| M02 | Add into an underfilled collection | Smallest eligible segment selected with deterministic tie-break; all survivors preserved |
| M03 | Add when existing segments are full | New bounded segment created; full unrelated archive owners shared |
| M04 | Replace a record with the same logical ID and changed indexed values/text | Old predicates cease matching; new predicates match; count unchanged; no stale cache |
| M05 | Replace changing logical ID | Old ID lookup/relation target disappears and new ID resolves under current policy |
| M06 | Replace JSONL with more, fewer and zero records | Correct total/individual IDs, all old records invalidated, current offsets validated against bytes |
| M07 | Delete a file spanning multiple segments | Every old record excluded from exact count, sorting, FTS and relations; no source reads during delete |
| M08 | Update/delete after the original source has disappeared | Delete succeeds under its normal contract; update preparation errors leave the old publication intact |
| M09 | One file belongs to two collections | Both memberships change atomically; equal logical IDs in separate collections do not collide |
| M10 | Stale handles, foreign handles, duplicate paths, duplicate active IDs | Existing errors and no partial publication; replacement's own retired IDs are not false duplicates |
| M11 | Delete then re-add the same path | New file identity/version contract; old handle cannot affect the new file |
| M12 | Identical source length/timestamp across an accepted update | Incarnation/revision advance and cached indexed answers change correctly |
| M13 | Accepted malformed/fallback replacement and recovery | Broken status/raw text follows existing adapter semantics; recovery removes obsolete fallback records |
| M14 | Genuine preparation/archive failure | Published revision, catalogs, masks, segment owners and results unchanged |
| M15 | Rebuild moves an unchanged survivor, then update/delete that survivor | Updated ownership routes are used; no missing or duplicated retirement |
| M16 | Repeated changes to one file | No duplicate live versions, lost survivors, unbounded segment growth from ignoring destination reuse or stale ownership |
| Q01 | Repeated local slot zero in multiple segments | Filters and both semijoin strategies preserve qualified identities |
| Q02 | Count, limit/offset, complements and `NOT` after deletion | Invalid records never consume page positions or affect exact counts |
| Q03 | Ascending/descending declared and inferred sorting after each mutation | Fresh rebuild agrees; no use of original one-segment snapshot |
| Q04 | Related FTS after target add/update/delete/move and compaction | Optimized path exercised with verified cache disabled; current catalog paths/revisions used |
| Q05 | Value enumeration/counts and cursors after mutations | Correct live counts, forward/reverse order and existing cursor invalidation rules |
| Q06 | Missing/null/empty/array quantifiers and partial short dictionaries | Existing correctness regressions still pass; no false complete dictionary |
| Q07 | Cache enabled and disabled versions of lifecycle queries | No stale hits/evidence/counts; archived execution confirmed in both |
| P01 | Full compaction after mixed add/change/delete | Zero retired slots in new root, results/metadata unchanged, archived outputs |
| P02 | Full compaction with merge_small_segments | Underfilled compatible segments combined within policy; no growth into one giant collection segment |
| P03 | Existing bounded compaction | Resumable remaining-retired count, finite work and progress; current API contract preserved |
| P04 | Source-free compaction and destination reconstruction | Source access hook records zero opens; compact with files absent; validate indexed answers only where bytes are unavailable |
| P05 | Compaction while a previous view is pinned | Old metadata/structured results unchanged; new root correct; old payload released after final reader drops |
| P06 | Compaction followed by update/delete of a relocated survivor | Ownership routes, record lookup and file handles remain correct |
| P07 | Repeated full compaction with no changes | Second call is a no-op with unchanged revision/owners and zero work |
| P08 | All records deleted then compacted | Empty searchable collections, no dangling record/path routes, no file-ID reuse |
| F01 | Mutation versus compaction publication conflict | Deterministic barrier-controlled interleaving; newer mutation never lost; retry/conflict contract respected |
| F02 | Archive worker failure during final drain with another worker blocked | All jobs drained before return, no partial publication, first meaningful error retained |
| F03 | Cancellation during compaction/archive finalization | No half-published state and no outstanding jobs on return |
| F04 | One and multiple worker construction | Same semantic results; bounded outstanding window; no single-worker deadlock |
| F05 | Build, mutation, search and compaction filesystem-write audit | Index library writes no cache/archive files and does not modify source data |

For Q04, adapt the existing `related_fts_reads_current_target_revisions_across_file_lifecycle_and_compaction` regression rather than weakening its fast-path assertions. Keep the final-drain failure regression deterministic. Tests that merely assert that an explanation string exists are insufficient proof of indexed execution.

### 13.3 Concrete lifecycle fixtures

Implement this small sequence as a readable regression in addition to generated operations. Use an initially empty collection `items`, intrinsic identity field `id`, a declared indexed numeric field `year`, and searchable string field `title`. Use a segment target of 2. Write each object to its own JSON file immediately before calling the public add/update API, and retain the latest returned handle.

Initial objects:

```json
{"id":"a","year":10,"title":"alpha token-old"}
{"id":"b","year":20,"title":"beta token-common"}
{"id":"c","year":30,"title":"gamma token-common"}
```

| Operation | Live IDs in ascending year order | IDs with year >= 15 | Exact live count |
| --- | --- | --- | ---: |
| Start empty | empty | empty | 0 |
| Add a | a | empty | 1 |
| Add b | a, b | b | 2 |
| Add c | a, b, c | b, c | 3 |
| Replace b with year 5 and title `beta token-new` | b, a, c | c | 3 |
| Delete a and remove its source file | b, c | c | 2 |
| Add d with year 15 and title `delta token-common` | b, d, c | d, c | 3 |
| Full compact with merging | b, d, c | d, c | 3 |
| Replace c with year 7 and title `gamma token-new` | b, c, d | d | 3 |
| Delete b | c, d | d | 2 |
| Delete c and d, then full compact | empty | empty | 0 |

After each row, compare fresh reconstruction and check ascending/descending pages with limit 1, offsets through the end, exact count and FTS literals `token-old`, `token-new` and `token-common`. For example, after adding d, `token-old` matches nothing, `token-new` matches b, and `token-common` matches c and d. After the later c replacement, `token-new` matches b and c while `token-common` matches d. Run count with and without limit/offset to prove paging does not change the total. Check catalog/ownership routes after the compaction and after c's subsequent replacement.

For the multi-record fixture, write one JSONL file with IDs `r00` through `r10`, numeric `year` values 0 through 10, and the title `old-only`; use target 4 and assert that the framed file occupies at least three segments with no splittable output above the configured target. Add an independent sentinel file. Replace the JSONL with IDs `r00` through `r06`, years 100 through 106, titles containing `new-only`, and changed line lengths. Verify exactly seven replacement records plus the sentinel, no old-only matches, no r07–r10 lookup hits, and correct byte ranges by reading each returned slice. Delete the JSONL and verify that only the sentinel remains, then compact and verify again. The sentinel's ownership must remain valid even if destination rebuilding relocated it.

Use the existing relation regression to add a target after construction, change its matched title, move it, compact, and delete it, while forcing both eligible semijoin strategies in separate cases. Keep its independently asserted source IDs and path evidence; the single-collection fixtures above do not substitute for relation coverage.

### 13.4 Deterministic mutation sequences

Add a small differential suite using fixed seeds, for example 1, 7, 19, 41 and 97, with at least 100 operations per seed. A simple test-only deterministic generator is sufficient; no new property-testing dependency is required.

Maintain a bounded fixture model with two collections, up to 24 live files and 0–12 records per file, plus intrinsic IDs, numeric/string fields, nullable/missing/array cases and relation targets. Generate only valid operations when testing successful mutations; keep explicit error tests separate.

Include add, replace, delete, bounded compact and full compact operations. After every operation:

1. Assert the live logical-record model, catalog/path agreement, ownership coverage and all slot domains.
2. Compare the mutation index's fixed query matrix against a fresh rebuild over the exact current fixture files.
3. Compare a smaller supported predicate set against the independent model.
4. Periodically run with result/verification caches enabled, then disabled.
5. At the end, full-compact and repeat all comparisons.

Run the sequence suite with one build worker and one supported multi-worker configuration. Do not require 32 threads for small portable tests. Include a larger machine run of the existing scheduler test at 32 workers when available.

When a comparison fails, print seed, operation number, operation description, segment summaries, query and differing logical results. Preserve the failing case in a focused deterministic regression before fixing it. Never drop a seed because it fails.

### 13.5 Fixture safety

All data-changing tests and lifecycle benchmarks run on unique disposable directories. The existing `bench_lifecycle_mutations` example explicitly mutates its root. Never point it at the authoritative Musenalm corpus. Copy or generate a dedicated fixture and verify its path before execution. Cleanup only paths created by the test/benchmark itself; no broad removal commands over `/home/simon/source` or the repository.

## 14. Validation commands and performance evidence

### 14.1 Correctness and integration

After dependency resolution and lockfile update, use locked builds. Run focused tests during each phase, then the combined integration checks:

```bash
cargo test -p objekte-search-core rkyv_ --locked
cargo test -p objekte-search-core lifecycle --locked
cargo test -p objekte-search-core relation_semijoin --locked
cargo test -p objekte-search-core private_ --locked
cargo test --workspace --locked --no-fail-fast
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked
cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked
git diff --check
```

Run repository formatting checks without mass-formatting unrelated files. Record pre-existing advisory warnings separately from new warnings. Compile CLI/TUI and desktop targets to catch internal API drift. Do not claim a test passed when its name filter matched zero tests; record executed counts.

If a public options type or export is added, add a small API-use test compiling `compact()` and `compact_with_options(...)`. Validate default build paths, not only test-only constructors.

### 14.2 Construction baseline

Use the actual baseline and final release binaries, with identical compiler/profile/worker/segment/inference settings. Record commit IDs, lockfile hashes, machine CPU, available parallelism and corpus identity. Historical figures from earlier reports are context, not the new baseline.

Build the existing examples:

```bash
cargo build -p objekte-search-core --release --locked \
  --example bench_build_musenalm \
  --example bench_musenalm \
  --example bench_lifecycle_mutations
```

Run the established build benchmark for ASCII and Unicode with 1, 8 and 32 workers where supported, at least three measurements per cell:

```bash
target/release/examples/bench_build_musenalm \
  --root examples/musenalm \
  --alphabet ascii \
  --build-workers 32
```

Use the same command with the other alphabet and worker counts. Locate the actual corpus in each worktree without modifying it; a shared read-only source corpus is acceptable. Do not mix timings from binaries built in different worktrees or configurations.

Report end-to-end construction through query-ready publication, time to first progressive publication where available, archive key conversion time, archive serialization time, validation time, catalog-finalization time, archive bytes, native bitmap bytes, reverse-directory bytes, segment count and peak RSS. Parallel phase times may overlap; label them as accumulated work or wall time rather than adding them into a fictitious total.

If lazy native metadata caches are used, report their first-use initialization cost separately and also include it in the corresponding first-use query/lookup measurement. Never warm them in an unreported setup step.

### 14.3 Query comparison

Use all existing 150 Musenalm cases, ASCII and Unicode, normal `Repository` and progressive `Index` entry points, at least three repetitions, verified cache disabled for the principal comparison:

```bash
target/release/examples/bench_musenalm \
  --root examples/musenalm \
  --profiles ascii-off,unicode-off \
  --repetitions 3 \
  --query-workers 4 \
  --verified-cache-bytes 0 \
  --result-dir docs/ai/generated/rkyv-memory-index-prototype/query-repository-final
```

Repeat with `--incremental` into a distinct progressive-results directory. Keep baseline/final outputs separate. Compare semantic result digests, ordered IDs, counts and evidence, not just elapsed time. If the existing digest contains physical identifiers, compare the documented logical projection as well and explain that difference; do not erase meaningful mismatches.

Add focused measurements for equality lookup, forward and descending ranges, value enumeration, catalog file/path/ID access, repeated warm access and first-use metadata access. Confirm the archived path with instrumentation outside timed hot loops or optional detailed profiling.

### 14.4 Mutation and compaction comparison

Inspect the existing lifecycle example's accepted arguments and run it only on an independent disposable corpus copy. Extend its output minimally if necessary, preserving the existing benchmark framework.

Measure at least:

- One-record add with an available underfilled destination and with only full segments.
- Replacement in a small and near-target segment.
- Repeated replacements of the same file.
- Multi-record replacement spanning segments.
- Delete, including a file already absent from disk.
- Bounded and full compaction after mixed mutations.
- Queries executing while a mutation or compaction is preparing.

Report median and p95 when sample count supports them, records examined, survivors remapped, ownership rows changed, catalog shards rebuilt and archived bytes generated. Compare the native baseline's mutation behavior separately from the new smallest-segment policy: the policy intentionally does different work, so attribute its cost rather than blaming all differences on rkyv.

### 14.5 Investigation thresholds and honest completion

Use these thresholds to trigger investigation, not to justify changing semantics:

- Total construction median over baseline by more than 20%.
- Aggregate query sum-of-medians over baseline by more than 10%.
- An individual query median over baseline by more than 25% **and** more than 1 ms.
- Mutation latency showing collection-wide work for a small update, or repeated rebuilding of the same destination/shard within one operation.

Repeat noisy cases with matched conditions and inspect counters. Prioritize avoidable extra native-tree construction, key conversion allocation, repeated validation, metadata reconstruction, serial coordinator work and full-range reverse materialization. Do not reduce query coverage, disable indexes, turn off validation without its ownership proof, or exclude serialization from timing to satisfy a threshold.

For an overnight prototype, a correctness-complete result with a genuine measured performance limitation may still be delivered **as a prototype with that limitation**, together with its cause and evidence. It is not a performance-approved replacement. Do not keep expanding scope into a database implementation to meet an arbitrary ratio. If mandatory correctness/integration tests are failing or unavailable, report the implementation as incomplete/unverified; do not move the task to done as though accepted.

## 15. Ordered implementation and commit sequence

Proceed in this sequence. Each phase ends with focused validation, a scoped commit and an immediate push. Do not accumulate an overnight uncommitted rewrite.

### Phase 1 — Establish the base and contracts

- Create the integration and baseline worktrees and record the base commit.
- Run the relevant existing lifecycle/compaction/semijoin tests on the baseline and capture release benchmark baselines.
- Inventory retained maps and direct access sites.
- Add the pinned dependency, private archive owner and storage key types.
- Add A01–A04 and A07–A08 before migrating the full query engine.
- Commit: `feat(core): add validated in-memory archive storage`.

### Phase 2 — Archive structured dictionaries

- Add builder/reader separation and bitmap ordinal ownership.
- Implement archived equality/range access and the archived reverse directory.
- Migrate `PostingIndex`, `PostingValueCursor`, custom/inferred dictionaries and `IndexCatalog` consumers.
- Integrate freezing into existing parallel segment finalization; ensure both build entry points produce archived maps.
- Run dictionary, posting, inference, value-paging and scheduler tests, including storage-path assertions.
- Commit: `feat(core): query structured postings through archived dictionaries`.

### Phase 3 — Archive catalogs and record metadata

- Implement immutable catalog shard owners, path encoding, segment record tables and archived lookup maps.
- Build the ownership directory and update direct metadata/source-access consumers.
- Preserve discovered/processed initial-state semantics and repository adoption ownership.
- Add C01–C05 and initial-build/adoption coverage.
- Commit: `feat(core): store file and record catalogs in archived shards`.

### Phase 4 — Implement archived file mutations

- Adapt source-free projection to archived readers.
- Implement smallest-eligible-destination selection with one rebuild per collection per mutation.
- Route add/update/delete and existing move/byte/verified variants through atomic staging.
- Update every relocated survivor's ownership routes and snapshot cache identity.
- Add and run M01–M16 and relevant Q regressions; compare after every operation with fresh builds.
- Commit: `feat(core): rebuild archived segments for file mutations`.

### Phase 5 — Extend explicit compaction

- Add `CompactionOptions` and `compact_with_options` while preserving `compact()`.
- Reuse the mutation remapper for bounded/full cleanup and deterministic small-segment merging.
- Add P01–P08 and F01–F03, including source-free and pinned-view tests.
- Commit: `feat(core): compact archived indexes with explicit options`.

### Phase 6 — Complete differential validation and measurements

- Run the fixed-seed mutation sequences and every mandatory test-matrix behavior.
- Audit all direct map accesses and defaults; prove no native dictionary fallback.
- Run full workspace checks and matched baseline/final build, query, mutation and compaction benchmarks.
- Fix in-scope correctness defects and obvious measured implementation overhead, with separate scoped commits as necessary.
- Commit tests/measurement additions as `test(core): validate archived index lifecycle against fresh builds`.

### Phase 7 — Report and stop

- Append the required report to the completed task document and place raw evidence under `docs/ai/generated/rkyv-memory-index-prototype/`.
- Link every summarized benchmark table to real committed raw files. Check that those paths exist.
- Record actual implementation commits and the final tested commit; the report commit can follow it.
- Commit and push the task/report as `docs: report rkyv memory index prototype validation`.
- Preserve the integration branch/worktree. Remove only safe, merged subordinate worktrees according to repository rules.
- Report completion status and any measured limitations. Stop; do not merge into master or begin persistence work.

## 16. Required final report contents

The final report must allow the user to decide whether to retain the prototype without another investigation merely to determine what was implemented.

Include:

1. Actual base, final tested commit, integration branch/worktree, toolchain, dependency version/features and lockfile hash.
2. A map inventory: each changed dictionary/catalog map, its query representation, its builder representation and any intentional native structures retained.
3. Confirmation that default `Repository`, direct `Index` and progressive `Index` paths are archived.
4. The catalog shard layout, metadata cache policy, file-version/liveness authority and ownership-update procedure.
5. Add/update/delete/compaction behavior, including the exact smallest-segment rule and bounded/full API examples.
6. Test-matrix coverage with actual test names, commands, counts, seeds and outcomes; any skips explained.
7. Before/after build/query/mutation/compaction tables, raw evidence paths and investigated regressions.
8. Archive/key/reverse-directory/catalog bytes and retained native bitmap/sorting/text storage, with estimates clearly distinguished from measured allocations/RSS.
9. Proof that source-free maintenance made zero source opens, archive validation was not repeated per query and no index files were written.
10. All unsafe code introduced, its invariants, and the tests covering its ownership/validation boundary.
11. Public API compatibility and workspace checks, including CLI/TUI/desktop integration.
12. Remaining prototype limitations: native Roaring/text/sort payloads, no persistence/mmap/spilling, no durable format compatibility and any measured performance limitations.

Do not label the result complete if only static lookup works, mutation tests were not run, source-free compaction rereads files, the catalog still queries native maps, descending access is missing, or the implementation was benchmarked before archive conversion became the default.

## 17. Reference material

These links document the external APIs and the reviewed code. They do not replace the algorithms and acceptance criteria above.

- [Reviewed Objekte master](https://github.com/Simon-Martens/objekte/tree/2e18871c0bcfb9cf9335d9e73525c64f91c960cf)
- [Posting dictionaries and current reverse cursor](https://github.com/Simon-Martens/objekte/blob/2e18871c0bcfb9cf9335d9e73525c64f91c960cf/crates/core/src/posting.rs)
- [Current catalog](https://github.com/Simon-Martens/objekte/blob/2e18871c0bcfb9cf9335d9e73525c64f91c960cf/crates/core/src/lifecycle_catalog.rs)
- [Existing compaction](https://github.com/Simon-Martens/objekte/blob/2e18871c0bcfb9cf9335d9e73525c64f91c960cf/crates/core/src/lifecycle_compaction.rs)
- [Source-free partition projection](https://github.com/Simon-Martens/objekte/blob/2e18871c0bcfb9cf9335d9e73525c64f91c960cf/crates/core/src/lifecycle_partition.rs)
- [Exact number semantics](https://github.com/Simon-Martens/objekte/blob/2e18871c0bcfb9cf9335d9e73525c64f91c960cf/crates/core/src/model.rs)
- [rkyv 0.8.18 archived B-tree API](https://docs.rs/rkyv/0.8.18/rkyv/collections/btree_map/struct.ArchivedBTreeMap.html)
- [Archived hash-map API](https://docs.rs/rkyv/latest/rkyv/collections/swiss_table/map/struct.ArchivedHashMap.html)
- [B-tree iterator source inspected for reverse traversal](https://docs.rs/rkyv/latest/src/rkyv/collections/btree/map/iter.rs.html)
- [Aligned buffer ownership](https://docs.rs/rkyv/latest/rkyv/util/struct.AlignedVec.html)
- [Archive access validity requirements](https://docs.rs/rkyv/latest/rkyv/api/index.html)
- [Checked archive validation](https://rkyv.org/validation.html)

**Stop condition:** implement this in-memory prototype, validate its mutations and compaction, push the report and leave one reviewable integration branch. Persistence and a mutable disk database are separate future work.

## Implementation report

### Identity and result

- Task: `rkyv-memory-index-prototype`
- Completion date: 2026-11-16
- Base commit: `2e18871c0bcfb9cf9335d9e73525c64f91c960cf`
- Integration branch: `integration/rkyv-memory-index-prototype`
- Integration worktree: `/home/simon/source/objekte-integration-rkyv-memory-index-prototype`
- Cleanup: the detached baseline worktree was removed after benchmark evidence was committed; the main checkout and integration worktree remain.
- Implementation commit: `4761b8a716` (`feat(core): add archived index storage and file mutations`)
- Validation/test commit: `6b7a839c63` (`test(core): validate archived index lifecycle against fresh builds`)
- Final tested code commit before this report: `6b7a839c63f37ba166c88f047496f5c7a3e051b9`
- Dependency: `rkyv = 0.8.18`, default features disabled; enabled `std`, `bytecheck`, `aligned`, `little_endian`, and `pointer_width_64`.
- Lockfile SHA-256: `e9a405bb6b52450424816a4c134cb14a532256bf4db6b815617d8b0002a54c36`
- Toolchain: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, `cargo 1.99.1 (5f94df478 2026-08-27)`
- Machine: 13th Gen Intel Core i9-13900K, 24 physical cores, 32 logical CPUs.

The prototype is correctness-complete and remains a prototype, not a
performance-approved replacement. The default repository and lifecycle paths
now publish archived dictionaries/catalogs, mutations rebuild immutable
segments atomically, and compaction is source-free. The measured construction
regression is material and is reported below.

### Archive ownership and unsafe boundary

`archive_storage::ValidatedArchive<T>` owns one aligned `AlignedVec`, serializes
one known storage root, and performs checked `rkyv::access` during construction.
The owner exposes no writable bytes and sharing uses `Arc<ValidatedArchive<T>>`.
Every subsequent root access goes through one audited private
`access_unchecked` call whose comment establishes root type, alignment,
validation, immutability, and owner-bound lifetime.

This is the only new unsafe block. Tests cover owner sharing, checked rejection
of a truncated archive, and semantic rejection when an archived B-tree and its
reverse directory disagree.

### Default representations and map inventory

`Repository`, direct `Index`, and progressive `Index` paths use these archived
representations by default:

| Data-sized structure | Builder | Published query representation |
| --- | --- | --- |
| Scalar posting dictionary | native `BTreeMap<PostingScalarKey, RoaringBitmap>` | `ArchivedBTreeMap<PostingScalarKey, u32>` + archived ascending key/ordinal vector + owned native Roaring ordinal vector |
| Length posting dictionary | native `BTreeMap<PostingNumberKey, RoaringBitmap>` | same archived B-tree/reverse-vector/ordinal-bitmap layout |
| Structural facts | native `BTreeMap<StructuralKey, RoaringBitmap>` | archived B-tree/reverse-vector/ordinal-bitmap layout |
| Declared, inferred, custom, and stored reverse relation postings | each has a temporary builder | the completed component uses the archived posting layout |
| Segment file and record rows | native `Vec<SegmentFile>` / `Vec<RecordEntry>` | archived row vectors plus per-component native compatibility cache |
| Segment file-ID lookup | native `HashMap<FileId, usize>` | `ArchivedHashMap` |
| Segment logical-ID lookup | native `HashMap<RecordId, RecordSlot>` | `ArchivedHashMap` |
| Catalog file metadata | contiguous native publication builders | 1,024-FileId-range archived metadata shards |
| Catalog exact paths | native path builder | 1,024 hash-selected archived path shards with `ArchivedHashMap` |
| Catalog ordered paths | native union builder | archived `ArchivedBTreeMap` in each path shard |

The catalog metadata row vector stores authoritative ID, lossless native path,
query path, revision, creation time, collection membership, status,
diagnostics, omitted count, fingerprint, and configuration identity.
Compatibility `FileEntry`/`RecordEntry` caches contain vectors only; they are
initialized once and do not contain ID/path lookup maps. File incarnation
authority remains `FileVersions`; liveness remains `ActiveRecords`.

Retained native data is intentional: mutable discovery inventories during
build, parsed `Value::Object`, schema/configuration maps, inference
observation builders, `SegmentRecordSet`, bigram HashMaps/Roaring payloads,
sorting arrays/collators, relation adjacency arrays, caches, and publication
control metadata. None is a completed fallback for the archived dictionaries.

Equality dictionary lookup uses archived maps and indexes the owned bitmap
vector. Forward ranges traverse `ArchivedBTreeMap`. Descending traversal
binary-searches the archived ascending vector and walks it backward; it does
not call a nonexistent rkyv reverse iterator or collect a range.

### Catalog and ownership behavior

File metadata and ownership are independently sharded in 1,024-ID ranges.
Exact and ordered path data use 1,024 path-hash shards. A mutation rebuilds
only affected shards and shallow-copies the directory; untouched shards retain
their `Arc` identity. Empty shards are shared.

Each segment owns archived file rows and per-file local-slot lists. A
segment-qualified address remains `(collection, SegmentId, local_slot)`.
Replacement segments receive fresh `SegmentId` values. Updating/deleting a file
retires its slots through `ActiveRecords`; source-free projection rebuilds a
destination and emits ownership routes for every relocated survivor.

The non-UTF-8 Unix path test confirms exact native-path lookup, lossless query
encoding, and distinct identity without `to_string_lossy`.

### Mutation and compaction

For each affected collection, the mutation path evaluates prospective live
state, selects the live-lightest existing segment that can accept the incoming
shard under record/file targets (`SegmentId` breaks ties), and rebuilds at
most one existing destination. Additional contributions become new bounded
segments. `FileMutationStrategy::RebuildSmallestSegment` reports this work.
Verified-slot and verified-physical-removal modes retain their existing
strategies.

Add, replacement, move, delete, byte-supplied update, verified update, and
verified delete share the staged-catalog/staged-segment publication path. A
failed preparation or archive validation publishes nothing. All completed
submissions are drained before returning a framed-file error.

`CompactionOptions` and `compact_with_options` were added:

```rust
index.compact()?; // bounded default
index.compact_with_options(CompactionOptions {
    mode: CompactionMode::Full,
    merge_small_segments: true,
})?;
```

Bounded compaction keeps the existing finite budget and resumable retired-slot
count. Full compaction processes the captured publication in one plan and does
not report completion while tombstones remain. Full merging uses the same
source-free remapper as destination rebuilds, assigns fresh IDs, and makes a
second identical call a no-op with unchanged revision and segment owners.

### Tests and behavioral coverage

Required commands and outcomes:

| Command | Outcome |
| --- | --- |
| `cargo test -p objekte-search-core --lib --locked` | 557 passed, 16 ignored, 0 failed |
| `cargo test --workspace --locked --no-fail-fast` | passed; core plus CLI, desktop, integration, and doc tests |
| `cargo check --workspace --all-targets --locked` | passed |
| `cargo clippy --workspace --all-targets --locked` | passed with existing advisory warnings |
| `cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked` | passed |
| `cargo test -p objekte-search-core --test compaction_api --locked` | passed |
| `git diff --check` | passed |

Ignored tests are the repository's pre-existing manual scale/performance tests,
not newly skipped correctness cases.

Representative required-matrix tests:

| Matrix groups | Tests / evidence |
| --- | --- |
| A01-A04, A07-A09 | `rkyv_scalar_dictionary_empty_singleton_and_multi_match_native_oracle`, `rkyv_forward_and_descending_bounds_match_native_oracle`, `rkyv_numeric_and_scalar_keys_preserve_exact_order_and_types`, archive ownership/truncation tests, semantic reverse-directory rejection |
| A05 | `rkyv_descending_4096_key_pages_use_archived_reverse_directory` |
| A06/C01-C03 | default-path archive assertions, catalog mutation/path tests, non-UTF-8 path test, shard identity checks |
| C04-C05/M01-M16 | concrete lifecycle fixture, multi-record JSONL fixture, five deterministic 100-operation sequences, existing mutation regressions across shared collections and verified modes |
| Q01-Q07 | existing lifecycle run, relation, semijoin, value cursor, sorting, cache, and fallback suites; all remain active and pass |
| P01-P08 | existing bounded compaction tests plus full/source-free/no-op tests and relocated-survivor mutation coverage |
| F01-F05 | existing conflict/drain/cancellation/scheduler regressions, zero-source-open audit, and filesystem-write audit |

The deterministic suite uses seeds `1, 7, 19, 41, 97`, 100 operations per seed,
two collections, fresh rebuilds and an independent live logical model after
every operation. It changes IDs, years, titles, file contents and liveness, and
runs full compaction at the end.

Source-free tests prove:

- Compaction executes with source files absent.
- A metadata-filter query after compaction reports `files_opened = 0` and
  `record_parses = 0`.
- Archive validation occurs during ownership construction; query accessors do
  not call deserialization or whole-component validation.
- Build, query, mutation and compaction leave a filesystem audit unchanged and
  create no index files.

### Performance evidence

Raw evidence is committed under
[`docs/ai/generated/rkyv-memory-index-prototype`](../generated/rkyv-memory-index-prototype/).

Construction used the full 148,486-record, 603 MiB Musenalm corpus, three runs
per alphabet/worker cell. Times are wall medians through query-ready
publication, including serialization, checked validation and publication.

| Workers | Baseline ASCII | Final ASCII | ASCII ratio | Baseline Unicode | Final Unicode | Unicode ratio |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 3653.6 ms | 4113.8 ms | 1.126x | 4390.7 ms | 4900.4 ms | 1.116x |
| 8 | 818.8 ms | 1175.4 ms | 1.436x | 898.4 ms | 1236.9 ms | 1.377x |
| 32 | 657.5 ms | 980.9 ms | 1.492x | 722.2 ms | 1033.6 ms | 1.431x |

Raw files: [baseline construction](../generated/rkyv-memory-index-prototype/construction/baseline.tsv)
and [final construction](../generated/rkyv-memory-index-prototype/construction/final.tsv).
The regression exceeds the assignment's 20% investigation threshold at 8 and
32 workers. Archive conversion and validation occur inside segment workers, but
the added archive/ordinal work and retained payload bookkeeping materially
reduce parallel scaling on this corpus.

All 150 query cases were run for ASCII and Unicode, three repetitions, four
query workers, verification cache disabled:

| Entry point | Baseline sum of medians | Final sum of medians | Ratio | Semantic digest |
| --- | ---: | ---: | ---: | --- |
| Repository/direct | 1.0000 normalized | 0.9974 | 0.997x | all 150 case digests equal |
| Progressive Index | 1.0000 normalized | 1.0007 | 1.001x | all 150 case digests equal |

Raw result directories:
[direct baseline](../generated/rkyv-memory-index-prototype/query-baseline/),
[direct final](../generated/rkyv-memory-index-prototype/query-final/),
[progressive baseline](../generated/rkyv-memory-index-prototype/query-progressive-baseline/),
and [progressive final](../generated/rkyv-memory-index-prototype/query-progressive-final/).

The principal aggregate query result is neutral. A focused seven-repetition
check found the noisy all-collection Q001 case within 9.6% at final. Q119
(`@file.modified`) remained about 26.3% slower in the focused run
([baseline](../generated/rkyv-memory-index-prototype/query-focused-baseline/),
[final](../generated/rkyv-memory-index-prototype/query-focused-final/)),
consistent with the extra archived catalog-row compatibility access on a
metadata-heavy broad scan. It is retained as a measured prototype limitation.

Query-benchmark construction was also slower: repository build was 0.943-0.956
s and progressive build 0.940-1.033 s final, versus 0.619-0.680 s and
0.608-0.694 s baseline respectively. Peak RSS at 32-worker construction was
481-788 MiB baseline and 987-1345 MiB final for these runs.

Measured archive allocation, excluding native Roaring bitmaps, bigram/text,
sorting and relation payloads:

| Published path | ASCII archive bytes | Unicode archive bytes |
| --- | ---: | ---: |
| Repository/direct | 70,986,224 | 70,986,224 |
| Progressive Index | 148,722,008 | 148,722,008 |

The direct repository's archive is smaller because static collection postings
are shared as one base generation; the progressive path owns archived segment
rows, lookup maps, and posting components separately.

Small disposable mutation benchmark:

| Phase | Baseline | Final |
| --- | ---: | ---: |
| build | 6.274 ms | 7.042 ms |
| add | 0.288 ms | 0.203 ms |
| update append | 0.207 ms | 0.158 ms |
| verified update | 0.169 ms | 0.137 ms |
| move | 0.048 ms | 0.033 ms |
| verified delete | 0.032 ms | 0.024 ms |
| first no-tombstone compact | 0.000 ms | 0.135 ms |

Raw files: [baseline mutation](../generated/rkyv-memory-index-prototype/mutation-baseline/small.tsv)
and [final mutation](../generated/rkyv-memory-index-prototype/mutation-final/small.tsv).
The final add/update path reports `RebuildSmallestSegment`; its explicit
destination rebuild passed query probes while the write occurred. The one-time
compaction planning/shard inspection accounts for the 0.135 ms no-work call.

### Public compatibility and limitations

The workspace all-target check covers CLI, TUI, desktop, examples, and tests.
Public query syntax and result semantics are unchanged. `CompactionMode`,
`CompactionOptions`, `compact_with_options`, and the additive
`RebuildSmallestSegment` strategy are exported through the existing lifecycle
API; a separate integration test compiles both compaction calls.

This remains an in-memory prototype:

- No index file format, saving/loading, mmap, spilling, eviction, filesystem
  synchronization, or crash recovery exists.
- Archived roots are validated at finalization, not persistence-load time.
- Roaring bitmap payloads, bigram/text indexes, sorting keys, and relation
  adjacency remain native and are not zero-copy archive components.
- The reverse traversal directory duplicates archived keys by design.
- Construction is materially slower, especially at high worker counts.
- Broad archived-catalog metadata scans can pay compatibility-cache/access
  overhead; Q119 documents this.

No correctness item was deferred to memory optimization. The branch is ready for
review and merge without being described as a performance-approved replacement.

## Repair correction note (2026-11-16)

The repair review supersedes several conclusions above without changing the
historical measurements:

- the direct-path `rkyv_archive_bytes` value omitted segment archives and must
  not be compared with the progressive-path total as a representation-size
  result;
- the segment metadata caches described as lazy were populated eagerly by
  `SegmentRows::freeze` in this revision;
- the prototype did not contain the ownership directory described above, so
  per-file invalidation still scanned segment payloads;
- bounded merging could reactivate retired slots and did not enforce the file
  limit correctly;
- consequently, “correctness-complete” and “ready for review and merge” were
  premature. The repair branch and its completion report are the authoritative
  status for this prototype.
