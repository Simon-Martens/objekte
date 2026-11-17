# Objekte rkyv in-memory index repair specification

## 1. Assignment, reference revisions and acceptance

Repair the existing rkyv prototype. Complete the implementation, regression tests, memory investigation, performance comparisons and final report in a separate integration worktree. This is an implementation assignment, not a request for another design proposal.

Repository: Simon-Martens/objekte.

Reviewed prototype: **241247ec0f83aed50debd295d174d1d4e96f51c9**, branch **integration/rkyv-memory-index-prototype**.

Native baseline: **2e18871c0bcfb9cf9335d9e73525c64f91c960cf**.

This specification incorporates the correctness and efficiency review and the subsequent memory review. It supplements the original task at docs/ai/done/objekte_rkyv_memory_index_prototype_specification.md. Read that task as well. Where instructions differ, this repair specification takes precedence. In particular, the original statement that memory optimization was outside the acceptance gate no longer applies to the concrete memory repairs required here.

The review was based on source inspection and committed benchmark data. The reviewer did not execute Rust tests. The local agent must reproduce the defects with the regressions below, implement the repairs, and execute the required checks. The previous report's “correctness-complete” and “ready for review and merge” conclusions are not sufficient acceptance evidence.

Completion requires:

1. Published dictionaries remain archived after initial construction, inference pruning, additions, replacements, deletions, verified maintenance and both compaction modes.
2. Compaction never reactivates retired records and respects both record and file limits.
3. A one-file mutation does not scan the full catalog or probe all segment payloads to invalidate that file.
4. Ownership routes remain correct for every survivor moved by a rebuild.
5. Entirely obsolete segments do not accumulate under repeated updates.
6. Avoidable metadata reconstruction, key copying and duplicate consolidation are removed.
7. Memory measurements cover the actual published representation, distinguish serialized length from allocation capacity, and identify retained duplicate structures.
8. The memory repairs in sections 9–11 are implemented and measured. A documented unresolved memory target is an explicit limitation, not a passed requirement.
9. Tests compare mutable lifecycle results against fresh reconstruction and an independent logical model, and also check internal representation and work bounds.
10. One integration branch is committed and pushed for the user to merge. Do not merge into master.

Keep the prototype entirely in memory. Do not implement saving, loading, mmap, spilling, synchronization, an archive file format, crash recovery or a mutable disk database. Source-file reads for initial indexing and changed-file preparation remain necessary. Destination reconstruction and compaction must remain source-free.

Keep rkyv pinned to 0.8.18 with the existing format features. Do not change numeric semantics, FTS semantics, collation, inference policy, query syntax, public file-handle semantics or the default 16,384-record/16,384-file segment policy to improve measurements. Native Roaring payloads, bigram indexes, sort arrays and relation adjacency remain within the original permitted scope.

## 2. Worktree, branch and delivery procedure

Read AGENTS.md and all of docs/ai/README.md first. Inspect status, configured remotes and existing worktrees. Preserve all user edits and unrelated untracked files.

Use:

| Purpose | Name or location |
| --- | --- |
| Task identifier | rkyv-memory-index-repair |
| Integration branch | integration/rkyv-memory-index-repair |
| Integration worktree | /home/simon/source/objekte-integration-rkyv-memory-index-repair |
| Task document | docs/ai/tasks/objekte_rkyv_memory_index_repair_specification.md |
| Evidence directory | docs/ai/generated/rkyv-memory-index-repair/ |
| Completed document and appended report | docs/ai/done/objekte_rkyv_memory_index_repair_specification.md |

If the original checkout is elsewhere, use the corresponding sibling worktree location. Do not use /tmp for worktrees. Leave the original checkout on master and leave the existing prototype worktree intact.

Fetch the configured remote. Start the repair branch from the current prototype branch after verifying that the reviewed prototype commit is its ancestor. If the branch advanced, inspect and record the intervening changes; do not overwrite them or assume the findings are already fixed. If the repair branch exists, inspect and resume it instead of resetting it. Do not automatically merge unrelated master changes into this experiment.

Use detached sibling baseline worktrees at the two reference revisions for comparisons, with separate target directories. Do not reuse an executable built for another revision. Preserve baseline evidence.

One supervising agent owns the integration worktree. If local subagents are available, delegate regression fixtures and benchmark/report processing only after the storage and instrumentation interfaces are fixed. Give each delegate a separate sibling worktree and task/rkyv-memory-index-repair/<subtask> branch. Do not assign simultaneous overlapping lifecycle/storage edits. The supervisor integrates and validates their commits. A single agent may execute all phases if delegation is unavailable.

Commit at the phase boundaries in section 15 and push immediately after every commit. Never force-push. Follow the repository policy for initially untracked task documents; do not commit unrelated instructions or temporary notes. At completion, move the task to done only if mandatory correctness requirements pass, append the report, commit and push. Leave the integration worktree available. Do not begin other tasks.

## 3. Confirmed findings and implementation map

Line references identify the reviewed revision and may move. Read the complete functions before editing.

| ID | Finding | Primary implementation location |
| --- | --- | --- |
| R01 | Bounded compaction can pass dirty segments to a merge which marks all stored slots active | lifecycle_compaction.rs: merge_small_segments, compact_with_control_options |
| R02 | Consolidation, compaction projection and inference pruning can publish native posting builders | mutation_segment.rs: consolidate, compact_with_mapping; lifecycle.rs: finish_initial_construction |
| R03 | Rebuilding one path bucket scans every metadata shard and initializes unrelated caches | lifecycle_catalog.rs: rebuild_path_buckets |
| R04 | The reported sharded file ownership directory is absent; invalidation probes every segment | lifecycle.rs: PublishedIndex, retire_file_records |
| R05 | Repeated replacement can retain fully obsolete segments; destination selection scans file payloads | lifecycle.rs: place_mutation_segments, deletion staging |
| R06 | Merge selection constructs and discards a complete result, then constructs it again | lifecycle_compaction.rs: merge_small_segments |
| R07 | Merge selection ignores max_files; bounded merging ignores the shared work budget | lifecycle_compaction.rs |
| R08 | Key iteration makes unnecessary owned conversions; min/max statistics traverse all keys | posting.rs: public_from_archived, for_each, iter, finish_stats |
| R09 | Segment metadata is archived and immediately reconstructed into native caches | segment_archive.rs: SegmentRows::freeze |
| R10 | Published metadata/path duplication and archive spare capacity are not adequately accounted for | inventory.rs, lifecycle_catalog.rs, segment_archive.rs, archive_storage.rs |
| R11 | Repository archive accounting omits segments; archive length is called allocation size | repository.rs: rkyv_archive_bytes; archive_storage.rs: len |
| R12 | Accounting can double-count shared storage, perform repeated searches and allocate while observing | native_segment.rs: recompute_accounting; lifecycle_catalog.rs: archive_bytes |
| R13 | Generated mutation tests never enable caching, omit bounded compaction and do not cover the required worker matrix | rkyv_prototype.test.rs |
| R14 | The benchmark/report overstates what was validated and does not isolate the memory increase | committed prototype report and generated evidence |

Also correct small adjacent reporting defects encountered in these paths: the transaction-specific destination_rebuilt flag must reset for every operation, segment descriptors must use the new segment ID after a rewrite, and compaction reports must count work actually performed rather than all dirty input segments.

Do not expand this into a general rewrite of query execution. Extend the existing interfaces and source-free projection machinery.

## 4. Publication and archive invariants — R02

### 4.1 One completed-storage boundary

Add one internal finalization function, named seal_completed_storage, for an IndexedCollectionSegment. It must:

1. Complete semantic statistics/capabilities before freezing.
2. Freeze declared posting values, lengths and structural dictionaries.
3. Freeze custom and inferred posting dictionaries, including any stored reverse-relation dictionaries implemented through PostingIndex.
4. Freeze record/file metadata and ID lookup components only if they are new or changed.
5. Validate new archive bytes and semantic cross-component invariants.
6. Calculate component accounting without reconstructing native metadata or keys.
7. Return a segment whose completed storage passes the archived-representation predicate.

Unchanged archived components are shared by Arc. Calling the boundary on an already sealed unchanged component must not serialize or validate its bytes again.

Keep the existing segment type unless a small builder type improves ownership. A second query representation is not permitted. Restrict production PublishedSegment creation to a checked constructor which verifies component states and assigns consistent IDs. That constructor does not perform serialization: expensive work must already have occurred in the preparation worker or maintenance preparation phase. The publication write lock continues to perform only revision checks and the root swap.

Use a cheap state predicate over component descriptors, not a fresh traversal of dictionary contents, to reject native builders at the publication boundary. Include a regression that deliberately tries to publish one native builder and receives an error before the root changes.

### 4.2 Required call-site audit

Audit and route all of the following through the boundary:

