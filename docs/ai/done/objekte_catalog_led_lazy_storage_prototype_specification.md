# Objekte: catalog-led storage backend with demand-loaded index components

Date: 2026-11-17  
Task identifier: `catalog-led-lazy-storage`  
Deliverable: a working, measured storage-backend prototype on one integration branch.

## 1. Assignment and precedence

Implement explicit persistence and reopening of the segmented index. Load and validate all file/record metadata, the index catalog, and the leaf catalog before accepting queries. Load query dictionaries, FTS dictionaries, posting leaves, field-state leaves, and sort columns only when execution needs them. Keep loaded data resident for repeated searches unless the caller sets a cache budget or explicitly releases it.

This is a follow-up to **objekte_native_publication_explicit_archiving_specification.md**, supplied with this task. Its native construction, explicit in-memory archiving, direct iterator serialization, publication, update, deletion, compaction, and ownership requirements remain in force. This document adds disk storage and overrides that document's prohibition on disk operations only for the explicit storage APIs defined here. Ordinary construction and `archive()` remain memory-only.

These are controlling decisions, not suggestions:

1. Native construction remains the default; do not serialize or create index files during ordinary build.
2. `archive()` remains explicit, in-memory, and query-preserving.
3. Add `archive_to(...)` to archive as necessary and durably save a complete snapshot. Add explicit `open_storage(...)`. Never silently open a cache from ordinary build.
4. Reopening reads all eager catalogs and liveness metadata before publishing a searchable root. It performs no source discovery, source parsing, query-payload loading, or bitmap reconstruction beyond the explicitly eager liveness/domain masks.
5. The eager index catalog says what each index provides, its coverage and exactness contracts, and where its lazy components are. The eager leaf catalog gives every leaf's location and planning metadata.
6. The planner must not load candidate dictionaries or posting leaves merely to enumerate, assess, or explain available access paths.
7. Store archived dictionaries and arrays in independently checked rkyv frames. Store posting bitmaps in the standard Roaring binary format.
8. FTS normal and skip bigram leaves use the same Roaring leaf mechanism. Preserve their distinct keys, alphabets, field/binding identities, coverage, and normalization versions.
9. All published components are immutable. Updates create replacement components and metadata. No in-place editing of archived trees, bitmaps, or committed disk objects.
10. Each saved generation has one manifest and is published by an atomic replacement of `CURRENT` after its dependencies are synchronized.
11. Cold I/O or validation errors propagate as query errors. They are never interpreted as absent keys, empty postings, or permission to silently rebuild from source.
12. The first implementation uses ordinary positioned reads into owned buffers, not mmap. This gives precise loading and ownership behavior and avoids mapping externally mutable index files. An mmap-backed buffer owner is a future optimization.
13. Default cache policy is `KeepLoaded`. A finite byte budget is an explicit alternative. There is no background memory-pressure daemon in this prototype.
14. Linux local filesystems on a 64-bit target are the supported persistence environment for this prototype. Keep other workspace targets compiling; unsupported storage operations return `UnsupportedPlatform`. Do not claim NFS, distributed filesystems, or Windows crash durability.
15. Implement correctness, fault injection, source-free restoration, update round trips, and benchmark evidence. A demonstration that only saves and loads a simple fixture is insufficient.

The user is assigning an implementation plan, not authorizing a merge into master. The implementing agent must follow the worktree workflow in section 3.

## 2. Evidence, dependencies, and existing code

### 2.1 Observed repository state

The repository inspected while preparing this specification is `Simon-Martens/objekte`.

- Reviewed Repair II tip: `dc371a42e666ef6c4d3c0519d6653326d0ad6d56`, branch `integration/rkyv-memory-index-repair`.
- Native comparison/master tip observed: `2e18871c0bcfb9cf9335d9e73525c64f91c960cf`.
- The remote branch `integration/native-publication-explicit-archiving` was not present when checked. Its specification is a prerequisite, not an implementation this document claims already exists.
- Core crate: `objekte-search-core`; library import: `objekte_search_core`.
- rkyv is pinned to `0.8.18` with `aligned, alloc, bytecheck, little_endian, pointer_width_64, std`.
- Roaring dependency is `0.11`; use the exact version in the implementation checkout's Cargo.lock. Do not upgrade it as part of this task.
- Existing dependencies include `sha2` and `libc`. Use SHA-256 for object/frame/leaf checksums. Do not add a database, compression framework, cache framework, async runtime, or serialization dependency.
- Workspace Rust minimum observed: `1.99`. Standard file locking APIs are available within that minimum.

These are historical review anchors, not the implementation starting point. The user will merge the archived-index changes into `master` before assigning implementation. Start this task from that merged `master` and record its actual commit plus relevant prerequisite commits in `revisions.json`. Preserve completed repairs. Do not start from a historical repair branch or claim that a historical benchmark measures this implementation.

### 2.2 Code requiring deliberate adaptation

| Existing location | Required adaptation |
| --- | --- |
| `archive_storage.rs` | Production constructor from owned aligned disk bytes; checked root ownership; read-only byte access for saving |
| `posting.rs`, `posting_values.rs` | Native/archived dictionary access independent of leaf residency; fallible lazy leaf resolution; no entry-vector staging |
| `bigram.rs` | Separate gram dictionaries from bitmap ownership; persistent descriptors; shared leaf loading; preserve build algorithm |
| `index_catalog.rs` | Split metadata-only access assessment from materialized execution access |
| `lifecycle_catalog.rs` | Persist and eagerly reopen metadata/path/ownership shards and current versions |
| `segment_archive.rs` | Eager record/file rows, record identity lookup and file-to-record membership; no source reconstruction |
| `native_segment.rs` | Segment domains and component descriptors, separate metadata validation from lazy payload validation |
| `inventory.rs` | Preserve file revisions, record locators, identity, diagnostics, liveness and lossless paths |
| `sorting.rs` | Stored typed keys and order arrays; native/archived sort views; demand loading |
| `repository.rs` and incremental/compaction modules | Common native/resident/disk-backed execution; affected-component rebuilding |
| `lifecycle_open.rs`, `lifecycle.rs` | Explicit storage open/save APIs and publication coordination |
| `source.rs` and source-access modules | Rebind registered readers and preserve source revision checks; do not parse at storage open |
| CLI and desktop callers | Compile with the new common abstractions; no automatic archive/save/open |

The reviewed `BigramIndex` has `normal: HashMap<u64, RoaringBitmap>` and `skip: HashMap<u64, RoaringBitmap>`. Packed construction blocks are temporary build machinery, not a separate disk format to preserve.

The reviewed `SortColumn` has typed keys, ascending and descending record-slot vectors, locale, options, and collation version. Preserve these semantics.

The reviewed `IndexCatalog` borrows materialized `PostingIndex` and `BigramIndex` objects. Its assessment routines call dictionary estimation and bitmap operations. Merely placing lazy wrappers around those objects would cause planning to load payloads. Fix this explicitly in section 7.

### 2.3 Required master base

The user will merge the archived-index/native-publication work into `master` first. This task starts only from that merged master commit. Verify native search-ready publication, explicit memory-only archive, direct serialization, mutation and compaction contracts in that commit; a branch name alone is not evidence of completion.

Do not base this work on `integration/rkyv-memory-index-repair`, the separate native-publication branch, or the historical commits above. Do not cherry-pick, merge or implement the prerequisite on the user's behalf. If the supplied master lacks the required changes, report the missing prerequisite during preflight and leave implementation unstarted. The user has already specified the resolution: merge that work into master before starting this task.

Record `git rev-parse refs/heads/master` immediately before worktree creation. That exact commit is the task base. Keep the original checkout unchanged; do not reset, switch, pull or merge it as part of this task. If the configured upstream and local master differ, record the difference and require the intended merged master to be available locally before creating the implementation branch. Do not silently choose a different base.

## 3. Worktree, supervision, and delivery

Read `AGENTS.md` and the complete `docs/ai/README.md` before execution.

- Integration branch: `integration/catalog-led-lazy-storage`.
- Sibling worktree: `/home/simon/source/objekte-integration-catalog-led-lazy-storage` when the original checkout is `/home/simon/source/objekte`.
- Task evidence: `docs/ai/generated/catalog-led-lazy-storage/`.
- Temporary notes: the original checkout's ignored `docs/ai/memory/catalog-led-lazy-storage/`.
- Create this integration branch from the verified merged `master`, not from an earlier implementation branch.
- Keep the original checkout on its existing main branch. No implementation, integration, builds that alter tracked files, or commits there.
- Use no worktree under `/tmp`.
- Use one supervisor. Delegation is permitted for the implementation under the fixed divisions in section 24; every delegated implementation uses its own sibling worktree and branch.
- Review explicitly staged paths. Commit coherent phases and push immediately after each commit, including merge and final report commits.
- Do not force-push, rewrite history, reset unrelated work, change remotes, or merge into master.
- Preserve user task-document edits. Follow repository rules for moving the final task to `done/` and appending its implementation report.
- Deliver one integration branch, its commit IDs, executed test evidence, measured results, and explicit limitations.

With the original checkout at `/home/simon/source/objekte`, create the worktree using:

```sh
git -C /home/simon/source/objekte worktree add \
  -b integration/catalog-led-lazy-storage \
  /home/simon/source/objekte-integration-catalog-led-lazy-storage \
  master
```

If this task's branch/worktree already exists, inspect its recorded base and resume only the matching task; do not overwrite it or create a second implementation silently.

Do not expand scope into a new query language, different segmentation policy, new source database, custom B-tree implementation, or user-interface redesign.

## 4. Terminology and immutable identities

Use these distinctions consistently in code and documentation:

- **Source file:** a user's JSON/Markdown/other adapter-supported file. Never write it from a storage operation.
- **Storage object:** a backend-owned immutable file under the explicitly selected storage directory.
- **FileId:** the existing source-file identity.
- **RecordId:** the existing stable logical record identity. Preserve its present scope and duplicate-ID rules; do not replace it with a local bitmap integer.
- **RecordSlot:** an integer in a particular immutable segment's record domain.
- **Segment version:** a particular immutable row/slot domain, not merely a segment number reused by later code.
- **Index descriptor:** eagerly available metadata describing an access path and its capabilities.
- **Dictionary frame:** a lazy, independently validated key-to-leaf lookup archive.
- **Leaf:** a compressed set of segment-local record slots. “Leaf” here means a posting payload, not an internal rkyv B-tree node.
- **Leaf ordinal:** a dense number within one leaf group/component. It is not a global record ID and is not necessarily key-sort rank.
- **Sort component:** precomputed keys/order arrays for one segment and sort configuration.
- **Storage generation:** one committed manifest selected by `CURRENT`.
- **Logical revision:** the existing revision of indexed content. Saving, loading a component, or changing residency does not change it.

Persist exact segment domain identity with each component descriptor. Use the digest of the canonical immutable record-slot rows and their ordering, together with collection/segment identity, as the disk domain identity. Exclude current liveness, mutable path-directory entries and later file-catalog revisions: a deletion must not change the domain identity of unchanged physical postings. Compute it during explicit storage preparation, not by adding hashing to ordinary startup.

Cache keys include storage-object digest, component identity and segment-domain identity, plus section/leaf ordinal. A cache must never reuse bitmap ordinal 7 from a previous rebuilt component just because both use the same numeric segment ID.

Keep stable ID scope explicit: where existing IDs are collection-scoped, the persistent record lookup key is `(CollectionId, RecordId)`. A global convenience lookup retains the current ambiguity behavior; never silently choose one collection.

