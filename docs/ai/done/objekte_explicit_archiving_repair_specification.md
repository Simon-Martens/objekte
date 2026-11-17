# Objekte: explicit archiving correctness and performance repairs

Date: 2026-11-17

Task identifier: `explicit-archiving-repair`

## 1. Objective and fixed decisions

Repair the implementation of native publication and explicit in-memory archiving. Preserve its useful architecture: construction produces searchable native structures; only an explicit application call to `archive()` creates and installs archived representations. Correct lifecycle, ownership, accounting, serialization, and measurement defects without moving archive work back into startup.

This is an implementation specification for the local agent. It follows `objekte_native_publication_explicit_archiving_specification.md`. Its original functional requirements remain in force except where this document explicitly clarifies implementation or reduces the benchmark matrix. This document authorizes the repairs described below when the user assigns it for implementation; it is not an instruction to merge into master.

The following decisions are fixed:

1. Startup, additions, replacements, moves, deletions, and compaction do not implicitly archive. Changed components produced by those operations are native; unaffected archived components remain archived.
2. `Index::archive()` and `Repository::archive()` remain explicit, blocking APIs. The default archive worker limit remains one. There is no automatic background converter or persistence mode in this task.
3. Queries use immutable snapshots and do not acquire the representation-transition gate. Conversion prepares replacement owners privately and installs one complete publication.
4. A recoverable error before installation leaves the original public state usable and unchanged. Old readers retain their own owners until released.
5. Posting dictionaries retain the current native `BTreeMap<StoredKey, u32>` and separate shared bitmap array. Preserve insertion-assigned bitmap slots. Do not return to bitmap-valued trees or reassign slots by sorted position.
6. Direct posting-map serialization and the streamed archived reverse directory are correct changes to retain. No application staging vector of dictionary entries, keys, references, or ordinals is permitted. The final archived reverse directory remains in scope as an existing query structure, not as removable staging.
7. Unchanged semantic payloads are shared. Archive preparation must not copy whole relation arrays, liveness domains, record inventories, identity arrays, query caches, FTS payloads, or sorting payloads merely to replace dictionaries.
8. Every current eligible owner, including owners retained through compatibility wrappers, must be represented in the archive plan and diagnostics. A stale base catalog must not hide changed current catalog shards.
9. Source files are not read, reparsed, modified, or rescanned by `archive()`. No index files, mmap, durable format, or disk-write API is added.
10. Maintain semantic validation and checked archive access. Performance improvements must remove redundant work, temporary ownership, or unnecessary reconstruction; they must not disable required validation or change query semantics.

The goal is native search availability plus efficient explicit conversion. Do not require build-plus-archive to beat identical native-only construction, and do not claim that deferring conversion removes its CPU or memory cost.

## 2. Review boundary and changes already made

Repository: `Simon-Martens/objekte`.

Reviewed branch: `integration/native-publication-explicit-archiving`.

Final inspected revision: **`29613ab1ccbcd9c01b2b44de3288ad6dab044736`**.

The source review first pinned `3e0ac2664bf355d8e521e342fbbd30ab30ce2a7c`. The subsequent comparison through `29613ab1` changed archive tests, a desktop test, and the completion document; it did not change the core implementation findings below. If the branch advances again, inspect its intervening diff and record the actual repair base. Do not overwrite a later valid fix.

This is a source and committed-evidence review. The author of this specification did not compile the repository or execute new benchmarks. Previously reported test passes and timing samples are attributed to the implementing agent's committed artifacts.

### 2.1 Credit and preserve completed changes

| Earlier observation | State at the reviewed revision | Required treatment |
| --- | --- | --- |
| Standalone `Repository::archive()` modified the live repository before success | Corrected by `1d3c7be663`: conversion now operates on a private candidate, then assigns it | Preserve the correction; extend late-failure coverage and remove destructive component replacement |
| No standalone failure/retry test | `repository_archive_failure_is_atomic_and_retryable` now exists | It injects the first freeze failure and checks a broad query count; add failure after successful components and selective-query verification |
| Archived-state replacement not exercised in the lifecycle test | Added by `2064f477dda53c80d5d6b355f0608bec53d2a215` | Preserve it; it does not cover archive immediately after a catalog-only deletion |
| Desktop test assumed native construction could not already be complete | Updated by `b7f1f0d36a`, recorded in the latest completion document | Retain the corrected timing assumption |
| Posting conversion required temporary entry vectors | Current posting archive source borrows the native map and streams both final structures | Do not redesign or regress this correct implementation |
| Catalog path and ownership conversion cloned complete source maps | Borrowed map sources are present | Preserve these adapters; fix metadata rows that still allocate owned intermediates |

### 2.2 Remaining findings

Line numbers below refer to the inspected implementation and are navigation aids, not patch instructions.

| Repair | Severity / purpose | Evidence and consequence |
| --- | --- | --- |
| R01 | Correctness | `PublishedIndex::storage_representation_counts`, `lifecycle.rs:633`, counts the repository catalog rather than the authoritative current catalog. `build → archive → delete → archive` can return a false no-op. `storage_usage`, around line 765, has the same root-selection problem. |
| R02 | Correctness | `Index::quiesce`, `lifecycle.rs:2971`, retains the exclusive transition permit while joining maintenance/reconciliation. A reconciliation worker can finish source preparation and block acquiring the shared permit at lines 3409/3428. Shutdown then waits for the blocked worker. |
| R03 | Failure safety | Public Repository rollback is now fixed, but `PostingIndex::archive_storage`, `posting.rs:2892`, still replaces live component fields with empty builders before a fallible conversion. Make the component operation itself non-destructive and strengthen tests. |
| R04 | Ownership and memory | `FileInventory.published_paths` retains native path-shard owners adopted in `PublishedIndex::from_repository_owned`. `Repository::archive_candidate`, around line 371, shares that inventory unchanged. Its native path maps can survive after all old views are dropped. Candidate cloning also copies substantial unchanged payloads. |
| R05 | Startup performance | `RecordLookup::native`, `segment_archive.rs:1167`, rebuilds an existing `HashMap<RecordId, RecordSlot>` as another map and allocates a new `Arc<str>` for every ID. This occurs before search readiness. |
| R06 | Conversion performance | `SegmentRows::archive`, around line 261, builds owned file and record vectors. `BorrowedCatalogRow`, `lifecycle_catalog.rs:345`, creates `StoredFileRow` in both serialization and resolution; the prevalidation loop creates it again. |
| R07 | Scheduling | `archive_segment_groups`, `lifecycle_archive.rs:227`, parallelizes collection groups, with a serial loop over segments inside each group. Catalog/base conversion precedes the pool serially. One collection cannot use multiple segment conversion workers. |
| R08 | Validation performance | Archived row/lookup validation constructs an `Arc<str>` and a set entry for every record, despite lookup supporting `&str`. Some ownership validations are repeated. Preserve invariants while eliminating these allocations and redundant passes. |
| R09 | Query performance | Ordinary metadata execution still calls `segment.record()` and `catalog.entry()`. `RecordRows::get()` invokes a full compatibility cache. One archived record access can reconstruct every record in its segment. |
| R10 | Reporting and measurement | Component counts, owner accounting, new-byte accounting, worker peaks, semantic-validation timing, and Repository installation timing need consistent definitions. The progressive benchmark captures its build timer after archive conversion. Current evidence mixes API paths and lacks compiled revision attribution. |
| R11 | Verification and delivery | The T01–T40 completion map claims some scenarios based on adjacent tests or source inspection rather than tests that execute those sequences. Replace those claims with explicit deterministic evidence. |

