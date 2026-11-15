# Objekte mmap index repair specification

**Task:** mmap-index-v1 repair and completion  
**Date:** 2026-11-15  
**Repository:** Simon-Martens/objekte  
**Integration branch:** integration/mmap-index-v1  
**Reviewed commit:** 81ba4ce200a5d2979e0d358e3fc6ea9e8bf0cf13  
**Original implementation baseline:** c54486f893a57fac0b1b80ed967ab4f5a5fd26c9

## 1. Objective and authority

Complete the existing mmap index implementation so that a published index uses the same packed representation in memory and on disk, opens without reconstructing its contents, and reads and validates pages only as operations require them.

The highest-priority correctness repair is preserving active-record visibility across Save and Load. A deleted or replaced record must never become visible again merely because an immutable run object was reused.

This is a continuation of the original Objekte mmap index v1 implementation plan. Preserve its public behavior and semantic requirements. This specification adds concrete repairs and acceptance criteria for the reviewed implementation; its explicit corrections take precedence over conflicting completion claims in the existing implementation report. Do not restart the project or replace the storage engine.

The findings below are grounded in the reviewed source revision. They are not measurements from an executed benchmark in this review environment. The implementing agent must reproduce the failures, establish executable baselines, and report actual results.

### Required final behavior

- Build is memory-only by default. Temporary mutable builders produce resident packed runs before publication.
- Save is explicit and writes packed bytes and publication metadata. Existing durable objects can be reused without copying or hashing their complete contents.
- Load is explicit. It opens immutable objects and small descriptors without enumerating every file, allocating one cell per record, rebuilding lookup maps, or reading unrelated posting payloads.
- Resident and mapped runs use the same dictionary, record-set, catalogue, record, and sort readers.
- Queries, metadata browsing, mutations, and compaction operate through those readers.
- File freshness checking remains an explicit lifecycle operation. Load does not crawl, stat, or parse source files.
- Compaction is source-free, bounded in temporary memory, and independent of Save.
- Corrupt mapped data produces typed errors at the operation that encounters it.

### Scope limits

Do not add SQLite compatibility, a general transactional database, a new query language, automatic persistence, an eager retained-memory loading mode, or a replacement compression library. Preserve source-format extensions and current search semantics.

Native maps and Roaring bitmaps remain valid temporary construction and query-scratch tools. They must not remain a second permanent representation of every published index.

## 2. Repository workflow

Read the current repository AGENTS.md and docs/ai/README.md before changing code. Inspect the current remote branch and worktree state; the reviewed commit is an audit reference, not permission to reset subsequent work.

Continue integration/mmap-index-v1 in its existing dedicated worktree, normally /home/simon/source/objekte-integration-mmap-index-v1. If no such worktree exists, create a sibling worktree under /home/simon/source using the current integration branch. Keep the original checkout on master and preserve its uncommitted and untracked files.

Use one supervising agent. Parallel implementation is optional after the shared reader, visibility, and object-reference interfaces have been agreed. If delegated, use dedicated sibling worktrees and task branches based on the integration branch:

| Work area | Suggested task branch suffix | Integration dependency |
| --- | --- | --- |
| Run visibility and lifecycle regression tests | visibility | First correctness repair |
| Bounded, fallible storage readers | readers | Foundation for all remaining work |
| Resident construction and record-set execution | resident-runs | Reader interfaces fixed |
| Catalogue and frontend metadata access | catalogue | Reader interfaces fixed |
| Save object reuse and cancellation | persistence | Object ownership interfaces fixed |
| Streaming compaction and admission | compaction | Visibility, packed construction, and readers integrated |

Use the existing repository task-branch naming convention, for example task/mmap-index-v1/readers. The supervisor integrates these changes into the single integration branch and runs combined validation. Do not merge into master.

Make focused commits at the phase boundaries below and push each completed commit as required by the repository workflow. Stage explicit paths. Do not force-push or reset user work. Keep supplied task documents uncommitted until the repository's completion procedure permits committing them; at completion, archive this task and its report according to docs/ai/README.md.

## 3. Findings and source map

Use these links to locate the reviewed behavior. Line numbers may change during implementation.