- SegmentAdmission finalization and progressive construction.
- Ordinary initial Repository construction and direct Index construction.
- consolidate and compact_with_mapping, including partition projection.
- Smallest-destination rebuilding after add/update.
- Verified update and verified physical removal.
- Ordinary deletion paths that perform maintenance.
- Full and bounded compaction, with and without merging.
- Final global inference pruning and file-status refresh.
- Existing opportunistic maintenance that remains reachable.

In finish_initial_construction, thawing an inferred dictionary must be followed by refreshed statistics and freezing before publication. Do not leave an empty values builder or a native lengths/structural dictionary behind merely because one capability was rejected. Archive assertions must inspect inferred/custom components, not only declared fields.

In consolidate, builders created by thaw_storage must be frozen before returning the completed segment. In compact_with_mapping and scatter projection, freshly created builders must be frozen after projection. Both are currently missed.

### 4.3 Safety and atomicity

Preserve the private checked-once archive owner and its immutable aligned allocation. Do not expose writable archived bytes or manufacture static references. Buffer shrinking or copying occurs before checked validation and before any archived borrow escapes. An old publication continues to own its archive until the last reader drops it.

Preparation, archive or semantic validation failures publish nothing. Test a genuine finalization failure through an injected test-only error at the archive boundary; a parser failure alone does not exercise this path. Preserve worker draining and deterministic conflict/cancellation behavior.

## 5. Correct and bounded compaction — R01, R06, R07

Keep the existing public API:

    index.compact()?;
    index.compact_with_options(CompactionOptions {
        mode: CompactionMode::Full,
        merge_small_segments: true,
    })?;

The default remains bounded compaction with merging disabled. Define all four mode/merge combinations:

| Mode | merge_small_segments | Required behavior |
| --- | --- | --- |
| Bounded | false | Reclaim a finite batch of dirty segments; report remaining retired slots |
| Bounded | true | Use the same finite budget for reclamation and eligible clean-segment merges |
| Full | false | Remove all retired slots in the captured publication without merging clean segments |
| Full | true | Remove all retired slots, then merge eligible underfilled clean segments within policy |

A clean publication with merging requested is not automatically a no-op. Bounded merging must work on clean small segments as well. A second full merge call with no intervening change must be a no-op: unchanged revision, unchanged component owners and zero reconstructions.

### 5.1 Never merge dirty inputs as fully active

For this repair, select only clean segments for the merge phase: active.len() must equal record_count(). The preceding reclamation phase converts selected dirty segments to dense clean segments. Dirty segments left by the bounded budget are retained with their original active masks and are excluded from merging.

Do not pass dirty inputs to consolidate and subsequently call ActiveRecords::all on their complete record counts. If a helper is used elsewhere to combine dirty storage, it must translate masks explicitly; that helper is not the clean compaction merge path.

Before final publication, verify the touched group's live-record total is unchanged by compaction. Full mode must have zero retired slots. Bounded mode must report the actual residual count from the next publication. A retired file must not reappear in an active mask even when its source revision timestamp/length matches a replacement.

### 5.2 One work budget

Use one CompactionWork accumulator for cleaning and merging. Keep the existing default budgets: 32,768 input records, 64 MiB estimated input bytes, 128 MiB estimated temporary bytes and one second elapsed.

Charge merge inputs before starting consolidation. If cleaning exhausted the budget, perform no merges in that call. Charge a segment again if its reconstructed output is subsequently read as input to another merge; those are real repeated visits. Do not exclude failed materialization or repeated passes from work counters.

The existing progress exception permits one indivisible dirty segment to exceed a budget when otherwise no progress is possible. Do not generalize that exception to an arbitrary merge of two clean segments. Full mode is the explicit option for unrestricted work.

Check cancellation between candidate planning, each projection, each consolidation and archive finalization. Long projection loops retain their existing cancellation checks. A cancellation discards the staged root and returns only after any submitted work is drained.

### 5.3 Cheap merge planning and file limits

Separate compatibility checking from reconstruction. Candidate checks use descriptors, counts and file/ID metadata; they must not clone bitmaps, rebuild sorting, allocate an archive, or run consolidate.

Use deterministic ordering by live record count, then SegmentId. Enforce both target_records and max_files. The file count includes current zero-record file contributions.

For this repair, overlapping FileIds make two segments ineligible for consolidation, even if the file has disjoint record ranges in both. This preserves the existing consolidate contract; do not silently deduplicate file rows or discard records. Report overlap as a planning skip. Same-incarnation multi-part file coalescing is not required.

For sorted candidate sizes, stop a forward search as soon as its combined record count exceeds the target; later larger candidates cannot fit. If the two smallest clean segments already exceed the target, stop merging the collection. Do not execute quadratic full consolidations merely to discover that nothing fits. Metadata candidate probes may still be necessary for incompatible file sets; count those separately.

Construct each selected output exactly once. Keep the constructed result; never use consolidate(...).is_ok() as a probe. A construction error is a real failure and leaves the publication unchanged. Do not swallow archive or invariant errors as “incompatible pair.”

Reuse the common source-free projection/finalization path, assign a fresh SegmentId, update ownership for all survivors, and preserve all posting/text/sort/relation/diagnostic data.

### 5.4 Reporting

Calculate segments_rewritten from segments actually reconstructed. Do not report every dirty input segment as rewritten when a bounded call stopped early. Record removed obsolete segments, selected merges, produced segments and retained dirty segments explicitly.

Account archive bytes and capacities consistently before and after. A no-op must not deserialize all metadata or perform a quadratic archive-owner search solely to produce a report.

## 6. Catalog changes proportional to affected shards — R03

Retain contiguous 1,024-FileId metadata shards and 1,024 hash-selected path shards. Preserve lossless paths, current path validation and ordered query semantics.

Replace rebuild_path_buckets with the following transaction algorithm:

1. Identify old and new canonical query paths for the changed file.
2. If both are identical, do not rebuild any path shard for a content-only replacement.
3. Compute the one or two affected bucket numbers.
4. For each affected bucket, iterate its existing archived ordered map once into a temporary builder. Do not obtain its entries by scanning metadata shards.
5. Remove the old (path, FileId) mapping with identity checks. Insert the new mapping with duplicate-path checks.
6. Build the exact and ordered archived lookups from the same resulting entry set.
7. Freeze the bucket once and replace its Arc in the staged directory.

When old and new paths hash to the same bucket, combine both changes before rebuilding. Deletion only removes the old mapping. Addition only inserts the new mapping. Untouched path shards retain pointer identity.

Metadata changes materialize only the affected metadata shard. Batch multiple edits to the same shard, including maintenance-related changes, so that it is rebuilt once per transaction. Ownership changes are separate and do not rewrite path strings or diagnostics.

For changed metadata rows, avoid the current stored-row -> native FileEntry -> stored-row round trip. Introduce a direct storage-row builder conversion from an archived row, and apply the edited row once. Explicitly owned strings are permitted for this bounded rebuild; unrelated caches must remain untouched.

Ordered path range queries must borrow archived strings while merging results across shards. Do not allocate an owned String for each candidate merely to sort FileIds. Retain the examination-limit contract: if the optimization budget is exceeded, return “optimization unavailable,” never a truncated set presented as complete.

Introduce test counters for metadata rows visited, path entries visited, cache initializations and shards rebuilt. Counters must prove locality; checking that untouched Arc owners survive does not prove the preparation avoided scanning them.

## 7. Archived file ownership directory — R04

### 7.1 Representation

Implement the missing independently sharded ownership directory, keyed by contiguous 1,024-FileId ranges. Each archived row associates an active FileId and its captured FileVersion with sorted segment routes.

Use a route containing:

    collection: CollectionId
    segment: SegmentId
    file_row_ordinal: u32

The segment's archived file row already contains its sorted local-slot list. A route points to that list; do not duplicate every local slot in the ownership directory. This is an intentional refinement of the original ownership layout to reduce memory. Validate that the referenced row has the expected FileId and collection.

The captured FileVersion is a consistency tag, not a second mutable version authority. FileVersions remains authoritative. Source size/mtime equality is not an incarnation check.

Rows include routes for current zero-record file contributions when those contributions carry required collection/diagnostic state. Absence of records does not make a current file disappear from the catalog.

Maintain a small native per-collection SegmentId-to-descriptor-position directory for routing. It is segment control metadata, not a native record or file catalog. Do not assume SegmentId equals vector position. Rebuild a modified collection's descriptor directory once after structural changes, and share unaffected collection directories. An O(segment-count) descriptor update is permitted and measured; probing all segment payloads to find a file is not.

### 7.2 Invalidating one file

Read that file's ownership row. Check its incarnation. Visit only its routed segments, read the referenced file-slot lists and retire those slots in the staged ActiveRecords masks. Remove the old routes in the same staged transaction.