## 5. Required loading policy

### 5.1 Eager before search readiness

Read, check checksums, validate rkyv structure, and validate cross-catalog associations for:

1. `CURRENT` and the selected manifest.
2. Source root binding, schema/configuration identity, source-reader and extension versions.
3. File metadata/path catalog and file versions.
4. Record metadata, record-ID lookup, file-to-record membership and segment ownership.
5. Segment identity order used for stable result order and tie-breaking.
6. Active-file and active-record masks, plus logical/raw-fallback domain classification needed to enforce query semantics.
7. **Index catalog:** every available access path, what it provides, whether it is complete, and how it may be used.
8. **Leaf catalog:** every posting leaf's object/component, byte offset, byte length, cardinality, codec and digest.
9. Small execution/planner summaries and component descriptors.

“Eager” means present in owned resident memory and already validated, not merely mmap-addressable. Retain archived catalog views where possible. Do not deserialize whole catalog shards into duplicate native maps just to answer normal metadata lookups.

File/record catalogs may include immutable retired rows needed by old postings and compaction, but current liveness must control active results. All eagerly required associations must be available without opening lazy dictionaries.

### 5.2 Lazy on execution demand

The following remain unopened and unvalidated at `open_storage` completion:

- Field-value B-trees and reverse directories.
- FTS normal and skip gram dictionaries for each field/representation/alphabet.
- Value posting bitmaps.
- Presence, null, structural, length, empty-selection, short-string-coverage and other query-specific leaves.
- Sort keys and ascending/descending/composite order arrays.
- Relation access dictionaries and payloads used only by relation queries.
- Additional stored query payloads exposed by supported built-in/custom bindings.

A field dictionary can load in full on first use. Its leaves remain independent. One query must not load all fields, all text representations, all sorts, or all leaves of a loaded dictionary.

Eager liveness masks are a specific exception to lazy bitmap loading. They are required for every query's correctness. Keep them separate from query-specific presence and null leaves.

### 5.3 Source files and readiness

Storage open does not traverse or stat every source file. It validates supplied definitions and registered readers against stored configuration and restores the saved snapshot. The snapshot describes the source revisions that were indexed.

Source materialization/scanning continues to use the existing revision checks. A changed source file cannot be read through stale byte offsets as though it were unchanged. An indexed-only query can reflect the saved snapshot until the caller explicitly reconciles changes. Document this snapshot behavior; do not imply that storage open refreshes the repository.

`SearchReady` on reopened storage is emitted only after all eager state is usable. It does not wait for lazy components. Emit the existing complete-initial-state event for a complete restored snapshot; distinguish restoration timing from construction timing.

## 6. Public API and fixed defaults

Add a public `storage` module. Reuse existing error and configuration types where practical, but preserve these entry-point semantics:

```rust
impl Index {
    pub fn archive_to(
        &self,
        directory: impl AsRef<Path>,
        options: StorageWriteOptions,
    ) -> Result<StorageWriteReport, IndexError>;

    pub fn open_storage(
        source_root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        repository_options: RepositoryOptions,
        directory: impl AsRef<Path>,
        options: StorageOpenOptions,
    ) -> Result<Self, IndexError>;

    pub fn open_storage_with_sources(
        source_root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        repository_options: RepositoryOptions,
        sources: SourceRegistry,
        directory: impl AsRef<Path>,
        options: StorageOpenOptions,
    ) -> Result<Self, IndexError>;

    pub fn storage_status(&self) -> Result<StorageStatus, IndexError>;
    pub fn trim_storage_cache(&self) -> Result<CacheTrimReport, IndexError>;
}
```

Provide the corresponding standalone `Repository` entry points, using `&mut self` for saving and `&self` for status/cache operations. Use the same underlying writer, reader and cache implementation; do not implement a second format.

`Index::archive()` and `Repository::archive()` keep the prerequisite's memory-only behavior. `archive_to` is the explicit convenience operation that includes required archiving. Do not add separate synonymous `save`, `flush` and `sync` public methods in this prototype.

`StorageWriteOptions`:

- `archive_workers: NonZeroUsize`, default 1; forward to the existing explicit archive machinery.
- No asynchronous/background flag.
- No “unsafe fast save” or no-fsync mode. Successful disk publication is synchronized.
- No compaction flag. Logical compaction is the existing separate operation.
- A synchronous caller may run this method in its own worker thread.

`StorageOpenOptions`:

- `cache_policy: CachePolicy`, default `KeepLoaded`.
- `max_concurrent_reads: NonZeroUsize`, default 4 per open backend shared by its views.
- `limits: StorageReadLimits` with explicit fields/defaults below.
- No eager-load-all-payloads, unchecked-validation, automatic-rebuild or automatic-reconcile option.

`CachePolicy` has exactly `KeepLoaded` and `Budget { resident_bytes: NonZeroU64 }`. A benchmark can use a very small budget. `trim_storage_cache` drops evictable entries and preserves data pinned by active queries/old views. It never discards unsaved data.

Read limits, configurable upward by the caller:

| Limit | Default |
| --- | ---: |
| Manifest body | 16 MiB |
| One eager catalog body | 256 MiB |
| Total eager catalog bytes | 2 GiB |
| One lazy archived dictionary/sort body | 256 MiB |
| One serialized Roaring leaf | 64 MiB |
| One decoded leaf allocation estimate | 128 MiB |
| One coalesced read span | 1 MiB |
| Gap allowed between coalesced requested leaves | 4 KiB |

Every length/count conversion and sum uses checked arithmetic. Limits are admission limits, not cache budgets. If a supported valid snapshot exceeds a configured limit, return `StorageLimitExceeded` with the limit and requested size; never truncate data or silently skip an index.

Storage directory paths are explicit. The caller can pass a temporary directory, a repository subdirectory, or a custom location. Resolve a relative path once. Do not add automatic directory discovery or alter the existing CLI's build defaults.

No new GUI is required. Add one core example and benchmark driver using these APIs. Existing CLI/TUI/GUI code must compile and retain ordinary native startup.

## 7. Eager index catalog and planner separation

### 7.1 Descriptor data

Persist an index descriptor for each collection/segment/access-path identity. The key includes:

- Collection and immutable segment domain.
- Field path, preserving array/path-selector semantics.
- Family: declared value, inferred value, named/custom value, structural, length, text, relation, or sort.
- Binding/extractor identity and version.
- Relevant normalization, alphabet, schema and sort configuration identity.

The descriptor must contain or reference:

- Value kind and multi-value/array behavior.
- Coverage count and the domain to which the completeness claim applies.
- Existing `admitted`, `complete`, `value_complete`, `empty_string_complete` and equivalent flags.
- Exact-answer versus candidate-only contracts.
- Supported comparison/operator/quantifier/derived-operation shapes.
- Presence/null/empty-selection/coverage leaf references and their counts.
- Dictionary distinct-key count and dictionary-frame locator.
- Component/leaf byte counts and estimated decoded costs.
- Text normal/skip dictionary locators and normalization versions.
- Sort configuration, collation version, key/order locators.
- Persisted proof facts needed by the current planner, such as absence of structural array/object values or complete scalar coverage.

Use structured enums/flags and existing semantic binders, not persisted function pointers or arbitrary executable planner expressions. Persist semantic inputs; execute the repository's current admission logic over them.

Create this descriptor from completed native/index metadata during finalization or explicit save. Do not compute it by reopening all payloads during storage open. Existing counts should be maintained or reused; any new construction work must appear in native-build benchmarks.

### 7.2 Two phases

Refactor access handling into:

1. **Metadata assessment:** bind a term, enumerate descriptors, check semantic compatibility/coverage, select candidates and estimate cost. No payload I/O.
2. **Selected access execution:** resolve only selected dictionary/leaf/sort handles and run the operation.

Do not retain a borrowed `&PostingIndex` or `&BigramIndex` as a prerequisite for assessment. Use an immutable descriptor handle and a typed execution resolver. This is an internal refactor; query language and result semantics remain unchanged.

For value-dependent cardinality unknown before dictionary lookup, use a conservative estimate: equality approximately covered_count / max(distinct_values, 1), bounded to the domain; range approximately half the covered domain; FTS a conservative binding-level estimate. These estimates affect cost only. An estimate of zero never establishes that a predicate has no matches.

Existing exact proof conditions that use bitmap relationships must have explicit persisted proof facts computed on validated completed data. If a condition is not represented, decline that optimization and retain its normal source verification; do not assume it. The implementation report must enumerate every existing `PostingAccess::assess` condition and its metadata equivalent. A regression that makes large classes of indexed queries scan is unacceptable.

`EXPLAIN` reports available/selected indexes, exactness, estimated rows, and cold/resident state without loading payloads. Presence of a cache entry may affect diagnostic cost, never semantic applicability.

After a dictionary is selected and opened, use actual leaf cardinalities from the eager leaf catalog to order intersections and stop early. FTS gram absence may prove no candidates only for a successfully loaded, compatible, complete dictionary. Missing files and decode failures are errors.

## 8. Eager file, record, and ownership catalogs

Persist the existing logical model; avoid introducing redundant global inventories.

| Catalog | Required association |
| --- | --- |
| Exact path directory | Existing scoped/lossless path identity → FileId |
| Ordered path directory | Existing prefix/range path behavior; preserve the existing ordered representation |
| File rows | FileId → path, revision, current version, status, memberships and diagnostics |
| File record membership | FileId/version and collection → record references, including zero-record files |
| Record lookup | Existing scoped RecordId → immutable segment address and row |
| Record rows | Stable ID, source FileId/version, byte range, source revision, class, diagnostics |
| Ownership routes | File/version → all segment routes that can contain that version |
| Liveness | Current active-file and per-segment active-record sets |
| Identity order | Per-segment ordering required for existing deterministic results |

Use archived hash maps for exact path/ID lookup, archived row arrays for metadata, and the existing compressed offsets/slots representation for file membership where suitable. Do not create both a global copy and a segment copy of every record entry. A global routing map may reference the canonical segment row.

Preserve non-UTF-8 paths through the existing lossless path representation. Query/display path normalization is not a substitute for filesystem identity. Preserve current path-key semantics and collision checks.

For metadata sharding, preserve the prerequisite's existing shard boundaries. Save only changed shard objects and reuse unchanged archived shard objects. Open all active shards eagerly. Do not reshape everything into a single newly cloned native map.

Queries must use borrowed archived row views for ordinary metadata. Legacy APIs that require native `FileEntry`/`RecordEntry` objects may materialize their documented compatibility cache on explicit access; do not populate the entire cache from storage open, generic query setup, status, or save. Report compatibility memory separately.

A record's byte range is tied to its indexed source revision and adapter. Preserve current source-reader identity/version and any additional adapter locator fields if the implementation base has added them. Never replace a richer adapter locator with an invented contiguous byte-range assumption.

## 9. Leaf catalog and storage references

### 9.1 Organization

A leaf group belongs to one immutable component/domain. Its owner descriptor stores the immutable pack-object digest and expected object length once. Its leaf rows are a dense archived array indexed by leaf ordinal.

A leaf row contains:

- `offset: u64`, absolute within the pack file.
- `length: u64`, serialized length.
- `cardinality: u64`.
- `decoded_bytes_estimate: u64`.
- `sha256: [u8; 32]`.

The standard nonempty row therefore has 64 bytes of logical fields before any format-specific framing. Record actual encoded size and native cache metadata size; do not hide the eager catalog overhead.

