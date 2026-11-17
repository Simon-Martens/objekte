# Objekte: native publication and explicit in-memory archiving

Date: 2026-11-17

Task identifier: `native-publication-explicit-archiving`

## 1. Required outcome and controlling decisions

Build and publish usable native indexes first. Archive them only when an application explicitly invokes `archive()`. After a successful archive operation, new queries use the archived dictionaries and metadata. Native construction, native querying, archived querying, additions, replacements, deletions, and compaction must all remain supported.

This specification supersedes the earlier requirements that every published component already be archived, that archive finalization occur before search readiness, and that the combined native-build-plus-archive operation match native-only build time. It also supersedes the preceding discussion of automatic background conversion. **There is no automatic archiving in this task.**

The implementing agent must use these decisions:

1. Native construction is the default and only startup construction policy in this task. Do not add a startup option that archives implicitly.
2. Publish immutable native segments as soon as their required semantic construction and validation are complete. Their representation does not determine search availability.
3. Provide an explicit, blocking `Index::archive()` API, with a corresponding API for the standalone `Repository` path. Applications may call it from their own worker thread.
4. Queries remain usable throughout archiving. Lifecycle operations that would publish data changes are excluded during archive preparation and installation, using the transition gate specified below.
5. Prepare a complete replacement publication privately, validate it, and install it atomically. A recoverable preparation failure leaves the current publication unchanged.
6. Serialize B-tree dictionaries directly from borrowed native map iterators. Do not construct a second native B-tree, collect keys or entries into temporary vectors, or clone keys merely to serialize them.
7. Release native maps from the new publication after the switch. Old query snapshots may retain the old maps until their last reader releases them. Do not retain a hidden native fallback for archived dictionaries.
8. Share unchanged native payloads, especially posting bitmaps, FTS indexes, and sorting payloads. Archiving must not deep-copy them.
9. New or rebuilt components produced by later mutations or compaction are native. Existing unaffected archived components remain archived. A subsequent explicit archive call converts only remaining native components.
10. Keep everything in memory. No index-file creation, cache writing, mmap, flush, sync, automatic persistence, durable format commitment, or changes to source files belong to this task.

The purpose of archiving is to establish directly queryable immutable representations and a clean conversion boundary that later persistence work can use. This task does not claim that the entire index is already a self-contained disk image: native bitmap, FTS, sorting, and compatibility payloads still require future persistence treatment.

## 2. Reviewed baseline and Repair II evidence

Reviewed branch: `integration/rkyv-memory-index-repair`.

Reviewed report commit: `dc371a42e666ef6c4d3c0519d6653326d0ad6d56`.

Repair II implementation measured by that report: `65cb66dc5bac2623a94f74cc4e12b110839ce813`.

Native comparison revision: `2e18871c0bcfb9cf9335d9e73525c64f91c960cf`.

Repair I predecessor: `e4a7471ffe777a9a4992dff34d7070edf372e873`.

The committed seven-sample construction results show substantial improvement. Values below are medians from the reported repository-progressive construction path; they are not new benchmark runs performed by the author of this specification.

| Alphabet | Workers | Native ms | Repair I ms | Repair II ms |
| --- | ---: | ---: | ---: | ---: |
| ASCII | 1 | 3671.0 | 4177.1 | 4016.4 |
| ASCII | 8 | 808.0 | 1192.5 | 987.6 |
| ASCII | 32 | 649.4 | 1073.4 | 832.1 |
| Unicode | 1 | 4393.9 | 5047.7 | 4775.5 |
| Unicode | 8 | 930.8 | 1345.5 | 1099.2 |
| Unicode | 32 | 702.9 | 1140.4 | 897.6 |

ASCII at 32 workers improved approximately 22.5% versus Repair I; Unicode improved approximately 21.3%. The paired median ratios to native were 1.287 and 1.270 respectively. These paired ratios differ slightly from ratios of separately calculated medians.

At 32 workers, end RSS fell from 839.6 to 721.6 MiB for ASCII and from 846.5 to 716.6 MiB for Unicode. Native end RSS was 575.2 and 596.4 MiB. Repair II therefore improved both construction and memory, without reaching native parity.

Preserve these qualifications:

- The construction TSV has 126 samples: three revisions, two alphabets, three worker settings, seven samples each. Record counts agree at 148,486. Compare signatures within an alphabet; ASCII and Unicode signatures differ legitimately.
- The report records 584 core tests passing, 17 ignored, and passing workspace checks. These are reported results, not a substitute for testing this change.
- Aggregate query sums were approximately 5.4% above native for direct queries and 4.7% above native for progressive queries, with one suite repetition. This is limited timing evidence.
- Final focused stdout and `query-summary.json` show Unicode Q119 medians of 6.937 ms direct versus 4.607 ms native, and 7.546 ms progressive versus 4.561 ms native. The implementation-report prose contains different earlier numbers. Use final raw evidence, identify executable revisions, and do not reproduce the stale prose values. Final progressive Unicode Q001 is also above its individual threshold and needs separate attribution from cold-query effects.
- One detailed run showed approximately 186 ms of bigram finalization in a critical contents segment. Its posting archive/completion interval was approximately 5 ms and row/lookup archive interval approximately 10 ms. Across concurrent segments, summed intervals are not removable wall time. Deferring archives alone does not promise to remove the entire build gap.
- Exact serialized archive bytes were 148,561,552 and capacity 233,658,336 in the reported inventory. Eliminating conversion staging does not automatically eliminate the approximately 81.2 MiB capacity slack or the retained reverse directory.
- First progressive publication latency was not emitted by the old benchmark harness. A separate direct-build matrix and updated mutation timings were not provided. This task must distinguish these measurements properly.

Evidence directory: `docs/ai/generated/rkyv-construction-performance/` at the reviewed report commit.

## 3. Integration workflow

Read `AGENTS.md`, `docs/ai/README.md`, this complete specification, and relevant task notes before implementation. The user has authorized this task and the representation changes explicitly described here.

Create a separate integration branch and sibling worktree:

- Branch: `integration/native-publication-explicit-archiving`.
- Worktree, when the original checkout is `/home/simon/source/objekte`: `/home/simon/source/objekte-integration-native-publication-explicit-archiving`.
- Evidence: `docs/ai/generated/native-publication-explicit-archiving/`.
- Temporary notes: the original checkout's ignored `docs/ai/memory/native-publication-explicit-archiving/`.