## 3. Workflow and implementation order

Read `AGENTS.md`, the complete `docs/ai/README.md`, the original specification, this document, and task notes. Keep the user's original checkout on its existing main branch, preserve unrelated working files and the index, and do not merge or commit to master.

Create a sibling integration worktree based on the latest inspected implementation branch:

- Branch: `integration/explicit-archiving-repair`.
- With the usual checkout location, worktree: `/home/simon/source/objekte-integration-explicit-archiving-repair`.
- Evidence: `docs/ai/generated/explicit-archiving-repair/`.
- Temporary notes: original checkout's ignored `docs/ai/memory/explicit-archiving-repair/`.

Use the existing repository remote. Stage explicit paths, commit coherent phases, and push after every commit. Keep the integration branch and worktree for the user's merge. Follow the repository's task-document lifecycle and preserve user additions.

One supervisor owns the contracts in this specification. A single agent may implement the task. If delegation is useful, use separate sibling worktrees and divide ownership into lifecycle/accounting, storage/serialization, and execution/benchmark verification. Do not let subagents independently choose publication policy, logical revision semantics, failure behavior, or ownership layout.

Implement correctness before broad performance restructuring. The required phase order is in section 16. Capture focused baselines before changing performance-sensitive ownership; do not spend time reproducing obsolete large benchmark matrices before fixing the known correctness defects.

## 4. R01 — enumerate the authoritative publication and all retained owners

### 4.1 One inventory contract

Introduce one internal enumeration facility, `ArchiveInventory`, used by archive planning, no-op detection, status, and storage accounting. It must distinguish:

- **Logical components:** the distinct semantic locations requiring a backing, such as a segment value dictionary or a catalog path shard.
- **Unique storage owners:** allocations shared by one or more logical components.
- **Residual native payloads:** intentionally unarchived bitmaps, FTS, sorting, relation adjacency, and documented compatibility inventories.

Expose existing public logical counts with consistent meaning; add separate owner counts where needed. Do not count one owner twice merely because a compatibility path aliases it. Do not silently reinterpret logical counts as allocation counts.

For an `Index`, traverse the current `PublishedIndex.catalog`, its current segments, its base collection data, and every retained eligible base/compatibility owner. Do not obtain the inventory by calling whole-Repository counts and adding current segments without controlling which catalog and segment roots that operation traverses.

For a standalone Repository, traverse its catalog, collections, segments, and inventory path backing. Shared segment references and shared empty dictionaries must be deduplicated for physical conversion and bytes.

### 4.2 Base and current catalogs

The current `PublishedIndex.catalog` is authoritative for lifecycle state, file versions, path changes, deletion status, and ownership routes. The repository may retain a base catalog or base inventory for base collection compatibility. Those are different semantic roles after mutations.

Preserve the base inventory's membership and row semantics. Do not solve the bug by unconditionally overwriting every base reference with the current catalog; a base-only caller may still index base rows by those IDs. Instead:

1. Make root roles explicit in the enumeration and assembly code.
2. Convert the current catalog's native owners.
3. Convert still-reachable eligible base catalog/path owners as well, if distinct.
4. Reuse converted owners when both roots shared a source owner.
5. Route lifecycle lookups to the current catalog and base-only compatibility operations to their base snapshot.
6. Remove obsolete base roots only when their callers have been redirected and their semantics are preserved. Do not retain an unused native root solely as a fallback.

No-op detection is valid only when every reachable eligible owner in that inventory is already archived. A residual native FTS or bitmap payload does not prevent a no-op. A native current catalog shard does.

### 4.3 Required sequence

Construct at least two files and fully archive. Delete one without adding, updating, or compacting anything. Before a second archive, assert mixed representation and native current catalog/ownership components. The second archive must convert those owners, keep the deletion, reuse unaffected archived dictionaries, and leave zero eligible native owners. A third call must be a genuine no-op.

Repeat for a compatible path-only move, last-file deletion, a zero-record file, and a multi-collection file. Check current exact-path and ordered-path results, NOT/complement results, ownership routes, and old snapshots. Do not insert compaction before the assertion: compaction creates new native segments and can mask the no-op bug.

## 5. R02 — shutdown without a transition-gate deadlock

### 5.1 Required ordering

Keep the gate contract for archive versus data publication. Change shutdown ordering, not query locking:

1. Under a short publication/control critical section, mark the index as draining/quiescing so no new publication can commit.
2. Request cancellation and stop admission of periodic and maintenance work. Extract task handles under their coordination locks, then release those locks.
3. Acquire the exclusive transition permit to wait for already admitted publishers/converters to leave. **Release that permit immediately after the drain check.** Do not hold it while waiting for or joining a task.
4. Wait for owned workers and the periodic worker with no publication, writer, transition, or maintenance registry lock held. Workers that were preparing outside the gate must be able to acquire a permit, observe cancellation, and terminate.
5. Mark quiescence complete only after workers finish. Preserve the existing report/error semantics.

Every writer must check quiescing after acquiring its transition permit and again at publication. The public archive operation must retain its precheck and post-acquisition check. User callbacks and progress/completion callbacks execute outside transition/publication/writer locks.

Retain the existing pure-query behavior during shutdown and archiving. Do not add a global lock to reads or move source callbacks under the transition gate just to avoid the race.

### 5.2 Concurrent close/resume

Make the close lifecycle states explicit: Running, Draining, and Quiesced. Use a small control mutex/condition variable or the existing equivalent to coordinate them; never hold its mutex while joining workers. Concurrent quiesce callers wait for the same drain completion without starting conflicting joins. `resume_after_quiesce()` succeeds only after completed quiescence; during Draining it returns the existing cancellation error rather than reopening publications. Preserve the documented ability to resume after a completed cancelled close.

Document the lock order and every deliberate release boundary. A worker must never be joined while its required lock is retained by the joining thread. Do not use timeout loops or `try_lock` spinning as the production fix.

### 5.3 Deterministic reproduction