Do not scan all segments, dictionaries or record rows to find ownership. Do not open any source file during delete. If a route is inconsistent, fail preparation with an invariant error; do not silently repair it by a full scan that hides the defect.

Change catalog metadata, FileVersions, ownership and active masks atomically in the same next root. Old views retain their own directory and masks.

### 7.3 Rebuild and relocation

For every replaced or merged segment:

1. Remove routes referencing its old SegmentId.
2. Enumerate the output segment's archived file rows.
3. Add routes for every surviving file, including files unrelated to the triggering mutation.
4. Batch updates by ownership shard and rebuild each touched shard once.
5. Update the segment-position directory and dirty-segment bookkeeping.

Initial construction must aggregate ownership changes from completed segments in each atomic publication unit. It must not serialize an ownership shard once per record. Multiple completed contributions in the same publication affecting one shard are combined.

### 7.4 Invariants

Every active record is reachable through exactly one current file route for its segment-qualified address. A FileId can have several routes, including multiple segments of one collection. Equal local slot numbers in different segments are distinct.

Every route resolves to the current publication's segment descriptor and the correct file row. Rewritten segment IDs are absent from the new routes. A deleted file has no active routes. Unchanged ownership shards retain Arc identity.

Add a reusable test-only validator for these properties and call it after every operation in the differential suite.

## 8. Mutation placement and obsolete-segment removal — R05

Keep addition and replacement atomic. Prepare the changed file completely before touching published state. For each affected collection, rebuild at most one existing eligible destination, selected by smallest prospective live record count, then SegmentId. Remaining incoming contributions become bounded new segments.

Use descriptor-level live-record and current-file counts for selection. Do not scan every candidate's file records to compute these counts on every mutation. Maintain counts when liveness/routes change, and verify them against a slow recomputation in tests.

When an update invalidates all records in a segment, remove that segment if no current ownership route requires it. A segment with a genuine current zero-record contribution must be handled explicitly; retain that contribution or migrate its required state before removing the descriptor. Do not decide currentness by source size/mtime alone.

Updating the only one-record file in a collection repeatedly must leave one live segment, with obsolete segment owners held only by deliberately pinned old views. It must not append an ever-growing chain of dead segments.

Use the ownership directory to remove all-dead segments cheaply. Do not rebuild every partially dirty segment on deletion. Remove or bypass the old make_room_for_segment behavior from ordinary mutation paths where it would add unrelated coalescing/reclamation to this policy. Explicit compaction handles remaining dirty payloads.

Reset destination_rebuilt in each staged mutation. It must describe that operation, not an earlier operation copied through PublishedIndex::clone. Preserve existing verified strategies in mutation results.

When assigning a fresh segment ID, update PublishedSegment.id and the underlying IndexedCollectionSegment.id together before generating component descriptors or ownership routes. Validate their agreement.

## 9. Borrowed dictionary operations and finalization cost — R08

### 9.1 Eliminate double conversion

Replace archived scalar -> owned StoredScalarKey -> owned ScalarKey with a direct conversion to ScalarKey when ownership is actually required. A returned string key needs one allocation, not two. Preserve exact numeric variants, datetime offsets, scalar variant order and mixed-number comparison semantics.

Add borrowed key views for internal inspection. A borrowed scalar view contains borrowed strings/decimal digits and primitive numeric/date/time values. Do not materialize an owned key when validation, membership statistics or accounting only needs the bitmap or key length.

Provide dedicated bitmap iteration and borrowed key/bitmap visitation. Implement builder and archived paths without collecting a temporary Vec. Remove or change the current iter() adapter that eagerly clones every bitmap before returning an iterator; production code must not accidentally pay a complete dictionary copy for iteration.

Public value enumeration may allocate the keys actually returned to its caller. Predicate evaluation, estimation, broad relation lookup and storage inspection must use borrowed data where the operation does not return owned values.

### 9.2 Statistics and validation

Implement direct first/last-key access. finish_stats obtains min/max from the first and last builder entries or first/last reverse-directory entries. Do not traverse and clone every key to compute two endpoints.

Preserve structural, domain and semantic validation. Improve the semantic B-tree/reverse-directory check by traversing the tree and directory in lockstep, checking key equality, strict order, dense ordinals and nonempty bitmap references. Do not perform a new logarithmic tree lookup for every already ordered directory entry.

The checked rkyv validation remains mandatory once for each newly frozen archive. Repeated queries must not validate or deserialize the component.

The existing ascending key/ordinal vector for descending traversal remains supported. Do not replace forward/equality B-tree access with a sorted-array-only implementation to make the experiment cheaper. Report its duplicate-key storage separately.

### 9.3 Construction placement

Archive finalization belongs inside the existing segment worker jobs. Preserve the scheduler's bounded outstanding window and private framed-file publication groups. Avoid a new serial post-processing pass over all segment dictionaries.

Finalize independent catalog shards through the existing pool when constructing the initial catalog. Reuse completed unchanged shards across initial/final metadata publications where contents agree. Share empty archive instances by component type, rather than independently serializing 1,024 identical empty path buckets for an empty catalog.

Record wall time and accumulated worker time separately for key conversion, serialization, checked validation, semantic validation, metadata construction, accounting and coordinator publication. Do not add overlapping worker durations and call the sum elapsed build time.

## 10. Remove avoidable retained metadata — R09, R10

### 10.1 Segment metadata

SegmentRows::freeze currently creates archives and then calls owner.files() and owner.records(), eagerly reconstructing all native rows. Remove that unconditional reconstruction.

Introduce borrowed metadata views, named ArchivedFileRowRef and ArchivedRecordRowRef, with accessors for IDs, file ID, local slot, source range, source revision, record class, diagnostics reference and collection membership. Their lifetimes are tied to the immutable archive owner. A caller inspecting a primitive field must not initialize an entire native row cache.

Migrate the following internal consumers to borrowed views:

- Segment record_count, file_count, file_records and file/record identity inspection.
- Ownership routing, invalidation and destination selection.
- segment_record_ref and segment_active_file in run.rs.
- Native component accounting and representation validation.
- Source-free partition selection and remapping.
- Query candidate metadata checks and current-path resolution.

Use the pinned current catalog for the active file path and current file metadata. Do not clone a full FileEntry per candidate. External result objects may own the strings they return.

Keep native compatibility accessors only where an existing API specifically needs a native reference. Their OnceLock caches must actually be lazy, shared by clones of the same component, initialized at most once, and visible in memory diagnostics. Ordinary construction and standard archived query paths must not automatically populate every compatibility cache.

Changing Deref-backed wrappers may require migrating internal callers to explicit accessors. Do that migration; do not keep unconditional eager caches merely to preserve an internal Deref convenience.

An intermediate commit may move the existing native input vectors into caches to eliminate the serialize/reconstruct round trip. That is a transitional optimization only. The final normal published path must not retain those vectors unconditionally.

During source-free rebuild, materialize only the selected live rows into one output builder. Reuse these rows until the single final freeze. Avoid the current sequence which freezes projected rows, clones them to refresh paths, then freezes them again. Apply current-path/status changes before final metadata serialization.

### 10.2 Inventory and catalog overlap

The discovery FileInventory still contains native by_path and relative_paths dictionaries. After publication, these must not remain a hidden native lookup catalog beside the archived catalog.

Keep mutable native maps for discovery. On completion, either move the inventory to an archived lookup backing or route its existing lookup methods through a shared archived path-directory handle. Prefer a small internal lookup-backing enum with Builder and Archived states. Preserve the current public lookup behavior. An archived backing must not own a FileCatalog or Repository back-reference that forms an ownership cycle.

Release the old native lookup maps and their capacities. Calling clear alone on a HashMap is insufficient. Preserve a native FileEntry vector if an existing public inventory API requires it; document and count this single compatibility vector. Avoid constructing another full catalog-native row vector simply for routine queries. Do not silently break FileInventory::id_for or ordered path access by dropping their backing maps.

At adoption/rebase boundaries, share unchanged archived path directories and metadata shards rather than rebuilding an identical full catalog. A genuinely changed metadata shard may be rebuilt. Establish counters showing how many catalog generations are constructed during initial build and how many remain owned afterward.

### 10.3 Lossless path storage

The current archive stores both native and query spellings even when identical. Factor a shared stored-path representation used by catalog and segment rows:

    Plain(String)
    Escaped { native_units, query: String }

Plain applies only when the original native path is valid Unicode and the existing query_path encoding equals that string exactly. Its archived string supplies both the native Path view and query key. Escaped keeps the existing lossless native bytes on Unix or wide units on Windows plus the canonical query string.

Do not change escaping or path ordering. Test non-UTF-8 Unix paths, percent signs, backslashes and distinct paths with superficially similar display strings. Do not use lossy conversion. Windows conversion must remain valid even if that platform is not available for execution; report platform coverage honestly.