The codec is declared once by the leaf-group descriptor as `RoaringPortableV1`. Ordinal is implicit in the row index. A canonical empty leaf has zero offset, length, cardinality and decoded estimate and an all-zero digest. It reads no bytes and returns a shared immutable empty set.

Store all leaf catalogs in eager metadata objects separate from payload packs. The index catalog refers to leaf-group identities and ordinals. An eager root catalog maps descriptors to their leaf arrays; do not allocate a mutable mutex or `Arc<RoaringBitmap>` for every unloaded leaf.

The runtime resolve key is `(component version/domain, leaf ordinal)`. It is not an unscoped ordinal.

### 9.2 Validation

At open, validate counts, references, object identity, expected object length, ranges, arithmetic, codec, and ordinal bounds. Nonempty ranges must be within the advertised pack size, outside its fixed header, and must not overlap dictionary frames or unrelated leaves. Exact shared references must use the same descriptor identity; reject accidental overlapping aliases.

Write each component into one contiguous pack span, with explicit frame/group spans in its descriptor. Write nonempty leaves in ordinal order within their group, recording monotonically increasing offsets. Store pack component spans in physical order. Validate bounds/nonoverlap by linear passes through these eager descriptors rather than sorting a second whole-index vector of ranges during every open. Empty leaves use their canonical zero range and do not participate in overlap checks.

Do not read leaf bodies to validate directory entries. When first loading a leaf:

1. Check configured read/allocation limits.
2. Read exactly the selected range or a bounded coalesced span containing it.
3. Verify its SHA-256.
4. Use checked Roaring deserialization.
5. Require exact consumption of the leaf range.
6. Check cardinality against the catalog and every set bit against the immutable segment slot domain.
7. Check required logical-domain eligibility for that leaf family.
8. Publish the decoded bitmap only after success.

Historical deleted slots may legitimately remain in immutable leaves. Check against the immutable record domain and family eligibility, then apply current liveness during query execution. Do not reject an old posting just because a record has since been retired.

Bound container counts and allocation estimates before passing hostile length fields to allocation-heavy decoding. Use the pinned crate's format and the segment slot bound. The checked decoder remains mandatory; checksum verification does not replace structural/semantic validation.

## 10. Dictionary and FTS layouts

### 10.1 Field dictionaries

Persist the prerequisite's key → leaf-ordinal archived B-tree and existing ascending reverse directory in one lazy dictionary frame. Leaf bytes are external references via the eager leaf directory, never children recursively embedded in that rkyv root.

Serialize borrowed ordered map iterators directly. Do not collect `Vec<(Key, ordinal)>`, `Vec<&Key>`, a second B-tree, or a cloned list of strings. Preserve insertion-assigned bitmap ordinals. The retained reverse directory is an actual query structure and remains allowed; populate its archived form directly as specified by the prerequisite.

A borrowed archive byte slice is written directly to the writer. Do not call `to_bytes` on an already archived representation, deserialize it to a native map, or copy it through a second whole-component staging buffer.

### 10.2 FTS dictionary and payload separation

Use the same native ownership principle as postings:

- Native gram dictionary: `HashMap<u64, u32>`.
- Bitmap payloads: one canonical `Vec<RoaringBitmap>` for that normal or skip dictionary.
- Published native owners: shared immutable dictionary and shared immutable payload group.
- Stored dictionary: archived hash map from gram key to leaf ordinal.
- Stored leaves: standard Roaring leaf group.

Keep normal and skip dictionaries distinct. Preserve ASCII/Unicode representations, raw text, raw fallback, field text, named text bindings, record_count and normalization versions.

Change the final builder emission to produce this dictionary/payload layout directly. Do not first finish a `HashMap<gram, RoaringBitmap>` and then build a second hash map. Preserve the packed block construction algorithm and ownership adoption wherever possible. Do not add per-record/per-gram `Arc` allocations.

Archive a native hash map through its borrowed iterator. Hash-map serialization does not require collecting and sorting keys. Do not reuse the existing `for_each_normal`/`for_each_skip` helper for persistence if it collects/sorts a temporary key vector; add a direct borrowed serializer path.

Open only the requested binding's selected gram dictionary. Collect the query's gram references, use eager cardinalities to choose the smallest first, and resolve further leaves only while needed. An empty intersection stops future loads. Short literals with no usable grams keep the existing source-verification behavior.

FTS is still a conservative filter. Loading stored bigrams does not eliminate exact text verification/evidence generation against the existing source or stored-text path.

### 10.3 Query-specific state leaves

Presence, null, empty selections, length postings, structural postings, short-string coverage and relation membership use the same leaf API. A descriptor can point directly to a state leaf without opening the value dictionary.

Do not group a small presence/null request with every leaf of the corresponding field. Lazy unit boundaries must permit a presence-only query to read its required state leaf and no value tree.

## 11. Sort storage and relation payloads

### 11.1 Sorting

Persist one independently loadable sort-key frame per segment/field/configuration, plus independently loadable order frames:

- Typed key array, indexed by RecordSlot.
- Ascending slot array.
- Descending slot array.
- Each existing configured composite order and its constituent identities.
- Locale, options, logical type, exact numeric encoding and collation version in the eager descriptor.

Use stable stored wrappers for native types without direct rkyv support, including dates/times and exact decimals. Preserve existing comparison rules; do not reduce exact numbers to f64.

String keys store the already constructed ICU sort-key bytes. Opening a saved configured sort must not regenerate collation keys or sort records again. Array access should borrow the archived data directly, without restoring a full native SortColumn.

Keep ascending and descending arrays initially. Reversing one array can change equal-key tie-break order and null/missing placement. No reverse-order redesign belongs in this prototype.

Cross-segment sorted execution loads selected order/key frames, performs the existing correct merge, and applies active-record filtering. Do not assume per-segment rank is globally comparable. Preserve stable RecordId tie-breaking and pagination behavior.

On first selected sort use, validate key-array length/types and the selected order array against the immutable slot domain, require the expected permutation/coverage, and verify sorted order using the same comparator and tie-break rules. Load the corresponding key frame for this check; keep that work within the selected sort family. Validate a composite order against its specified constituent keys. Do not validate every sort in the segment at open.

Persist configured and already published stable sort structures. Query-created inferred sort caches remain ephemeral unless they already form part of published state; do not force their construction during save or open. Rebuilding an explicitly requested ephemeral order is measured separately from loading a persisted one.

### 11.2 Relations and custom bindings

Persist all published relation dictionaries/contributions required to restore current relation-query behavior without source parsing. Catalog descriptors state target collection, ID normalization, coverage, binding identity, and component references. Endpoint resolution and backlinks remain lazy where not part of eager identity routing.

Inventory every component reachable from the publication before implementing the writer. Produce `component-coverage.md` mapping each to eager metadata, lazy dictionary, Roaring leaf, sort array, other stored payload, or explicitly ephemeral cache.

Built-in/custom extraction that produces ordinary scalar/structural/text indexes uses these standard codecs. Do not serialize trait objects. Store registered reader/extractor identity/version and require a compatible registration on reopen.

If a published custom payload genuinely has no defined codec and cannot be derived from saved components without source access, fail save before changing CURRENT with `UnsupportedComponent` naming the binding. Do not silently omit it. Existing benchmark/test fixtures and built-in functionality must all round-trip; report any unsupported extension as a prototype limitation.

Result caches, verified-predicate caches, query plans, execution pools and source-reader objects are runtime state, not disk payloads.

## 12. Disk layout and format

### 12.1 Directory

Within the caller-selected storage directory, use:

- `CURRENT`: fixed 128-byte commit record.
- `writer.lock`: exclusive writer coordination.
- `readers.lock`: shared backend lifetime lease; exclusive offline pruning.
- `objects/`: immutable content-addressed `<64 lowercase hex SHA-256>.meta` and `.pack` files.
- `staging/`: task-owned uncommitted temporary files with create-new names.

Do not put repository paths, field names or source filenames into object filenames. Components and catalogs refer to object digests and typed numeric identities.

Use one new payload pack per affected immutable segment domain per save. Put the newly encoded dictionaries, leaves and sort frames for that segment into this pack; each component retains its own frame/leaf references. Unchanged components continue referencing their previous packs. Collection-wide components without a segment use one new pack per affected collection. Small metadata shards remain metadata objects. This avoids a file and synchronization operation per field or bitmap while permitting positioned reads of individual frames/leaves.

Unchanged packs and metadata shards are reused across generations. Changed components are grouped into the new segment/collection pack; no unchanged sibling is copied merely to place it beside a replacement. Do not append to or overwrite committed packs. A partially referenced old pack can retain unused bytes until logical compaction produces fresh packs; report this internal disk fragmentation. Offline pruning can remove whole unreferenced packs, not holes within live packs.

### 12.2 Fixed envelope

Every object starts with a 64-byte envelope:

| Byte range | Field |
| --- | --- |
| 0..8 | ASCII magic `OBJSTOR1` |
| 8..10 | little-endian format version, 1 |
| 10..12 | little-endian object kind: metadata=1, payload pack=2 |
| 12..16 | little-endian header length, 64 |
| 16..64 | reserved, all zero |

Metadata objects contain exactly one rkyv body after the envelope, with the root at the standard end position. The catalog's stored-type/schema discriminant is in the body and checked against the manifest reference.

Pack frames/leaves start at 16-byte-aligned offsets; zero padding is included in the pack object digest. Dictionary/sort frame references store exact offset, body length, expected stored type/version and SHA-256 of the body. A leaf reference is defined in section 9. Relative rkyv pointers remain entirely within one body.

The whole object digest determines its filename. Compute it while writing the fixed envelope and body/padding. Never include the filename/digest in the bytes being hashed. Object byte lengths live in descriptors/manifests.

A metadata object's whole digest is checked when eagerly read. A pack's filename is an object identity; lazy open does not hash the whole pack. Each requested frame/leaf verifies its own digest. Whole-pack verification is available only through an explicit integrity audit, not normal open.

Use fixed rkyv format features from section 2. Envelope format version 1 selects exactly that pinned rkyv format profile and the version-1 stored root schemas before any archive access. Record crate version, schema version, pointer width, endianness and relevant engine format versions in the manifest as additional consistency checks after checked access. Any future profile/layout change must change the envelope version rather than relying only on an archived field that cannot safely select its own decoder. Reject unknown envelope versions before interpreting archived bytes. No backward migration or best-effort decode is required in this prototype.

### 12.3 CURRENT

Encode manually, with no native struct transmutation:

| Byte range | Field |
| --- | --- |
| 0..8 | ASCII magic `OBJCURR1` |
| 8..12 | u32 little-endian version, 1 |
| 12..16 | u32 flags, zero |
| 16..24 | u64 storage generation |
| 24..56 | manifest object SHA-256 |
| 56..64 | u64 manifest object length |
| 64..80 | 128-bit store identity |
| 80..96 | 128-bit logical index lineage identity |
| 96..128 | SHA-256 of bytes 0..96 |

Generate initial store/lineage identities only during explicit storage initialization, using operating-system randomness through a small Linux helper or an existing repository facility. Do not add disk/randomness calls to every normal index insertion. Reopened state preserves lineage identity. Generation increments use checked arithmetic.

The manifest includes its storage generation, logical revision, lineage/store identity, configuration fingerprint, complete-state flag, catalog object references, component references, counts and accounting. It is a new small metadata object for each changed saved snapshot.

The current manifest must describe one complete publication. Never mix catalogs from one logical revision with leaves from another.