Provide a test source/adapter barrier inside reconciliation preparation, before transition acquisition. Start reconciliation, wait until the barrier is reached, start quiesce, observe its draining state, then release preparation. Both operations must terminate, no late publication may install, and queries must remain valid. Cover add and replace preparation, periodic reconciliation shutdown, an archive waiting for/holding the gate, and concurrent resume rejection.

Use channels/condition variables and test deadlines as failure guards. Do not use sleeps as proof of ordering. The existing paused-archive test must remain, but it is not a replacement for the reconciliation barrier test.

## 6. R03 — preserve transactional conversion at every boundary

The public standalone transaction introduced in `1d3c7be663` is the right correction. Keep candidate preparation and final replacement for both APIs.

Change dictionary conversion to borrow its source:

```rust
fn archived(&self, context: &mut ArchiveContext) -> Result<Self, String>;
```

The concrete context may be split into borrowed plan data and a worker-local serializer, but the source dictionary is immutable. Native published dictionaries produce validated replacement owners sharing the original bitmap array. Already archived dictionaries return a shallow owner clone. Builders return an error without mutation.

`PostingIndex` must prepare values, lengths, and structural replacements into locals and assign them only after all succeed. Remove `mem::replace(field, PostingDictionary::builder()).archive_native()?` from fallible conversion. Use the same principle for file/record lookups, rows, catalog shards, and container assembly. No error branch may leave an empty placeholder in a reusable object.

Prepare all fallible report data, generation arithmetic, owner associations, and candidate validation before the commit point. After installing a publication, the operation must not return a recoverable preparation error from a second lock acquisition or a fresh status read. Build the success report from the validated candidate and operation statistics. For Index, verify source root identity, instance, logical revision, representation generation, and quiescing under the short installation lock.

Generation advances once for a successful operation that performs conversion. A no-op or failed operation does not advance it. Logical revision, IDs, source revisions, and file versions do not change for archiving. Overflow must be rejected before conversion where the required next generation is already known.

### Failure injection

Replace or supplement thread-local `fail_next_freeze` with operation-scoped test controls passed to workers. Support failure at a specified component identity and stage: serialization, checked-byte validation, semantic validation, final candidate validation, or immediately before installation. Thread-local state in the coordinator alone does not inject failures in Rayon workers.

For both APIs, fail after at least two distinct components have successfully prepared. Capture original status, generation, logical revision, owner identities, selective query results, and row/path/ownership results. Verify all are unchanged, candidates are released, and retry succeeds. A match-all count alone cannot detect an emptied posting dictionary.

On failure, stop admitting new conversion work, allow active jobs to finish or cooperatively exit, join them, and discard unpublished output. Do not leave detached jobs or partial conversions in the current root. Recoverable errors are covered; process abort and allocator abort are not transactional recovery scenarios.

## 7. R04 — release replaced maps and make candidate assembly shallow

### 7.1 Fix the actual path owner

The existing `native_lookup_maps_released()` test checks only `by_path` and `relative_paths`. It does not inspect owners reached through `published_paths`, so it can pass while the old maps remain live.

Provide a representation-aware `FileInventory` path backing. Native construction may use the existing direct maps; after catalog adoption the inventory references a `PathDirectory` for the corresponding base snapshot. During archiving create a new inventory wrapper whose path backing references the converted directory. Do not mutate a shared old inventory in place, and do not retain the old path directory behind the archived wrapper.

Make the inventory's immutable file-row array a shared owner so replacing this wrapper does not clone every `FileEntry`. Build its vector privately, move it into `Arc<Vec<FileEntry>>` at publication, and update read-only consumers through slices. Keep the external inventory/query behavior. Update serde and all in-repository clients for any exposed field-type adjustment; record that source-level API adjustment in the report.

For standalone Repository archiving, release redundant inventory direct path maps by adopting the matching converted base path directory. Do not change base file membership or swap in a semantically different current catalog.

### 7.2 Shared unchanged payloads

Eliminate the deep work in `Repository::archive_candidate()` and `(*published.index).clone()`.

Use immutable shared owners for these completed data categories:

| Data | Required ownership treatment |
| --- | --- |
| Repository base fingerprints | Shared vector; do not clone every fingerprint for conversion |
| Collection generation, base record inventory, base ID lookup, identity order | Share completed owners across representation candidates; do not recreate native maps or arrays |
| Segment identity order | Shared vector, moved once out of its builder |
| Segment/base logical and empty-array bitmaps | Shared completed bitmap owners; changed lifecycle operations construct their own replacements |
| RelationIndex and RelationContribution arrays and their nested strings/bitmaps | Share completed arrays across archive candidates; no per-edge copying |
| Posting bitmap arrays, FTS, custom text, sorting | Preserve existing shared owners |
| Verified and inferred-order caches | Share the existing synchronized cache owner; do not clone its contents while holding its lock |
| Query worker pool/source registry with immutable shared data | Reuse the existing runtime and owners |

Use coarse ownership per completed array/component, not one new Arc per row, key, relation edge, or bitmap member. `Arc<Vec<T>>`, `Arc<HashMap<...>>`, and shared immutable component owners are the default choices. Builders retain ordinary owned containers and move them once when publishing. Do not build a second container merely to obtain the shared form.

The public logical APIs and method names remain unchanged. Where a currently exposed read-only field needs a shared type, update its documented type and all clients rather than creating a hidden eager compatibility copy. Do not silently change a returned view into a full owned clone. Existing explicit compatibility accessors may remain lazy.

Cache entries must own their referenced immutable storage or store stable logical identifiers. Sharing a cache must not make it contain borrowed pointers into released native maps. Preserve normal logical-mutation invalidation; representation-only conversion does not invalidate otherwise valid entries.

### 7.3 Required ownership proof

Use weak probes for actual native map/row owners: posting maps, record-ID maps, file lookup maps, catalog metadata/path/ownership shards, and inventory path directories. A weak reference only to the outer segment is insufficient.

Before archiving, pin an old view. After conversion, fresh views must use the new backings, payload owner identities must be unchanged, and old map owners must remain alive through the pinned view. After dropping it and operation-held references, all replaced eligible native owners must be gone. Repeat without a pinned view, with multiple shared references, after a catalog-only mutation, and through the standalone API. A successful owner-release test does not require RSS to fall immediately.

Preserve direct native startup. Do not pay for a full alternate layout at native publication to make archive preparation appear cheaper.

## 8. R05 — retain the first record-ID map

Change native `RecordLookup` backing to retain the builder's **`HashMap<RecordId, RecordSlot>`** under one shared owner. `RecordLookup::native(values)` moves that map directly. It must not allocate a new hash table, copy ID strings, or convert every value to a second primitive representation.

Keep `RecordId`'s current public ownership unless another assigned task already changes it. Add a correct borrowed `str` lookup implementation for `RecordId` if needed, with equality and hashing consistent with `RecordId`. Then `get(&str)` must not allocate an owned key. Test long IDs, Unicode IDs, empty/missing lookups where valid, and hash/equality consistency.