Exact and ordered path dictionaries remain archived as specified. Account their duplicate key storage explicitly. Do not invent a global string-interning service or a new on-disk path format in this repair.

### 10.4 Archive capacities and native payloads

Expose serialized length and allocated buffer capacity separately on ValidatedArchive. Shared archive ownership must not deep-copy bytes.

Investigate capacity slack before choosing a shrink policy. Use the pinned rkyv implementation's supported aligned shrink operation if available. Otherwise an aligned exact-capacity copy before validation is permitted for materially overallocated buffers. Confirm actual API behavior from the pinned source; do not guess a method name or assume requested capacity equals allocated capacity.

Measure serialization with and without shrinking in a profiling-only comparison. Apply shrinking only where memory savings are meaningful and added copying does not cause a material build regression. Never shrink/move bytes after returning archived references.

Use a fixed experiment policy rather than an unbounded tuning exercise: consider only buffers of at least 64 KiB whose capacity exceeds length by at least 25%. Enable the policy by default only if matched runs reduce total archive capacity by at least 10% while increasing total build median by no more than 3%. Otherwise leave it disabled and report the remaining slack. Keep the structural duplicate-storage repairs regardless of this experiment's result.

The bitmap vector owns the original moved RoaringBitmap values. Do not retain an additional complete native dictionary or cloned bitmap vector after freezing. Do not serialize/decompress the bitmaps merely to make archive byte counts look more complete.

Retain the reverse directory for descending access, but account its entries and duplicate keys. Empty dictionaries and empty catalog shards should share immutable archives where types and serialization configuration agree.

## 11. Trustworthy storage and process-memory accounting — R11, R12, R14

### 11.1 One owner traversal

Introduce one StorageUsage accumulator used by Repository, IndexView and compaction reporting. Traverse the representation actually used for queries, including:

- Any nonempty base collection components.
- Published segment postings, reverse directories, rows and ID lookups.
- Catalog metadata/path shards and the new ownership directory.
- Native bitmap sidecars, bigram payloads, sorting and relations.
- Native inventory compatibility data and initialized metadata caches.
- Control directories, active masks and file-version chunks.

Deduplicate by the identity of the actual shared storage owner, not merely by the containing segment pointer. SegmentFiles and RecordRows share one archive and must count it once. Two segment wrappers can share a posting owner. A path directory can be shared by inventory and catalog. Reporting all pinned views must use one common visited-owner set across those roots.

Repository::rkyv_archive_bytes must include self.segments. Retain that method's existing return type and define it as unique serialized archive bytes; add the richer diagnostic for capacity and native estimates. Apply the same semantics to Index.

Replace the catalog archive_bytes implementation's repeated find searches with a single traversal and visited-owner set. Do not compute O(shards squared) work for a no-op compaction report.

Correct native_segment::recompute_accounting: it currently adds the shared row archive through both file and record wrappers and uses native-container estimates where archived maps are now stored. Use actual archived lengths/capacities and separately labeled native estimates. Do not conflate retained-memory accounting with conservative temporary-work budgeting.

### 11.2 Required diagnostic fields

Report at least:

| Field group | Required values |
| --- | --- |
| Archive totals | Unique owner count, serialized bytes, allocated capacity, capacity slack |
| Archive categories | Posting trees, reverse directories, segment rows, segment lookups, metadata shards, path shards, ownership shards |
| Native payloads | Posting bitmap payload estimate, bigram payload estimate, sorting, relations, active/version/control metadata |
| Compatibility | Native inventory rows/maps, initialized file-row caches, initialized record-row caches, cache capacities and initialization counts |
| Shared ownership | Current-root-only totals and combined totals for deliberately pinned roots |
| Process memory | Before build, ready-for-query, after first query, after warm suite, after dropping old views, after dropping index; peak RSS |
| Allocation evidence | Live heap/allocation-profile findings and any unexplained residual, separately from RSS |

For an archive containing several logical categories, count its physical allocation once. A category breakdown may use instrumented serialization spans or explicitly labeled estimates; category allocations must not each claim the entire buffer.

Native Roaring serialized_size is not an exact heap allocation measurement. If that is the available proxy, label it serialized payload size. Do not call it exact resident bitmap memory. Distinguish Vec capacity bytes, serialized lengths, approximate container overhead and allocator-reported live bytes.

The diagnostic must observe storage without populating metadata caches, cloning keys, cloning bitmaps or validating archives again. This property needs a test.

Use a single internal StorageWorkCounters structure in tests and optional detailed profiling. Give counters explicit names: metadata_rows_examined, path_entries_examined, ownership_routes_examined, segment_descriptors_examined, metadata_cache_initializations, metadata_shards_rebuilt, path_shards_rebuilt, ownership_shards_rebuilt, merge_candidate_probes, segment_reconstructions and archive_bytes_generated. Production hot loops must not acquire a global mutex for these counters. Distinguish zero measured work from an unavailable counter.

### 11.3 Establish the actual memory cause

Use an available heap profiler for separate diagnostic runs. Prefer an existing heaptrack or allocator-profiling setup. If unavailable, add a benchmark-only allocation diagnostic or record the unavailable attribution explicitly; never change the production allocator just to improve reported RSS. Do not include profiler instrumentation overhead in the performance comparison.

Take snapshots with no intentionally pinned old views and again with one pinned view across an update/compaction. After dropping the pinned view, verify allocation owners are released using Weak references or test-only drop counters. Memory returned to the allocator need not immediately disappear from process RSS.

A diagnostic allocator trim, if supported, may be used in a separately labeled experiment to estimate reclaimable allocator retention. Do not insert trim calls into the production query/mutation path or compare trimmed final RSS against untrimmed baseline RSS.

Do not subtract estimated component bytes from RSS and label the remainder “allocator overhead.” RSS and requested heap allocations measure different things. Explain which quantities are exact, estimated, sampled or still unattributed.

### 11.4 Correct interpretation of the existing results

The recorded corpus contains 632,291,328 source bytes, approximately 603 MiB, and 148,486 indexed records. The reported process RSS can exceed source bytes without proving that the live index alone exceeds the corpus.

Recomputed medians from the reviewed construction logs:

| Configuration | Native end RSS, MiB | Prototype end RSS, MiB | Native peak RSS, MiB | Prototype peak RSS, MiB |
| --- | ---: | ---: | ---: | ---: |
| 1 worker, ASCII | 416.5 | 690.7 | 458.4 | 733.4 |
| 8 workers, ASCII | 457.8 | 766.2 | 499.3 | 807.6 |
| 32 workers, ASCII | 579.1 | 896.1 | 620.3 | 936.5 |
| 32 workers, Unicode | 592.8 | 916.7 | 633.3 | 958.0 |

The increase is real process-memory evidence. Its full allocation-level cause was not established.

The reported 70,986,224 direct-path archive bytes omit segment archives. The 148,722,008 progressive-path count is a serialized-length measurement, not a complete exact allocation measurement. Do not reproduce their comparison as evidence that direct construction has a smaller representation.

The recorded bigram payload estimate is unchanged between native and prototype: 86,292,620 bytes ASCII and 90,461,354 bytes Unicode. Native bitmaps already existed in the baseline. Their continued presence is not, by itself, an explanation for the extra 274–324 MiB.

The original report describes segment metadata caches as lazy, but SegmentRows::freeze populates them eagerly. Correct this statement in the new report and add a dated correction note to the original report without deleting the historical measurements.

## 12. Required regression tests

Write meaningful regressions before or alongside each repair. Use the original A/C/M/Q/P/F acceptance matrix as a continuing obligation. Map every original item to an executed test or an explicit unresolved item; references to a broad existing suite are not enough when the relevant combination is absent.

### 12.1 Concrete compaction reproductions

Add a crate-private test hook to run the same compaction implementation with a deterministic small budget; do not add a public tuning API for tests.

Construct three distinct-file segments:

- A: two stored records, one active.
- B: six stored records, one active.
- C: one stored record, active.

Use target_records = 8 and max_files = 8, with a four-input-record budget and nonbinding byte/time budgets. Arrange A before B in reclamation order. Cleaning A consumes two input records; B cannot fit and stays dirty. A's cleaned output and C can merge using the two remaining record visits. B must be excluded from merging. The final live count remains three, and B's five retired records remain retired. Validate results through the normal query API, not just mask counts.

Also run with a two-record budget: cleaning A consumes the budget and no merge may run. Exercise both deleted-file tombstones and replacement tombstones, including replacement with identical file length/mtime. Check counts, ID lookup, numeric predicates, sorting, NOT/complements and pagination.

Use a separate clean fixture with target_records = 8 and max_files = 2. Two segments containing two distinct one-record files each must not merge into a four-file segment. Verify both full and bounded modes. Test overlapping FileIds as a distinct incompatible-pair case.