### 12.4 Integrity and compatibility

Use SHA-256 through existing `sha2`. It detects corruption, not authenticated provenance. All reads remain bounded and structurally checked.

Never read unused aligned-buffer capacity or allocator padding into the saved representation. Persist serialized lengths only, with explicit zero file-alignment padding.

No monolithic bincode serialization of the repository. No pointer values, mutex state, Arc layout, Rust enum memory dump, or external rkyv references. Every archive root must remain valid after all original native objects and serializer temporaries have been dropped.


## 13. Save operation and durable publication

### 13.1 Preconditions and store ownership

`archive_to` requires a completed logical publication, just as explicit archiving does. Reject incomplete construction promptly before waiting for the transition gate, and recheck after acquiring it.

A new target directory may be created only when its parent exists. Initialize the backend-owned directories and lock files explicitly. Refuse a nonempty directory that is not this storage format; never repurpose it by deleting its contents. A failed first save may leave recognizable backend-owned staging/orphan objects but no committed snapshot.

Maintain a store binding per explicitly used destination, containing store identity, lineage identity and the exact last-observed `CURRENT` token. On first open it comes from the committed snapshot; after a successful first save it comes from the new commit. Exporting to a new destination preserves the existing destination's binding so a later save can still check its expected parent.

- Saving back to the same bound store requires the current token to equal the expected parent.
- A second independently opened writer that has become stale returns `StorageConflict`. Never overwrite its peer's newer generation.
- A newly built unrelated index cannot overwrite a nonempty store just because source paths match.
- Saving to a fresh directory exports a complete snapshot and establishes a new store binding while preserving logical lineage.
- No force-overwrite option is provided.

The prototype allows one active save per Index/Repository and one writer per storage directory. Other processes may open/read committed snapshots concurrently.

### 13.2 Lock order

Use this order:

1. Exclusive index transition permit from the prerequisite.
2. Shared storage lifetime lease on `readers.lock`.
3. Exclusive nonblocking `writer.lock`.
4. Existing short publication lock only for root capture/install.

For operations on an already open backend, reuse its shared lease; do not reacquire the same file lock through a cloned descriptor. A fresh store operation obtains one shared lease that becomes part of its retained StoreSession.

Fail competing disk-writer acquisition with `StorageBusy`. Do not wait while holding a global cache or publication lock.

Queries do not acquire the index transition permit or writer lock. They pin their immutable root/StoreSession and load through the cache. Mutations publishing new logical roots wait at the transition gate during save. This prototype deliberately serializes save against mutations rather than attempting concurrent optimistic snapshot saving.

Completion callbacks run only after transition/writer locks are released. Test a callback that invokes a subsequent archive/save operation.

### 13.3 Algorithm

Implement one transaction coordinator with explicit stages and fault-injection points:

1. Precheck completion/quiescing and destination eligibility.
2. Acquire locks in section 13.2; compare the expected parent CURRENT token.
3. Capture the authoritative complete publication.
4. Run the prerequisite's internal archive preparation/install for native eligible dictionaries/catalogs, without recursively calling the public lock-taking method. Preserve its atomic in-memory conversion behavior.
5. Capture the resulting publication and keep its logical revision fixed for the save.
6. Enumerate unique component owners and build the persistent dependency graph. Reuse immutable stored components, existing archive buffers and shared native leaf groups.
7. Prepare missing catalog/dictionary/sort encodings from borrowed views. Group newly encoded components into the segment/collection packs specified in section 12 and write their leaf directories with bounded working memory. Generate component summaries and canonical domain identities.
8. Install all new immutable payload/catalog objects. Reuse unchanged objects in the same store. For export to a new store, stream-copy required immutable objects and verify their whole-object digest; never decode/reencode them solely to export.
9. Construct and validate the manifest referencing exactly the saved snapshot. Install it as an immutable metadata object.
10. Synchronize every newly installed object's file data and metadata, then synchronize `objects/`. Initialization also synchronizes required directory entries in the store and its parent.
11. Write a new 128-byte CURRENT record to a create-new staging file in the store root, flush and `sync_all` that file.
12. Atomically rename that temporary file over `CURRENT` within the same filesystem.
13. Synchronize the store directory. Only then report durable success.
14. Update the in-memory store binding and install backing descriptors/cache registration for the same logical publication, preserving loaded data. Do not evict or reload data as a side effect of saving.
15. Release locks, notify callers, and drop temporary owners outside the publication lock.

Prepare the nonfallible bookkeeping needed by step 14 before the commit rename. Storage-backend installation must not have a recoverable allocation/validation step after a known successful durable commit. Process-wide OOM/abort is not a recoverable transaction guarantee.

Memory conversion in step 4 can succeed even if a later disk write fails. Document this precisely: logical contents/revision stay unchanged, and the previous disk CURRENT remains authoritative before step 12, but the in-memory representation may already be archived. Do not claim a combined RAM/disk atomic transaction.

If the snapshot has already been saved to this store and neither its logical contents nor persisted component set changed, return `unchanged=true` with zero new serialization, zero payload reads, zero writes and zero fsync calls. Reading/checking the small CURRENT token under the writer lock is allowed. Cache residency changes alone never create a storage generation.

### 13.4 Writing objects without whole-index staging

Use one streaming writer with a 1 MiB reusable output buffer by default. Archive conversion workers may run up to the requested bound; do not introduce a second unbounded disk-writing pool.

Write rkyv bodies from existing immutable byte slices. Serialize each Roaring leaf directly into the counting/hash writer, obtaining offset, exact length and digest. Do not expand bitmap members, clone bitmaps, create a `Vec<Vec<u8>>` for all leaves, or assemble one giant repository byte vector.

A small owned vector containing the final leaf-directory rows is allowed: it is persistent metadata, not staging of keys or bitmap payloads. Avoid duplicate row vectors where a borrowed serializer can use the authoritative table.

Reuse writer scratch across components. Track native payload, archive output, metadata output, serialization scratch, buffered I/O and SHA work separately.

Create new files in `staging/`. Install an immutable final object using a no-overwrite operation on the same filesystem; on Linux use `std::fs::hard_link` from the completed staging file to the digest filename, then synchronize the file and remove the staging name. If the target already exists, verify its size and whole digest before treating it as deduplicated. Never truncate or replace an existing object. No cross-filesystem fallback copy into a visible final filename.

Synchronization order for new dependencies is mandatory even if the operating system appears to have cached all writes. `BufWriter::flush` and dropping a File do not establish crash durability.

### 13.5 Failure states and cancellation

Before the CURRENT rename:

- Errors/cancellation leave the old CURRENT unchanged.
- Unreferenced new objects and staging files may remain for offline pruning.
- Return the real error and transaction stage.
- Native/archived in-memory queries remain usable.

After CURRENT rename but before successful store-directory synchronization:

- A sync error has an uncertain durable outcome.
- Return `CommitUncertain` with generation/manifest identity and underlying error.
- Do not delete either generation's dependencies, restore an old CURRENT blindly, or tell the caller the save definitely failed.
- Mark the store binding as requiring reconciliation. A later save rereads CURRENT, validates the observed candidate manifest and retries directory synchronization before proceeding. If this cannot establish a valid parent, return the explicit error.

After successful directory synchronization, cancellation cannot turn the committed operation into an “unchanged disk” failure. Report committed success and whether cancellation arrived after the commit point.

No default fallback to an older manifest on corruption. That could resurrect deleted records. A corrupt CURRENT/manifest returns an error requiring an explicit rebuild/recovery decision.

## 14. Open operation

`open_storage` never calls `Repository::open` or another source-building constructor internally.

1. Resolve the supplied storage and source-root paths once.
2. Acquire the shared storage lifetime lease. Verify format-owned directory/lock structures.
3. Read exactly one CURRENT record; validate magic, version, flags, checksum and bounds.
4. Read its exact manifest object, check whole-object digest and envelope, then validate the archived body.
5. Compare configuration, reader/extension registrations, collation and normalization versions, source-root binding, and logical complete-state requirements.
6. Read and validate all eager catalogs referenced by that manifest. Deduplicate shared catalog owners.
7. Validate file/record/version/membership/ownership/domain/descriptor/leaf-reference consistency using eager metadata only.
8. Build the lightweight runtime root from eager owners and lazy typed handles. Reuse existing worker/runtime configuration supplied by the caller. Do not reconstruct trees, gram filters or sorted order.
9. Publish that root atomically and set search-ready/completion status.
10. Return the Index/Repository and its open timing/statistics.

CURRENT can change while opening: the reader keeps the single valid manifest it captured. Committed objects are immutable and retained. Do not repeatedly restart just because another process saved a newer generation.

Do not open all payload pack file descriptors during storage open. Lazy pack existence/size/header checks occur when first accessed. Missing eager data fails open. Missing lazy data fails its first selected query. Document that readiness is not a full integrity audit.

The prototype binds to the supplied normalized source root and configuration fingerprint. Reopening under a different source root returns `IncompatibleConfiguration`; relocation support is outside this task. Relative source paths within a matching root remain lossless.

Configuration validation does not recreate data-derived inference from source. Persist the final inferred declarations, observations needed for later correct mutation/compaction, and their admission state. Preserve all schema-free inference behavior from the saved publication.

A missing/unsupported source registration fails open before search-ready. A parameter used only for execution, such as query worker count or cache budget, may differ if it does not alter indexed semantics.

## 15. Demand-loading API and ownership

### 15.1 Storage handles

Add typed internal handles for dictionaries, sorts and bitmap groups. Separate logical immutable references from transient residency.

Conceptually:

```rust
enum ComponentBacking<T> {
    Resident(Arc<T>),
    Stored(StoredComponentRef<T>),
}

enum BitmapLease {
    ResidentGroup {
        owner: Arc<NativeBitmapGroup>,
        ordinal: u32,
    },
    Decoded(Arc<CachedBitmap>),
}
```

These sketches define ownership, not permission to implement unsafe self-references. A lease borrows a bitmap or archived root only for a lifetime tied to the owning lease.

Freshly built native components use the resident path directly. Do not force every native lookup through the disk cache, descriptor hashing or a mutex. The native-only build/query path remains a first-class fast path.

Stored handles contain immutable locations/type identity and a shared StoreSession, not a permanent strong reference to the cached payload. Otherwise eviction cannot release it.

A query acquires selected component leases once per relevant execution scope and reuses them in inner loops. Do not repeatedly resolve/lock/increment Arc counters for every record tested.

### 15.2 Checked archive owners

Extend `ValidatedArchive` with a production disk-byte constructor that takes ownership of an already aligned allocation. Read into that allocation directly; do not read a full component into Vec<u8> and then copy it into AlignedVec.

Use checked access on the exact frame body and retain the same immutable allocation. All subsequent borrowed roots are tied to that owner. Any internal unsafe root accessor must document the prior validation, alignment, immutability and lifetime conditions.

Opening a dictionary runs semantic checks once: key order/uniqueness, numeric/typed encoding, ordinal bounds, reverse-directory agreement and component-domain agreement. It must not resolve leaves merely to check their values. Leaf semantic checks occur when each leaf loads.

After eviction, a new disk read is a new allocation and must be checked again. Do not persist a “validated once” bit that authorizes unchecked access to later file bytes.

A stored-but-unloaded component is archived in encoding, but is not a resident validated owner. `archive()` treats it as already encoded and leaves it cold. Status must distinguish encoded representation from loaded/validated residency.