Archive directly from the existing hash-map iterator. A borrowed key adapter writes the existing ID text as an archived string; a value adapter writes the `RecordSlot`'s u32. No transformed map, sorted key vector, or per-ID Arc allocation is permitted. Preserve the map's checked cardinality and the independent identity-order array's ordering contract.

At native publication, compare the moved map's allocation/capacity and ID owners with the builder's. Under a targeted allocation counter, the wrapper may allocate its Arc/control ownership but must not allocate proportional to record count. Include first-batch and mutation/compaction builders that finish through the same API.

## 9. R06 — serialize metadata through borrowed adapters

### 9.1 Segment rows

Replace the owned `SegmentRowsArchive { files: collect(), records: collect() }` input with a `SegmentRowsArchiveSource` borrowing native file and record slices. Keep the same final logical row representation. Write final archived arrays through exact-sized borrowed iterators.

Implement narrow borrowed adapters for file rows, record rows, diagnostics, membership IDs, record slots, optional field paths, and stored paths. The adapters serialize scalars by value and strings/sequences from existing references. They must not construct `StoredSegmentFile`, `StoredRecord`, `StoredDiagnostic`, or nested owned vectors solely as intermediate serialization inputs.

Use the pinned rkyv interfaces and resolver contracts. The `resolve` method must resolve fields from the same logical borrowed values and returned resolvers, without allocating a new owned copy. Document each unsafe `Place` projection and keep it local. Do not transmute native layouts or persist native pointers.

### 9.2 Catalog rows

Replace `BorrowedCatalogRow`'s calls to `StoredFileRow::from_entry` with field-level borrowed serialization and resolution. Validate the native row through borrowed fields before writing, then validate the resulting archived associations without reconstructing a native row. An adapter named “borrowed” is not sufficient if it builds owned strings or vectors internally.

Keep the already direct catalog ID/path/ownership map iterators. Preserve exact hash-map load factors and key hashing behavior required by the pinned implementation.

### 9.3 Paths and uncommon diagnostics

Plain paths borrow their existing query/native text. Unix escaped paths borrow original native bytes plus the existing query key. Preserve lossless non-UTF-8 behavior and reserved escaped-key behavior.

On Windows, the native UTF-16 encoding may require bounded per-path conversion because `OsStr` does not expose a borrowed u16 slice. Permit one reusable worker-local path buffer for that unavoidable encoding, with its capacity and allocation counted separately. This is not permission for a vector of all paths or for copying ordinary Unix strings. Verify the actual target-platform API before claiming a borrowed UTF-16 slice exists.

Diagnostics must preserve source identifier/version, category, message, field path, byte/line/column offsets, optional ranges, record references, and omitted counts. Healthy rows must take the allocation-free empty-diagnostic path.

### 9.4 Allocation acceptance

Allowed: final archive buffers, final archived reverse directories, rkyv resolver/allocator scratch, bounded job descriptors, invariant-validation scratch, and unavoidable bounded platform encoding scratch.

Forbidden: application vectors containing all dictionary entries/references, a second native lookup map, owned row arrays staged for archive conversion, and temporary copied strings/diagnostic vectors for each row.

Record serializer scratch separately. Do not claim zero allocation; final archived strings must still be written. Confirm counters with an allocation profile capable of detecting replacement allocation paths, not only counters in the deleted functions.

## 10. R07 — schedule independent conversion owners, not collection groups

Replace collection-group scheduling with a single bounded conversion plan shared by both APIs. The job boundary is a unique independently convertible owner: a posting dictionary map, a combined segment-row owner, a file/record lookup owner, or a catalog metadata/path/ownership shard. Preserve association data needed for validation in typed descriptors.

Plan construction is allowed to allocate a vector of small component descriptors. It must not contain dictionary entries, full metadata rows, copied strings, bitmaps, or whole segment clones. Deduplicate source owners by type and allocation identity for the lifetime of this operation. Do not use a global pointer cache after those owners can be freed.

1. Enumerate current and retained base owners using R01.
2. Register already archived owners as reused without serializing or validating them again.
3. Create native conversion jobs with stable destination keys.
4. With one worker, execute inline without constructing a Rayon pool.
5. With more workers, use one archive pool bounded by the requested limit, capped to the useful job count. Do not create one pool per collection, segment, or shard family.
6. Produce typed replacement results into private result slots. Completion order must not change destination association, bitmap slots, collection ordering, segment IDs, or representation generation.
7. Assemble private wrappers from results, then perform component-association and publication checks.
8. Install once. Stop job admission and drain active work on failure.

All conversion categories, including base collection dictionaries and catalog shards, use this schedule. Do not leave a large serial preamble outside the worker limit. Per-job serializers own their output; reuse worker scratch where supported without copying completed buffers to recycle them. Do not add nested unconstrained parallelism inside a job or occupy the query pool with the blocking archive coordinator.

Archive an identical shared empty dictionary once per distinct owner/type and reuse its archive. If one map owner appears with distinct bitmap owners, validate each distinct archive-to-payload association; do not confuse dictionary deduplication with sharing unrelated payloads.

Test one collection with many segments, one large catalog with little posting data, many collections, and shared owners. A deterministic worker barrier must show multiple eligible jobs running when worker limit and job count permit it. It must also show peak active jobs never exceeds the limit. Do not assert that 32 workers must be faster; actual timing remains empirical.

## 11. R08 — validate without rebuilding keys or repeating unchanged checks

Preserve two distinct checks: byte-level archive validity and logical relationships between archived components and shared payloads. Their timings must be distinguishable.

For record-ID validation, query the lookup directly with `record.id.as_str()`. Do not allocate `Arc<str>` or `String`. Prove uniqueness with the existing slot association: each dense row's ID must map to its own distinct slot, and lookup length must equal row count. Two rows with the same ID cannot both satisfy that condition. If an additional set is retained for a separately documented invariant, store borrowed strings, not new owned IDs.

For files, similarly use lookup length, row ID/ordinal correspondence, and checked record ownership. Preserve all required checks: dense slots, valid ranges/revisions, collection membership, exact file/record coverage, valid references, dictionary bounds, and live-domain association. Add a compact visitation bitmap where exact one-time record ownership must be demonstrated. Validation scratch is allowed; another metadata representation is not.

Prepare an invariant table before removing any repeated pass. For each invariant record its producer guarantee, native-publication check, archive-byte check, and archive semantic check. Newly generated bytes are checked exactly once before being admitted as a validated owner. Existing archived owners are reused without rechecking their bytes. Shared owner conversion is checked once, with additional checks only for distinct payload associations.