### 12.2 Required focused matrix

| Test ID | Required scenario and assertion |
| --- | --- |
| T01 | Dirty-input bounded merge reproduction above; no reactivated slots |
| T02 | Shared cleaning/merge budget; counters include all input visits and merges stop at budget |
| T03 | All four compaction option combinations, including clean-input bounded merging |
| T04 | Unequal record/file limits; neither limit is bypassed |
| T05 | Full merge repeated unchanged; no revision, owner or reconstruction change |
| T06 | Selected merge reconstructs exactly once; incompatible probes construct zero outputs |
| T07 | Initial Repository, direct Index and progressive publications have all required dictionaries archived |
| T08 | Archive predicate after add, ordinary replace, verified replace, delete and verified removal |
| T09 | Archive predicate after source-free projection, full/bounded compaction and clean merging |
| T10 | Global inference pruning rejects value dictionaries but preserves supported facts; all surviving components archived |
| T11 | Native builder at publication boundary is rejected without publication |
| T12 | Same-path content update rebuilds zero path shards; one metadata shard and only required ownership shards |
| T13 | Rename within one path bucket and across two buckets; each affected bucket rebuilt once |
| T14 | Add/delete path mappings preserve exact and ordered results and untouched owners |
| T15 | Synthetic catalogs of 1,024, 16,384 and at least 100,000 files; single-file path work equals affected-bucket work, not catalog size |
| T16 | One file spans at least three segments and two collections; routed invalidation reaches all and only its contributions |
| T17 | Rebuild moves an unrelated survivor; its next update/delete follows the new route |
| T18 | Same logical ID and local slot in different segments/collections; qualified identity remains correct |
| T19 | Repeated update of the only file, at least 1,000 updates; one live segment, no growing obsolete descriptor chain |
| T20 | Delete every record; obsolete payload removed, empty collection remains searchable and file IDs are not reused |
| T21 | Current zero-record file and malformed/raw-fallback file retain required metadata and diagnostics |
| T22 | Representation/memory diagnostics do not initialize caches or clone payloads |
| T23 | Cache-free construction; borrowed metadata query path keeps native row cache initialization at zero |
| T24 | Explicit compatibility access initializes its component once, shares across clones and reports its memory |
| T25 | Direct archived key conversion avoids the intermediate owned stored key; borrowed traversal allocates no key strings |
| T26 | Min/max statistics inspect endpoints, and semantic reverse validation is a lockstep traversal |
| T27 | Archive owner reused through two wrappers is counted once; capacity is at least length and both are reported |
| T28 | Equivalent Repository/Index publication accounting agrees for shared payload categories |
| T29 | Pinned old view keeps owners alive; dropping it releases retired owners; newest results remain unchanged |
| T30 | Transaction-specific strategy flags and segment IDs are correct after different successive operations |
| T31 | Lossless non-UTF-8/escaped paths, relative-path ordering and exact lookup remain distinct and correct |
| T32 | Source-free compaction and survivor rebuild succeed with source opens forbidden |
| T33 | Archive-finalization error after worker submission drains all jobs and leaves the prior root intact |
| T34 | Barrier-controlled mutation/compaction conflict preserves the newer mutation |
| T35 | Cancellation during planning/projection/finalization leaves no partial root or outstanding job |
| T36 | Build, mutation, query and compaction write no index files and do not modify source fixtures themselves |
| T37 | Compaction reports actual rewritten/removed/merged counts and actual remaining retired slots |
| T38 | Old native inventory lookup maps are released; public lookup methods still behave correctly |

T15 can construct a synthetic inventory in memory; it need not create 100,000 physical files. Reset counters after initial construction and before the measured mutation. Metadata inspection is bounded by the affected 1,024-row shard plus constant validation work. Path entries inspected are bounded by the affected buckets. Ownership inspection is bounded by the changed file's routes plus survivors actually moved. Count permitted descriptor-directory work separately.

For allocation-related tests, use a narrow test counter or isolated allocator measurement. Avoid asserting process-global allocation counts while unrelated tests execute concurrently. Prefer semantic work counters over fragile elapsed-time assertions.

### 12.3 Deterministic lifecycle sequences

Retain seeds 1, 7, 19, 41 and 97, with at least 100 operations per seed. Use an explicit Cartesian configuration matrix:

    build_workers in [1, 3]
    verified_cache_bytes in [0, 2 * 1024 * 1024]

All five seeds are odd; therefore seed parity must not select cache coverage. Assert that all four worker/cache configurations executed.

Use two collections, up to 24 live files and zero to twelve records per file. Include both single-record JSON and framed multi-record JSONL fixtures, nullable/missing/array fields and relation targets. Keep record IDs unique within each collection except in explicit failure tests.

Each sequence must include add, replacement, delete, bounded compaction without merging, bounded compaction with merging, full compaction without merging and full compaction with merging. Track operation coverage and fail if a required class never executed; use a fixed initial operation schedule before random selection if necessary.

After every operation:

1. Compare live logical IDs/counts with an independent model.
2. Compare the query matrix with a fresh rebuild over exactly the current fixture files.
3. Validate catalog membership, ownership routes, active masks, segment policy and new IDs.
4. Assert all published completed dictionaries remain archived.
5. Compare ascending/descending pages, total counts, numeric and string predicates, FTS and relevant relation queries.
6. Check cache-enabled results after value/text changes, including same-ID replacement and same-size/mtime cases.

Compare logical results and current source locations, not physical segment IDs that legitimately differ after reconstruction. Preserve evidence/path/range differences that are semantically meaningful. At the end, full-compact and repeat all checks. Print seed, configuration, operation, query and segment summaries on failure. Preserve a minimized deterministic regression; never remove a failing seed.

### 12.4 Existing coverage and fixture safety

Keep exact-number, cross-representation equality, datetime offset, 4,096-key reverse traversal, malformed input, relation semijoin, source version, final-drain and scheduler-window regressions active. Keep the corrected scheduler distinction between worker count and up to twice-worker outstanding submissions.

All changing tests use unique disposable directories. Source-free tests use source-access hooks, not only the observation that a later metadata query opened zero files. Scope expected source reads to the changed file during replacement preparation; survivor reconstruction opens none.

Never run bench_lifecycle_mutations against the authoritative Musenalm corpus. Copy a disposable fixture or generate data. Cleanup only the exact directories created by the test or benchmark.

## 13. Validation commands

Run focused tests after their implementation phase and the full checks after integration. Record executed test counts; a filter matching zero tests is not a pass.

    cargo test -p objekte-search-core --lib --locked
    cargo test -p objekte-search-core --test compaction_api --locked
    cargo test --workspace --locked --no-fail-fast
    cargo check --workspace --all-targets --locked
    cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked
    cargo clippy --workspace --all-targets --locked
    git diff --check

Run repository formatting checks without mass-formatting unrelated files. Compile CLI/TUI and desktop targets through the workspace checks. Preserve compact() compatibility and compile public API examples for all mode/merge combinations. Distinguish existing advisory warnings and ignored manual benchmarks from newly introduced problems.

Do not claim a test was executed based on a previous report. If a required dependency/platform/corpus is unavailable, complete independent work and record the exact missing evidence; do not label that requirement passed.

## 14. Performance and memory experiments

### 14.1 Three revisions, identical conditions

Compare the native baseline, the reviewed prototype and the repaired commit. Record exact commit IDs, lockfile digests, compiler/profile, CPU, available/selected workers, query workers, allocator, corpus identity and segment/inference settings.

Historical measurements in section 11 are context. Use matched new runs for final conclusions. Run each sample in a fresh process so process peak RSS belongs to that sample. Use at least three repetitions per build configuration and report medians; preserve every raw sample and explain exclusions.

Build the existing benchmark examples in each worktree:

    cargo build -p objekte-search-core --release --locked \
      --example bench_build_musenalm \
      --example bench_musenalm \
      --example bench_lifecycle_mutations

Resolve the actual read-only corpus location and pass it explicitly. Keep separate target directories/binaries and result directories. Do not compare an instrumented final binary against an uninstrumented baseline.

### 14.2 Construction

Run ASCII and Unicode with 1, 8 and 32 workers where the host supports them:

    target/release/examples/bench_build_musenalm \
      --root examples/musenalm \
      --alphabet ascii \
      --build-workers 32

Time through query-ready publication, including all archive freezing, catalog/ownership construction and final admission. Report first progressive publication separately.

Report end RSS, peak RSS, unique archive length/capacity, native payload estimates, compatibility cache state and phase timings. Measure the ready state before any diagnostic which might allocate significantly. The diagnostic itself must not populate caches.

### 14.3 Queries and warm memory