### 15.3 Positioned I/O and bounded batching

Use positioned reads on shared immutable file handles. Do not race a mutable seek cursor across queries and do not serialize every read through a single global file-position mutex.

Acquire a read permit only around I/O. Do not hold cache locks, the publication lock or transition gate while waiting for I/O or validating/deserializing.

Cache up to 64 opened pack handles per StoreSession, using reference-counted leases for active readers. An entry can leave this handle cache while an active read retains its File. Opening every pack in a large repository would exhaust descriptors; repeated warm queries should not reopen each needed file.

`resolve_many` takes selected missing leaf references, groups by pack, orders by offset, and coalesces adjacent requested ranges when the total span is at most 1 MiB and intervening gaps are at most 4 KiB. The temporary list of selected read requests is allowed; it is not index serialization staging.

Read gaps may contain unrequested bytes, but do not deserialize or admit unrelated leaves because their bytes happened to be fetched. Count requested payload bytes and physical span bytes separately.

Do not eagerly load a whole bitmap group for a one-leaf query. Broad range operations can submit bounded batches of selected leaves. FTS loads the smallest selected leaf first and intersects incrementally, allowing early termination before scheduling remaining reads. No speculative background prefetch is required.

## 16. Cache behavior, concurrency, and reclamation

### 16.1 Single load per component

Use 64 cache shards selected by a stable hash of the complete component/leaf identity. Entries are created only for requested units. Entry state is:

- Loading: one owner performs read/validation; waiters share completion.
- Ready: immutable resident owner.
- Failed: classified error associated with that immutable input and load attempt.

Use short mutex sections and condition variables or the repository's existing equivalent. Never hold a shard lock during disk I/O, hashing, decode, arbitrary callbacks, or waiting for another entry.

For a concurrent request of the same cold unit, precisely one read/decode is started. Waiters obtain leases to the same resident owner. Different units can load up to the I/O limit.

A waiting query may cancel independently. It does not cancel a load still needed by another query. For the prototype, the synchronous loader finishes its bounded unit even if its own query is cancelled, then publishes the result and honors cancellation. No detached jobs survive backend shutdown.

Cache corruption/format/domain failures as permanent for that stored identity, avoiding repeated reads of known-bad immutable bytes. A transient I/O error or cancelled unstarted attempt is delivered to current waiters, then permits a later explicit query to retry. Never leave a permanent Loading state after an error/panic unwind.

### 16.2 Policies

`KeepLoaded` retains every successfully loaded unit until explicit trim, backend close, or replacement makes its cache ownership obsolete. There is no arbitrary time-based expiration.

`Budget` uses a second-chance clock over admitted units:

- One reference bit is set on a cache hit; no linked-list relocation on every hit.
- An insertion exceeding the cache's owned-resident target sweeps candidates.
- Clear a referenced unit's bit and skip it once; remove unreferenced eligible units.
- Never wait for a query to release a pinned unit.
- A unit larger than the budget may be loaded for the query but is not retained after its last lease. Record an oversize bypass.
- Pinned data, eager catalogs and unsaved native data may exceed the budget. Report these categories rather than claiming the budget bounds total RSS.

Native build payloads are not cache entries before storage backing exists. Unsaved data cannot be evicted. Saving must preserve its resident owners without a copy.

### 16.3 Retaining freshly saved data without duplication

A fresh publication already owns native bitmap groups. After committing their disk representations, transfer their shared owners into a **resident group cache unit** and replace the new root's payload backing with stored references that can resolve through that unit. Old views may still own the prior resident group.

Do not allocate one Arc or clone a bitmap per leaf merely to register a warm group. A group unit owns the existing Arc<Vec<RoaringBitmap>>; a BitmapLease pins the group and selects an ordinal.

A warm group is evicted as a group. Later cold accesses can cache individual decoded leaves. This provides precise per-leaf lazy loading after reopen without forcing per-leaf allocation overhead on normal native construction.

Apply the same ownership rule to native sort collections if they are grouped by their existing owner: retain one accounted warm owner and expose selected views. Eviction may release the group; subsequent reloads remain per stored frame.

Avoid retaining obsolete native key maps behind leaf owners. The prerequisite posting separation and section 10's FTS separation exist to make this possible. After conversion, the new publication must not retain an entire native dictionary solely to keep its bitmap values alive.

If a native object is still held by an old IndexView, dropping the cache entry does not reclaim it yet. Count pinned retired owners separately. Never fabricate immediate RSS reduction from removing a cache key.

### 16.4 Leases and store lifetime

Every stored component handle/old snapshot that can initiate a future read retains the StoreSession and its shared `readers.lock` lease. Every loaded component/leaf used by a query remains valid after cache trimming.

Closing the Index prevents new work through that handle but does not invalidate already pinned views. Explicitly test late lazy loads through an old view after the current root has been replaced and the Index handle dropped.

Track cache entries, hits, misses, load joins, evictions, bypasses, in-flight loads, loaded bytes, pinned bytes, native group bytes and eager metadata separately. Cached serialized leaf staging bytes are discarded after decode. Do not intentionally retain both serialized leaf buffers and decoded bitmaps in the application cache; the OS may separately retain file pages.

## 17. Query semantics and error propagation

Refactor query paths to accommodate fallible component loading. Do not preserve an infallible `Option<&RoaringBitmap>` API by mapping load failures to None.

Distinguish:

- Key absent from a successfully opened complete dictionary.
- Empty leaf explicitly declared by a validated catalog.
- Index unavailable for this predicate because coverage/semantics do not apply.
- Required index unreadable/corrupt/incompatible.
- Source materialization unavailable or source revision changed.

Only the first two can produce a known empty restriction. Semantic unavailability keeps the existing fallback path. Storage errors propagate to the existing query error surface.

Apply active-record/domain masks for positive, negative, range, count, relation and sorted paths. In particular, `NOT` and `!=` complement against the correct active eligible universe, not every physical slot in a saved segment.

A streaming query can discover an error after producing earlier results. Deliver the terminal error/incomplete status; never mark it as a successful complete result. Buffered queries return the error. Cache no “complete” result for such an execution.

Keep limit, offset, evidence, count, cancellation, invalid/raw-fallback classifications, duplicate-ID handling and deterministic order identical across native, resident archived, mixed, and reopened lazy backings.

Metadata-only catalog inspection and EXPLAIN must cause zero lazy payload loads. Indexed query execution may still read source files when the existing semantics require verification, extraction, evidence or result contents. Storage instrumentation must separate index I/O from source I/O.

## 18. Add, replace, delete, move, and compact after reopen

All current mutation APIs remain supported after `open_storage`. Preserve the existing immutable publication and version/liveness model.

- **Add:** parse the added file through the existing adapter, create native new/affected components, update catalogs and publish. Existing cold unrelated components remain cold.
- **Replace:** retire the old version and append the new version. Use eager file ownership/membership to identify affected segment routes. Rebuild only what the existing segmentation policy requires.
- **Delete:** retire file/record identities through eager catalog/liveness updates. Do not open value/FTS/sort payloads merely to remove postings; old leaves remain filtered by current liveness until compaction.
- **Move:** preserve the existing file identity/revision semantics and path-dependent index invalidation. Rebuild only indexes whose meaning depends on the path/metadata that changed.
- **Compact:** read the affected stored components and eager rows, merge live data and create native replacement components. No source parsing is permitted for source-free compaction.

Do not choose a different target segment or compaction policy simply because it is already warm. Preserve the prerequisite's deterministic smallest-eligible-segment policy and limits.

When a rebuild needs an archived dictionary or leaf, resolve it through the same checked loader. Propagate errors and leave the current logical publication unchanged. Do not eagerly thaw every segment or build a full duplicate repository.

Later mutations yield native/mixed state again. They do not automatically archive or save. A subsequent `archive_to` persists changed components and metadata and reuses unchanged disk objects.

A deletion-only save writes changed eager metadata, liveness and a manifest, but rewrites no unrelated dictionary/leaf packs. A no-change save does no serialization or payload I/O. A rebuilt component may write all of its replacement frames/leaves into the new segment pack; partial in-place leaf updates are outside this prototype.

Persist enough inference, relation and identity information for source-free compaction after cold reopen. A design that can answer queries but needs to reparse sources to compact fails acceptance.

## 19. Integrity audit and offline storage pruning

### 19.1 Explicit audit

Add `storage::audit(directory, options) -> AuditReport` as a blocking maintenance API. It acquires a shared store lease, pins one CURRENT manifest, validates all referenced metadata/frames/leaves and whole object digests, and returns a complete report or classified errors. It performs no source parsing and does not modify the index.

Default audit parallelism is 1; options accept a nonzero worker limit. Auditing is not invoked by open, save reuse, a query, or status. Report audit cost separately from opening.

This is the operation that proves all saved payloads are readable; ordinary search-ready deliberately proves only eager metadata and loadability contracts.

### 19.2 Offline pruning

Logical index compaction and disk object reclamation are distinct. Add `storage::prune(directory) -> PruneReport` as an explicit offline operation.

- Acquire an exclusive nonblocking `readers.lock` lease, then an exclusive nonblocking writer lock.
- Return `StorageBusy` while any backend/old view in any cooperating process retains a shared lease.
- Validate CURRENT and its manifest before deleting anything.
- Traverse all objects referenced by that one current manifest and its eager catalogs/descriptors; this needs no leaf payload reads.
- Remove only unreferenced backend-owned object files and recognized staging files. Never follow symlinks or delete unrelated files.
- Preserve all current dependencies. Synchronize changed directories before reporting completion.
- If current metadata cannot be validated, remove nothing.
- No automatic rollback-generation retention in this prototype. Offline prune retains the current generation's complete reachable graph and removes older/unreferenced objects.

This offline policy avoids online reader-registration and cross-process garbage-collection complexity. After compaction and save, disk bytes may remain until the caller drops all readers and invokes prune. Report this honestly.

Read-only processes cooperate through the shared lease. Protection against an unrelated program deliberately rewriting files outside the protocol is not provided; owned input buffers and checksums still prevent stale mapped-memory access.

## 20. Diagnostics and material risks

`StorageStatus` and reports must expose:

- Native/archived/stored encoding state separately from unloaded/loading/resident/failed state.
- Logical revision, storage generation, lineage/store identity and last successful save token.
- Eager catalog counts and bytes by file/record/index/leaf/domain category.
- Stored object count and live referenced bytes by dictionary, leaf, sort and metadata.
- Serialized length versus aligned allocation capacity.
- Resident decoded bitmap bytes; warm native group bytes; compatibility caches.
- Current cache-owned, query-pinned, retired-view-pinned and in-flight bytes where measurable.
- Source reads/parses separately from index reads.
- Physical read calls/bytes versus requested frame/leaf bytes.
- Cold load, checksum, bytecheck, semantic validation and Roaring decode timings.
- Cache hits/misses/joins/evictions/oversize bypasses.
- Save bytes newly serialized, bytes written, objects reused, objects copied, sync calls and sync wall time.
- Open phases through search-ready.
- Transition-gate wait time and mutation delay during saves.
- Failed/corrupt components and last classified error.

Do not sum overlapping worker intervals and present the sum as wall time.

The completion report must address these risks with evidence:

1. Eager file/record/leaf catalogs impose a memory floor that scales with the number of records and leaves.
2. The leaf directory can be material overhead for very small bitmaps. Report bytes per leaf and ratio to leaf payload, including empty rows.
3. Warm cache lookup/ownership overhead can regress tiny queries even when disk is unused.
4. Cold queries may touch many segments, creating random I/O; batch where applicable and measure actual reads.
5. Native sources, archive output and write buffers overlap during explicit save.
6. Queries and save contend for CPU/memory bandwidth despite short publication locks.
7. Default KeepLoaded intentionally allows retained payload memory to grow.
8. Eager catalogs, unsaved native state and pinned readers are outside an eviction target.
9. Source changes are reconciled explicitly; storage open restores an indexed snapshot.
10. A failure after CURRENT rename can have an uncertain durable outcome.

Do not add broad warnings or permission prompts to normal product flows. Surface concrete errors and accurate status through the established API.


## 21. Required tests and fixtures

### 21.1 Test harness

Add a narrow internal `StorageIo` abstraction used by the writer/reader, with the real Linux implementation and a deterministic test implementation. It must expose counters and named fault points for open, positioned read, write, flush, file sync, object install, CURRENT rename and directory sync. Do not scatter test-only sleeps or environment-variable branches through query logic.

Use deterministic barriers/channels for concurrent load, save, mutation, trim and shutdown tests. Fault injection must target a known operation number or transaction stage, not a probabilistic timing window.

The primary fixture contains:

- At least two collections and three segments in one collection.
- Zero-record, one-record and multi-record files.
- Valid JSON and JSONL records; at least one registered-reader fixture and raw-fallback/invalid records.
- Repeated and distinct string values, Unicode, escaped strings, long strings.
- Exact integers, decimals that f64 cannot distinguish, negative numbers and booleans.
- Present/null/missing/empty-array/array/object and length cases.
- Normal/skip bigrams in ASCII and Unicode; separate named field text bindings.
- Ascending, descending, equal-key, null/missing and composite sorting.
- Relations across segments/collections.
- Non-UTF-8 source paths on Linux.
- Deleted/replaced versions and partially live segments.

Keep fixtures small enough for repeated fault tests. Use the existing independent evaluator/fresh-build comparison as the semantic reference. Compare complete IDs, order, count, pagination, evidence/classification and errors, not only result lengths.

For source-free open/compaction tests, keep the supplied source-root directory identity available while denying source-file reads through the instrumented reader. Check indexed-only results and I/O counters during that interval. Restore normal source access before comparing FTS verification/evidence or full record materialization. Failure to read source contents for a query that inherently requires them is not evidence that the index restoration or compaction read sources.

Every row below requires an executed test or parameterized case. Multiple rows may share a harness, but the final report must map each ID to its actual test command/name and outcome.

### 21.2 Format, eager state, and laziness

| ID | Required proof |
| --- | --- |
| S01 | Ordinary native build creates no storage directory, performs no archive serialization and preserves search readiness |
| S02 | Memory-only archive creates no files and preserves all query semantics |
| S03 | Save/reopen restores file, record, index and leaf catalogs before search-ready |
| S04 | Open counters show zero lazy dictionary/sort-frame reads, zero leaf reads, and zero query-leaf deserializations |
| S05 | Open performs no source discovery/read/parse and constructs no value trees, gram dictionaries or sort orders |
| S06 | File/path/RecordId lookup and index-capability listing work after eager open without payload reads |
| S07 | EXPLAIN and candidate-index assessment read no payloads even with many available indexed fields |
| S08 | Exact filter loads its selected field dictionary and required leaf only; unrelated fields/FTS/sorts stay cold |
| S09 | A presence-only or null-only query loads the required state leaves and no value tree |
| S10 | FTS loads only its selected binding/normalization dictionaries and needed leaves; field-value/sort indexes stay cold |
| S11 | A missing gram in a complete dictionary produces empty candidates without loading unrelated leaves |
| S12 | An early empty intersection prevents subsequent FTS leaf loads |
| S13 | One requested sort loads the necessary sort frames only; unrelated sort fields remain cold |
| S14 | Repeated warm query performs zero additional index file reads, checksum passes or decodes for retained units |
| S15 | Empty leaf resolves without disk I/O; sparse/array/bitset/run Roaring containers round-trip |
| S16 | Single-leaf requests do not load all sibling leaves; coalesced requests decode/admit only requested leaves |

### 21.3 Catalogs, semantics, and ownership

| ID | Required proof |
| --- | --- |
| S17 | Path → file → records and RecordId → file/version/locator agree for zero/one/many records |
| S18 | Multiple collections, duplicate-ID policy, segment-local slots and stable IDs remain unambiguous |
| S19 | Non-UTF-8 paths round-trip without loss and preserve exact lookup versus display/query paths |
| S20 | File source revisions and adapter identities survive; stale locators are not used against changed source bytes |
| S21 | Archived dictionary insertion-assigned ordinals survive key-order differences; reverse ranges return correct leaves |
| S22 | Inclusive/exclusive/unbounded numeric/string/date ranges match native results |
| S23 | Null/missing/empty selections, arrays, derived lengths and negative predicates preserve domain semantics |
| S24 | Exact large/decimal numbers retain precision through dictionaries and sort keys |
| S25 | Configured sorting preserves ties, null/missing placement, both directions, composites and cross-segment pagination |
| S26 | Relations/backlinks/ID normalization match before save and after lazy reopen |
| S27 | FTS ASCII/Unicode/skip/normal/raw-fallback/named-field results and evidence match native execution |
| S28 | Lazy archive root references remain valid through cache trim and caller moves while their lease lives |
| S29 | Old native key maps are reclaimed after conversion and last old view; no permanent dictionary fallback is retained |
| S30 | Saving reuses bitmap-group ownership without deep bitmap clones or per-leaf Arc construction |
| S31 | Catalog open and ordinary queries do not populate whole native FileEntry/RecordEntry compatibility caches |
| S32 | Already encoded cold components remain cold during archive/status/no-change save |

### 21.4 Cache and concurrent behavior

| ID | Required proof |
| --- | --- |
| S33 | Concurrent first access to one component performs exactly one read/decode and returns a shared owner |
| S34 | Different components can load concurrently within the configured limit |
| S35 | A waiting query can cancel without invalidating another query's load |
| S36 | Transient I/O failure permits a later retry; corruption is retained as a classified failure; no stuck Loading state |
| S37 | Budget eviction and explicit trim preserve pinned queries and eventually release unpinned loaded units |
| S38 | An oversize leaf/frame serves its query without indefinite cache retention or retry loops |
| S39 | Eager catalogs and unsaved native data are never evicted as ordinary payload cache entries |
| S40 | Native group cache eviction releases the whole group when unpinned; later reads can reload individual leaves |
| S41 | Old views can initiate late lazy loads after mutation/current-root replacement and Index-handle drop |
| S42 | Rebuilt component/slot-domain identity prevents stale cache reuse for reused numeric ordinals |
| S43 | File-handle cache remains bounded; active positioned reads survive handle-cache eviction |
| S44 | Concurrent queries remain possible during save; mutations wait and then apply to the correct root without lost changes |
| S45 | Completion callback can invoke archive/save after locks are released; quiesce/cancellation cannot deadlock |
| S46 | Warm native queries bypass disk-cache resolution; measure and assert structural counters rather than timing in unit tests |

### 21.5 Mutation and compaction round trips

For each mutation case, compare: the live mutated instance, a saved/reopened instance, and an independently rebuilt reference over the edited fixture.

| ID | Required proof |
| --- | --- |
| S47 | Add a file after cold reopen; record membership, filters, FTS, sorting and relations update correctly |
| S48 | Replace a multi-record file whose old version spans segments; retire every old record and publish only the new version |
| S49 | Delete a file after reopen; all query families exclude it and deletion performs no query-payload reads |
| S50 | Deletion-only save changes catalogs/liveness/manifest and writes zero posting/dictionary packs |
| S51 | Move a file preserves correct identity and rebuilds required path-dependent indexes only |
| S52 | Cold source-free compaction works with source files inaccessible and matches pre-compaction logical results |
| S53 | Partial and full compaction preserve mixed native/archived/cold segments, record-slot remapping and old-view results |
| S54 | Save → reopen → mutate → save → reopen works across multiple generations with unchanged object reuse |
| S55 | No-change save performs zero payload reads, serialization, writes or syncs; repeated archive keeps cold frames unloaded |
| S56 | Export to a fresh store copies unchanged cold objects without decoding, remains self-contained and reopens after the original store is unavailable |
| S57 | Save failure during/after mutation leaves the last successful disk snapshot coherent and live RAM state usable |
| S58 | Deterministic 128-step add/replace/delete/move sequence with periodic save/reopen/compaction matches a fresh reference after every checkpoint |

### 21.6 Corruption, durability, and compatibility

| ID | Required proof |
| --- | --- |
| S59 | Bad CURRENT magic/version/checksum/length and incompatible manifest features fail before publication |
| S60 | Truncated/corrupt eager catalog, bad routes, duplicate active identity or invalid leaf range fails open |
| S61 | Corrupt or missing unused lazy payload does not trigger an eager read; selecting it produces a storage error |
| S62 | Corrupt dictionary pointers/order/ordinals and reverse-directory disagreement are rejected on first use |
| S63 | Wrong Roaring checksum/cardinality/domain/trailing bytes and unreasonable decoded sizes are rejected |
| S64 | Wrong sort type/slot permutation/tie-order metadata/collation version is rejected at the appropriate open/load stage |
| S65 | An extension/schema/normalization version mismatch fails explicitly; unsupported payload cannot be silently omitted on save |
| S66 | Failure at each pre-CURRENT write/flush/object-install/file-sync stage leaves the previous CURRENT unchanged |
| S67 | CURRENT rename failure preserves the old committed snapshot; object files are never partially overwritten |
| S68 | Directory-sync failure after CURRENT rename returns CommitUncertain and preserves both generations' dependencies |
| S69 | Child-process termination at deterministic transaction barriers yields an old or new complete snapshot, never a mixed one |
| S70 | Sync-operation trace verifies dependency file syncs, object-directory sync, CURRENT-file sync, rename and root-directory sync order |
| S71 | Two independent writers to one store cannot overwrite each other; stale expected-parent token returns StorageConflict |
| S72 | Reader opening during commit pins one valid manifest and continues using it after another generation commits |
| S73 | Incomplete initial publication cannot be saved as complete; empty completed index can save/open/query |
| S74 | Storage read limits and checked arithmetic reject oversized/overflowing lengths without excessive allocation |
| S75 | Source/configuration incompatibility is not treated as an empty index or implicit rebuild |
| S76 | Full audit detects corruption in unused payloads while ordinary open remains lazy |
| S77 | Prune returns StorageBusy while any cooperating process/old view holds a shared lease |
| S78 | Offline prune retains all current dependencies, removes orphan/obsolete owned objects and preserves unrelated files |
| S79 | A corrupt current manifest causes prune to delete nothing; no automatic fallback resurrects old records |
| S80 | Workspace CLI/TUI/GUI compile and ordinary startup does not invoke archive/save/open implicitly |

Use process-abort tests to check application crash behavior, and trace tests to check sync order. Do not describe these as a physical power-loss experiment. Do not induce actual machine crashes or alter global mount settings.

Add small malformed-byte/property tests using existing test dependencies or a deterministic local generator. Exercise range arithmetic, bad ordinals, path encoding, typed scalar/sort encoding, and repeated failed loads. Do not add a large fuzzing infrastructure as a prerequisite to completion.

## 22. Benchmarks and acceptance thresholds

### 22.1 Baselines and environment