Do not repeatedly validate unchanged FTS, sorting, or relation payloads during a representation-only switch. Do not initialize compatibility caches to validate rows. Borrow the native or archived views directly.

This repair is not authorization to trust serialized bytes without checking them or to remove metadata invariants because the source “was already valid.” Conversion bugs can change associations, so candidate relationships still require verification.

## 12. R09 — avoid full metadata materialization on ordinary archived queries

The committed report records substantial cold Q001 differences and acknowledges compatibility-row materialization. The inspected executor also contains concrete full-cache routes: `segment.record()` calls at approximately lines 433, 620, and 642 lead through `RecordRows::get()` to `values()`. Generic metadata paths use `catalog.entry()`. These are actionable routes; do not attribute the entire Q001 gap to them without profiling its actual call path.

Use the existing `SegmentRecordRowRef` and catalog/file borrowed views as the common internal execution interface. Extend them with any missing scalar/class/diagnostic accessors. Change internal metadata evaluation and result assembly to consume those views instead of requiring `&RecordEntry` or `&FileEntry` from a full compatibility array.

Apply this to record byte-range filters, collection/root metadata, file metadata, identity comparison, source-read locators, relation resolution, and hit construction. Keep owned public results at the output boundary. Materialize only the selected result or explicitly requested compatibility object when ownership is required; do not populate every record in the segment to return one result.

Avoid allocation per scanned candidate where borrowed fields suffice. A small scalar probe used by metadata predicate evaluation is acceptable; copying every candidate ID/path into it is not. Native execution continues to borrow native rows and must not incur a new allocation or lock per row.

Retain explicit full-row compatibility accessors for callers that request them. Rename internal call sites or document them so they cannot be mistaken for cheap borrowed access. Do not populate compatibility caches in `archive()`, status, storage accounting, or benchmark setup to conceal the first-query cost.

Test a selective first query on a freshly archived large segment with empty result/verification caches and assert zero full file/record/catalog cache initialization. Cover metadata filters and a normal hit-producing query, then separately call the explicit compatibility accessor and verify lazy behavior. Profile cold Q001 and Q119 with verified/result caches disabled; repeated result-cache hits are not evidence that archived execution itself became cheaper.

## 13. R10 — trustworthy diagnostics and separate timings

### 13.1 Operation report

Keep existing report fields where possible and add fields rather than overloading their meaning. Required definitions:

| Metric | Meaning |
| --- | --- |
| converted/reused components | Logical component roles from the operation's source and installed inventory |
| converted/reused owners | Unique physical eligible owners; shared owners counted once |
| newly serialized/capacity bytes | Sum of output allocations created by this successful operation, counted once; not a saturating difference of two root totals |
| current archive bytes/capacity | Unique current owners reachable from the installed publication, including retained eligible base references |
| residual native payload bytes | Explicit unarchived categories, with estimates labeled as estimates |
| peak active conversion jobs | Measured across all conversion categories; serial conversion is one, no-op is zero |
| serializer work sum | Sum of serializer intervals across jobs, not wall time |
| validation work sum | Byte validation plus semantic validation, or two explicitly separate fields; do not omit semantic checks |
| preparation wall time | Inventory/planning, candidate assembly, pool setup, conversion, and candidate checks before installation, with subphases when material |
| installation wall time | Actual final switch interval for both APIs; Repository must not report a hard-coded zero |
| retirement time | Release of operation-owned retired structures, measured separately if material |
| total wall time | Public call entry through success-report completion, including waits and operation cleanup |

Compute post-conversion accounting from the candidate before installation. Do not perform a fallible `self.view()?` after committing just to construct a report. Drop operation-held source and precheck roots deliberately outside publication locks, rather than allowing hidden long-lived locals to make release timing misleading. Old application views may still pin them.

Avoid deep payload scans for every no-op and status call. Cache immutable component byte summaries at construction/conversion where the information is already available; combine them by owner identity. Cache population during a query must be accounted through a cheap length/capacity estimate without reconstructing its contents.

Fix catalog accounting: `file_row_cache_bytes += cache.len()` counts entries, not bytes. Report array capacity and known owned variable payloads consistently. Native ownership shards must not silently contribute zero when nonempty. Preserve honest caveats for hash control bytes, B-tree nodes, allocator rounding, and shared string accounting; do not present approximate heap totals as exact RSS. Use a single seen-owner context throughout the current publication traversal so base/current aliases do not duplicate bytes.

### 13.2 Benchmark timer corrections

In `bench_musenalm.rs`, capture native-ready/build completion **immediately after `wait_complete()`**, before any archive call. The direct Repository branch already captures its open timer before archiving; both API paths must use the same phase definitions.

Emit native build/open time, first-searchable event, explicit archive wall time, and combined time as separate fields. Capture native end RSS before conversion, archived end RSS before queries, and post-query RSS separately. Label the historical progressive `build_ns/open_ns` field as combined when archive was enabled; do not reuse it as native startup.

Run each alphabet/configuration in a fresh process for memory comparisons. The existing benchmark loops ASCII and Unicode within one process, so later RSS/high-water samples include allocator retention and earlier-process history. Do not interpret the published 690 MiB versus 1367 MiB native RSS values as an isolated alphabet comparison.

Fix executable provenance. The committed manifest's `binary_build_revision` is null while `code_revision` identifies the checkout. Compile the benchmark with an embedded implementation revision, record executable SHA-256, build revision, execution checkout revision, lockfile hash, command, and dirty state. A checkout SHA alone does not prove which source produced a reused binary. Build a clean committed implementation before the final measurements.

Do not compare Index one-worker archive timings directly with Repository eight/thirty-two-worker timings and attribute the difference to scheduling. Keep the API path, corpus, build policy, queries, and process conditions identical within each worker scaling comparison.

### 13.3 Existing evidence to retain, with limits

The committed samples attribute their implementation to `cd9b8275`, before the standalone transaction correction. They are historical context, not a benchmark of the final reviewed code.

| Published single sample | ASCII | Unicode |
| --- | ---: | ---: |
| Native startup, 32 workers | 745.907 ms | 805.718 ms |
| Explicit archive, one worker | 175.230 ms | 149.572 ms |
| Explicit archive, eight workers | 178.895 ms | 172.518 ms |
| Explicit archive, thirty-two workers | 288.846 ms | 161.846 ms |

The archive samples report 149,111,376 serialized bytes, 233,891,392 capacity bytes, and a maximum of seven simultaneous jobs at the larger worker limits. The scheduler's collection-group structure explains why more segments inside one collection cannot increase concurrency; these single samples do not establish scaling or its absence statistically.

The report states 592 core tests passed, 17 ignored, and passing workspace/frontend checks. The completion record and later test commits must be distinguished from benchmark executable revisions. Preserve the original evidence without rewriting it to imply that repaired code was measured.

## 14. R11 — required regression coverage