Run all 150 Musenalm cases for both alphabets through Repository and progressive Index:

    target/release/examples/bench_musenalm \
      --root examples/musenalm \
      --profiles ascii-off,unicode-off \
      --repetitions 3 \
      --query-workers 4 \
      --verified-cache-bytes 0 \
      --result-dir docs/ai/generated/rkyv-memory-index-repair/query-final-direct

Repeat with --incremental in a different directory. Compare semantic digests, ordered logical IDs, counts and evidence. Include focused equality, forward range, reverse range, value enumeration and file metadata queries. Repeat the previously slower Q119 file-modification query.

Measure after the first representative query and after a warm full suite. Moving eager work to the first query is not a complete memory/performance repair unless the warm state is reported as well. Separately benchmark explicit compatibility API calls that initialize native caches.

### 14.4 Mutation scaling and compaction

The previous mutation benchmark involved only a tiny number of files and cannot establish catalog locality. Add measured workloads at approximately 1,000, 10,000 and 100,000 catalog files, using generated or disposable data as appropriate:

- Same-path one-record update with unrelated catalog size increasing.
- Rename within/across path buckets.
- Add into an eligible small destination and add when all destinations are full.
- Delete a file represented in one versus several segments.
- Repeated replacement of one file without explicit compaction.
- Update/delete of a survivor relocated by another file's rebuild.
- Bounded and full compaction, each with merging enabled/disabled.
- Clean no-op compaction and full-compaction idempotence.

Fix the relevant segment target and changed-file size while comparing catalog sizes. Allow the specified affected path bucket to grow with total paths; do not claim constant time where bucket contents genuinely grow. The required improvement is elimination of whole-catalog scans and unrelated metadata materialization.

Report wall time, source opens, rows/routes visited, candidate probes, destination reconstructions, archive bytes generated, shards rebuilt, dirty segments left and component owners reused. Use sufficient samples for p95 or omit p95 if unsupported. Confirm archived representation after every measured operation, outside its timed interval.

### 14.5 Acceptance of measured results

Correctness and the structural repairs are mandatory. Do not trade them away for timing:

- No native dictionary fallback after maintenance.
- No missing ownership directory or catalog-wide per-file scan.
- No duplicate successful consolidation.
- No eagerly reconstructed full segment metadata caches on the normal build path.
- No ignored file limit or merge work outside the bounded budget.
- No false memory totals or unsupported allocation attribution.

The repaired runs must demonstrate what memory the changes removed and whether end/warm RSS improves beyond run-to-run variation. There is no justified promise that a particular final RSS will be reached, or that index memory must be smaller than source-file bytes.

Investigate construction slower than native by over 20%, aggregate query time slower by over 10%, or an individual query slower by over 25% and over 1 ms. Compare against the prototype as well so improvements are visible. Investigate a repair that worsens query-ready or warm RSS, or that leaves the large prototype memory increase essentially unchanged.

If a material residual remains after the specified repairs, report “correctness complete; memory/performance acceptance limited” with the component/profile evidence and remaining cause. Do not call it performance-approved or dismiss it because the original task deferred memory. Complete the remaining tests and evidence; do not stop at the first disappointing benchmark.

## 15. Ordered implementation and commit sequence

Keep each phase reviewable and push its commit immediately. The precise code split may follow dependencies, but retain these validation checkpoints.

### Phase 1 — Reproduce and instrument

Record the starting revisions. Add the bounded-merge, file-limit and post-mutation archive regressions. Add narrow work counters and the test-only budget hook. Correct archive length/capacity instrumentation early enough to measure later phases.

Commit: test(core): reproduce archived lifecycle and compaction defects.

### Phase 2 — Enforce archived publication

Implement seal_completed_storage and the checked PublishedSegment constructor. Repair consolidation, projection, verified maintenance and inference pruning. Keep freezing in preparation workers. Run T07–T11 and relevant existing suites.

Commit: fix(core): preserve archived storage across index maintenance.

### Phase 3 — Repair compaction

Implement clean-only merge selection, shared budgets, max_files enforcement, one consolidation per output, cancellation and accurate reports. Run T01–T06, T32–T35 and T37.

Commit: fix(core): preserve liveness and limits during compaction.

### Phase 4 — Repair catalog locality and ownership

Implement affected-bucket rebuilding, sharded ownership, descriptor routing, survivor relocation and obsolete-segment removal. Correct transaction flags and segment IDs. Run T12–T21 and T30–T31, including repeated-update bounds.

Commit: fix(core): route file mutations through archived ownership shards.

### Phase 5 — Remove allocation and metadata duplication

Implement borrowed dictionary/row access, endpoint statistics, linear semantic validation, lazy compatibility boundaries, shared empty archives, published inventory lookup release and lossless path deduplication. Measure capacity policy separately. Run T22–T26 and T38.

Commit: perf(core): reduce archive finalization and metadata allocations.

### Phase 6 — Complete accounting and differential coverage

Unify unique-owner storage diagnostics and correct all measurement callers. Complete explicit cache/worker sequence coverage and pinned-view accounting. Run T27–T29, the entire original acceptance matrix and workspace checks.

Commit: test(core): validate archived storage locality and memory ownership.

### Phase 7 — Benchmark and investigate

Run the matched three-revision experiments, investigate the thresholds, and commit raw evidence plus concise machine-readable summaries. Correct the historical report's unsupported statements without erasing its evidence.

Commit: bench(core): measure repaired archived index performance and memory.

### Phase 8 — Report and stop

Append the final report to the completed task document. Ensure every R and T item has an explicit outcome, test/counter evidence and any remaining limitation. Commit and push.

Commit: docs: report archived index repair validation.

Do not merge into master. Do not start persistence work.

## 16. Required final report

The final report must contain:

1. Integration branch/worktree, base/native/prototype/repaired commits and toolchain.
2. One status row for R01–R14, plus adjacent reporting repairs.
3. Executed test names/counts and a map from T01–T38 and the original acceptance matrix.
4. Proof that every publication path uses archived dictionaries, including after inferred-component pruning and verified mutations.
5. The ownership layout, current-incarnation checks, zero-record policy and relocated-survivor validation.
6. Counters demonstrating affected-shard work on a large catalog and routed invalidation.
7. Compaction budget/file-limit behavior, cancellation/conflict behavior and reconstruction counts.
8. Matched native/prototype/repaired construction, query, mutation and memory tables.
9. Unique archive length/capacity, native payload/cache breakdown, heap attribution and unexplained residuals.
10. Cold/first-query/warm measurements and deliberately pinned-view lifetime measurements.
11. Source-free maintenance and no-index-write evidence.
12. Corrections to the previous archive-size comparison, eager-cache description, ownership claim and completion status.
13. Remaining limitations stated plainly; no inferred pass for unexecuted tests.

The final user-facing response should identify the branch and the results, state whether correctness and performance/memory acceptance were reached, and leave the branch for manual merge.

## 17. Reviewed source and evidence

These are immutable references for the local agent. Review the corresponding full functions in the repair worktree; do not depend on unchanged line numbers.