Start from the completed Repair II branch tip, initially the reviewed report commit above. If the local Repair II branch has advanced, record the actual base, inspect the intervening diff, and retain valid repairs. Do not start from the original prototype or silently omit later fixes. Leave the original checkout and its main branch unchanged. Do not use `/tmp` for worktrees, merge into master, force-push, or discard user changes.

Use one supervisor. Optional delegated work must use separate sibling worktrees: posting storage and serialization; metadata/catalog backing; lifecycle/API and verification. The supervisor owns shared contracts and integrates changes. Subagents must not independently choose archive semantics, ordinal rules, readiness semantics, or publication policy. A single agent may perform all work.

Commit coherent phases, review explicitly staged paths, and push after every commit. Preserve uncommitted task documents according to the repository workflow. Deliver one tested integration branch for the user's merge. A performance limitation must be reported separately from functional completion; do not relabel it a speed improvement or weaken a requirement silently.

## 4. Semantic invariants and readiness

Separate logical completion from physical representation.

| State | Queryable | Data changes allowed | Archive action |
| --- | --- | --- | --- |
| Private builder | No | Builder-owned mutation | None |
| Published native | Yes | Through replacement publications | Eligible on explicit call |
| Private archive preparation | Existing publication stays queryable | Publishing writers wait at transition gate | Validate candidate |
| Published archived | Yes | Through replacement publications | Reuse unchanged archives |
| Published mixed | Yes | Through replacement publications | Convert native components only |

Native published maps are immutable just as archived maps are. Never mutate a map or bitmap that an existing `IndexView` can read. A native backing is not permission to bypass version checks, source revisions, liveness masks, or replacement publications.

Preserve existing progressive semantics: searches against a published subset remain explicitly incomplete. A completed segment is not evidence that all files, memberships, relations, or inference decisions are complete.

Add `search_ready: bool` to `InitialBuildStatus` and `IndexStatus`, or expose exactly equivalent accessors if repository compatibility requires keeping a public struct layout. It becomes true when the first fully usable publication of records is available; a successfully completed empty index is also search-ready. Discovery-only catalogs do not set it true. Partial-build cancellation/failure may leave a readable last publication; preserve the existing incomplete/error indicators.

Keep `InitialBuildPhase::Complete`, `construction_complete`, `initial_construction_complete`, `InitialBuildTask::wait_complete()`, and `IndexTimingsMs::full_available` tied to completion of native logical construction. No application must wait for archives to execute normal searches. `search_available` continues to describe first searchable publication and must now occur without archive conversion. Archive time must not be added retroactively to startup timing fields.

Search-ready means all previously supported behavior for that publication works: exact/range predicates, descending traversal, lengths, structural facts, FTS, raw fallback, relations, sorting, count, offset, limit, evidence, and record/file metadata access. Do not defer mandatory FTS packing, final inference policy, or relation correctness to `archive()`.

## 5. Public API and operation semantics

Expose the following behavior, using the repository's established module exports and error vocabulary:

```rust
impl Index {
    pub fn archive(&self) -> Result<ArchiveReport, IndexError>;
    pub fn archive_with_options(
        &self,
        options: ArchiveOptions,
    ) -> Result<ArchiveReport, IndexError>;
    pub fn archive_status(&self) -> Result<ArchiveStatus, IndexError>;
}

impl Repository {
    pub fn archive(&mut self) -> Result<ArchiveReport, IndexError>;
    pub fn archive_with_options(
        &mut self,
        options: ArchiveOptions,
    ) -> Result<ArchiveReport, IndexError>;
    pub fn archive_status(&self) -> ArchiveStatus;
}
```

`ArchiveOptions` contains a validated nonzero worker limit. The default is one worker. Provide a checked constructor for an explicit worker count; reject zero. Do not silently substitute the machine CPU count for the default. A larger pool is created or reused only inside an explicit operation, and its setup cost is measured. Do not occupy the query pool with a blocking archive coordinator.

`archive()` is blocking for its caller. On success, the current publication uses archived versions of every eligible native component reachable from that publication. Existing archived components retain their owners and are not serialized or checked again. A second call with no intervening eligible change reports zero converted components and zero newly generated archive bytes.

An archive call on incomplete initial construction returns `IndexError::IncompleteBuild` promptly. It must not accidentally wait for construction through a lock and then pretend it was accepted earlier. Calls on quiescing indexes fail with the existing cancellation/closed-state error. Concurrent archive calls serialize; the second checks the state after obtaining its transition permit and normally becomes a no-op.

Define a representation state with `Native`, `Mixed`, and `Archived`. This describes the eligible dictionary and metadata components in section 8, not whether FTS/bitmap payloads are a durable file image. Count logical components separately from unique allocation owners. Empty components must have a native representation before the first explicit archive call; do not trigger lazy creation of rkyv empty archives during startup.

`ArchiveStatus` must expose representation state and native/archived eligible component counts. `ArchiveReport` must expose at least:

- logical index revision and instance identity;
- representation generation before and after the operation;
- converted and reused component counts;
- newly written serialized bytes and aligned capacity bytes;
- total current archive serialized/capacity bytes;
- total operation wall time, transition-gate wait, preparation wall time, installation wall time;
- separately labeled serializer and validation work intervals, with worker sums identified as sums;
- worker limit and actual peak active conversion jobs.

Do not include a claimed RSS reduction or immediate deallocation count unless actually measured. Returning from `archive()` does not imply that older views have released their owners.

Do not increment logical `IndexRevision`, file versions, or record identities for a representation-only change. Maintain a separate internal representation generation for diagnostics and tests. Do not invalidate correct result, verified-predicate, or relation caches solely because archiving occurred. Any cache containing references into storage must instead own the corresponding immutable source or use stable logical identifiers.

No `IndexView::archive()` method is required: a pinned view describes an immutable publication. The owner APIs replace the current publication; they do not alter an old view in place.

## 6. Native posting layout and ownership

Use one native dictionary and one bitmap payload array from construction onward:

```text
private builder:
    BTreeMap<StoredKey, u32>   // key -> bitmap slot
    Vec<RoaringBitmap>        // canonical payload array

published native dictionary:
    Arc<NativePostingMap<StoredKey>>
    Arc<Vec<RoaringBitmap>>

published archived dictionary:
    Arc<ValidatedArchive<OrderedPostingArchive<StoredKey>>>
    the same Arc<Vec<RoaringBitmap>>
```

This is a deliberate internal ownership change. The prior `BTreeMap<Key, RoaringBitmap>` embeds bitmap owners in the map. Keeping that exact layout while retaining old readers would require bitmap copying, retaining the entire native map behind the archive, or unsafe ownership arrangements. The separate payload array lets native and archived dictionaries share the payload without any of those compromises. Measure the additional payload-array access on insertion and querying; do not assert it is free.

Create this layout in the builder itself. **Do not first build `BTreeMap<Key, RoaringBitmap>` and then rebuild it as `BTreeMap<Key, u32>` at publication or archiving.** No second tree is allowed. Use one entry lookup for insertion where the current key APIs permit it. An occupied entry identifies its existing bitmap. A vacant entry receives a checked new bitmap slot and a new payload. Preserve direct stored-key conversion from Repair II.

Bitmap slots are insertion-assigned identifiers, not sorted-key ranks and not record slots. For example, inserting beta and then alpha may produce beta -> 0 and alpha -> 1; sorted iteration then yields alpha -> 1 followed by beta -> 0. Preserve these values during archiving. Never substitute `enumerate()` ordinals for existing bitmap slots.

Remove assumptions that directory position equals bitmap ordinal from `validate_published_dictionary` and related tests. Required checks become:

1. Keys are strictly ordered under the established stored/archived comparison semantics.
2. Every dictionary bitmap slot is in bounds and associated with exactly one key.
3. The archived tree and its reverse directory agree on key and bitmap slot.
4. Every referenced bitmap satisfies record-domain/liveness eligibility invariants.
5. An unreferenced payload slot, if retained after removal of an empty posting, must contain an empty bitmap. Nonempty unreachable payloads are an error.

Do not reorder payloads just to recover sorted-rank ordinals. Normal source-free compaction produces a fresh compact payload array. Empty-map clearing releases both containers. Inference pruning must construct only the retained postings and must not keep a large obsolete payload array solely to preserve one retained bitmap.

Preserve first-batch adoption: transfer the compatible map and payload array together. For later batches, traverse the source map, take each owned bitmap from its array slot using an empty replacement, and merge or move it with the existing checked record-slot rebasing. Do not enumerate bitmap members when an offset-zero ownership transfer suffices. Do not create a vector of key/bitmap pairs to perform this merge.

Wrap completed payloads in `Arc` once. Published clones share maps and payload arrays. Avoid an `Arc` allocation per posting, per key, or per record. Avoid `Arc::make_mut` on shared published payload arrays during an archive operation. Actual mutation rebuilds may create changed payloads as before, but conversion alone must not.

## 7. Direct serialization without intermediate entry vectors

The source of a posting archive must borrow the completed native `BTreeMap<StoredKey, u32>`. Its B-tree serializer passes `map.iter()` directly to `ArchivedBTreeMap::serialize_from_ordered_iter`, or uses rkyv's ordinary `Serialize` implementation for that exact map field. The latter already uses the former internally. `rkyv::to_bytes(&borrowed_source)` remains an appropriate outer API.

Keep the iterator exact-sized. Use the map length after native completion has removed empty posting keys. Do not attach a filter that loses the exact-length contract and do not guess or fake `ExactSizeIterator`. No sort is needed for a B-tree iterator. Preserve mixed-number comparison, string/ID distinctions, date/time encodings, and structural-key ordering exactly.

The current archived ascending directory is a retained query structure for reverse traversal. Keep that functionality in this task. Write its final archived entries directly from a second traversal of the same native map using `ArchivedVec::serialize_from_iter` and a borrowed entry adapter. In rkyv 0.8.18 this requires an exact-sized, clonable iterator; cloning a B-tree iterator duplicates cursor state, not the map or its keys. The adapter archives each borrowed key as its existing archived key type and copies its u32 bitmap slot. It must not archive a Rust reference or pointer as a persistent link to the native source. Do not substitute `serialize_from_unknown_length_iter`: that API cannot handle items that write additional payload during serialization, which includes variable-length string keys.

Two iterator traversals are acceptable: one for the archived tree and one for the final reverse directory. They are not permission to materialize a native `Vec<OrderedPostingEntry<_>>`, `Vec<&Key>`, `Vec<(&Key, u32)>`, a second map, or a cloned list of strings. Keep the existing archived directory's key duplication visible in memory reporting. Removing that retained directory or implementing a different B-tree traversal is outside this task.

Distinguish these allocations precisely:

| Allocation | Allowed? | Reason |
| --- | --- | --- |
| Existing bitmap and record arrays used by the native index | Yes | Actual resident query data |
| Final aligned archive output buffer | Yes | The requested archived representation |
| Final archived reverse directory | Yes | Current descending-query representation |
| rkyv serializer resolver/allocator scratch | Yes, measured | Required serialization machinery |
| Small component-job descriptors | Yes, bounded | Scheduling metadata, not copied dictionary entries |
| Application-owned temporary vector of every map entry/key/reference | No | Prohibited conversion staging |
| Temporary second B-tree or transformed owned-key hash map | No | Repeated construction and allocation |

The requirement is not a claim that serialization performs zero allocations. Reuse scratch within a worker where supported; completed archives must own their output allocations. Do not copy a completed archive merely to recycle its output buffer. Preserve the existing evidence-based decision against unconditional shrinking unless a separate measurement demonstrates a benefit.

The current record-ID lookup starts as a `HashMap` and then sorts a borrowed-entry vector to create an archived B-tree. It is equality-only in the reviewed implementation. Replace its archived backing with rkyv's archived hash map and serialize from the existing native hash-map iterator through borrowed key/value adapters. Do not sort a new vector or build an extra B-tree solely for archiving. Preserve documented identity iteration ordering through the existing identity-order structure where required. If any caller implicitly depends on hash iteration order, correct that caller rather than establishing a new ordering contract.

Likewise, archive catalog/file/ownership hash dictionaries directly from their native maps. Use rkyv's hash-map construction APIs and their required key hashing/equality rules; never copy a native hash table's raw buckets or reuse hash values from an incompatible hasher. For native ordered path maps, use their B-tree iterators directly.