Use independent logical results and fresh-rebuild comparisons where possible. Inspect exact state and actual owners, not just an outer object's lifetime or a representation label generated by the same buggy counter. Failure and scheduling controls must be scoped to one operation and work with parallel execution.

| Test | Required proof |
| --- | --- |
| A01 | Default Repository and Index startup, including empty collections and relations, produces no archive bytes or checked archive conversions before explicit invocation |
| A02 | Progressive first searchable publication and full native completion retain their existing incomplete/complete semantics |
| A03 | Archive → delete → archive, without compaction or addition, converts changed current catalog owners and preserves deletion |
| A04 | Last-file deletion and zero-record-file deletion have correct status, ownership, and subsequent archive/no-op behavior |
| A05 | Path-only move after archive preserves current exact/ordered lookup, base compatibility semantics, and old views |
| A06 | Distinct base/current catalogs and aliased roots are inventoried correctly; physical owners are not double-counted |
| A07 | Repository late failure after successful conversions preserves selective value/length/structural queries, metadata, owners, status, and retry |
| A08 | Index late failure preserves publication pointer, generation, revision, old/new view results, and retry |
| A09 | Injected worker serialization, byte-validation, semantic-validation, and pre-install failures drain jobs and release candidates |
| A10 | Failed component conversion leaves a reusable PostingIndex unchanged; no empty-builder placeholder remains |
| A11 | Generation overflow and cancelled/quiescing entry fail before installation; successful no-op does not advance generation |
| A12 | Reconciliation add preparation paused outside the gate cannot deadlock quiesce |
| A13 | Reconciliation replace preparation and periodic worker shutdown cannot deadlock quiesce |
| A14 | Archive/quiesce overlap prevents late installation and still permits queries against the prior snapshot |
| A15 | Concurrent close callers coordinate; resume during Draining is rejected and resume after complete quiescence works |
| A16 | Completion/progress callback invoking archive or status does not recursively wait on a retained transition/publication permit |
| A17 | Actual native posting, row, lookup, catalog, ownership, and inventory-path owners survive pinned old views and are released after their last reader |
| A18 | A cold standalone Repository archive releases redundant direct inventory path maps and preserves base lookups |
| A19 | Archive shares FTS, sort, posting bitmaps, logical domains, identity arrays, relation arrays, inventories, and synchronized cache owners |
| A20 | Native RecordLookup moves the original map and ID owners without proportional allocation; borrowed lookup allocates no key |
| A21 | RecordLookup direct hash serialization matches native lookup for long/Unicode IDs and survives source release |
| A22 | Segment/catalog row serialization creates no owned staging row vectors or copied per-row strings/diagnostics |
| A23 | Paths, diagnostics, memberships, byte ranges, raw fallback, and optional values round-trip, including non-UTF-8 platform cases |
| A24 | Record/file validation rejects duplicate IDs, missing or duplicate ownership, invalid ranges, wrong collection, and bad lookup slots without key reconstruction |
| A25 | One collection with many segments can execute multiple conversion jobs; requested worker limit is never exceeded |
| A26 | Catalog/base jobs participate in the same bounded schedule; one-worker conversion requires no pool |
| A27 | Shared source owners convert once, destination associations remain correct, and already archived owners are neither serialized nor checked again |
| A28 | Repeated no-op preserves owner identities, produces zero new bytes/jobs, and does not traverse full posting payloads |
| A29 | A query pinned before archive and a query started during a paused conversion both complete with correct results |
| A30 | Cold archived selective queries and metadata predicates initialize no whole segment/catalog compatibility cache |
| A31 | Explicit compatibility access still materializes lazily and correctly; native query path gains no per-row allocation or lock |
| A32 | Ascending/descending ranges, insertion-assigned bitmap slots, empty maps, lengths, and structural/custom/inferred indexes remain equivalent |
| A33 | Add/replace/delete/move in native, archived, and mixed states match a fresh rebuild; archive never resurrects a retired record |
| A34 | Source-free bounded/full compaction accepts mixed inputs, publishes native changes, and later archive reuses untouched owners |
| A35 | Native/archived relations and multi-collection ownership remain equivalent with nonempty relation fixtures |
| A36 | Logical revision, instance, file handles, record locators, source versions, and valid query-cache results survive representation conversion |
| A37 | New/current bytes, logical counts, owner counts, cache bytes, and worker peaks match independently inspected fixtures |
| A38 | Native-ready benchmark timing excludes an intentionally paused archive; combined timing includes it |
| A39 | Both public APIs emit actual installation and validation timings with valid phase definitions; compiled revision is present |
| A40 | Repeated build/archive/mutate/archive/drop cycles release replaced owners and worker scratch, without accumulating threads |
| A41 | Archive reads no source content and writes no source or index files |
| A42 | CLI/TUI/desktop/all examples compile, preserve native readiness, and do not insert automatic archive calls |

Retain the original T01–T40 tests and map them to these more specific scenarios. Do not count an inherited native-only lifecycle test as proof of archived/mixed behavior unless that test actually runs those states. Preserve the new archived replacement test.

Use deterministic small fixtures for concurrency and failure tests, a large enough fixture for ownership/allocation/query-cache behavior, and the existing multi-seed lifecycle model with archive calls inserted between mutations. Test negative queries that would reveal dropped postings, not only match-all counts.

## 15. Focused benchmark protocol

The previous work records user authorization for smaller benchmarks. Use the bounded protocol below for this repair rather than reinstating the original exhaustive seven/five-sample matrix. It is large enough to distinguish obvious regressions and phase errors without delaying correctness delivery for a broad benchmark campaign.

### 15.1 Revisions and conditions

Compare the final reviewed native-publication base B, the repaired implementation F, and the original native reference N (`2e18871c0bcfb9cf9335d9e73525c64f91c960cf`) where its matching build is available. Record any adapter differences. Do not present current native versus current archived query parity as parity with historical native construction.

Use the same corpus/schema, worker settings, segment policy, allocator, toolchain, page-cache policy, query settings, and release build. Every build/archive memory sample starts in a fresh process. Record compiled revision and binary digest. Do not include unrelated profiles or queries before measuring native startup and archive memory.

### 15.2 Required runs