Use the full Musenalm dataset and the repository's existing query suite/harness. Record the actual dataset path locally; do not hard-code the author's machine path into library code. Reuse the previous Repair II evidence scripts where appropriate, extending their modes rather than inventing a different query workload.

Record:

- Native comparison revision N: the native baseline from section 2.
- Merged master task-base revision P, which includes the completed native-publication/archive changes.
- Final storage-backend revision S.
- Compiler/profile/target, allocator if configured, CPU, memory, OS, filesystem, storage device, worker counts.
- Dataset content/configuration signature and record/file/collection/segment counts.
- Exact Cargo.lock and all runtime options.
- Whether OS file pages are warm. Fresh process does not mean cold disk.

Use release builds from isolated worktrees. Do not compare differently configured scanners, alphabets, inference settings, query limits, source caches or result caches without labeling the difference.

No privileged global page-cache dropping. Required cold-component tests start a fresh process/open with an empty application payload cache; label them **unloaded components, OS cache state observed/unspecified**. An optional controlled storage-cold experiment must document its method and limitations, and must not evict unrelated user workloads.

### 22.2 Native construction remains a protected path

Compare P and S native-only construction at 1, 8 and 32 workers, ASCII and Unicode, with seven interleaved samples per configuration. Include both standalone Repository and progressive Index entry points; record first search-ready and full completion separately.

Structural requirements:

- Zero storage writes/reads and zero implicit archive conversions.
- No mandatory cache registration or per-leaf synchronization allocation.
- No second tree/hash map built merely for later storage.
- Preserve FTS block construction, bounded scheduling and ownership adoption.

Target: no material regression. Investigate paired median regressions exceeding both 3% and 10 ms. Report N as historical/native context as well, but P is the direct before/after for this storage change. Do not promise this backend makes initial construction faster than native.

If the FTS dictionary/payload separation causes measurable overhead, profile that path and repair entry lookups/allocations. Do not remove the required disk semantics or reintroduce duplicate maps to hide the result.

### 22.3 Save and sync

Measure independently, with at least five samples each:

1. First `archive_to` from a fresh native build.
2. `archive_to` after explicit in-memory `archive()`.
3. No-change save.
4. Save after add.
5. Save after multi-record replacement.
6. Deletion-only save.
7. Save after partial/full compaction.
8. Export of a cold reopened snapshot to a fresh store.

Separate archive CPU, additional format encoding, hashing, buffered write time, file/directory synchronization, object/catalog work, lock wait and total wall time. Report peak/end RSS and exact stored bytes by category.

For first-save worker sensitivity, use archive workers 1, 8 and 32 while keeping the disk writer policy fixed. Do not bury serialization/sync cost inside startup metrics.

No-change save must satisfy its zero-work structural contract. Deletion-only save must rewrite no query packs. Same-store unchanged cold objects must not be read merely to prove they are reusable. Export may stream-copy and checksum them, but must show zero Roaring decodes and zero dictionary reconstruction.

Full save necessarily writes all required newly stored payloads. Do not compare its total time directly with dictionary-only archive time and call the difference a regression without showing the added bytes/work.

### 22.4 Eager open

Measure at least seven independent process opens per alphabet:

- CURRENT/manifest read and validation.
- File/record metadata read/validation.
- Index catalog and leaf catalog read/validation.
- Liveness/domain reconstruction.
- Runtime binding and publication.
- Total time to search-ready, CPU, bytes read and end/peak RSS.

Acceptance requires zero lazy payload reads/decodes, zero source reads/parses, zero native reconstruction of value/gram dictionaries, and zero sort work.

Report eager memory as both absolute MiB and bytes per file/record/index/leaf. Compare with full built-index resident memory. Metadata may be a substantial fraction; do not claim near-zero open memory.

Record catalog serialization and allocation overhead separately. Do not create native hash-map mirrors merely to win a small lookup benchmark without showing their memory cost.

### 22.5 Query matrix

Use all queries in the current canonical suite (historically 150 in Repair II), both alphabets, the same limit/count/evidence/options, and both public entry paths.

Measure these states separately:

| State | Meaning |
| --- | --- |
| Native P | Completed prerequisite native publication |
| Native S | Final native publication, storage unused |
| Resident archived S | Explicit in-memory archive, no disk open |
| Freshly saved S | Save completed; original payloads remain warm |
| Reopened cold S | Eager catalogs ready, application payload cache empty |
| Reopened warm S | Required components loaded from an earlier equivalent query |
| Mixed S | Reopened snapshot with later native updates |
| Budgeted S | Reopened, finite cache budget, controlled eviction/reload |

Correctness must match across every applicable state. Use at least five suite repetitions for warm timing and ten repetitions for previously sensitive Q001/Q119 and new material outliers. Preserve current query identities rather than trusting stale prose from prior reports.

For cold-component measurements, reset/open between queries when attributing an individual query's first-use cost. A sequential suite is a separate “session warming” benchmark and must not label every query cold.

Warm-query investigation thresholds: aggregate median total over 3% slower than the equivalent resident baseline; individual query over both 10% and 0.5 ms slower. These are investigation/acceptance-review thresholds, not unit-test timing assertions. Include distributions and actual work counters. Explain unresolved regressions rather than declaring unconditional success.

### 22.6 Human search sequence and cache budget

Add a deterministic sequence modeling repeated use:

1. FTS query in one collection.
2. Slightly refined FTS query sharing grams/binding.
3. Add a filter on one field.
4. Change only the filter value.
5. Add sorting on one field.
6. Repeat the earlier query.
7. Query a different collection.
8. Return to the first collection.

Run with KeepLoaded and 64 MiB/256 MiB budgets. Report first/repeated latency, cache hit/join/eviction counts, loaded families and memory. A budget below eager metadata size is still a payload budget; show that distinction.

Add a broad range query across many leaves and many segments to quantify coalescing. Report physical reads/bytes, useful requested bytes, overread bytes and decode time. No assertion that batching improves every workload without measurements.

### 22.7 Mutation, compaction, and memory

Benchmark add/replace/delete/move and partial/full source-free compaction after cold reopen and after warm queries. Compare with native P/S and report components loaded, sources parsed, objects rewritten and save costs separately.

Measure memory at:

- Before/after native build.
- Before/peak/after archive.
- Before/peak/after first save.
- After eager open.
- After one-field, one-FTS and one-sort query.
- After the full warm suite.
- After cache trim.
- With and without a pinned old view.
- During compaction and concurrent loads.

Use allocation ownership/counters for reclamation claims. RSS and process peak RSS are separate; process peak never decreases. Report archive capacity slack, eager metadata, decoded bitmaps, sort frames, compatibility caches, query scratch and in-flight I/O. Do not double-count owners shared between current/old views.

### 22.8 Required evidence files

Store under `docs/ai/generated/catalog-led-lazy-storage/`:

- `implementation-report.md` and `component-coverage.md`.
- `revisions.json` and `environment.json`.
- `test-matrix.tsv`, mapping S01–S80 to executed cases.
- `construction-samples.tsv` and `construction-summary.tsv`.
- `save-samples.tsv` and `save-summary.tsv`.
- `open-samples.tsv` and `open-summary.tsv`.
- `query-samples.tsv`, `query-summary.tsv` and semantic comparison results.
- `mutation-samples.tsv` and `compaction-samples.tsv`.
- `cache-sequence.tsv` and `storage-io-counters.tsv`.
- `memory-summary.tsv` and `object-size-summary.tsv`.
- `fault-injection-results.tsv`.
- Exact commands/scripts needed to reproduce results, with dataset paths parameterized.

Raw samples are mandatory. Each summary must be generated from those samples, with units and statistic definitions. Do not paste older numbers into final tables.

## 23. Implementation modules and required internal contracts

Prefer a contained new module tree under `crates/core/src/storage/`:

| Module | Responsibility |
| --- | --- |
| `mod.rs` | Public options, reports and maintenance exports |
| `format.rs` | CURRENT/envelope encoding, stored types, version/limit checks |
| `manifest.rs` | Immutable dependency graph and generation validation |
| `catalog.rs` | Eager index/leaf descriptors and catalog read views |
| `io.rs` | Positioned reads, bounded writer, locks, real/test I/O |
| `writer.rs` | Save transaction, object reuse/export, durable commit |
| `reader.rs` | Eager open and source/configuration binding |
| `cache.rs` | Shared loads, leases, policies, accounting |
| `bitmap.rs` | Roaring serialization, bounded checked decoding, leaf resolution |
| `sort.rs` | Stored sort wrappers and borrowed archived sort views |
| `maintenance.rs` | Explicit audit and offline prune |
| `tests/` | Fixtures, faults, concurrency and round-trip tests |

Keep existing domain behavior in posting/bigram/sorting/lifecycle modules. Do not duplicate the query engine inside storage.

The supervisor defines these interfaces before delegated work:

1. `StoredComponentRef` and its complete cache/domain identity.
2. Eager `IndexDescriptor` and coverage/proof representation.
3. `LeafGroupDescriptor`/leaf row layout.
4. `ComponentLease`/`BitmapLease` and borrowed cursor lifetimes.
5. `StorageIo`, read limits and fault stages.
6. `StorageError` classification and conversion into Index/query errors.
7. Source/configuration identity representation.
8. Save/open status and report counters.

Use precise professional names. Avoid opaque `data`/`blob` variables where a dictionary frame, leaf range, source file or pack object is meant.

Error categories must include: UnsupportedPlatform, UnsupportedFormat, IncompatibleConfiguration, UnsupportedComponent, IncompleteBuild, StorageBusy, StorageConflict, StorageLimitExceeded, CorruptCatalog, CorruptComponent, MissingObject, StorageIo and CommitUncertain. Preserve the underlying operation/path/component identity without leaking unrelated file content.

Do not use unsafe casts to simplify format parsing. All unsafe code introduced for archive ownership or system APIs must be localized, documented, and covered by validation/ownership tests.

## 24. Phases, commits, and delegation boundaries

The supervisor owns final architecture and integration. Do not let subordinate implementers choose a different cache policy, disk format, commit protocol, eager/lazy split, or mutation semantics.

| Phase | Work and commit boundary | Required evidence before proceeding |
| --- | --- | --- |
| 0 | Verify merged master, record that base, create the separate integration worktree, reproduce native baseline | `revisions.json`, archive/native-publication contract status, clean worktree record |
| 1 | Component inventory, descriptor/proof model, error/options API, I/O test harness | Complete coverage table; failing laziness/round-trip tests |
| 2 | Format envelopes, CURRENT, manifest, archived eager catalogs, checked readers | Format/bounds/corruption tests; no source dependency |
| 3 | Common dictionary/leaf/sort handles and metadata-only planner | Native semantics pass; EXPLAIN/assessment no-payload contract |
| 4 | Direct dictionary encoders, FTS payload separation, Roaring writer/decoder, stored sorts | Native build allocation checks; codec round trips; no staging maps |
| 5 | Writer with object reuse and sync protocol | Failure-stage and concurrent-writer tests |
| 6 | Eager open, lazy resolvers and single-load cache | S03–S16, S33–S43; source-free open |
| 7 | Mutation/compaction after reopen, saved warm-owner transfer | S47–S58 plus old-view ownership tests |
| 8 | Audit/prune, example and frontend compatibility | Maintenance tests and workspace compilation |
| 9 | Combined workspace tests and full performance matrix | Raw evidence and semantic equivalence |
| 10 | Review, repair regressions, complete task report, commit and push | One tested integration branch, S01–S80 mapping, honest verdicts |