Custom adapters must use the pinned rkyv version's supported `Archive`, `Serialize`, resolver, and `Place` contracts. Keep all necessary unsafe code small and documented. Do not transmute native collections into archived collections, manufacture lifetimes, retain borrowed source addresses in archive bytes, or construct unvalidated roots. Preserve checked alignment and overflow handling.

## 8. Eligible components and native read coverage

The native startup path must not leave hidden archive prerequisites. Implement native and archived backings for every component listed here:

| Component | Native publication | Explicit archive result |
| --- | --- | --- |
| Declared value, length, structural dictionaries | Native maps plus canonical bitmap arrays | Archived dictionaries, same bitmap arrays |
| Inferred/custom structured dictionaries | Same rule | Same rule |
| Base/segment relation posting dictionaries | Native completed dictionaries | Archived completed dictionaries |
| Segment file and record rows | Owned native rows under shared immutable ownership | One checked row archive shared by file/record views |
| Segment file lookup | Native hash map | Archived hash map |
| Segment record-ID lookup | Native hash map | Archived hash map |
| Catalog metadata shards and ID lookup | Native immutable shard data | Archived shard data |
| Catalog exact/ordered paths | Native hash/B-tree maps | Archived hash/B-tree maps |
| Catalog ownership routes and lookup | Native immutable shard data | Archived shard data |

FTS bitmaps, raw/fallback/custom text indexes, sorting payloads, liveness bitmaps, and required public compatibility inventories remain shared native payloads in this task. Archive reports must identify them separately. Do not serialize their full contents merely because they are reachable from the segment root.

Retain source rows and lookup maps from the first build rather than serializing and then reconstructing them to implement native mode. Reuse canonical inventory owners and move completed owned rows where possible. Do not introduce an additional whole-repository metadata copy as a native fallback. Existing unavoidable public compatibility copies must be measured and documented.

Give file/record accessors a representation-independent borrowed view: native and archived variants expose the same ID, file, byte range, revision, status, diagnostics, memberships, and lossless path behavior. Do not initialize an entire compatibility cache to answer one status/path/record query. Native arrays can supply direct borrowed rows. Archived arrays keep their existing lazy compatibility behavior for explicit compatibility APIs.

Audit representation-specific helpers such as `archived_record_slots`, `archived_record`, and `archived_path`. Generalize internal callers that need logical data; keep raw archived access only where the caller explicitly operates on an archived component. Never unwrap an archived-only helper merely because the index is search-ready.

Preserve per-file ownership directories and their version checks. Native catalog mutations must rebuild only affected native shards, as archived catalog mutations previously rebuilt affected archived shards. Unchanged owners survive catalog rebase. No whole-catalog scan/rebuild may be introduced for a single file operation.

## 9. Publication, concurrency, and atomic failure

Use immutable publication roots and existing snapshot ownership. Do not add a lock to every dictionary lookup or introduce independently mutable representation cells inside old views for this task.

For `Index`, add a dedicated representation-transition `RwLock<()>`:

- Ordinary data-publication operations acquire a shared transition permit before capturing the publication they will stage from and retain it through commit or rollback.
- `archive()` acquires an exclusive transition permit before capturing its source publication and retains it through preparation and installation.
- Pure queries, acquiring an `IndexView`, and reading immutable metadata do not acquire this gate.
- Existing mechanisms still arbitrate ordinary writers against each other. The transition gate specifically prevents a prepared old native root from replacing a newly archived root with the same logical revision.

Apply the shared permit to all publishing writers: explicit add/update/delete/move, reconciliation, compaction, initial segment publication, final inference/metadata publication, and any internal publication helper. Acquire it inside the worker executing a task; do not move a standard-library lock guard across threads. Nested helpers receive an existing permit rather than reacquiring the gate. Define and document one lock order: transition permit, existing writer/task coordination, short publication lock. Review quiesce paths for exceptions explicitly.

Prepare source input and invoke user extension callbacks outside publication-critical locks where possible. A file prepared earlier may retain its logical expected version, but after acquiring the shared permit the staging operation must obtain the authoritative current root and revalidate it. Never install a whole replacement root assembled before acquiring the permit. Dispatch publication/completion callbacks only after releasing the shared permit and publication lock, so an application may call `archive()` from a completion callback without recursively waiting on its own transition permit. Add this reentrant completion-callback case to T22/T23 coverage.

Precheck incomplete/quiescing state before waiting for the exclusive permit, then recheck after acquisition. Do not hold the publication read/write lock while serializing, validating, allocating, waiting for worker jobs, or dropping large retired owners.

The archive algorithm is:

1. Acquire the exclusive transition permit and capture the current root.
2. Verify logical construction is complete and the index is not quiescing.
3. Enumerate unique eligible owners; reuse already archived owners. Schedule native conversion with the requested bounded worker limit. Do not duplicate conversion for shared owners reachable by several paths.
4. Borrow each native source to serialize its replacement. Validate its archive bytes and the component's semantic association with shared payloads.
5. Assemble a shallow replacement root containing the converted component owners and all unchanged semantic state. Share worker pools, source bindings, compatible caches, FTS/sort payloads, and unchanged archives; do not recreate a query pool or deep-clone the repository.
6. Check the complete candidate, including catalog/row/lookup/ownership consistency. Preserve logical revision, IDs, active masks, file versions, counts, configuration identity, and source revisions exactly.
7. Take the publication write lock briefly. Recheck quiescing and source identity, install the candidate with the next representation generation, and release the lock.
8. Drop operation-held source roots and temporary references outside the publication lock, release the transition permit, and return the report.

For `Repository::archive(&mut self)`, prepare the candidate using the same conversion implementation. Replace its storage fields only after all preparation/validation succeeds. The mutable borrow supplies exclusivity for that repository value; shared immutable segment/catalog owners held elsewhere remain valid. Do not implement repository archiving by rebuilding from source files or by creating a second query runtime.

If a component fails, cancel admission of further jobs, drain/join already started jobs, release every unpublished candidate, and return the error. The current publication, its representation generation, and its usability stay unchanged. This is not a transactional guarantee against process termination or allocator abort; do not attempt to catch undefined behavior or promise recovery from process-wide OOM.