1. **Native build:** three paired alternating B/F runs for ASCII and Unicode at 32 build workers, without archiving. Include both Repository and progressive Index entry points, or explicitly identify their shared core and measure the wrapper separately. Add one-worker checks on the dominant build path to expose ownership overhead. Obtain three comparable N samples at 32 workers if available; otherwise report historical comparison as unverified rather than substitute older numbers.
2. **Archive:** three fresh-process F samples at worker limits 1, 8, and 32 for each alphabet on a fixed 32-worker build. Use one API consistently for each scaling table. Separately exercise and time the other public API at the default and one parallel setting. Record new/current bytes, capacity, CPU time, all phase timings, actual peak jobs, and memory before archive, after archive, and after releasing pinned views.
3. **Topology:** one-collection/many-segment and catalog-dominated fixtures at 1 and 8 archive workers, three samples each. Confirm concurrency with tests independently of performance.
4. **Query semantics:** the complete existing 150-case suite for both alphabets through native and archived direct/progressive paths, plus mixed-state coverage. One semantic run is sufficient to expose mismatches; performance conclusions use three repetitions with result/verified caches controlled. Record first execution separately.
5. **Outliers:** ten cold-execution samples for Q001/Q119 and any new material outlier, using fresh archived publications or cleared caches as appropriate. Keep truly cold first-query and subsequent execution measurements distinct. Also run queries while archive conversion is active and report contention.
6. **Lifecycle:** three samples for add, replace, delete, compatible move, and source-free compaction from native/archived states; measure subsequent archive and repeated no-op separately. Include a catalog-only change.
7. **Allocation:** one separate instrumented native-build run and one archive run for each major entry point. Attribute copies, output bytes, scratch, row materialization, map construction, and owner lifetime. Instrumentation timings are not substituted for release timings.

Target native-ready time at or below B and towards N. Investigate a median startup regression above 3% and 10 ms. For query timing investigate aggregate regressions above 3%, or individual regressions above 10% and 0.5 ms, with additional targeted samples before attribution. Small-sample medians must not be described as statistically established parity. Faster default archive and reduced transient allocation are desired outcomes; report actual changes rather than impose an invented speedup percentage.

Functional requirements remain mandatory even if a large-worker benchmark does not improve. Default one-worker behavior must remain practical. Do not force 32 workers, shrink buffers unconditionally, warm compatibility caches, omit index families, or weaken semantics to obtain favorable timings.

### 15.3 Evidence files

Save `revisions.json`, `native-build-samples.tsv`, `archive-samples.tsv`, `query-summary.json`, `lifecycle-samples.tsv`, `allocation-summary.tsv`, `ownership-report.md`, `test-coverage.tsv`, `benchmark-commands.md`, and `implementation-report.md` in the repair evidence directory. Include raw query digests and relevant profiler summaries. Preserve the prior task's samples and explain historical measurement errors without rewriting old observations.

## 16. Commit phases and acceptance

| Phase | Required implementation | Verification checkpoint |
| --- | --- | --- |
| 1 | Record base and add failing current-catalog/no-op, reconciliation-deadlock, and late-failure tests | Reproductions isolate the defects without relying on timing sleeps |
| 2 | R01 authoritative inventory/counts and R02 shutdown ordering | Catalog-only archive and reconciliation shutdown tests pass |
| 3 | R03 borrowed non-destructive component conversion and operation failure controls | Both APIs pass late-failure/retry and worker-drain tests |
| 4 | R04 shared immutable payloads and inventory path ownership; R05 original ID-map retention | Actual-owner release and no-proportional-native-lookup-allocation proofs |
| 5 | R06 borrowed row/path/diagnostic serializers and R08 allocation-free key validation | Round-trip/negative tests; no staged row arrays or key copies |
| 6 | R07 unique-owner bounded scheduling and private result assembly | Single-collection/catalog concurrency, deduplication, cancellation, and equivalence |
| 7 | R09 borrowed query metadata path | Cold query cache-initialization tests and focused latency results |
| 8 | R10 diagnostics/timer/provenance corrections | Accounting fixtures and separate native/archive metrics |
| 9 | R11 complete combined verification and focused benchmarks | Coverage map, ownership proof, exact revisions, and honest performance verdicts |
| 10 | Final review, completed task document/report, commit and push | One integration branch delivered; master unchanged |

Run focused tests after each phase. On the final combined implementation run at least:

```text
cargo test -p objekte-search-core --lib
cargo test -p objekte-search-core --test compaction_api
cargo test --workspace
cargo check --workspace --all-targets
cargo check -p objekte-search-core --all-targets --features detailed-profiling
cargo clippy --workspace --all-targets
cargo fmt --all -- --check
git diff --check
```

Build and execute the existing `explicit_archive` example against a writable test fixture and the release benchmark examples used for the evidence. If a command changes with the current repository, use its documented equivalent and record why. Do not report unexecuted checks as passed. Keep existing advisory lint warnings distinct from new failures.

Functional acceptance requires all correctness, ownership, direct-serialization, no-implicit-archive, query, and lifecycle requirements above. Performance acceptance is a separate explicit verdict for native build, default archive, parallel archive, queries, mutations, and peak/live memory. A remaining gap must be reported with measured attribution or marked unmeasured; it cannot disappear into “all tests passed.”

The final report must identify already-correct code that was preserved, each R01–R11 repair, each A01–A42 test and executed command, public type adjustments, residual native categories, known limitations, benchmark executable revisions, and the integration worktree. Correct the earlier overbroad test-coverage/ownership claims with a dated addendum linked to the new evidence. Do not erase the historical report or claim its tests covered the new scenarios.

## 17. Source references

All implementation links below are pinned to the final reviewed revision.