When multiple implementers are useful, use these fixed subdivisions after phase 1 contracts are committed:

- `task/catalog-led-lazy-storage/format-writer`: format, object writer, commit/fault machinery and writer tests.
- `task/catalog-led-lazy-storage/lazy-access`: cache/I/O leases and posting/FTS/sort resolution against approved descriptors.
- `task/catalog-led-lazy-storage/open-catalogs`: eager metadata open, capability catalog and runtime binding.
- `task/catalog-led-lazy-storage/validation`: independent fixtures, model comparisons and benchmark extensions.

Each gets a sibling worktree based on the recorded integration commit. The supervisor retains ownership of lifecycle publication, cross-module API changes and final merge conflict resolution. Do not concurrently edit shared type definitions without an explicit handover.

A single agent may execute all phases sequentially. The task's size does not authorize silently reducing the requested behavior. If time runs out, preserve tested commits and identify the exact remaining acceptance rows.

## 25. Verification commands and completion criteria

Run the repository's documented formatting/check/test commands. At minimum, from the integration worktree:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test -p objekte-search-core
cargo test --workspace
```

Run the storage concurrency/fault suite under release mode as well where relevant to optimized code. Use the project's existing benchmark commands with the modes described in section 22. Do not invent an executable that the repository does not contain and then claim to have run it.

Required prototype completion:

1. Native build and explicit memory-only archive retain their behavior.
2. Successful explicit save is a complete synchronized snapshot.
3. Eager open restores metadata, index catalog and leaf catalog, with zero search-payload reads.
4. All supported query families execute across native, warm archived, cold reopened and mixed state.
5. First use loads only selected components, repeated use stays warm, and eviction is ownership-safe.
6. Add/replace/delete/move and source-free compaction remain correct after reopen.
7. Unchanged object reuse, deletion-only saves and no-change saves satisfy structural limits.
8. Corruption, I/O failure, cancellation and uncertain commits have tested, explicit outcomes.
9. Required workspace tests and S01–S80 cases are executed.
10. Performance/memory evidence is reported by operation; material regressions are repaired or explicitly block unconditional acceptance.
11. The integration branch, completed task document and report are committed and pushed.

Do not mark the prototype complete merely because the file format works. Conversely, distinguish an operationally correct prototype with measured performance limitations from an untested or incomplete implementation.

The final report must give separate verdicts for correctness, eagerness/laziness, durability, lifecycle/mutations, native construction, warm-query overhead, cold-query latency, memory and disk amplification. Include unsupported platforms/extensions, any read-limit adjustments, and the cost of eager metadata. State which requirements passed, failed or were unmeasured.

Do not merge into master. Retain the integration worktree and branch for user review. Remove subordinate worktrees only after their work is merged/pushed and unrelated/untracked files are preserved.

## 26. Primary references and implementation anchors

Repository anchors refer to the historical inspected Repair II commit; the implementation must start from the user's merged master as required by section 2.3.

- [Repository workflow](https://github.com/Simon-Martens/objekte/blob/dc371a42e666ef6c4d3c0519d6653326d0ad6d56/docs/ai/README.md)
- [Existing archive owner](https://github.com/Simon-Martens/objekte/blob/dc371a42e666ef6c4d3c0519d6653326d0ad6d56/crates/core/src/archive_storage.rs)
- [Planner/access catalog](https://github.com/Simon-Martens/objekte/blob/dc371a42e666ef6c4d3c0519d6653326d0ad6d56/crates/core/src/index_catalog.rs)
- [File catalog and ownership](https://github.com/Simon-Martens/objekte/blob/dc371a42e666ef6c4d3c0519d6653326d0ad6d56/crates/core/src/lifecycle_catalog.rs)
- [Segment rows and lookups](https://github.com/Simon-Martens/objekte/blob/dc371a42e666ef6c4d3c0519d6653326d0ad6d56/crates/core/src/segment_archive.rs)
- [Native segment publication](https://github.com/Simon-Martens/objekte/blob/dc371a42e666ef6c4d3c0519d6653326d0ad6d56/crates/core/src/native_segment.rs)
- [FTS/bigram implementation](https://github.com/Simon-Martens/objekte/blob/dc371a42e666ef6c4d3c0519d6653326d0ad6d56/crates/core/src/bigram.rs)
- [Sort keys and order arrays](https://github.com/Simon-Martens/objekte/blob/dc371a42e666ef6c4d3c0519d6653326d0ad6d56/crates/core/src/sorting.rs)
- [Source readers and revision checks](https://github.com/Simon-Martens/objekte/blob/dc371a42e666ef6c4d3c0519d6653326d0ad6d56/crates/core/src/source.rs)
- [Pinned rkyv BTreeMap serialization](https://docs.rs/rkyv/0.8.18/src/rkyv/impls/alloc/collections/btree_map.rs.html): native B-tree serialization already delegates to ordered iteration.
- [rkyv archived hash-map implementation](https://docs.rs/rkyv/0.8.18/src/rkyv/collections/swiss_table/map.rs.html): borrowed exact-size iterators; preserve archived/native hash equivalence.
- [rkyv checked access](https://docs.rs/rkyv/0.8.18/rkyv/api/high/fn.access.html): use checked access when admitting disk bytes.
- [Roaring portable format](https://github.com/RoaringBitmap/RoaringFormatSpec): existing compressed format, no need to invent a bitmap encoding.
- [Rust Roaring API](https://docs.rs/roaring/latest/roaring/bitmap/struct.RoaringBitmap.html): serialize_into/deserialize_from; check the project's locked 0.11 source rather than adopting latest-only APIs.
- [Rust File synchronization and locking](https://doc.rust-lang.org/std/fs/struct.File.html): sync_all and explicit file locks.
- [Linux fsync semantics](https://man7.org/linux/man-pages/man2/fsync.2.html): file synchronization alone does not persist containing-directory entries.
- [Linux rename semantics](https://man7.org/linux/man-pages/man2/rename.2.html): atomic replacement within one filesystem.

## 27. Agent handoff checklist

Before the first implementation commit, confirm in the task notes:

- The original checkout is unchanged and the integration worktree is separate.
- The task branch starts from the user's merged master; its exact commit and archive/native-publication contract checks are recorded.
- The eager set explicitly includes file/record metadata, index catalog and leaf catalog.
- The index capability/proof contract and leaf identity/codec are agreed with this specification.
- The existing planner's payload reads have been enumerated.
- Every published component has a persistence classification.
- Native startup remains free of storage work.
- CURRENT/manifest/object reuse, error states and cache ownership match this document.
- Tests and benchmarks are tied to the required IDs and configurations.

These checks are implementation work, not a request for the user to decide routine details. Carry out the fixed design and report concrete blockers only when required repository facts or capabilities contradict it.



## Implementation report

Task identifier: `catalog-led-lazy-storage`

Completion date: 2026-11-17

Base commit: `15f45465ed0b7455577fce5431de50dfeea1ee16`

Integration branch: `integration/catalog-led-lazy-storage`

Final implementation code revision: `ee52c8239c56fbe82b7c67a0e9b368dc4d9b8d8d`

Integration worktree: `/home/simon/source/objekte-integration-catalog-led-lazy-storage`

### Result

The explicit storage backend is implemented on the integration branch. Native
construction and memory-only archive remain storage-free. `archive_to` publishes a
complete synchronized snapshot; `open_storage` restores checked eager catalogs without
source reads or lazy payload reads. Reopened publications execute the supported filter,
FTS, sort, relation, pagination, evidence and count paths without silently rebuilding
from source. Add, replace, delete, move, multi-generation save/reopen, cold export and
source-free compaction remain functional. Corruption, read/write/sync failures,
writer conflicts, uncertain commits and process abort at commit barriers have explicit
tests.

The implementation uses separate checked rkyv file, record, index and leaf catalogs;
independent value/length/structural posting leaves; independent normal/skip FTS leaves;
per-column/composite sort frames; lazy base relation payloads; query-level cache pins;
post-unpin budget enforcement; bounded pack handles; and deterministic fault/sync
instrumentation. `S01` through `S80` are mapped to executed evidence in
`docs/ai/generated/catalog-led-lazy-storage/test-matrix.tsv`.

### Integration commits

There were no subordinate implementation branches. The integration branch contains the
phased implementation and evidence commits, including:

- `0299fd4a4b` explicit storage scaffold and checked round trip
- `40fdbdf7dd`, `26c3a58e9f` lazy posting/text/composite leaves
- `9343eb2e27`, `12cc11702f` independent stored sort frames
- `a646d436d0`, `46edcb478d`, `585951e78e` writer fencing, faults and commit barriers
- `5bd087f7f4`, `8b0206584e` reopened mutation and compaction
- `24a33cdb99` archived eager catalogs and leaf directory
- `003c4d7fd8` bounded decoded posting leaves and query pins
- `ee52c8239c` reproducible storage benchmark harness
- `58eb56f422` final generated acceptance evidence

### Validation

Executed from the integration worktree:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo test --release -p objekte-search-core --test storage_roundtrip
```

All commands passed at the final code revision. The workspace run reported 617 core
tests passed and 17 ignored, plus passing CLI, desktop, integration, process and doc
tests. Release storage round trips reported 59 passed. An initial parallel workspace
run exposed one timing-sensitive archive-cancellation test; it passed in isolation and
in the final full workspace rerun.

### Benchmark

The final user-authorized reduced-scope benchmark is recorded in
`docs/ai/generated/catalog-led-lazy-storage/reduced-scope-benchmark.md`. It reuses the
completed ASCII Musenalm run with the normal 32-worker configuration.

- Native build: 832.574 ms.
- Explicit in-memory archive: 311.232 ms.
- First synchronized save: 2.664 s; 1,077 new objects; 516.66 MB referenced storage.
- Seven eager opens: 1.418 s median, 39 metadata reads, zero lazy/source reads.
- Q001: native 6.765 ms; resident archived 44.540 ms; reopened first 12.784 ms.
- Q119: native 5.260 ms; resident archived 5.449 ms; reopened first 5.437 ms.
- Metadata Q034: native 14.990 us; resident archived 16.916 us; reopened first
  72.534 us.
- All three queries retained identical counts and ordered-result digests across native,
  resident-archived and reopened states.
- Offline audit verified all 1,077 objects with no error.

No comparable storage-backend baseline was available, so the report gives absolute
timings and makes no speedup claim.

### Verdicts and limitations

Correctness, supported durability, lifecycle/mutation behavior and structural native
construction pass the exercised requirements. Eagerness/laziness, warm-overhead
comparison and disk amplification are partial, while eager metadata memory is a measured
limitation.

The following are explicit, correctness-preserving gaps rather than hidden passes:

- Presence/null/empty-selection masks remain eager instead of independent state leaves.
- Segment-local relation contributions remain coarser than endpoint-level lazy payloads.
- Runtime snapshot detail is still JSON carried inside bounded checked rkyv frames;
  eager decode contributes materially to open latency and memory.
- Per the user's final reduced scope, worker-count/alphabet matrices,
  mutation/compaction benchmark matrices and repeated cold/warm series were not run.
  Their correctness remains covered by the passing tests above.
- FTS remains conservative and retains source verification where exact text, regex or
  evidence semantics require it.
- Persistence is supported only for 64-bit Linux local filesystems. No NFS, distributed
  filesystem, Windows crash-durability or physical power-loss guarantee is claimed.

The integration branch and worktree are retained for user review. No merge into
`master` was performed. No subordinate worktrees or task memory remain to clean up.