For shutdown/quiesce, set the existing quiescing flag under its short lock, release that lock, then coordinate with the transition gate. The archive operation checks cancellation/quiescing between components and before installation. No shutdown path may wait for the transition gate while holding the publication lock. Existing queries finish against their pinned views. No detached archive workers survive return from an operation or completed shutdown.

The explicit operation may delay publishing file changes while it runs. This is an intentional bounded implementation tradeoff that must be documented and measured. It must not block query access for the duration of conversion. Concurrent mutation tests must prove this distinction.

## 10. Ownership release and memory accounting

After successful installation, the new root must not own the converted native B-tree/hash-map backing, directly or through a fallback, serializer wrapper, compatibility holder, report, or retained operation object. The archive owns its independent aligned bytes. Shared bitmap arrays retain their original owners without duplication.

An old `IndexView` legitimately owns the old native maps. The operation cannot forcibly invalidate it. Once the operation's temporary source references and the last old view are dropped, those map allocations and their native key allocations must be released. This is allocator deallocation, not a promise that RSS immediately falls; allocator caches may retain freed pages.

Do not remove compatibility data that remains part of a documented public interface. Attribute it explicitly. Test map-owner reclamation independently of compatibility cache ownership and bitmap owner retention, rather than concluding that an unchanged RSS means nothing was freed.

Atomic whole-publication installation means the native source and prepared archive buffers coexist until commit. A one-worker default limits active scratch, not the sum of all completed candidate archives. Report that peak honestly. Increasing archive workers increases possible scratch/output overlap and must be measured. Do not hide conversion memory in a process that exits before recording peak RSS.

Keep storage diagnostics correct for native-only, mixed, archived, and pinned-old-view cases. Deduplicate shared allocation owners in totals. Report native map estimates, shared payloads, archive length, archive capacity/slack, compatibility caches, and pinned obsolete owners separately where measurable. Do not count a shared bitmap once per dictionary representation.

## 11. Addition, replacement, deletion, and compaction

Preserve file-level atomicity and the established smallest-eligible-segment policy. Do not reintroduce in-place mutation of published native maps.

| Operation | Required behavior after this change |
| --- | --- |
| Add file | Build native contribution; rebuild the smallest eligible destination or add a native segment under existing limits; publish native changed components |
| Replace file | Retire old records, update file/catalog versions, build replacement natively, and publish atomically |
| Delete file | Update liveness/catalog/ownership state; retain old posting entries until normal rebuild or compaction; do not archive or rebuild every posting dictionary |
| Move file | Preserve existing identity/path semantics and update only required metadata/index components |
| Compact | Read native and archived sources through common readers, rebuild live data natively, and publish atomically without source reads |
| Archive again | Reuse untouched archived components and convert newly native components only |

If a mutation must rebuild an archived destination, read that destination and construct a new native replacement. Do not first archive native input to reuse an archived-only maintenance function. Do not thaw unrelated archived components. Unchanged archived dictionaries may remain shared inside an otherwise native/mixed segment when their semantics and record-slot domain are unchanged.

After archiving, ordinary mutation may make the current state `Mixed` or `Native`. There is no persistent 'archive mode' that forces later writes to serialize automatically. Applications decide when to invoke `archive()` again.

Retain source-free compaction, current record-slot remapping, duplicate-ID detection, multi-collection ownership, zero-record-file handling, incomplete-build restrictions, and old-view semantics. Archive itself is representation-only: it neither removes tombstones nor changes segment policy, compacts records, reparses files, or refreshes filesystem metadata.

## 12. Implementation map and client integration

Inspect and adapt these reviewed sites rather than adding a disconnected alternative path:

| File | Required work |
| --- | --- |
| `posting.rs` | Native map/bitmap ownership, immutable native backing, common cursors, direct borrowed serializers, slot-aware validation |
| `archive_storage.rs` | Keep checked immutable buffer ownership; make conversion reachable only through explicit archive preparation |
| `native_segment.rs` | Replace archive-only publication checks with completed semantic-storage checks that accept either backing |
| `mutation_segment.rs` | Native completion, native row/lookup publication, representation-independent consolidation and projection |
| `segment_archive.rs` | Native/archived rows and lookups; remove sorted record-ID staging vector; borrowed logical accessors |
| `lifecycle_catalog.rs` | Native/archived metadata/path/ownership shards and component-preserving updates/rebase |
| `lifecycle.rs` | Transition gate, archive installation, ownership release, status, unchanged logical revision |
| `lifecycle_initial.rs`, `lifecycle_open.rs` | Correct search readiness and build completion without serialization |
| `repository_incremental.rs`, `repository.rs` | Remove implicit freeze calls in startup/finalization/base relations; standalone archive API |
| `lifecycle_partition.rs`, `lifecycle_compaction.rs` | Native/mixed source-free maintenance and native outputs |
| `storage_usage.rs` | Backing counts and nonduplicated owner accounting |
| Core exports, examples, CLI/TUI/GUI call sites | API compilation, readiness behavior, absence of hidden archive calls |

Audit every `freeze_storage`, `seal_completed_storage`, `ValidatedArchive::freeze`, `freeze_source`, `all_archived`, and `is_archived` call or assertion. A normal data operation must not archive accidentally through a helper. Split semantic sealing from archive conversion with precise names; do not keep an old method name that silently performs a different expensive operation.

Remove archive-only assertions from generic mutation and query tests and replace them with representation-independent correctness assertions. Keep explicit archived-state tests in the archive suite. Assertions that all bytes are validated remain mandatory where an archived variant is constructed.

CLI/TUI/GUI build and query flows must use native publications without automatically calling `archive()`. Add a small documented library example demonstrating build, search, archive, search again, mutate, and archive again. A new graphical archive button or CLI persistence command is outside this task. Compile all existing frontends and update status consumers so native-ready indexes are not shown as still indexing merely because they are unarchived.

## 13. Required tests

Use deterministic fixtures and synchronization barriers. Do not prove concurrency with sleeps. Compare query contents, ordering, counts, evidence, pagination, identities, and errors where applicable. Reuse the existing independent model and fresh-rebuild comparison helpers.