- [Reviewed implementation](https://github.com/Simon-Martens/objekte/tree/29613ab1ccbcd9c01b2b44de3288ad6dab044736)
- [Archive APIs, candidate assembly, scheduler, and report](https://github.com/Simon-Martens/objekte/blob/29613ab1ccbcd9c01b2b44de3288ad6dab044736/crates/core/src/lifecycle_archive.rs)
- [Current publication, catalog counting, reconciliation, and quiesce](https://github.com/Simon-Martens/objekte/blob/29613ab1ccbcd9c01b2b44de3288ad6dab044736/crates/core/src/lifecycle.rs)
- [Repository candidate and collection ownership](https://github.com/Simon-Martens/objekte/blob/29613ab1ccbcd9c01b2b44de3288ad6dab044736/crates/core/src/repository.rs)
- [Inventory retained path directory](https://github.com/Simon-Martens/objekte/blob/29613ab1ccbcd9c01b2b44de3288ad6dab044736/crates/core/src/inventory.rs)
- [Segment row and identity lookup conversion](https://github.com/Simon-Martens/objekte/blob/29613ab1ccbcd9c01b2b44de3288ad6dab044736/crates/core/src/segment_archive.rs)
- [Catalog adapters, conversion, and accounting](https://github.com/Simon-Martens/objekte/blob/29613ab1ccbcd9c01b2b44de3288ad6dab044736/crates/core/src/lifecycle_catalog.rs)
- [Posting map sources and fallible replacement](https://github.com/Simon-Martens/objekte/blob/29613ab1ccbcd9c01b2b44de3288ad6dab044736/crates/core/src/posting.rs)
- [Metadata execution paths](https://github.com/Simon-Martens/objekte/blob/29613ab1ccbcd9c01b2b44de3288ad6dab044736/crates/core/src/executor.rs)
- [Benchmark timing and executable manifest](https://github.com/Simon-Martens/objekte/blob/29613ab1ccbcd9c01b2b44de3288ad6dab044736/crates/core/examples/bench_musenalm.rs)
- [Current archive tests](https://github.com/Simon-Martens/objekte/blob/29613ab1ccbcd9c01b2b44de3288ad6dab044736/crates/core/src/rkyv_prototype.test.rs)
- [Implementation report and claimed coverage](https://github.com/Simon-Martens/objekte/blob/29613ab1ccbcd9c01b2b44de3288ad6dab044736/docs/ai/generated/native-publication-explicit-archiving/implementation-report.md)
- [Archive samples](https://github.com/Simon-Martens/objekte/blob/29613ab1ccbcd9c01b2b44de3288ad6dab044736/docs/ai/generated/native-publication-explicit-archiving/archive-samples.tsv)
- [Native build samples](https://github.com/Simon-Martens/objekte/blob/29613ab1ccbcd9c01b2b44de3288ad6dab044736/docs/ai/generated/native-publication-explicit-archiving/native-build-samples.tsv)
- [Original specification and completion record](https://github.com/Simon-Martens/objekte/blob/29613ab1ccbcd9c01b2b44de3288ad6dab044736/docs/ai/done/objekte_native_publication_explicit_archiving_specification.md)

Inspect the actual local base and pinned dependency APIs before implementation. This specification defines work to complete; it is not evidence that those repairs have already been executed.

## Implementation report

- Task identifier: `explicit-archiving-repair`
- Completion date: 2026-11-17
- Base commit: `29613ab1ccbcd9c01b2b44de3288ad6dab044736`
- Integration branch: `integration/explicit-archiving-repair`
- Integration worktree: `/home/simon/source/objekte-integration-explicit-archiving-repair`
- Evidence: `docs/ai/generated/explicit-archiving-repair/`
- Implementation commit: `bfc1f0a1c2`

### Implemented repairs

- Current and retained base catalog roots now participate in inventory,
  no-op detection, status, and unique-owner accounting. Catalog-only deletion
  followed by archive converts changed current-catalog owners, preserves the
  deletion, and reaches a genuine generation-preserving no-op.
- Close lifecycle states are explicit as Running, Draining, and Quiesced.
  Quiescence drains admitted publishers through a short transition critical
  section and releases the gate before stopping and joining periodic,
  reconciliation, and maintenance work. Concurrent close callers share the
  drain and resume during Draining is rejected.
- Posting dictionaries prepare archived replacements from borrowed published
  maps and retain the canonical bitmap owner. Failed component conversion no
  longer installs empty builder placeholders.
- Operation-scoped fault controls are shared with worker threads and cover
  serialization, checked-byte validation, semantic validation, final-candidate
  validation, and pre-installation. Both public APIs preserve revision,
  generation, owners, metadata, row/path/ownership lookups, and selective
  query results on late failure and succeed on retry.
- Inventory file rows, fingerprints, identity order, relations, segment rows,
  and path wrappers use shared completed ownership. Archive replaces the
  inventory direct path maps with converted base path directories.
- Native record-ID publication retains the builder's
  `HashMap<RecordId, RecordSlot>`. Archiving serializes it directly and native
  lookup uses borrowed `str` keys.
- Segment rows serialize from borrowed file, record, diagnostic, path, scalar,
  membership, and slot adapters without staging full owned row vectors.
  Catalog metadata iterators are direct; the retained owned per-row catalog
  adapter is the limitation below.
- Segment conversion jobs are unique-owner jobs flattened across collections,
  bounded by the requested workers and useful work. A deterministic
  one-collection/many-segment test observes concurrent jobs.
- Archived validation uses borrowed IDs, direct lookup probes, and one exact
  ownership visitation bitmap.
- Metadata execution uses borrowed segment record rows and direct file scalar
  methods. Q001 and Q119 cold regressions were removed; cold metadata
  execution no longer initializes whole compatibility caches.
- Archive reports distinguish logical components, owner counts, converted and
  reused owners, current/new bytes and capacity, serializer/validation work,
  preparation/installation/retirement/total time, worker limit, and actual
  peak jobs.
- `bench_musenalm` captures native completion before archive, separates archive
  and combined time, separates native-ready and archived RSS, records
  executable digest and dirty state, and embeds the compiled revision.

### Validation

```text
cargo test -p objekte-search-core --lib
  600 passed; 17 ignored; 0 failed

cargo test -p objekte-search-core --test compaction_api
  1 passed

cargo check --workspace --all-targets
  passed

cargo check -p objekte-search-core --all-targets --features detailed-profiling
  passed

cargo test --workspace
  default parallel run hit the existing private-drain test's 120-second timeout;
  RUST_TEST_THREADS=1 cargo test --workspace then passed completely

cargo clippy --workspace --all-targets
  passed with the repository's existing advisory warnings

cargo fmt on changed Rust files
  passed; whole-workspace fmt check still exposes pre-existing formatting drift

git diff --check
  passed
```

The completed task document and implementation report are committed separately
after final code validation. See `generated/explicit-archiving-repair/` for raw
construction/query samples, exact commands, test coverage, accounting, and
benchmark verdicts.

### Performance

The user's smaller benchmark authorization remains in force. Three fresh
construction samples per alphabet and worker setting and one complete semantic
query run per path/alphabet were executed.

- Median native build: ASCII 764.760 ms, Unicode 823.043 ms.
- Median explicit archive: 152.944/153.714 ms at one worker,
  113.024/113.210 ms at eight workers, and 111.499/108.627 ms at 32 workers.
- Archive output: 149,152,416 serialized bytes, 233,957,056 aligned capacity
  bytes, and 84,804,640 bytes capacity slack.
- Complete direct queries: 1.070855x native overall.
- Complete progressive queries: 1.033761x native overall.
- All 300 native/archived alphabet/case result digests are identical.
- Cold median Q001 is 6.774/7.156 ms native and 8.766/8.645 ms archived for
  ASCII/Unicode. Cold median Q119 is 5.446/5.277 ms native and
  6.481/6.447 ms archived.

### Remaining limitations

- Catalog metadata serialization still constructs one owned `StoredFileRow`
  adapter per row. It does not stage a vector, but it is not the allocation-free
  borrowed-row path required for final parity with segment rows.
- Base collection and catalog conversion still form a serial prefix before the
  bounded segment schedule instead of participating in one queue with segment
  jobs.
- Dedicated lifecycle-mutation timing samples were not collected; lifecycle
  correctness is covered by the model and focused mutation tests.

These limitations prevent claiming every R06/R07 performance subgoal is fully
closed. They do not weaken the required public archive correctness, atomicity,
ownership, query semantics, or no-implicit-archive behavior.