| ID | Finding | Reviewed implementation |
| --- | --- | --- |
| R1 | Saving an already packed run preserves its old embedded coverage but not its current active mask; Load can reactivate retired records. | [native_pack.rs](https://github.com/Simon-Martens/objekte/blob/81ba4ce200a5d2979e0d358e3fc6ea9e8bf0cf13/crates/core/src/index_storage/native_pack.rs), [manifest.rs](https://github.com/Simon-Martens/objekte/blob/81ba4ce200a5d2979e0d358e3fc6ea9e8bf0cf13/crates/core/src/index_storage/manifest.rs), [packed_restore.rs](https://github.com/Simon-Martens/objekte/blob/81ba4ce200a5d2979e0d358e3fc6ea9e8bf0cf13/crates/core/src/index_storage/packed_restore.rs) |
| R2 | Posting views request complete arenas, causing full-range checksum validation at Load or first lookup. | [posting.rs](https://github.com/Simon-Martens/objekte/blob/81ba4ce200a5d2979e0d358e3fc6ea9e8bf0cf13/crates/core/src/index_storage/posting.rs), [bigram.rs](https://github.com/Simon-Martens/objekte/blob/81ba4ce200a5d2979e0d358e3fc6ea9e8bf0cf13/crates/core/src/index_storage/bigram.rs), [backing.rs](https://github.com/Simon-Martens/objekte/blob/81ba4ce200a5d2979e0d358e3fc6ea9e8bf0cf13/crates/core/src/index_storage/backing.rs) |
| R3 | Fresh builds and mutations still publish native structures; packing happens during Save. | [mutation_segment.rs](https://github.com/Simon-Martens/objekte/blob/81ba4ce200a5d2979e0d358e3fc6ea9e8bf0cf13/crates/core/src/mutation_segment.rs), [lifecycle_open.rs](https://github.com/Simon-Martens/objekte/blob/81ba4ce200a5d2979e0d358e3fc6ea9e8bf0cf13/crates/core/src/lifecycle_open.rs), [lifecycle.rs](https://github.com/Simon-Martens/objekte/blob/81ba4ce200a5d2979e0d358e3fc6ea9e8bf0cf13/crates/core/src/lifecycle.rs) |
| R4 | Load reconstructs catalogue entries, lookup maps, slot metadata, identity order, and a per-record cache array. Frontend helpers also enumerate complete catalogues. | [packed_restore.rs](https://github.com/Simon-Martens/objekte/blob/81ba4ce200a5d2979e0d358e3fc6ea9e8bf0cf13/crates/core/src/index_storage/packed_restore.rs), [inventory.rs](https://github.com/Simon-Martens/objekte/blob/81ba4ce200a5d2979e0d358e3fc6ea9e8bf0cf13/crates/core/src/inventory.rs), [desktop runtime.rs](https://github.com/Simon-Martens/objekte/blob/81ba4ce200a5d2979e0d358e3fc6ea9e8bf0cf13/crates/desktop/src/controller/runtime.rs) |
| R5 | Changed Save copies complete mapped objects and hashes them before determining that they already exist. | [segment.rs](https://github.com/Simon-Martens/objekte/blob/81ba4ce200a5d2979e0d358e3fc6ea9e8bf0cf13/crates/core/src/index_storage/segment.rs), [native_pack.rs](https://github.com/Simon-Martens/objekte/blob/81ba4ce200a5d2979e0d358e3fc6ea9e8bf0cf13/crates/core/src/index_storage/native_pack.rs), [store.rs](https://github.com/Simon-Martens/objekte/blob/81ba4ce200a5d2979e0d358e3fc6ea9e8bf0cf13/crates/core/src/index_storage/store.rs) |
| R6 | Compaction repeats whole-input scans for each 2,048-slot output fragment and does not implement the required tier consolidation. | [stream_compact.rs](https://github.com/Simon-Martens/objekte/blob/81ba4ce200a5d2979e0d358e3fc6ea9e8bf0cf13/crates/core/src/index_storage/stream_compact.rs), [lifecycle_compaction.rs](https://github.com/Simon-Martens/objekte/blob/81ba4ce200a5d2979e0d358e3fc6ea9e8bf0cf13/crates/core/src/lifecycle_compaction.rs) |
| R7 | Mapped-row failures can panic, while some candidate and estimate paths convert storage errors into missing indexes or empty results. | [inventory.rs](https://github.com/Simon-Martens/objekte/blob/81ba4ce200a5d2979e0d358e3fc6ea9e8bf0cf13/crates/core/src/inventory.rs), [access.rs](https://github.com/Simon-Martens/objekte/blob/81ba4ce200a5d2979e0d358e3fc6ea9e8bf0cf13/crates/core/src/index_storage/access.rs) |

Also repair the related query costs in access.rs and record_set.rs: materializing each gram into an owned bitmap before intersection, materializing payloads again for estimates, and applying generic per-record operations where compressed-block operations are available.

## 4. Storage and publication invariants

### 4.1 Canonical representation

A published run consists of an immutable packed search object, a run identity, a persisted tier, and publication-specific visibility. Its bytes may be resident or mapped. Readers must not depend on which backing owns those bytes.

The mutable builder is private to construction. Sealing must finish packed records, file parts, dictionaries, record sets, sort structures, and required coverage before publication. Release obsolete builder state after the publication no longer needs it.

Loading may allocate descriptors proportional to collections, runs, and explicitly bounded header metadata. It must not allocate or traverse structures proportional to all files, records, dictionary entries, posting blocks, or object bytes.

### 4.2 Identity and visibility

Keep these identities distinct:

- File ID: monotonic catalogue identity, never reused.
- File version: revision of file metadata, including path changes.
- Content generation: identity of the indexed file contents.
- Local record ID: identity within that content generation.
- Physical slot: run-local posting address, which compaction may change.
- Publication: immutable view of the catalogue, runs, and visibility state.

The logical record address remains collection, file ID, content generation, and local record ID. Moving a file must preserve content generation when existing lifecycle rules allow metadata-only movement. Replacing contents retires the old generation.

Do not make old file parts appear current by attaching the latest catalogue revision to them. Retired content must be filtered before resolving source access or exposing result metadata.

### 4.3 Durability and mapped-file ownership

Preserve immutable content-addressed objects, writer exclusion, atomic CURRENT publication, generation-directory durability, reader leases, and compare-and-swap publication for concurrent compaction.

Never rewrite or truncate an object while it may be mapped. Garbage collection must respect live readers and referenced generations. A successful Save must not falsely mark a newer in-memory publication as saved.

Preserve the existing assumption that published mapped object files are owned by the index store and are not modified externally. Checksums detect encountered corruption; they do not make concurrent modification of a mapping safe.

## 5. R1: Persist current visibility independently of immutable run contents

### Required design

Add an explicit visibility reference to each manifest run reference. Store visibility using the packed record-set representation in a small immutable object or publication metadata section. Bind it to the referenced run object and its slot universe.

Support at least bounded All, Empty, and encoded record-set descriptors. All means all valid slots in that run, not the whole collection. Embedded run coverage describes the immutable run; it cannot substitute for visibility after an update or deletion.

The effective set for a query is the posting intersected with the publication's run visibility. Apply it consistently to typed queries, grams, ranges, counts, sorting, relations, identity enumeration, and metadata access.

An active-count field is useful summary metadata, but cannot encode which records are active. Retain it only with an unambiguous relationship to the persisted visibility descriptor.

### Implementation requirements

1. At mutation publication, retire the affected slots in the old run's visibility and publish the replacement run and catalogue update atomically.
2. Use immutable shared visibility pages or blocks with bounded copy-on-write updates. Do not introduce a full mask scan at Load.
3. Save the visibility belonging to the pinned publication, even when all search-object bytes are reused.
4. Load attaches a visibility reader directly. It must not reconstruct visibility by scanning all records against the catalogue.
5. Bind descriptors to the correct run and slot count. Reject out-of-range references and mismatched identities with typed errors.
6. Remove completely inactive runs when permitted without changing stable record identities.
7. Scope caches to object and publication visibility identities. An old view must retain its old visibility when a new publication retires records.

Bump the experimental format version where necessary. Old objects without the required visibility semantics must be rejected clearly or handled by an explicit, documented compatibility path that is correct. Do not silently accept them by trusting active_count, and do not rebuild source data implicitly.

### Regression tests

Create an initial run containing at least two distinct source files. Save and Load it so the original run is mapped. Retire only one file, leaving the run partially active. Save again without compaction, reload, and compare complete results with the current in-memory publication.

Cover deletion and replacement separately. The replacement must use a different searchable value, so old and new postings are distinguishable. Verify result identities, counts, ordering, record locators, source revisions, and absence of errors referring to retired files.

Extend the matrix to movement, multiple records per file, multiple collections, mixed resident and mapped runs, repeated Save/Load cycles, pinned old views, and mutations concurrent with Save. Assert that unchanged search objects retain their identity while visibility changes.

This regression must fail on the reviewed implementation before the repair is accepted. A test that compacts before saving does not exercise the defect.

## 6. R2 and R7: Bounded, fallible page access

### 6.1 Replace whole-arena borrowing

Posting and bigram views must hold backing ownership plus checked offsets and lengths. Creating a descriptor view must not borrow or validate the complete containing arena.

Separate operations for opening a descriptor, locating a block, and reading that block. A key lookup should touch only its dictionary path, descriptor, and required posting blocks. A record lookup should touch only the necessary record and file-part metadata.

Audit every call to backing.read and equivalent byte-slice access. In particular, remove the whole-arena access from posting view construction and bigram posting access.

Read persisted cardinalities from bounded descriptors. Planner estimates must not decode all postings or construct temporary Roaring bitmaps merely to estimate cardinality. A filtered estimate may be conservative; label estimates as estimates and never use an upper bound as an exact result count.

Avoid rebuilding a vector of every block descriptor on each access. Keep directories directly readable and searchable in their packed form.

### 6.2 Integrity granularity and validation

Validate object headers, section bounds, offset arithmetic, descriptor tags, and directly referenced fixed metadata before exposing them. Validate page and block contents when accessed.

For dictionary pages, use integrity units aligned with the 8 KiB page layout. For posting and metadata payloads, validate only bounded integrity units intersecting the requested span; do not checksum an entire arena. If retaining a different unit size, justify it with measured read amplification and retain the bounded-access contract.

Checksum caching must not require one eagerly allocated synchronization object per page or hold an object-wide mutex while hashing unrelated large ranges. Use bounded or compact state and short synchronization sections. If cached validation is evicted, revalidation must remain correct.

Do not replace full-arena reads with a hidden initial checksum traversal, eager mapping population, blanket prefaulting, or full-object SHA verification during Load. Compute object identity while sealing or writing; mapping admission and lazy integrity validation have different responsibilities.

### 6.3 Error propagation

Expose fallible storage access throughout the call chain. Where an index can legitimately be absent, use a result that distinguishes success-with-no-index from a storage error.

Remove storage-related expect calls in packed record access. Do not erase storage errors with ok(), unwrap_or_default(), zero estimates, empty candidate sets, or scanner fallback.

Carry errors through planning, candidate access, execution, sorting, relation traversal, posting-value browsing, metadata enumeration, CLI, TUI, and desktop handlers. Attach object identity and section/page context without excessive repeated formatting on the successful path.

An absent optional index may use the existing fallback. A corrupt index must report corruption. A count, truncated result list, or empty page produced after an I/O failure must not be presented as a complete successful answer.

Internal assertions for proven in-memory invariants may remain. Focus on failures reachable from persisted bytes, checked offsets, and I/O.

### Acceptance tests

- Use an object with many integrity units and unrelated posting arenas. Opening it reads only the admitted metadata and does not validate all payloads.
- A selective exact lookup and a selective gram lookup read only their required dictionary and posting spans.
- Planner-only estimates touch descriptors but do not decode posting payloads.
- Corrupt an unused payload unit before opening the store. Load and an unrelated valid lookup succeed; the operation that touches the corrupt unit returns a typed error.
- Invalid header bounds and arithmetic overflow are rejected at admission. Invalid nested structures are rejected when traversed.
- The process survives corrupt record, dictionary, posting, sort, and catalogue reads. Frontends report the operation failure without losing the previous usable view.
- Run the same reader conformance cases against resident and mapped backing.

Prepare malformed files before mapping, or use an isolated test backing. Do not corrupt a live mapped file in a test.

## 7. R3: Publish resident packed runs and preserve query performance

### 7.1 Move sealing to construction

Audit fresh builds, progressive publication, add, update, rebuild, verified-update fallback, inferred fields, and extension-produced records. Every completed run must be sealed before it becomes searchable.

Remove native-only published paths represented by packed: None after migrating their consumers. Keep temporary compatibility adapters only during development, with an explicit removal phase. Do not ship a permanent Native/Packed execution split.

All public operations must use packed accessors for both resident and mapped runs. Retain ownership of resident byte buffers without cloning their contents into additional complete structures.

Build must not create a store or temporary index file. Memory pressure may cause an explicit resource error under the existing contract; it must not silently enable persistence.

Save must no longer be the normal point at which dictionaries, record sets, records, or sort indexes are constructed. Metadata serialization and small visibility changes remain valid Save work.

### 7.2 Compressed record-set operations

Use descriptor cardinalities to select the most selective initial gram. Intersect subsequent postings through borrowed block cursors, materializing only the needed candidate/result representation.

Implement or verify specialized Array/Array, Array/Bitmap, Array/Run, Bitmap/Bitmap, Bitmap/Run, and Run/Run operations. Symmetric operand order may share implementations. Dense bitmap intersection and count should use word operations and population count; runs should use interval operations.

Support the required intersection, membership, count, and union behavior without enumerating every member of dense or contiguous sets when a block operation suffices. Empty, All, Range, and Inline descriptors need direct handling.

Avoid decoding the same posting once for an estimate and again for execution. Reuse query-local scratch and cursor state where ownership permits. Keep caches bounded and avoid warming the whole index as a side effect.

Posting-value pagination and dictionary ranges must seek and stream. Do not first collect every matching dictionary entry and posting into an owned vector to return a small page.

### 7.3 Semantic preservation

Retain the full existing semantic matrix: exact numeric ordering, absent/null/empty distinctions, array facts and selectors, completeness rules, declared and inferred types, normal and skip grams, ASCII and Unicode normalization, field-specific text, sorting and collation, relations, invalid-format fallback, and custom extractors.

Preserve stable record addresses and actual verified-update strategy reporting. A slot-preserving optimization may fall back to append replacement if required by immutable runs; report that strategy rather than retaining native runtime indexes solely for it.

### Acceptance tests

After a memory-only Build and after each mutation family, inspect the publication through a test-only hook: all completed search runs must have resident packed backing and no duplicate complete native index.

Run identical semantic queries against fresh resident data and a saved/reloaded mapped publication. Compare complete record identities and ordering, not just counts. Test scratch reuse and block-operation behavior with allocation and decoded-block counters.

Measure packed sealing as part of Build. Moving work from Save to Build is required, but must remain within the performance gates in section 12.

## 8. R4: Use the packed catalogue and record tables directly

### 8.1 Catalogue and identity access

Replace catalogue_entries and eager restored-entry reconstruction with a packed catalogue base and a bounded mutation overlay. Use persisted path lookup structures instead of rebuilding HashMaps at Load.

Retain the original logical catalogue-page contract, including 64-row pages and the bounded overlay policy, normally 4,096 entries. Bulk-merge an exhausted overlay into resident packed metadata; this must not save automatically or enumerate source files.

Persist and access directly:

- File ID high-water state and tombstones.
- File version, content generation, native lossless path, and indexed revision.
- File-part references and local record ranges.
- Stable identity order used by queries and metadata operations.
- Existing diagnostics and status information.
- Path lookup and the ordering needed for bounded browsing.

Do not reconstruct per-run file-index HashMaps, vectors of all file records, or complete identity-order arrays while loading. Remove redundant inventory clones and duplicate path-map construction.

### 8.2 Record views and caching

Replace Vec<OnceLock<RecordEntry>> sized to the record count. It allocates work proportional to all records even before any row is decoded and retains decoded rows without a useful bound.

Use borrowed row/page views or small owned values on demand. If decoded values need caching, use a bounded cache keyed by object identity and row/page identity. Include publication identity when the value contains mutable catalogue or visibility information.

Review APIs that require an infallible reference to a permanently resident RecordEntry. Change the internal interface to a fallible view or handle rather than recreating the old storage model behind a cache.

A row handle must keep its backing and reader lease alive. Cache eviction must not invalidate a live borrowed view or retain entire obsolete publications unintentionally.

### 8.3 Frontend access

Replace desktop and TUI helpers that eagerly materialize all file IDs, per-collection vectors, or directory ancestors at open.

Return metadata in bounded pages, with 256 rows as the normal frontend page size unless an existing UI contract specifies otherwise. Directory browsing must seek or use a persisted child directory structure so that returning one directory page does not enumerate its complete descendant subtree.

Cache small visible pages and request more on navigation. Preserve stable continuation behavior for a pinned publication. A publication change should invalidate or restart a continuation explicitly.

An explicit operation to list all files may still enumerate them through an iterator. Startup and first-page access must not invoke it implicitly.

### Acceptance tests

Compare Load for equal run/collection counts at increasing file and record counts. Assert zero complete catalogue/record enumeration and zero per-record cache-cell allocation.

Opening the first catalogue page and one directory page must decode only bounded relevant pages. Exercise large directories, deeply nested paths, non-UTF-8 paths where supported, tombstones, moves, and overlay merges.

Verify explicit freshness checking still compares the correct indexed revision with source metadata and retains its existing cancellation and reporting behavior.

## 9. R5: Save references and changed bytes

### 9.1 Object ownership and reuse

Introduce a save-source abstraction that distinguishes resident packed bytes from an already durable immutable object reference. A durable reference carries its known object identity, byte length, store identity, and lifetime protection; it must not call object_bytes to create an owned copy.

Retain resident object identity when sealing computes it. Avoid computing the same full digest repeatedly across Save attempts. A digest retained from the manifest is an object reference, not a claim that Load eagerly verified every byte.

Save planning must collect descriptors and references, not a vector containing complete copies of all run objects.

For a Save to the same store:

1. Pin one in-memory publication.
2. Acquire the destination writer lock and inspect the destination generation.
3. Resolve unchanged object references under that protection.
4. Write only new resident objects and changed metadata/visibility.
5. Complete the existing durable generation-publication protocol.
6. Report which in-memory publication was saved and whether a newer one remains unsaved.

Any unchanged-Save fast path must validate the destination state under the writer lock. An earlier status check alone cannot establish that the destination is still current.

If Save targets another store or a referenced object is unavailable there, transfer bytes with a bounded buffer and verify identity while streaming. Do not create a complete Arc copy of a mapped object. Same-store reuse may assume the normal immutable-store ownership contract; a separately requested full audit can read all bytes.

### 9.2 Costs and cancellation

An unchanged same-store Save must not read, checksum, hash, or copy all search payloads. A changed same-store Save must not do so for untouched old objects.

The original v1 allowance to rewrite the full catalogue during an explicit Save remains valid. Report that metadata cost separately; it does not justify repacking all search data.

Check cancellation and report progress during planning, metadata serialization, new-object transfer, and generation preparation. Do not defer all cancellation until after packing the entire index.

Preserve atomic publication boundaries: before CURRENT replacement, cancellation leaves the previous generation usable; after the durable commit point, report the completed Save accurately. Preserve staging cleanup, parent-directory synchronization, and reader leases.

### Acceptance tests

- After Load, an unchanged same-store Save copies and hashes zero old search payload bytes.
- After replacing one file, Save reuses all unaffected immutable search objects and persists the new visibility.
- After a pure move, search-object identities remain unchanged.
- A Save to another location uses bounded transfer memory and produces equivalent reload results.
- Cancellation works in planning, metadata, and transfer phases.
- Fault injection at every existing durability boundary leaves either the previous valid generation or the committed new generation.
- A concurrent mutation remains unsaved when an older pinned publication finishes saving.
- Concurrent writer tests exercise the unchanged-Save path under the destination lock.

## 10. R6: Implement streaming tiered compaction

### 10.1 Selection and run levels

Persist the run level in the manifest and restore it unchanged. Fresh runs enter level 0. Merge the four oldest eligible runs at the same level and promote the output to the next level, as required by the original plan.

Fully active runs must be eligible for consolidation. A collection with no tombstones may still require compaction to control lookup fan-out. Remove completely inactive runs without rewriting their payloads where possible.

Maintain one compaction worker and at most one compaction publication per index. Enforce the 64 searchable-run admission threshold per collection before publishing additional runs.

Do not equate an output run with a fixed 2,048-record fragment. A run may contain many bounded pages or blocks. Large-file staging must consolidate its fragments before publication when necessary, so one mutation cannot exceed the threshold or deadlock waiting to admit its own fragments.

### 10.2 Merge algorithm

Implement a k-way ordered merge over selected input dictionaries. For each key, read the relevant posting blocks, filter current visibility, translate physical slots, and emit bounded output blocks.

Construct the output record order and slot translation once per selected merge. Preserve collection, file ID, content generation, local record ID, and indexed source locator. Do not reset local_record_start to zero for every output fragment.

Use monotonic cursors, rank information, compact file-part intervals, or another bounded translation scheme. Do not allocate a source-slot-sized remapping vector afresh for every output fragment. Any translation structure must be included in the scratch budget.

Allow a bounded number of necessary passes over input metadata, but do not rewalk all records, dictionaries, or postings for each output fragment. Per-merge work should scale with selected input metadata, compressed payloads, and output, rather than output-fragment count multiplied by the complete input.

Merge sorting, relation, coverage, and auxiliary structures with equivalent bounds. Compaction must not parse or stat source data.

### 10.3 Memory, concurrency, and cancellation

Enforce the 128 MiB temporary builder/scratch budget. Account separately for resident output, resident inputs, mapped resident pages, and pinned prior publications. A scratch limit alone does not bound total memory.

Background output remains resident until an explicit Save. Do not silently persist or discard unsaved output to meet the budget. If admission cannot progress within configured resources, return a typed resource error while retaining the last usable publication.

Check cancellation between keys and within large postings/output-block construction. A single very frequent value must not make compaction uninterruptible.

Preserve compare-and-swap publication. If the input publication changes, do not overwrite newer visibility, catalogue updates, or mutations. Use the existing bounded retry policy or defer safely; report repeated contention rather than looping indefinitely.

### Acceptance tests

- Four fully active level-0 runs consolidate and the promoted level survives Save/Load.
- Increasing output-page count does not multiply complete input scans; verify instrumented input visits.
- A high-cardinality field, one extremely frequent value, mixed container types, and a multi-record file spanning parts compact correctly.
- Stable logical addresses and indexed locators survive compaction.
- Cancellation, resource exhaustion, 64-run admission, and one large staged mutation preserve a usable index.
- Mutations during compaction are retained; old pinned views remain valid.
- Source-read and source-stat counters remain zero.

## 11. Implementation phases and commit boundaries

Each phase must leave the integration branch buildable. Run focused tests after each change and the full applicable checks at integration boundaries. Do not postpone all measurement until the architectural migration is complete.

### Phase 0: Establish baseline and instrumentation

Record the current branch head and changes since the reviewed revision. Read the parent specification and existing report. Locate the actual benchmark commands and fixtures.

Add the failing R1 regression and test counters needed to distinguish descriptor reads, payload validation, full enumeration, allocations, and object reuse. Capture the reviewed branch's behavior before repairing it. Preserve a reproducible baseline checkout without changing the user's master checkout.

Suggested commit: test(storage): reproduce visibility persistence and measure packed access

### Phase 1: Repair visibility persistence

Implement publication-specific persisted visibility, format admission, and reload correctness. Cover partial retirement without compaction before proceeding with optimization.

Suggested commit: fix(storage): persist publication run visibility

### Phase 2: Establish bounded fallible readers

Replace whole-arena views, add bounded integrity validation, propagate storage failures, and make descriptor-only estimates possible. Agree the shared catalogue/record and save-source interfaces before parallel work uses them.

Suggested commits: refactor(storage): read packed blocks through bounded views; fix(storage): propagate packed access failures

Run an early Load and first-query benchmark. If open still scales with posting payload size, resolve the cause before moving on.

### Phase 3: Migrate published construction and set execution

Seal all completed runs before publication, route resident execution through the same readers, implement compressed-block operations, and remove permanent native runtime branches.

Suggested commit: refactor(index): publish resident packed runs

Run Build and warm-query benchmarks at this point. Identify regressions while the construction and query changes are isolated.

### Phase 4: Remove catalogue and metadata reconstruction

Replace restored inventory maps and per-record cell arrays with packed bases, bounded overlays, direct identity cursors, and lazy frontend pages.

Suggested commits: refactor(catalogue): access packed metadata without reconstruction; perf(desktop): page indexed metadata on demand

### Phase 5: Make Save reuse immutable objects

Integrate reference-based planning, retained identities, bounded cross-store transfer, lock-correct no-op detection, and cancellation through all expensive phases.

Suggested commit: perf(storage): reuse durable index objects during save

### Phase 6: Replace repeated compaction scans

Implement persisted levels, source-free streaming merges, bounded translation, admission control, and concurrency handling.

Suggested commit: fix(index): stream tiered packed run compaction

### Phase 7: Validate and document

Run the complete correctness, lifecycle, corruption, fault-injection, frontend, and performance matrix. Remove obsolete compatibility code and temporary diagnostic scaffolding; retain useful low-overhead counters and reproducible benchmark harnesses.

Add a repair report under docs/ai/generated using the repository's current convention. Reference the original completion report and correct its unsupported completion claims without discarding historical measurements.

Suggested commit: docs(storage): report mmap index repair validation

Stop at a pushed integration branch ready for the user's merge decision. Do not merge master.

## 12. Validation and performance gates

### 12.1 Required measurements

Compare the original baseline, the reviewed implementation, and the final branch where the same workload can run. Record exact commits, hardware, compiler, release profile, filesystem/storage, dataset revision, and query configuration. If the current integration head differs, identify it separately.

Use the existing Musenalm workload and repository query suite, including both ASCII and Unicode configurations where supported. Small synthetic benchmarks supplement this workload; they do not replace it.

Add synthetic fixtures that isolate large file counts, many records within one file, many distinct keys, dense and sparse postings, long keys, many runs, and partial retirement. A large single-JSONL fixture does not measure the cost of restoring hundreds of thousands of file paths.

| Measurement | Required separation |
| --- | --- |
| Build | Extraction, temporary construction, sealing, publication, peak memory |
| Load | Open/map/admission time, validated bytes, decoded rows, allocations |
| First query | Cold and warm filesystem cache; separate from Load |
| Warm queries | Result cache disabled for comparison; optional cache-on results separately |
| Metadata | First catalogue page, next page, path lookup, directory page |
| Save | Unchanged, one-file update, move, new destination; metadata and search bytes separately |
| Compaction | Fully active merge, retirement-heavy merge, run count, input visits, scratch and total memory |
| Memory | After Build, after Load, after first query, after broad queries, after old views release |

Distinguish application validation bytes from operating-system reads and page faults. mmap can create a large virtual address range without reading it; virtual size alone is not evidence of demand paging or a memory regression.

Report cold-cache methodology honestly. If a reliable cold run cannot be produced, label it unavailable rather than renaming a warm run. Do not manipulate a shared machine's global cache without suitable authorization.

Run repeated independent samples, report medians and dispersion, and preserve raw output. Use at least five measured samples for expensive lifecycle operations and enough repeated query samples to distinguish noise at sub-millisecond scale.

### 12.2 Architectural gates

The following must be asserted by counters or equivalent deterministic tests:

- Load performs zero source discovery, source parsing, and source freshness checks.
- Load performs zero complete file/record reconstruction and allocates no cell for every record.
- Load does not traverse complete posting arenas or hash complete search objects.
- Unrelated posting payload blocks are not requested merely to construct a view or estimate a query.
- A selective lookup reads a bounded search path and necessary posting spans; broad queries may legitimately read broad data.
- Same-store reuse copies and hashes zero untouched old search payload bytes.
- Every completed resident run uses the canonical packed representation.
- Compaction performs bounded passes per selected merge and reads zero source bytes.
- Run admission and configured memory budgets are enforced.

For scaling tests, hold run and collection counts fixed while increasing file count, record count, and payload bytes independently. Explain remaining open costs, including object count and fixed metadata, rather than claiming constant-time behavior from a single small fixture.

### 12.3 Performance thresholds

Retain the parent plan's thresholds:

- Fresh packed Build must be no more than 20% slower than the original comparable baseline.
- The geometric mean of warm-query medians must be no more than 10% slower than that baseline.
- Investigate individual representative queries exceeding 25% regression, accounting for measurement noise and absolute latency.

Evaluate Build and query performance as well as Load. A faster open obtained by moving a full scan into the first query is a failure of this specification.

Do not promise a new absolute millisecond target without measurements. Report absolute timings, scaling, and the deterministic work counters together. If a threshold fails, identify the cause and repair it or mark the task incomplete; do not loosen the threshold silently.

### 12.4 Correctness and repository checks

Run the repository's required format, spacing, workspace test, and lint commands using the active toolchain and lockfile. Resolve any conflict between formatter output and the spacing check in the repository's intended way; do not claim incompatible checks both pass without evidence.

Include all applicable core, CLI, TUI, and desktop tests. Document platform-dependent checks that could not run. Use release-mode benchmarks; debug test timings are not performance evidence.

Add differential query tests, property tests for compressed set operations and numeric ordering where appropriate, persistence lifecycle tests, and malformed-storage tests. Avoid tests that merely duplicate implementation steps.

## 13. Completion report and definition of done

The final report must contain:

1. The starting, reviewed, baseline, and final commit identifiers.
2. An R1–R7 table linking each repair to code and executable tests.
3. The final packed publication, visibility, and object-reuse design.
4. Format-version changes and explicit behavior for older experimental stores.
5. Actual validation commands and results, with unavailable checks identified.
6. Benchmark tables, raw-result locations, memory accounting, and explanations of material regressions.
7. Evidence that unchanged objects are reused, Load avoids full reconstruction, and first-query work remains selective.
8. Remaining limitations, resource behavior, and any unresolved requirement.

The task is complete only when all seven findings are resolved, resident/mapped parity is established, required lifecycle and corruption tests pass, the run and memory limits hold, and the performance gates are met with the representative workload.

If a required dataset or environment is unavailable, deliver the implemented branch and an explicit list of unverified gates. Do not mark the entire task complete or merge-ready on synthetic results alone.

### Agent handoff

Continue integration/mmap-index-v1 using this specification and the original mmap index v1 plan. Reproduce and repair visibility persistence first. Then establish bounded fallible readers, publish resident packed runs, remove eager metadata reconstruction, reuse immutable objects during Save, and implement streaming tiered compaction. Keep the original master checkout unchanged, use dedicated worktrees for delegated work, push focused commits, and finish with one tested integration branch and a measured repair report. Do not merge into master.

---

## Implementation report

Task identifier: `mmap-index-v1` repair  
Completion date: 2026-11-15  
Base commit: `81ba4ce200`  
Integration branch: `integration/mmap-index-v1`  
Integration worktree: `/home/simon/source/objekte-integration-mmap-index-v1`  
Final repair head: `430d54a8c0`

### Summary

The integration branch now publishes packed runs for every construction and
mutation family instead of deferring sealing to Save. Visibility is persisted
per run, Load opens the packed catalogue and run objects directly, packed
record pages and identity order are read on demand, durable objects are reused
during Save, compaction performs a four-run same-level k-way merge with
persisted levels, and packed planner/access failures propagate as typed errors.

The implementation reports, commands, and residual limitations are recorded in
`docs/ai/generated/mmap-index-repair-report.md`. Validation passed
`cargo test --workspace --locked --offline` with 935 tests, the repository
formatting/spacing checks, and `cargo clippy --workspace --all-targets`
(exit 0, warning-level diagnostics only). Release-mode Musenalm performance
measurements were run as a quick checkpoint; the full 3,000-sample gate and a
same-schema build baseline remain unverified.

### Commits

- `163a8e9afd` resident packed publication and packed execution parity.
- `141c6627c6` tiered packed merge and packed catalogue opening.
- `8a08af99d8` packed metadata paging and bounded record caches.
- `a2bf950f78` lazy packed identity order.
- `430d54a8c0` packed file ranges, directory index, overlay merge, strict admission.

The task specification above is preserved unchanged; this report is appended as
required by the repository completion workflow.