| ID | Required proof |
| --- | --- |
| T01 | Default Repository and Index builds publish native eligible components and perform zero archive serializations/checked archive validations, including empty shells and relations |
| T02 | Progressive native publication becomes searchable before construction completes; incomplete results stay marked incomplete |
| T03 | Completed empty and zero-record-file indexes are search-ready without archiving |
| T04 | Blocking build and `wait_complete()` return usable native indexes and do not wait for an archive operation |
| T05 | Every declared/inferred/custom predicate, FTS, raw fallback, relation, sort, count and pagination path works before archiving |
| T06 | Explicit archive converts all eligible current components, and a fresh view demonstrably selects archived readers |
| T07 | Native/archived query results and independent-model results match, including exact mixed numbers, Unicode, long strings, null/absence, arrays and empty selections |
| T08 | Ascending/descending bounded ranges preserve inclusive/exclusive and unbounded semantics after direct iterator serialization |
| T09 | Insertion order differs from key order; bitmap slots survive unchanged and reverse traversal returns the right postings |
| T10 | Empty-posting removal cannot leave nonempty unreachable payloads; empty maps and slot-overflow handling are correct |
| T11 | First-batch map/bitmap adoption transfers owners; later merges preserve offsets without staging key/bitmap vectors |
| T12 | Source adapters serialize directly from maps: zero owned key clones, zero application entry-vector construction, and no second native tree |
| T13 | Borrowed serialization sources can be dropped after construction; archive access remains valid, aligned and checked |
| T14 | Malformed archived bytes, inconsistent dictionary/directory slots, and invalid metadata/lookup associations fail validation before installation |
| T15 | Long-running query pinned before archive completes correctly on native storage while a later query uses the archive |
| T16 | A query started while conversion is paused at a test barrier completes before that barrier is released |
| T17 | A recoverable failure after several prepared components leaves the original publication, generation, owners and queries unchanged; retry succeeds |
| T18 | Repeated archive without changes converts zero components and reuses archive allocation identities |
| T19 | Old native map owners survive a pinned old view and are released after the operation and old view are dropped |
| T20 | Native-to-archived conversion preserves bitmap/FTS/sort allocation ownership without deep copying payloads |
| T21 | Logical revisions, file versions, record locators and result-cache validity survive archiving |
| T22 | Concurrent archive calls are serialized safely; the second rechecks state and does not duplicate conversion |
| T23 | Initial incomplete-build and quiescing errors are prompt and do not publish partial archives |
| T24 | Quiesce/shutdown during archive drains work, cannot deadlock, and prevents late publication |
| T25 | Add a file in native-only, archived, and mixed states; queries match a fresh rebuild before and after another archive |
| T26 | Replace a file in all three states, including changed record counts and identical size/mtime; old records disappear from the current view |
| T27 | Delete a file in all three states; NOT/complements, relations and result counts exclude retired records without rearchiving unaffected dictionaries |
| T28 | Move/path update in all three states preserves required identity semantics and exact/prefix path queries |
| T29 | Mutation prepared/queued around archive cannot restore an older native root, lose a deletion, or overwrite catalog ownership |
| T30 | Smallest-eligible-destination rebuild and append-only fallback preserve segment limits and produce native changed components |
| T31 | Source-free bounded/full compaction works on native, archived and mixed inputs with source access made unavailable |
| T32 | Archive after mutation/compaction reuses untouched archived owners and converts only new native owners |
| T33 | Native and archived catalog exact/ordered path lookup preserve Unicode and lossless non-UTF-8 behavior on supported platforms |
| T34 | Catalog rebase and immediate mutations preserve ownership versions/routes without whole-catalog rebuilding |
| T35 | Healthy native and archived field/metadata queries do not initialize unrelated compatibility caches |
| T36 | Repeated build/archive/mutate/archive/drop cycles release owners and do not accumulate worker threads or scratch |
| T37 | Standalone Repository archive has the same semantic conversion/no-op/failure behavior as Index archive |
| T38 | Archive has zero source reads/parses and creates/writes no index or source files |
| T39 | Status and storage accounting distinguish current native/mixed/archived owners, shared payloads, capacity slack and pinned old views |
| T40 | CLI/TUI/GUI compile and search readiness consumers do not trigger automatic archiving |

Run the deterministic multi-seed lifecycle sequence with archive calls inserted before and after mutations and compaction. Keep duplicate-ID, file-conflict, raw-fallback, cancellation, multi-collection, and large-file coverage from Repair II.

Use test-only weak-owner probes and scoped counters to prove deallocation and absence of staging. Corroborate them with an allocation profile; a counter placed only in the old implementation is not proof that a replacement allocation path is absent. Count key clones separately from archive key bytes, which must be written. Allow required rkyv scratch and final buffers.

Required final checks include the existing core/workspace suites, compaction API integration tests, all-target frontend checks, detailed-profiling build, release benchmark examples, formatting and diff checks. Use the repository's actual package names and documented commands. Run focused tests during development and the complete required checks on the final combined implementation. Do not claim unexecuted checks passed.

## 14. Benchmark protocol

Benchmark native construction and explicit archive conversion as separate operations. The user is no longer asking the combined operation to beat native-only construction.

Use exact executable revisions for original native N, Repair II R, and this implementation F. Record machine/CPU, available workers, allocator, toolchain, lockfile hash, corpus, schema, segment policy, query workers, cache settings, and commands. Preserve corpus contents. Warm source-page cache consistently and use a fresh process per construction sample.

### 14.1 Native startup

Measure F with no archive call. Record discovery, first searchable publication, full native-ready time, first representative query, startup end RSS, peak RSS, source reads/parses, and archive-generation counters. Required archive generation before an explicit call is zero.

Run ASCII and Unicode with 1, 8 and 32 build workers where supported. Use seven alternating paired N/F samples per configuration. Retain matched R samples for context. Record both public entry points: if Repository and blocking Index share a core construction implementation, say so and measure the adapter separately rather than pretending there are independent algorithms. Also measure the progressive API's first-publication event directly.

The target is native-ready time at or below the original native baseline. Report paired median ratios and 95% paired-bootstrap intervals. Investigate a measured slowdown exceeding 3% and 10 ms. If parity is not attained, report the actual gap and measured cause; do not claim it by excluding necessary native finalization or changing the definition of ready. Do not reintroduce archive work into startup to improve a different benchmark.

Isolate the native key-to-bitmap-slot ownership change in a commit and compare insertion/query costs, repeated and unique keys, low/high cardinality and long strings. Its purpose is safe ownership sharing; its performance must be measured rather than assumed.