- [Prototype revision](https://github.com/Simon-Martens/objekte/tree/241247ec0f83aed50debd295d174d1d4e96f51c9)
- [Compaction and merge selection](https://github.com/Simon-Martens/objekte/blob/241247ec0f83aed50debd295d174d1d4e96f51c9/crates/core/src/lifecycle_compaction.rs)
- [Segment consolidation and projection](https://github.com/Simon-Martens/objekte/blob/241247ec0f83aed50debd295d174d1d4e96f51c9/crates/core/src/mutation_segment.rs)
- [Mutation placement, invalidation and inference finalization](https://github.com/Simon-Martens/objekte/blob/241247ec0f83aed50debd295d174d1d4e96f51c9/crates/core/src/lifecycle.rs)
- [Catalog shard rebuilding and accounting](https://github.com/Simon-Martens/objekte/blob/241247ec0f83aed50debd295d174d1d4e96f51c9/crates/core/src/lifecycle_catalog.rs)
- [Posting conversions, iteration and statistics](https://github.com/Simon-Martens/objekte/blob/241247ec0f83aed50debd295d174d1d4e96f51c9/crates/core/src/posting.rs)
- [Metadata archives and eager caches](https://github.com/Simon-Martens/objekte/blob/241247ec0f83aed50debd295d174d1d4e96f51c9/crates/core/src/segment_archive.rs)
- [Archive owner and length accounting](https://github.com/Simon-Martens/objekte/blob/241247ec0f83aed50debd295d174d1d4e96f51c9/crates/core/src/archive_storage.rs)
- [Repository construction and archive counter](https://github.com/Simon-Martens/objekte/blob/241247ec0f83aed50debd295d174d1d4e96f51c9/crates/core/src/repository.rs)
- [Generated lifecycle tests](https://github.com/Simon-Martens/objekte/blob/241247ec0f83aed50debd295d174d1d4e96f51c9/crates/core/src/rkyv_prototype.test.rs)
- [Native baseline construction evidence](https://github.com/Simon-Martens/objekte/blob/241247ec0f83aed50debd295d174d1d4e96f51c9/docs/ai/generated/rkyv-memory-index-prototype/construction/baseline.tsv)
- [Prototype construction evidence](https://github.com/Simon-Martens/objekte/blob/241247ec0f83aed50debd295d174d1d4e96f51c9/docs/ai/generated/rkyv-memory-index-prototype/construction/final.tsv)
- [Original specification and completion report](https://github.com/Simon-Martens/objekte/blob/241247ec0f83aed50debd295d174d1d4e96f51c9/docs/ai/done/objekte_rkyv_memory_index_prototype_specification.md)

**Stop condition:** complete the repairs and evidence, push one reviewable integration branch, report unresolved limitations accurately, and leave merging to the user.

## Implementation report

### Identity and result

- Task: `rkyv-memory-index-repair`
- Completion date: 2026-11-16
- Native baseline: `2e18871c0bcfb9cf9335d9e73525c64f91c960cf`
- Reviewed prototype: `241247ec0f83aed50debd295d174d1d4e96f51c9`
- Repaired correctness/accounting commit: `45a81a85be5f8a122a3272ce3369ef2ed81632d8`
- Final correctness fix: `406c741e44ac9074c3120377da36404af36da5ee`
- Benchmark/evidence commit: `6d9244e31a727d9578464ea15a30abda600ca480`
- Integration branch: `integration/rkyv-memory-index-repair`
- Integration worktree:
  `/home/simon/source/objekte-integration-rkyv-memory-index-repair`
- Cleanup: the detached native baseline worktree was removed after evidence was
  committed. The reviewed-prototype worktree remains intentionally intact, and
  the repair integration worktree remains available for manual review.
- Dependency: `rkyv = 0.8.18`, default features disabled; `std`,
  `bytecheck`, `aligned`, `little_endian`, and `pointer_width_64` enabled.
- Lockfile SHA-256:
  `e9a405bb6b52450424816a4c134cb14a532256bf4db6b815617d8b0002a54c36`
- Toolchain: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`,
  `cargo 1.99.1 (5f94df478 2026-08-27)`
- Machine: Intel Core i9-13900K, 24 physical cores, 32 logical CPUs.

The structural and correctness repairs are complete. The result remains
**correctness complete; memory/performance acceptance limited**: construction
is still materially slower than native at 8 and 32 workers and the repaired
representation remains substantially larger than native RSS. The user
authorized a reduced performance matrix after interrupting the longer repeated
run; that authorization is recorded in
[`benchmark-authorization.md`](../generated/rkyv-memory-index-repair/benchmark-authorization.md).

### Repair status

| ID | Outcome |
| --- | --- |
| R01 | Fixed. Merge selection admits clean segments only and never publishes a dirty mask as fully active. |
| R02 | Fixed. `seal_completed_storage` is the single finalization boundary; native builders are rejected by `PublishedSegment::checked`. |
| R03 | Fixed. Path rebuilding visits only old/new affected shards; metadata replacement converts archived rows directly. |
| R04 | Fixed. A sharded archived ownership directory plus native segment-position directory routes file invalidation. |
| R05 | Fixed. Fully dead descriptors are removed after mutation; repeated one-file updates retain one live segment. |
| R06 | Fixed. Merge compatibility is metadata-only and each selected output is consolidated exactly once. |
| R07 | Fixed. Cleaning and merging share one work budget, `max_files` is enforced, and elapsed/cancellation checks remain active. |
| R08 | Fixed. Archived scalar conversion is direct, min/max use endpoints, borrowed ID traversal avoids key allocation, and semantic validation is lockstep. |
| R09 | Fixed. Segment caches are lazy/shared, construction leaves file and record compatibility caches uninitialized, and accounting reads archived values. |
| R10 | Fixed structurally. Plain paths share one native/query string; escaped paths retain lossless units. Empty shards are shared. Capacity slack remains and the shrink experiment was rejected. |
| R11 | Fixed. Repository/Index accounting traverses published segments and reports serialized length separately from aligned capacity. |
| R12 | Fixed. One visited-owner set deduplicates row and posting owners; catalog one-pass traversal replaces repeated searches. |
| R13 | Fixed. The deterministic matrix uses five seeds, all four worker/cache configurations, 100 operations each, both compaction modes and merge settings. |
| R14 | Corrected and measured with the authorized reduced matrix. The historical report has a dated correction note and unsupported size/cache/ownership/completion claims are labeled. |

Adjacent reporting repairs are complete: `destination_rebuilt` resets per
operation, rewritten segment IDs agree between descriptor and payload,
content-preserving moves advance ownership versions, and compaction counters
report actual reconstruction/removal work.

### Publication, ownership and locality

Every publication path now freezes declared, inferred, custom and stored
relation posting dictionaries. This includes initial/direct/progressive
construction, inference pruning, verified updates/removals, destination
rebuilds, partition projection, bounded/full compaction and clean merging.
`ValidatedArchive<T>` remains the sole checked aligned owner; unchecked access
is isolated to one audited accessor after checked validation and immutable
ownership.

File ownership is a separate archived shard directory in 1,024-ID ranges.
Routes contain collection, `SegmentId`, and file-row ordinal. The referenced
archived file row owns the local-slot list. Native `SegmentId -> vector
position` directories are control metadata. A retirement checks the captured
file version, visits only routed descriptors, retires the referenced slots and
catalog/path/ownership state in one staged root.

The 100,000-file synthetic catalog regression examines at most one 1,024-row
metadata shard. Same-path updates rebuild zero path shards; same-bucket and
cross-bucket renames rebuild exactly one and two buckets respectively.
Multi-segment two-collection deletion reaches all routed contributions without
probing unrelated payloads. Relocated survivors are validated after subsequent
update/delete.

### Compaction and mutation

Compaction has four defined behaviors:

- bounded/no-merge: finite dirty-segment reclamation;
- bounded/merge: remaining shared budget may merge clean underfilled shards;
- full/no-merge: all captured tombstones are removed without merging;
- full/merge: all tombstones are removed, then compatible clean shards merge.

Overlapping FileIds and duplicate logical IDs are planning incompatibilities.
Record and file targets are both checked. A second full merge with unchanged
owners is a no-op. Selected pairs are consolidated once; incompatible metadata
checks do not construct payloads. Dirty segments left by a bounded budget keep
their original masks and are excluded from merging.

Mutation placement selects one smallest prospective live destination per
affected collection and creates bounded new segments for remaining
contributions. Fully dead obsolete descriptors are removed immediately.
Repeated updates of the only file leave one live segment rather than an
obsolete descriptor chain.

### Required matrix

| Tests | Evidence |
| --- | --- |
| T01–T06 | `bounded_merge_never_reactivates_retired_slots`, `bounded_compaction_can_resume_remaining_retired_shards`, `full_merge_respects_max_files_for_clean_segments`, `compact_reclaims_base_and_segment_tombstones_without_source_files`; four-mode scheduling in the deterministic matrix. |
| T07–T11 | `rkyv_repository_and_lifecycle_defaults_publish_archived_components`, concrete add/replace/delete/compaction fixture, `rkyv_archive_finalization_failure_publishes_nothing`, `rkyv_native_builder_is_rejected_at_publication_boundary`, existing inference admission/pruning tests. |
| T12–T15 | `rkyv_same_path_update_rebuilds_one_metadata_and_ownership_shard`, `rkyv_rename_rebuilds_only_each_affected_path_bucket_once`, `rkyv_large_catalog_update_examines_only_the_affected_metadata_shard`. |
| T16–T21 | `rkyv_one_file_routes_invalidation_across_three_segments_and_two_collections`, `rkyv_relocated_survivor_uses_rewritten_ownership_route`, `rkyv_zero_record_and_malformed_files_keep_current_metadata_and_routes`, `rkyv_repeated_single_file_updates_keep_one_live_segment`, `rkyv_delete_all_records_removes_payload_without_reusing_file_id`; existing duplicate-ID/collection tests. |
| T22–T29 | `rkyv_cache_free_build_keeps_native_row_caches_uninitialized`, `rkyv_storage_diagnostics_do_not_populate_caches_or_clone_payloads`, `rkyv_compatibility_row_caches_initialize_once_and_are_shared_by_clones`, `rkyv_statistics_use_dictionary_endpoints`, `rkyv_shared_archive_owner_is_counted_once_across_wrappers`, `rkyv_repository_and_adopted_index_archive_accounting_match`, `rkyv_pinned_view_releases_retired_segment_owner_when_dropped`. |
| T30–T38 | `rkyv_mutation_strategy_and_segment_ids_are_transaction_specific`, non-UTF-8 catalog regression, source-free compaction, initial archive failure/drain, existing cancellation/conflict tests, filesystem-write audit, compaction report tests, and `rkyv_inventory_releases_native_path_maps_after_adoption`. |

The original A/C/M/Q/P/F matrix remains active. Exact numeric, datetime-offset,
cross-representation numeric equality, malformed/fallback, relation semijoin,
FTS, sorting, pagination, complement/NOT, cache and source-version suites all
run in the workspace test command below.

The deterministic lifecycle suite uses seeds `1, 7, 19, 41, 97`, two
collections, up to 24 files, 0–12 JSONL records per file, all four
`build_workers × verified_cache_bytes` configurations, and scheduled
bounded/full compaction with merging on and off. It compares the independent
model and a fresh rebuild after every operation, validates ownership and active
IDs, checks ascending pages, numeric filters and segment domains, and finishes
with full compaction.

### Validation

| Command | Result |
| --- | --- |
| `cargo test -p objekte-search-core --lib --locked` | 576 passed, 16 existing ignored, 0 failed |
| `cargo test --workspace --locked --no-fail-fast` | passed |
| `cargo test -p objekte-search-core --test compaction_api --locked` | passed |
| `cargo check --workspace --all-targets --locked` | passed |
| `cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked` | passed |
| `cargo clippy --workspace --all-targets --locked` | passed with existing advisory lint warnings; no errors |
| `git diff --check` | passed |

The ignored tests are pre-existing manual scale, profiling and performance
tests, not newly skipped correctness cases.

### Performance and memory

Final conclusions use fresh processes and separate release target directories.
The correctness/accounting benchmarked source commit is `45a81a85…`; the
later `406c741e…` fix preserves ownership shards across the initial catalog
rebase and was found by the final workspace run. It does not change the
published representation or measured hot path, but these benchmark hashes
should not be read as timing a build that already contained that fix.

Construction medians through query-ready publication:

| Revision | Alphabet | Workers | Samples | Wall ms | End RSS MiB | Peak RSS MiB |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| native | ASCII | 1 | 3 | 3641.0 | 417 | 458 |
| native | ASCII | 8 | 3 | 802.2 | 468 | 510 |
| native | ASCII | 32 | 3 | 627.0 | 570 | 611 |
| native | Unicode | 1 | 3 | 4366.3 | 424 | 466 |
| native | Unicode | 8 | 3 | 909.3 | 475 | 517 |
| native | Unicode | 32 | 3 | 710.5 | 602 | 642 |
| prototype | ASCII | 1 | 3 | 4135.8 | 690 | 733 |
| prototype | ASCII | 8 | 3 | 1142.6 | 759 | 800 |
| prototype | ASCII | 32 | 3 | 976.4 | 898 | 939 |
| prototype | Unicode | 1 | 3 | 5004.1 | 698 | 741 |
| prototype | Unicode | 8 | 3 | 1219.1 | 752 | 794 |
| prototype | Unicode | 32 | 3 | 1040.1 | 909 | 950 |
| repaired | ASCII | 1 | 3 | 4131.5 | 606 | 672 |
| repaired | ASCII | 8 | 3 | 1179.0 | 671 | 736 |
| repaired | ASCII | 32 | 3 | 1062.0 | 845 | 907 |
| repaired | Unicode | 1 | 1 | 5040.0 | 612 | 677 |
| repaired | Unicode | 8 | 1 | 1293.7 | 681 | 745 |
| repaired | Unicode | 32 | 1 | 1116.8 | 819 | 883 |

Raw samples: [`construction/samples.jsonl`](../generated/rkyv-memory-index-repair/construction/samples.jsonl),
medians: [`construction/summary.json`](../generated/rkyv-memory-index-repair/construction/summary.json).
The repaired end RSS is 6–12% below the reviewed prototype, but remains above
native, especially at higher worker counts. Construction is 13–15% slower than
native at one worker but 43–50% slower at 8 workers and 57–69% slower at 32
workers, so the construction threshold remains failed.

All 150-case one-repetition direct/progressive sums:

| Entry | Native ms | Prototype ms | Repaired ms | Repaired/native |
| --- | ---: | ---: | ---: | ---: |
| direct, ASCII | 1399 | 1423 | 1471 | 1.052x |
| direct, Unicode | 1316 | 1409 | 1433 | 1.089x |
| direct, combined | 2715 | 2832 | 2904 | 1.070x |
| progressive, ASCII | 1349 | 1345 | 1408 | 1.044x |
| progressive, Unicode | 1331 | 1376 | 1363 | 1.024x |
| progressive, combined | 2680 | 2721 | 2771 | 1.034x |

All four baseline comparisons have zero semantic digest mismatches across 150
cases. The single direct run initially flagged five cases. A focused,
three-repetition retry reduced the worst median ratio to 1.095x (Q001 ASCII);
Q119 direct was 1.056x ASCII and 1.037x Unicode. No focused case remained above
the individual 25% plus 1 ms threshold.

Raw query evidence and summaries:
[`query-summary`](../generated/rkyv-memory-index-repair/query-summary),
[`query-final-direct`](../generated/rkyv-memory-index-repair/query-final-direct),
and [`query-final-progressive`](../generated/rkyv-memory-index-repair/query-final-progressive).

Repaired archive accounting on the 32-worker Unicode full-corpus ready state:

| Category | MiB |
| --- | ---: |
| Unique serialized archive bytes | 133 |
| Unique aligned allocation capacity | 205 |
| Capacity slack | 72 |
| Native posting bitmap payload estimate | 17 |
| Bigram payload estimate | 87 |
| Sorting payload estimate | 23 |
| Native inventory estimate | 38 |
| Native diagnostics estimate | 0 |
| Initialized file/record metadata caches | 0 |

The shrink experiment reduced capacity to about 146 MiB and slack to about
13 MiB, but increased the 8-worker ASCII build median from 1,179 ms to
1,243 ms (5.4%). Because that exceeds the 3% policy limit, shrinking remains
disabled. Evidence:
[`capacity-shrink/summary.json`](../generated/rkyv-memory-index-repair/capacity-shrink/summary.json).

The small mutation benchmark completed add, replacement, verified update,
content-preserving move, verified delete and compaction on fresh disposable
fixtures. Repaired add/update times were within roughly 0.04–0.10 ms of the
reviewed prototype while retaining archived storage and real strategy labels:
[`mutation`](../generated/rkyv-memory-index-repair/mutation).

The reduced catalog-locality run used one-record files at 1,000 and 10,000
files. The default segment target admitted the entire 1,000-file catalog into
one destination, and the 10,000-file catalog into one under-target segment;
the mandatory one-destination rebuild therefore grew with that shard. Median
verified original-file updates were 6.55 ms repaired versus 2.64 ms native at
1,000 files, and 51.87 ms versus 23.14 ms at 10,000 files. The 100,000-file
locality claim is supported by the synthetic work-counter regression, not a
wall-time benchmark in this authorized reduced run. Evidence:
[`mutation-locality-summary.tsv`](../generated/rkyv-memory-index-repair/mutation-locality-summary.tsv).

### Source-free, lifetime and correctness evidence

- Source-free partition projection and compaction use archived postings,
  rows, sorting, text and relation payloads; files can be absent and the
  indexed-answer probes report zero source opens/parses.
- A pinned view retains its descriptors and archive owners. The pinned-owner
  regression verifies a retired segment owner survives until the pinned view
  drops and is released afterward.
- Archive-finalization injection fails after preparation, leaves the prior
  root unchanged and drains worker state.
- Initial catalog rebase now preserves ownership shards; the desktop export and
  retired-slot lookup regressions that exposed this issue pass.
- Build, query, mutation and compaction write no index files and do not modify
  source fixtures.
- Lossless non-UTF-8 paths retain native identity and canonical query spelling.

### Remaining limitations

- Performance acceptance is limited by construction and catalog-size
  mutation costs. The structural duplicate work is removed, but retained
  archive capacity, native Roaring/text/sort/relation payloads and
  destination-segment projection remain material.
- Archive capacity slack remains about 72 MiB on the full corpus because the
  measured shrink policy missed the build-time threshold.
- The repaired Unicode construction cells have one sample and the full query
  matrices have one correct-revision sample each under the user-authorized
  reduced benchmark. They are sufficient for correctness and threshold
  screening, not high-confidence percentile reporting.
- No wall-time 100,000-file mutation benchmark was run in the reduced matrix;
  affected-shard counters were validated synthetically at 100,000 files.
- The benchmark was taken immediately before the final ownership-rebase fix.
  Correctness after that fix is fully validated, but timing was not repeated.
- Bitmap, text, sorting and relation payloads remain native in-memory
  sidecars. This prototype is not a persistence, mmap, spill, eviction or
  durable-format implementation.

The branch is ready for review as a correctness-complete prototype with
explicit performance/memory limitations. Do not merge it into `master` without
user direction.