### 14.2 Explicit archive

For each archive sample, build a fresh F native index, record native-ready time, start a separate archive timer, call the public API, then verify representative queries and storage state. Record preparation/serialization/validation/installation intervals, process CPU time, newly generated bytes, capacity, scratch, end RSS, and peak RSS across the complete build-plus-archive process. Report `native build + archive` as an additional total, never as native startup time.

Run archive worker limits 1, 8 and 32 on a fixed 32-worker native build for both alphabets, with five fresh-process samples per archive setting. Retain the default one-worker result prominently. Do not present summed worker intervals as wall time. Capture an allocation profile in separate instrumented runs.

Add measured cases for repeated no-op archive, archive after one addition/replacement/deletion, archive after compaction, and archive while an old native view is pinned. Measure memory again after releasing the old view; distinguish live allocated bytes from allocator-retained RSS. Report the transient overlap required by atomic conversion.

Directly demonstrate that no application entry-vector staging or second native tree is allocated during dictionary conversion. Compare archive operation time against Repair II's equivalent finalization work only when the measured scope is comparable. The combined operation may remain slower than native-only building; that is expected work, not a failed startup measurement.

### 14.3 Query and lifecycle behavior

Run the full 150-case suite on native, fully archived, and mixed publications through direct and progressive query paths, for both alphabets. Semantic mismatches are failures. Measure cold first query separately from warm queries. Keep verified-cache/query-worker settings identical across comparisons.

Measure queries during an explicitly running archive operation as well. The publication lock must not cause archive-length query stalls, but CPU/cache/memory-bandwidth contention must still be reported. Use at least five repetitions for suite timing and ten for Q001/Q119 and any other material outlier. Investigate aggregate regressions over 3%, or individual regressions over 10% and 0.5 ms, against the appropriate baseline. Existing Repair II regressions must remain visible; do not declare them fixed merely because native and archived results match.

Rerun addition, replacement, deletion and bounded/full compaction timing on native and archived inputs. Correctness tests do not substitute for mutation benchmarks. Record time spent waiting for an archive transition separately from normal mutation work.

Do not weaken semantic validation, change segment limits or inference admission, omit index families, warm all compatibility caches, or retain duplicate native indexes to achieve favorable timings.

### 14.4 Evidence files

Retain `revisions.json`, `native-build-samples.tsv`, `native-build-summary.tsv`, `archive-samples.tsv`, `archive-summary.tsv`, `query-results/`, `lifecycle-results/`, `allocation-summary.tsv`, `ownership-report.md`, and `implementation-report.md` under the task evidence directory. Preserve raw samples and exact reproduction commands. Report passed, failed, unmeasured, and inconclusive separately.

## 15. Implementation phases and acceptance

| Phase | Scope | Checkpoint |
| --- | --- | --- |
| 1 | Record base and reproduce Repair II evidence; add tests/counters for native startup and explicit conversion | Baselines and failing contract tests |
| 2 | Native posting map/bitmap ownership and common cursors; direct map serialization and reverse-directory streaming | T07–T14, T20; isolated allocation/timing comparison |
| 3 | Native row/lookup/catalog/ownership backings and common readers | T05, T33–T35; no hidden archive dependency |
| 4 | Semantic native sealing and startup publication; readiness and base relations | T01–T05, frontend compile |
| 5 | Shared conversion engine, explicit APIs, transition gate and atomic installation | T06, T15–T24, T37–T39 |
| 6 | Native/archived/mixed mutations and source-free compaction | T25–T32; full lifecycle model |
| 7 | Ownership release, no-op conversion, diagnostics, client example | T18–T21, T36, T40 |
| 8 | Full validation and separate build/archive/query/mutation benchmarks | Evidence and honest performance assessment |
| 9 | Final review, task completion report, commit and push | One integration branch; user performs merge |

Important choices above are fixed. Do not substitute automatic background archiving, partial per-component publication, a permanent native fallback, an alternate disk database, a hash accumulator for ordered fields, a new reverse-traversal algorithm, or a startup mode matrix. Do not add a dependency merely to avoid using the existing synchronization and rkyv APIs.

Functional acceptance requires native search-ready publication, a successful explicit archive switch, equivalent queries and mutations across representations, no prohibited conversion staging, atomic failure, proper owner release, and source-free/no-write operation. Performance acceptance is reported independently against the measured targets. If a required safeguard fails, preserve useful tested commits and report the blocking finding; do not silently mark the task fully accepted.

The final implementation report must map every T01–T40 test to executed evidence, list public API changes, describe native/archived/shared ownership, enumerate all residual native payload categories, record supported concurrency behavior, and give separate startup/archive/query/mutation/memory verdicts. Follow `docs/ai/README.md` for task-document completion, pushing and subordinate-worktree cleanup. Keep the integration worktree and branch for review; never merge master automatically.

## 16. Review anchors

- [Repair II report](https://github.com/Simon-Martens/objekte/blob/dc371a42e666ef6c4d3c0519d6653326d0ad6d56/docs/ai/generated/rkyv-construction-performance/implementation-report.md)
- [Repair II construction summary](https://github.com/Simon-Martens/objekte/blob/dc371a42e666ef6c4d3c0519d6653326d0ad6d56/docs/ai/generated/rkyv-construction-performance/build-summary.tsv)
- [Repair II query summary](https://github.com/Simon-Martens/objekte/blob/dc371a42e666ef6c4d3c0519d6653326d0ad6d56/docs/ai/generated/rkyv-construction-performance/query-summary.json)
- [Posting sources, conversion and cursors](https://github.com/Simon-Martens/objekte/blob/dc371a42e666ef6c4d3c0519d6653326d0ad6d56/crates/core/src/posting.rs)
- [Current archive-only segment admission](https://github.com/Simon-Martens/objekte/blob/dc371a42e666ef6c4d3c0519d6653326d0ad6d56/crates/core/src/native_segment.rs)
- [Initial readiness and completion](https://github.com/Simon-Martens/objekte/blob/dc371a42e666ef6c4d3c0519d6653326d0ad6d56/crates/core/src/lifecycle_initial.rs)
- [Lifecycle publication and file replacement](https://github.com/Simon-Martens/objekte/blob/dc371a42e666ef6c4d3c0519d6653326d0ad6d56/crates/core/src/lifecycle.rs)
- [Catalog storage](https://github.com/Simon-Martens/objekte/blob/dc371a42e666ef6c4d3c0519d6653326d0ad6d56/crates/core/src/lifecycle_catalog.rs)
- [Segment metadata and identity lookup](https://github.com/Simon-Martens/objekte/blob/dc371a42e666ef6c4d3c0519d6653326d0ad6d56/crates/core/src/segment_archive.rs)
- [rkyv 0.8.18 BTreeMap serialization implementation](https://docs.rs/rkyv/0.8.18/src/rkyv/impls/alloc/collections/btree_map.rs.html)
- [rkyv archived B-tree construction API](https://docs.rs/rkyv/0.8.18/rkyv/collections/btree_map/struct.ArchivedBTreeMap.html)
- [rkyv archived vector iterator serialization](https://docs.rs/rkyv/0.8.18/rkyv/vec/struct.ArchivedVec.html#method.serialize_from_iter)

The implementing agent must inspect the actual local base and pinned dependency APIs rather than relying on line numbers or changing the lockfile to match an example. This document specifies the implementation to build; it is not evidence that the new implementation or its tests have already been executed.

## Implementation report

Task: `native-publication-explicit-archiving`

Completion date: 2026-11-17

Base revision: `dc371a42e666ef6c4d3c0519d6653326d0ad6d56`

Integration branch: `integration/native-publication-explicit-archiving`

Implementation commits:

- `92b8e59ae7` — native publication, explicit archive APIs, native/archived
  records and dictionaries, transition gating, readiness, and focused tests
- `cd9b8275cd` — direct borrowed catalog/row serialization, cache-free archive
  validation, explicit archive benchmark controls, and final lifecycle coverage
- `1d3c7be663` — transactional standalone Repository archive failure/retry
- `b7f1f0d36a` — desktop incremental-open test accepts a construction task
  that has already completed natively

### Implemented scope

Native startup is now the only implicit construction policy. Published native
segments and the standalone repository are queryable immediately after semantic
construction and validation. No startup path serializes an archive or waits for
archive conversion.

`Index` exposes `archive()`, `archive_with_options()`, and `archive_status()`.
`Repository` exposes the corresponding mutable-owner APIs. Archive options are
validated and default to one worker. The archive operation:

- acquires an exclusive representation-transition permit;
- rechecks construction and quiescing state;
- prepares a private shallow replacement publication;
- directly serializes borrowed posting maps, lookup hash maps, catalog
  hash/B-tree maps, and metadata/ownership shards;
- validates every newly constructed archive;
- checks row, lookup, dictionary, and ownership associations;
- installs the replacement atomically under the short publication lock; and
- leaves the logical revision and IDs unchanged.

Pinned pre-archive views retain their native owners. Once those views and the
operation's temporary references are dropped, the replaced native maps are
releasable. Shared bitmap arrays, text payloads, sorting payloads, and liveness
owners are not deep-copied merely to archive dictionaries.

Native and archived readers cover declared, inferred, custom, structural,
length, relation, file-row, record-row, path, and ownership paths. Mutations
from native, archived, and mixed roots publish native changed components;
untouched archived components retain their owners. Source-free compaction uses
the common readers and produces native compacted segments.

### Tests and validation

Required/focused tests executed:

- `cargo test -p objekte-search-core --lib`: 592 passed, 17 ignored, 0 failed
- `cargo test --workspace`: passed, including CLI, desktop, integration, and
  compaction API tests
- `cargo check --workspace --all-targets`: passed
- `cargo test -p objekte-search-core --test compaction_api`: passed
- `cargo check -p objekte-search-core --all-targets --features detailed-profiling`:
  passed
- `cargo clippy --workspace --all-targets`: completed successfully with the
  repository's existing advisory warnings
- `cargo run --release -p objekte-search-core --example explicit_archive -- <root>`:
  passed native → archive → search → mutate → archive → search
- `git diff --check`: passed

The T01–T40 evidence mapping is recorded in
`docs/ai/generated/native-publication-explicit-archiving/implementation-report.md`.
Focused coverage includes native startup counters, progressive readiness,
incomplete publication rejection, concurrent archive calls, paused archive
queries, quiesce during archive, weak-owner release, no-op allocation reuse,
direct serialization counters, payload owner identity, atomic failure, mixed
mutations, deletion, source-free compaction, and non-UTF-8 catalog paths.

### Benchmark results

The user authorized a smaller benchmark, so the evidence uses one repetition
per case rather than the full seven/five-sample protocol. The benchmark
artifacts identify the exact executable revision and preserve raw samples in
the generated evidence directory.

Fixed Musenalm 150-case run:

| Path | Native | Archived | Archived/native |
| --- | ---: | ---: | ---: |
| ASCII | 601.567 ms | 597.728 ms | 0.99362x |
| Unicode | 601.152 ms | 603.509 ms | 1.00392x |
| Both | 1202.719 ms | 1201.237 ms | 0.99877x |

The progressive path produced the same 300 semantic result digests and
measured 0.97307x native for ASCII, 1.05243x for Unicode, and 1.01185x over
both. The archived cold Q001 remains a visible one-time cost; warm Q001 and
Q119 medians are reported separately in `query-summary.json`.

Native startup measured 745.907 ms ASCII and 805.718 ms Unicode with zero
archive bytes. Explicit archive at one worker measured 175.230 ms ASCII and
149.572 ms Unicode, producing 149,111,376 serialized bytes and 233,891,392
allocated bytes (84,780,016 bytes slack). Eight-worker and thirty-two-worker
runs did not consistently improve wall time; the measured scheduler peak was
seven active conversion jobs for this segment topology.

### Deviations and limitations

- The requested smaller benchmark was used instead of the full repeated
  protocol. This is explicitly recorded and no full-run claim is made.
- Relation adjacency payloads remain native because they are outside the
  archive representation defined by this task.
- Archived cold compatibility-row materialization is a measured residual cost;
  warm reusable queries remain comparable.

### Cleanup status

The integration worktree remains in
`/home/simon/source/objekte-integration-native-publication-explicit-archiving`
for review. No subordinate worktree was created for this task. Temporary
`docs/ai/memory/native-publication-explicit-archiving/` notes are removed after
the completion commit.
