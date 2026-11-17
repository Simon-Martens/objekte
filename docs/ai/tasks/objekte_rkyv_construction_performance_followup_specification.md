# Objekte rkyv construction performance follow-up specification

## 1. Assignment and acceptance objective

Complete the construction and allocation repairs described here after the current rkyv memory/index repairs. The objective is to recover native index construction performance, preferably improve it, while retaining correct archived publication, file addition/replacement/deletion, source-free compaction, and query behavior.

This is a separate implementation assignment. It supplements `objekte_rkyv_memory_index_repair_specification.md`; it does not replace that document or authorize abandoning its unfinished requirements. Read the predecessor specification and its implementation report before editing. This document supersedes its construction-performance acceptance threshold: merely investigating a regression greater than 20% is insufficient. A substantially slower build is an unmet requirement even if correctness passes.

Reference revisions:

| Reference | Revision |
| --- | --- |
| Original native baseline | `2e18871c0bcfb9cf9335d9e73525c64f91c960cf` |
| Original rkyv prototype | `241247ec0f83aed50debd295d174d1d4e96f51c9` |
| Initial follow-up inspection | `1999c49fdf156853bc85253e3dc0f8a85e1155fc` |
| Subsequent committed repair inspection governing the priority changes below | `45a81a85be5f8a122a3272ce3369ef2ed81632d8` |
| Follow-up starting revision | Record the actual completed predecessor repair commit at execution time |

Revision note: the subsequent inspection found larger construction costs than the original clone-focused list. Sections 3.1 and 6.4–6.9 and the revised phase order take precedence. In particular, reusable lookup keys and a new field-slot architecture are benchmark-dependent experiments, not mandatory changes merely because they reduce allocations. The inspected repair head has implementation/test commits but no committed repair benchmark directory or completion report; do not describe it as a measured slowdown or speedup.

The findings below came from source inspection, not new timing or allocation measurements. They establish redundant operations; they do not establish the milliseconds or bytes saved by each repair. Some aggregation defects predate rkyv. Removing inherited costs may offset serialization overhead, but native parity is not guaranteed by the library choice or by completing a checklist.

Keep the implementation entirely in memory. Keep rkyv 0.8.18, existing archive format features, query semantics, exact numeric behavior, Unicode behavior, collation, inference policy, default segment limits, worker selection and public lifecycle behavior. Do not add persistence, mmap, spilling, compression formats, another database engine, or a mutable archived-tree implementation.

The user expects measured native performance. Implement the specified repairs, measure the remaining gap, and continue with the bounded experiments in section 9 when necessary. If parity remains unmet, deliver the tested branch and exact blocker as unfinished performance work. Do not call the task performance-complete, silently relax the target, or claim success merely because the result improved over the prototype.

## 2. Integration worktree and dependency procedure

Read `AGENTS.md` and all of `docs/ai/README.md`. Preserve user changes and unrelated untracked files. The original checkout stays on master; all implementation occurs in a separate worktree.

| Purpose | Required name or location |
| --- | --- |
| Task identifier | `rkyv-construction-performance` |
| Integration branch | `integration/rkyv-construction-performance` |
| Integration worktree | `/home/simon/source/objekte-integration-rkyv-construction-performance` |
| Task document | `docs/ai/tasks/objekte_rkyv_construction_performance_followup_specification.md` |
| Evidence | `docs/ai/generated/rkyv-construction-performance/` |
| Completed document | `docs/ai/done/objekte_rkyv_construction_performance_followup_specification.md` |

Use an equivalent sibling location if the repository resides elsewhere, never `/tmp`. Fetch the configured remote and inspect existing worktrees before creating anything. Resume an existing follow-up branch without resetting it.

Complete and validate the predecessor first. Freeze its commit as the repaired comparison revision. Create this integration branch from that exact commit, so the final branch includes the prerequisite repairs. If those repairs have already been merged, start from a master revision containing them and record the relationship. Do not branch from the old prototype, mix changing predecessor results into a benchmark, cherry-pick an arbitrary subset of repairs, or edit the predecessor agent's worktree while it is running.

The source references below identify the inspected repair revision. Reconcile them with the actual starting commit. A finding already fixed requires evidence and a regression check, not a second implementation. Preserve predecessor correctness improvements.

One supervisor owns the integration worktree. If local subagents are used, give each an isolated sibling worktree and a `task/rkyv-construction-performance/<subtask>` branch. Delegate independent fixture or benchmark work after interfaces are agreed; keep overlapping edits to posting/archive/aggregation code under one owner. A single agent may execute all phases. Integrate and validate into one final branch.

Commit and push at the phase boundaries in section 11. Do not force-push or merge into master. A report with an unmet performance gate remains a partial result; do not move the task to done simply because correctness passes. The final user response must state which acceptance gates passed.

## 3. Findings and required implementation outcomes

| ID | Observed cost or audit obligation | Required outcome |
| --- | --- | --- |
| C01 | `PostingDictionary::freeze` constructs a second native ordinal B-tree and clones every distinct key | Serialize ordered entries directly without the temporary ordinal tree |
| C02 | Value insertion clones through `ScalarKey` and `StoredScalarKey`; repeated values allocate temporary keys | One ownership conversion; adopt reusable lookup only if it beats the single-search insertion path |
| C03 | First batch is merged into empty structures by enumerating bitmap members | Transfer compatible offset-zero batch ownership directly |
| C04 | Every field merge drains, sorts and reconstructs the field container | In-place merging first; private field slots only if they provide an additional measured gain |
| C05 | Finalization clones sort payload arrays, then clears and shrinks them | Construct sort configuration without cloning payloads |
| C06 | Record/file/lookup serialization uses additional owned intermediate strings and containers | Borrow source fields into serialization or move required ownership once |
| C07 | Collection metadata, native ID lookup and identity order are reconstructed alongside archived segments | Remove unused duplicates; construct required compatibility data once without warming segment caches |
| C08 | Late relation reconstruction can create native dictionaries after other finalization | Audit and enforce archived state for all reachable query posting dictionaries |
| C09 | Build accounting does not yet isolate these costs and overlapping temporary owners | Attribute time, allocations, retained storage and peak overlap through publication |
| C10 | Final rebase serializes the complete catalog again, including unchanged paths, and resets ownership | Reuse path/ownership components; rebuild only changed metadata shards and preserve live routes |
| C11 | Eligibility/domain validation creates owned keys and runs repeatedly around sealing | Bitmap-only checks; one complete validation per changed component generation |
| C12 | Publication reconstructs full catalog/segment file objects to inspect a few fields or empty diagnostics | Borrowed checks and diagnostic-free fast path; batch actual metadata edits by shard |
| C13 | Inference-only pruning deep-clones segment payloads, rewrites unchanged rows and thaws dictionaries before discarding them | Component-local changes; share unchanged payloads and replace discarded dictionaries directly |
| C14 | Every new record ID probes all earlier segments and allocates a temporary global lookup key | Allocation-free empty-base check; bounded-memory construction duplicate registry experiment |
| C15 | Empty repository shells run full initialization, creating unnecessary pools and empty archives | Lightweight configured construction state; reuse the task pools and configuration |

C08 is a required reachable-path audit, not a claim that the predecessor leaves it unfixed. At the inspected revision, relation finalization calls `PostingDictionary::from_public(contribution.reverse_postings())`; determine whether the completed predecessor now freezes or eliminates this dictionary. Verify with a relation fixture, not an empty-schema assertion.

Native temporary builders are allowed. There is no requirement to delete every `BTreeMap` or `HashMap`: schema/control maps and genuinely needed accumulation maps are legitimate. Distinguish temporary construction structures, native payloads intentionally retained, public compatibility data, and accidental duplicate indexes.

### 3.1 Revised priority: remove complete passes before redesigning builders

Implement and measure in this order: C11 bitmap-only validation and duplicate-pass removal; C10 catalog reuse/parallel shard finalization; C12 borrowed publication checks; C01 direct ordered serialization; C02 direct ownership conversion and C05 configuration-only sorting; C03/C04 aggregation; C13 inference pruning and C15 shell initialization. C14 is an explicit scaling experiment because an ID registry can increase peak memory.

This order is based on identifiable work eliminated, not an invented estimate of time saved. The 32-worker build is particularly sensitive to serial coordinator work. Saving a small allocation inside a parallel worker cannot compensate for an unnecessary serial whole-catalog pass. Profile the coordinator and worker stages separately.

Do not delete validation wholesale. Preserve each invariant at the right boundary, avoid allocating data it does not inspect, and avoid rechecking unchanged immutable components. A new check is not a performance optimization; record its cost separately from a removed check or copy.

## 4. Ordered archive construction and insertion — C01, C02

### 4.1 Direct serialization from ordered entries

Keep one accumulation dictionary per posting component. After removing empty postings, consume it exactly once. Move each key into one ordered entry vector, assign a checked dense `u32` ordinal, and move each `RoaringBitmap` into its final native sidecar vector. Do not clone keys or bitmap payloads during this ownership transfer.

The temporary data at this boundary is:

```rust
// Conceptual ownership layout; adapt existing types rather than adding aliases.
entries: Vec<OrderedPostingEntry<K>>
bitmaps: Vec<RoaringBitmap>
```

Implement a private serialization source named `OrderedPostingMapSource` over a borrowed ordered-entry slice. Its archive type is the existing archived B-tree type. Its serialization method supplies borrowed key/ordinal pairs to rkyv's `ArchivedBTreeMap::serialize_from_ordered_iter`; its resolution method uses `resolve_from_len`. Integrate the source with the archive root through the existing serialization abstraction or a small private `Archive`/`Serialize` adapter. Keep generic support for scalar, length and structural dictionaries.

The iterator must expose an accurate remaining length and ascending, strictly unique keys. Obtain that guarantee from the owned map or a checked ordered-entry constructor. Do not falsely implement `ExactSizeIterator` for an arbitrary filtered or merged iterator. Remove empty entries before assigning ordinals and producing the final exact-length slice.

Serialize the existing ordered reverse-traversal directory from the same borrowed entries. Do not collect a second owned vector and do not reconstruct a native map to satisfy a derived field type. A private archive source type may change; the published logical representation and checked access remain unchanged. No persisted format compatibility is required because this experiment has no saved files.

The output still contains both the archived B-tree and the ordered directory. This task removes the extra native tree and its key allocations; it does not claim to remove duplicate archived key bytes. Keep the directory for current descending traversal. Do not substitute sorted-array lookup for the required archived B-tree or retain native keys behind the published dictionary.

Reserve the two vectors from the exact surviving entry count. Drop consumed native builders and temporary source vectors as soon as serialization and semantic verification no longer need them. The sidecar owns the original moved bitmap payloads. Do not serialize/decompress those bitmaps as part of this repair.

Preserve checked archive validation and the predecessor's linear semantic validation. Test key order, dense ordinals, bitmap correspondence and all traversal directions. Custom archive code must use rkyv's supported placement/resolver facilities; do not add layout casts between unrelated structs or fabricate references into resizable buffers.

### 4.2 Eliminate build-time intermediate owned keys

The existing `ScalarKey::from_value(value)` followed by `StoredScalarKey::from_scalar(&key)` copies string/ID payloads twice. This is different from the predecessor's archived-key read conversion repair.

Add a direct checked conversion from `Value` to the stored key representation, and a consuming conversion from owned `ScalarKey` where that type already exists. Move owned string and exact-decimal buffers through consuming conversions. Borrowed input requires at most one owned copy when inserting a new dictionary entry. Preserve scalar variant ordering, number equality/order across representations, datetime comparison by instant, and retained datetime offsets.

The production baseline for this change is direct conversion followed by one `entry` search. The revised goal is faster insertion, not the smallest possible allocation counter at any cost. Establish that result first. Then prototype a private reusable lookup key: populate retained string/decimal capacity, call `get_mut`, and transfer the key into `entry` on a miss. It avoids allocation on warmed hits but performs two tree searches on misses and still copies string bytes into scratch. It is not automatically faster.

Measure this prototype against the single-search path on repeated-key, mixed-cardinality and all-unique inputs, plus full Musenalm. Retain it only if the gain is distinguishable from noise and it does not introduce a material regression on high-cardinality fields or the full build. Otherwise keep direct single-search insertion and record the scratch experiment as rejected. Do not invent an adaptive cardinality policy or keep an extra per-key hash index to satisfy an allocation counter.

If the predecessor already provides a lawful allocation-free heterogeneous lookup, benchmark it instead of duplicating scratch logic. Equivalent borrowed and stored comparisons must agree for all variants. Never assume a temporary borrowed enum can be returned as a reference from `Borrow`, and do not use unsafe casts or a side hash table containing a second copy of every key.

If retained, scratch storage stays private to a builder/worker and is released at finish. Retain at most 64 KiB of unused string/decimal scratch capacity per builder; discard unusually large scratch buffers after use without truncating keys. This cap is a retention policy, not an input limit. Report its capacity and high-water behavior. Zero hit allocations is evidence for the scratch experiment, not a substitute for a faster build.

For multi-valued fields, remove the second owned-key copy in the per-record deduplication path. Posting insertion is idempotent for the same record slot, so repeated direct insertion may replace an owned-key set when it preserves all statistics and coverage semantics. Verify array lengths, empty selections, nulls and completeness separately. If another operation needs unique values, use reusable storage and move resulting keys; do not retain a fresh owned-key set per record.

## 5. Batch aggregation and sorting — C03, C04, C05

### 5.1 Adopt the first compatible batch

The current aggregate begins empty, then reconstructs the first batch's postings through `append_shifted_bitmap`. Add an explicit first-batch adoption path. Prefer an internal aggregate constructor from the first `BatchContribution` so an entire set of empty maps does not have to be allocated first.

Adoption requires all of the following:

- The contribution belongs to the expected collection/schema and matches field definitions, custom bindings, alphabet, inference configuration, sorting bindings and relation declarations.
- Its effective segment-local base is zero and its record/coverage domains satisfy existing admission checks.
- No previous contribution has established aggregate state, even if its bitmaps happened to be empty.
- Moving the contribution does not bypass file framing, diagnostics, record ownership or pending publication-group accounting.

Move structured values/length/structural maps, their bitmaps, custom postings, inference accumulation, logical/empty-array masks and compatible bigram contributions. Move relation contributions and initial sort-key vectors when their types and slot domains agree. Add narrow constructors where needed; do not rebuild them through a public query representation.

Inspect inference and custom completeness handling before adoption. Empty schema state can contain configuration or counters that are semantically meaningful. Preserve that state explicitly; an empty bitmap does not prove an empty accumulator. A first zero-record contribution still follows configuration checks. Handle a completely empty collection without manufacturing a nonempty batch.

Later batches continue to use checked rebasing and merging. A nonzero base, an existing destination key or an overlapping slot domain must not take the ownership shortcut. Do not replace append/union with assignment merely because a destination bitmap is empty. Retain overflow, ordering, coverage and mismatch errors.

### 5.2 Stable field slots during aggregation

Remove the repeated `mem::take(fields) -> Vec -> sort -> jobs -> HashMap` cycle in `PostingCollection::merge_shifted_in_pool` on the segment construction path.

First implement the simpler in-place merge: consume the incoming field map, look up each corresponding destination field, and merge the owned incoming value directly. Preserve field count, missing/unexpected path, schema and coverage errors. This keeps the destination `HashMap` and its capacity intact and eliminates temporary sorted work vectors. It need not add a new field registry or change the public representation. Retain the existing family-level and segment-level parallelism.

Benchmark the in-place path against the current per-field parallel setup on narrow and wide schemas at 1/8/32 workers. Serial merging within one segment can win when many segments already run in parallel; it can also lose on wide schemas. The measured outcome determines whether finer field parallelism is needed.

Only then, if field merging remains material, experiment with a private `SegmentPostingAggregate` using stable destination slots and reusable optional incoming slots. Reuse compiled field identifiers when available; otherwise construct a deterministic path-to-slot registry once. Consume each incoming value once, validate coverage before mutation, and merge safe disjoint slices through the existing pool. Construct a final public map once if required. Runtime-discovered inference fields remain in the inference accumulator.

Keep the slot-vector version only if it improves the full build or a relevant wide-schema workload without materially regressing the main cases. Otherwise deliver the simpler in-place path. C04 is satisfied by removing repeated container reconstruction with an evidenced non-regressing implementation; a new architecture is not itself an acceptance condition.

Preserve bounded scheduling and cancellation. Use the existing pool, not a nested thread pool or a lock per field. Use safe disjoint slice iteration; do not introduce raw-pointer aliasing to manufacture parallel access. Benchmark both narrow schemas and many-field schemas so scheduler overhead is visible.

### 5.3 Construct sort configuration without payload cloning

Replace `collection.sorting = first.index.sorting.clone()` followed by clearing/shrinking keys and order vectors. Add an explicit `SortCollection::configuration_only` constructor or equivalent named operation.

Copy only binding metadata: field path, logical type, locale, options, collation version, composite definitions, directions and tie-breaking configuration actually needed by consumers. Allocate no key arrays or record-order arrays in this constructor. All payload vectors must begin with length zero and capacity zero. Preserve empty-collection configuration from the collection definition when no segment exists.

Do not add another archived copy of sorting as part of this task. Segment sort payloads remain in their existing native representation, shared by their existing owners.

## 6. Serialization intermediates and compatibility metadata — C06, C07, C08

### 6.1 Borrow serialization input

`StoredRecord::from_record` currently allocates a new ID string; row construction collects owned file/record intermediates before producing archive bytes. `RecordLookup::freeze` similarly converts owned IDs into new strings in another native map. Audit these paths and catalog/file lookup construction in the actual predecessor result.

Introduce private serialization sources borrowing existing IDs, path encodings, diagnostics and row slices. Use wrappers/resolvers to emit the required archived owned representation. Borrowed source lifetimes last through serialization only; they must not become external pointers in the archive. Source strings must be droppable immediately after freezing while the archive remains valid.

For necessary normalization or lossless path encoding, compute the transformed value once and keep that one bounded temporary until its output is written. Preserve the predecessor's Plain/Escaped distinction and exact non-UTF-8 paths. Do not copy an already normalized native/query spelling just to fit an intermediate struct.

Use checked scalar conversion for file/record IDs. Avoid collecting a second full native lookup map merely to transform wrapper IDs to integers or clone string keys. Use the corresponding supported rkyv map adapter/iterator machinery; the B-tree ordered-iterator API is not a hash-map construction API. If a hash serializer requires exact-length materialization, retain only the minimal entry/reference vector, not another owning string dictionary.

Moving original native vectors into unconditional compatibility caches is not an acceptable final implementation. Ordinary build, diagnostics and ordinary queries must retain the predecessor's lazy-cache behavior. Do not increase first-query cost to conceal construction work.

### 6.2 Collection-wide compatibility audit

At the inspected revision, `segment_metadata_generation` walks segment rows and constructs a `CollectionGeneration`, native `id_lookup`, global `identity_order`, masks and global relation contributions. Finalization also clones the discovered inventory. This is distinct from a segment's lazy row cache.

Produce `metadata-ownership.md` before changing this layout. For each structure list its owner, construction sites, public type exposure, production readers, retained capacity and whether ID/path bytes are shared or copied. Include direct Repository, progressive Index, relations, CLI/TUI/GUI compilation and pinned old views. Public `CollectionState` fields must be checked explicitly; replacing an exposed `HashMap` or `Vec` with an empty placeholder silently breaks the API.

Apply these decisions in order:

1. Remove full metadata/index copies that have no reachable consumer. Remove their construction as well as retained allocation.
2. Migrate internal-only primitive metadata reads to the existing borrowed archived row views and catalog. Do not initialize segment row caches to construct the global compatibility layer.
3. Where the current public contract requires a native collection vector/map, retain exactly one canonical compatibility instance. Build it once from borrowed archived rows or retain the already constructed required instance; do not reconstruct it twice during initial/final publication. Share existing immutable `Arc` ID/path payloads when the types permit.
4. Move inventory ownership only after worker/admission references and old publications no longer need it. Use `Arc::try_unwrap` only with a correct sharing fallback. Do not use unsafe uniqueness assumptions or invalidate earlier progressive views.
5. Construct required identity ordering once, after final record IDs and global slot mappings are fixed. Reuse an already valid order; do not assume per-segment ID ordering is globally sorted. A replacement algorithm must preserve deterministic tie-breaking.
6. Count unavoidable public compatibility storage explicitly. Do not claim that all metadata is archived while such objects remain, or hide a remaining cost under “legacy.”

This is not permission for a broad public API redesign. If removing the final substantial duplicate requires changing exposed field types, complete the safe reductions and measure the remaining cost. Record the exact API dependency as a blocker to further reduction. Do not abandon C01–C06 or weaken the native-performance gate because C07 encounters such a dependency.

### 6.3 Final relation and publication audit

Audit relation postings created after segment finalization, including global reverse dictionaries and cloned relation contributions. Share immutable contributions when safe or transfer ownership into the required aggregate; do not remove required cross-segment relation resolution.

Every dictionary reachable through a production archived query path must be in its completed state before publication. Freeze late-created dictionaries once after their final change, or remove them only when consumers demonstrably use the segment representation instead. No-op finalization must not freeze the same component again. Include declared and inferred relation fields, dangling targets, self-relations, incoming/outgoing traversal, and target changes/deletions in tests.

The publication lock remains limited to validation of generation/conflict state and root replacement. Do not move serialization, scanning or compatibility reconstruction under that lock.

### 6.4 Catalog construction, rebasing and ownership — C10

At `45a81a85`, `FileCatalog::from_parts` groups the complete inventory and builds metadata/path shards in serial loops. `FileCatalog::rebase` calls `from_parts` again at final construction. This rewrites all path dictionaries despite stable file IDs and paths. Native rebase did not serialize these structures, so the similar high-level call sequence now has a materially different cost.

Replace whole-catalog rebase with component-preserving finalization. Verify inventory identity/configuration; reuse unchanged path shard owners and active membership; update final fingerprints, configuration and genuinely changed status/diagnostics in metadata shards. Initial fingerprints may change every metadata shard: rebuild those shards once if necessary, but do not use that fact to rebuild unchanged paths or ownership. Batch all row changes to a metadata shard before one serialization.

The inspected `from_parts` also initializes empty ownership shards. Consequently, assigning its result in final `rebase` discards ownership routes already accumulated by segment publication. Preserve the current versioned ownership directory when rebasing the same files and segments. Add a direct progressive-build regression that calls update/delete immediately after final publication. A test that first converts through `Index::from_repository` can mask this defect by reconstructing ownership; it is not a sufficient reproduction.

Finalize independent initial metadata/path shards and changed final metadata shards through the existing build pool. Keep output shard order deterministic, errors propagated and the pending work window bounded. Do not allocate a new thread pool per catalog or shard. Release grouped inputs once consumed. Publication receives completed immutable shard owners; it performs no serialization under the root-swap lock.

Count initial/final path, metadata and ownership shard builds separately. For a clean initial build with unchanged paths, finalization must rebuild zero path shards and must preserve the live ownership routes. Repeated finalization of an unchanged component must not serialize it again. Do not omit required fingerprint work to satisfy a counter.

### 6.5 Validation without key materialization or repeated full passes — C11

`PostingIndex::validate_eligible` uses `for_each(|_, bitmap| ...)`; that dictionary adapter constructs `ScalarKey` values from stored/archived keys even though the callback discards them. `validate_domain` and other value-only consumers must be audited too. `for_each_bitmap` already exists in the repair and is the immediate low-risk replacement for callbacks that inspect only bitmaps. Keep all domain, presence, null, structure and coverage invariants.

At the inspected head, ordinary segment construction calls eligibility validation before freezing, after freezing, and again inside `seal_completed_storage`. The last boundary also recomputes statistics already produced earlier. Remove this redundant sequence. Separate the logical posting validation of a completed builder, structural validation of newly written archive bytes, and semantic archive/ordinal correspondence validation. Each is necessary for its own purpose; three equivalent full posting scans are not.

Choose one completion boundary to perform the final logical validation. When an earlier producer needs a local invariant for a later calculation, retain that narrow prerequisite, not another complete dictionary traversal. New archive bytes still pass checked rkyv access once and archive correspondence checks once. Existing immutable owners retain their validation status. A changed dictionary, row component or logical domain invalidates the applicable completion state and requires fresh checks.

Use an internal completed component/builder distinction or equivalent private state whose validity follows the actual ownership/mutation paths. Do not add a caller-controlled `already_validated` boolean or skip safety checks based solely on whether storage is archived. All producer paths must prove the correct record domain. An already completed unchanged segment must not repeat full row/lookup validation or accounting scans merely because it passes another publication boundary.

Extend counters to separate logical posting visits, copied validation key bytes, checked archive validations, semantic archive visits, row cross-checks and accounting visits. Require zero key string allocations for bitmap-only validation. Preserve corruption and out-of-domain tests. Compare native and rkyv validators: the native version iterated `.values()` without constructing public keys.

### 6.6 Allocation-free ordinary publication checks — C12

`prepare_initial_segment` currently obtains an owned catalog file entry and clones it before asking the segment to merge diagnostics. `merge_file_diagnostics` reconstructs a complete `SegmentFile`, including its path, memberships and record-slot vector, even when no diagnostic exists. `with_segment_mode` and processed-membership handling obtain further owned catalog entries to inspect primitive metadata. Final metadata copying repeats owned access. Eliminating eager caches did not eliminate these transient reconstructions.

Add borrowed catalog accessors for status, revision, creation time, collection membership and native-path comparison. Use the existing `matches_revision` where it is sufficient, and complete the missing borrowed checks. Ordinary healthy files with no diagnostics must not allocate a `FileEntry` or `SegmentFile` for publication validation, membership accounting or a no-op diagnostic merge.

Read status and diagnostic counts directly from archived rows first. When diagnostics exist, decode only those diagnostic fields required for the actual merge. Do not materialize the record-slot vector or path merely to merge diagnostics. Apply status/diagnostic changes as one shard-local batch per atomic publication group; avoid calling full shard reconstruction once per changed file. Preserve exact diagnostic identity, omitted counts and current status semantics.

`build_metadata_shard` currently calls `row.to_entry()?` and discards the owned result to validate a stored row. Replace this with direct validation of the stored fields and lossless path encoding. Keep malformed status/path/diagnostic rejection; avoid the stored-row-to-native-row round trip. Likewise, an intentionally owned public getter should construct its final native result directly, rather than first allocating another owned stored row.

Test healthy files, numerous diagnostics in the same shard, zero-record files and multi-record files. Counters must show zero full-file materializations on the healthy publication path and at most one changed metadata-shard rebuild per atomic group. This does not authorize delaying visible file status or making separate publication groups appear atomically complete.

### 6.7 Inference pruning must modify only affected components — C13

At the inspected head, `needs_inference_prune` alone enters the same branch as a status refresh. That branch clones `IndexedCollectionSegment`, materializes file and record rows, serializes them again, and refreshes sorting before applying the inference change. The clone can copy native bigram/sort payloads even though only an inference component changes. Within pruning, `thaw_storage` reconstructs values, lengths and structural dictionaries before the value dictionary is cleared.

Separate status and inference changes. An inference-only update must share file/record archives, file/ID lookups, sorting and bigram payloads. A status-only change must not thaw unrelated postings. If the current aggregate type prevents sharing unchanged large native payloads, introduce narrowly scoped immutable owners for those payloads or another component-level replacement mechanism; do not mutate a segment still owned by a pinned view.

When an inference decision drops the value dictionary, replace it directly with the canonical completed empty dictionary. If exact empty-string postings must survive, borrow and retain/copy only that bitmap, then construct the tiny replacement dictionary. Preserve unchanged lengths and structural dictionaries; do not deserialize them through `thaw_storage`. Recompute statistics only for the altered component and validate its new capabilities/domain once.

Preserve queries against earlier progressive views, including their earlier inference decisions. Never mutate shared archives or assume `Arc` uniqueness across publication callbacks. Measure this path with inference enabled and global decisions that actually prune dictionaries; a schema that never enters the branch cannot establish the saving.

### 6.8 Duplicate-ID checking during construction — C14

`with_segment_mode` checks each incoming ID against every previously published segment, then constructs a temporary `RecordId` to probe the base collection even when the base collection has no records. For N records across S similarly sized segments, the failed cross-segment probes grow on the order of N times S. This work runs during coordinator preparation. It predates rkyv but is a potential opportunity to beat native, especially with many segments.

First eliminate the unconditional owned-key conversion for an empty base ID lookup and use lawful borrowed lookup for nonempty bases. Do not replace exact duplicate checking with hashes alone or drop checks because files usually have unique IDs.

Then benchmark a construction-only per-collection exact ID registry owned by the build coordinator. Insert/check each admitted ID once, release the registry at completion, and preserve duplicate detection across files, segments and multi-record inputs. Keep collection namespaces separate. Stage a physical-file publication group transactionally: failed validation/cancellation must not leave IDs committed to the registry. Revision conflicts still prevent publishing work prepared against a changed root.

The registry is temporary and must not become another retained native catalog. Measure its peak memory and copied ID bytes. Compare it against the existing checks on both few-segment and many-segment workloads. Retain it only when the benefit justifies the measured temporary memory and full-build results improve without violating the predecessor's memory requirements. Record a rejected experiment honestly. Preserve the exact duplicate error and old-view behavior.

### 6.9 Avoid full build machinery for empty repository shells — C15

The progressive builder calls `build_fresh(..., false)` for initial and final repository shells. Even with discovery disabled, this routine initializes a build pool, constructs/finalizes empty components, drops that pool, and creates query workers. The real build has its own pool. This setup is inherited, but its empty finalization now includes archives.

Introduce a private lightweight configured-state constructor for these shells, reusing prepared configuration and task-owned pools where their lifetimes permit. Preserve the query pool required by already published views; share immutable worker-pool ownership when appropriate instead of invalidating a live view. Do not process empty schemas through every ordinary finalization step merely to obtain headers.

Measure pool creation/destruction counts and empty component serialization counts. Preserve source registration, configuration identity, sort/inference bindings, cache isolation and empty collection behavior. Do not solve this by re-enumerating files or reusing a mutable Repository across independent published views. Avoid a broad rewrite of public constructors.

## 7. Allocation and critical-path evidence — C09

Retain the predecessor's unique-owner memory accounting. Extend its optional work counters rather than adding an independent conflicting reporting system.

| Phase | Required evidence |
| --- | --- |
| Value extraction/insertion | Owned key creations, copied key bytes, repeated-key hit allocations, scratch capacity |
| Batch aggregation | First-batch transfers, bitmap members enumerated, field-container rebuilds, merge scratch allocations |
| Dictionary freezing | Surviving keys, temporary ordinal-tree constructions, archive length/capacity, serialization and validation durations |
| Validation passes | Logical/semantic/structural visits, owned keys produced solely by validation, and completed-component reuse |
| Catalog finalization | Initial/final shard builds by kind, unchanged path/ownership owner reuse, per-publication metadata rebuilds |
| Coordinator setup | Empty-shell finalizations, pool creation/destruction, duplicate-ID probes and temporary registry bytes |
| Row/lookup serialization | Intermediate owned ID/path/diagnostic bytes, temporary entry capacity |
| Collection finalization | Compatibility rows/maps/order arrays created, inventory copies, sort payload clones, relation dictionary states |
| Publication | Query-ready wall time, coordinator time, validation/freezing on the publication path |
| Lifetime | End RSS, peak RSS, live allocations, native caches, old pinned publications, worker-buffer capacity |

Logical work counters alone are not allocation proof. Use a scoped allocation-counting microbenchmark and a heap/allocation profiler supported by the local environment. Record the tool, build options and scope. Keep instrumentation out of the normal release benchmark, or apply identical instrumentation to both revisions in a separate experiment. Do not add global locks or per-record atomic increments to production hot loops.

Report wall time separately from accumulated worker CPU/phase time. Overlapping worker durations are not additive elapsed time. Measure enough coordinator/worker attribution to establish whether serialization, memory bandwidth, allocation, aggregation or a serial final pass controls the remaining gap.

Release finished input buffers promptly. Reuse only scratch owned exclusively by the worker; never overwrite an archive retained by a current or pinned publication. Keep the scheduler's bounded outstanding window. Do not retain all completed batches or all intermediate archive roots until the whole repository finishes.

The acceptance measurement includes parsing, extraction, aggregation, final serialization, structural validation, semantic validation, catalog/ownership construction and query-ready publication. Also report total command wall time separately. Do not stop the timer before finalization, force a cleanup only in the final benchmark, call allocator trimming only for one revision, or move required work into the first query.

## 8. Required regression and allocation tests

Keep predecessor R01–R14/T01–T38 coverage. Add the following tests with explicit names in the final report. Expected results must use native reference operations or an independent logical model; comparing two paths sharing the same new serializer is insufficient.

| ID | Required test and evidence |
| --- | --- |
| P01 | Ordered-source archive versus native map for empty, singleton, node-boundary and multi-level trees; lookup, missing keys, forward/reverse iteration and bounded ranges |
| P02 | Mixed scalar types, exact large decimals, Unicode/string/ID values, number equality and datetime offset cases retain native order and results |
| P03 | Empty postings removed before ordinals; dense ordinal-to-bitmap correspondence; no extra native ordinal-tree construction or key clone during freeze |
| P04 | Unordered/duplicate input rejected by the private source constructor; inaccurate iterator length not accepted as valid source; checked access and semantic corruption regressions retained |
| P05 | Direct insertion removes intermediate key copies; scratch-lookup experiment measures warmed hit allocations and extra searches on misses separately, including all-unique and mixed-cardinality cases |
| P06 | High-cardinality insertion owns each new key once; compare single-search and scratch paths; retained scratch preserves mixed variants, oversized values and bounded unused capacity |
| P07 | Multi-value duplicates, nulls, empty strings/arrays, incomplete fields and inferred/custom postings preserve coverage, lengths and result sets |
| P08 | Offset-zero first-batch adoption matches a reference shifted merge for every moved family, with no bitmap member enumeration in the adoption operation |
| P09 | Nonzero-base, second-batch, mismatched-schema, empty-batch and overflow cases do not take an invalid adoption path; overlap remains rejected |
| P10 | Multi-batch field aggregation preserves every field and custom/inferred component under 1/8/32 workers; no per-batch field-container reconstruction/sort |
| P11 | Sort configuration-only construction preserves all bindings/composites/options; payload lengths and capacities zero; first/last page, ties and descending sort unchanged |
| P12 | Borrowed row/lookup serialization survives dropping input buffers and source vectors; empty, Unicode/non-UTF-8 path and diagnostic cases retain exact metadata |
| P13 | Normal build and diagnostics do not initialize segment compatibility caches; public compatibility access remains correct and its one-time cost is reported |
| P14 | Collection compatibility data constructed once, global ID lookup/identity order correct across segments and collections; pinned views remain valid |
| P15 | Relation-enabled final Repository and progressive Index contain no reachable native posting builders; relation lookups remain correct after target update/delete and compaction |
| P16 | Archive/conversion failure and cancellation publish nothing; old views remain usable and temporary storage is released |
| P17 | Addition, replacement and deletion after optimized construction match fresh reconstruction and the independent model before/after bounded and full compaction |
| P18 | Repeated build/drop and deliberately pinned old views distinguish retained current storage from expected old-generation retention; no owner cycle introduced |
| P19 | Final catalog rebase rebuilds zero unchanged path shards, preserves ownership versions/routes, and permits immediate update/delete on the completed progressive Index without re-adoption |
| P20 | Bitmap-only validators allocate no key strings; changed components receive required validation once, unchanged sealed components avoid repeated full passes, and corruption/domain checks still fail correctly |
| P21 | Healthy file publication materializes no complete file/segment rows; real status/diagnostic edits rebuild each affected metadata shard once per atomic publication group |
| P22 | Inference-only pruning shares row/lookup/bigram/sort owners, retains only required empty-string postings without thawing unrelated dictionaries, and preserves pinned pre-pruning views |
| P23 | Duplicate-ID experiment detects duplicates across segments/files and within a physical-file group, handles rollback/conflicts, and reports probe counts plus temporary registry bytes on few/many segments |
| P24 | Lightweight repository shells preserve empty/nonempty behavior and pool/view lifetimes while eliminating unnecessary build-pool creation and repeated empty finalization |

Use large nonempty strings and Roaring payloads in allocation tests so a cheap empty allocation cannot masquerade as ownership transfer. Do not require pointer equality for allocator internals or exact total allocation counts across unrelated standard-library versions. Count the operation under test; exclude fixture setup and necessary bitmap capacity growth. A test-only clone/work counter is useful but must be corroborated by scoped allocation evidence.

P17 must include changed record counts in a multi-record file, identical size/mtime replacement via the explicit mutation API, zero-record files, malformed/raw fallback files, changes of indexed values, relation target changes, sorting, FTS, NOT/complements and pagination. Keep source-free compaction checks and existing stale-view/cache tests. Use disposable fixtures; never mutate the Musenalm reference corpus.

Run repository formatting, relevant core/workspace tests, release benchmark compilation and CLI/TUI/GUI compile checks using the repository's actual manifests and documented commands. Discover exact package names once; do not invent commands or silently skip a target. Record toolchain/dependency failures separately from passing checks. New serialization adapter code requires focused tests and a review of every added unsafe block; use safe supported APIs wherever possible.

## 9. Measured optimization sequence if a gap remains

Follow the revised order in section 3.1 and measure each isolated change against the completed predecessor. Complete C01–C15, distinguishing mandatory work removal from rejected benchmark-dependent experiments, and measure the combined result. Do not start by replacing the entire initial accumulation architecture.

If native parity remains unmet, select the largest measured remaining cause and run the following bounded experiments where applicable, in this order. Record each experiment even when rejected; retain only a demonstrated improvement without semantic or memory regressions.

1. **Finalization buffer reuse.** Reuse serializer scratch within a worker and reserve output capacity using prior comparable components or a conservative measured estimate. The completed archive must own its allocation. Do not copy every finished archive into another buffer merely to recycle the original. Quantify over-reservation and peak overlap.
2. **Batch reduction overhead.** Measure one versus several compatible batches within the unchanged segment/file limits. Optimize repeated merging and scheduler work without changing default segment policy or required progressive visibility. Do not serialize fine-grained batches only to deserialize/rebuild them during segment completion.
3. **Ordered merge finalization.** If reconstructing the aggregate native dictionary dominates, experiment with merging sorted batch dictionaries, combining equal-key postings, and producing the exact-length ordered entry vector directly. Use checked slot rebasing, preserve key equality rules and move bitmap payloads whenever offsets permit. Keep this private to construction; do not add a second persistent query representation.
4. **Alternative initial accumulation.** Only if tree insertion still dominates, compare one private vector-and-sort/group prototype against the retained native builder. Include low/high cardinality, long strings, multi-valued fields and exact numbers. A vector of every occurrence can greatly increase peak memory; reject it if it merely exchanges build time for the prototype's memory problem. Do not switch to a hash accumulator without separately validating hashing/equality for all mixed numeric keys.

Each experiment needs a focused microbenchmark and full query-ready construction measurement. A microbenchmark improvement alone is not acceptance. Do not tune default node fanout, replace reverse traversal, change record IDs, reduce validation or omit index families as an unexplained shortcut.

If the residual is dominated by necessary archive serialization/validation or required public compatibility storage, quantify it. Identify the concrete architecture or API change that would be needed next. Do not invent an overnight speed guarantee or keep making unrelated changes after the cause is established.

## 10. Benchmark protocol and native-performance gates

### 10.1 Revisions and measurement matrix

Use separate worktrees, target directories and executables for native baseline N, completed predecessor repair R, and this follow-up F. The original prototype is historical context; it is not the primary performance target. Record exact commits and build commands. Compare on the same machine, release profile, toolchain, allocator, corpus, schema, flags, segment policy and CPU availability.

Measure both direct Repository construction and progressive Index construction; both must finish with equivalent fully usable indexes. Run ASCII and Unicode at 1, 8 and 32 workers where supported: twelve build configurations. Unsupported configurations are unmeasured, not passed. Use the same actual corpus for all runs and verify file/record/index-family counts and query result digests.

The decisive comparison is F versus N. F versus R isolates this follow-up's improvement. Refresh native measurements; old reported numbers from another run or machine are not a pass/fail baseline.

Warm the source-file page cache consistently for the primary CPU/construction comparison, with one untimed warm-up per executable/configuration. Every timed sample runs in a fresh process. Do not confuse warm source cache with a preexisting built index. Report cold-source measurements separately if performed; never mix them into primary medians.

Run at least seven paired N/F rounds per configuration, alternating execution order by round. Preserve every raw sample, with an explicit exclusion reason for an externally interrupted run. Use at least three matched R samples per configuration for attribution, and repeat more if an R/F difference is close to noise. If primary results are close to the gate or unstable, extend once to fifteen paired rounds; do not rerun selectively until a favorable sample appears.

Record query-ready wall time, command wall time, first progressive publication latency, end RSS, peak RSS, unique archive length/capacity, live native payload estimates and cache state. Gather allocation profiles separately from timing runs. Hash source/configuration manifests outside the timed interval.

### 10.2 Construction gate

For each paired round calculate `F_query_ready_ms / N_query_ready_ms`. Report its median and a 95% paired-bootstrap interval, using a fixed recorded seed and 10,000 resamples of complete pairs. Report raw medians for both executables as well.

The target is a median ratio **at or below 1.00** in every supported configuration, including both 32-worker alphabets and both entry paths. The 32-worker cases are primary optimization cases, not a license to hide a single-worker regression.

Mark a configuration passed only when its median paired ratio is at or below 1.00 and the upper confidence bound is at or below 1.03. The 3% bound limits uncertainty; it is not permission to accept a measured 3% slowdown. A median above 1.00 is reported as a remaining regression. A wide interval is inconclusive, not proof of parity. After the fifteen-round extension, report unresolved noise accurately rather than inventing certainty.

Preserve first-publication behavior. Investigate a first-publication median worse by more than 5% and 10 ms, even if final publication is faster. Do not defer a material amount of required build work into first query, shutdown, a background task or an explicitly invoked compatibility call that standard consumers immediately need.

The final overall construction gate requires all supported cells to pass. A table with one passing cell is not “native performance restored.” Faster-than-native claims require the improvement to exceed the measured uncertainty.

### 10.3 Query, mutation and memory safeguards

Run the full existing Musenalm query suite through both entry paths and alphabets, with the predecessor's exact result, ordering and evidence comparisons. Keep caching configuration identical. Measure first representative query and warm suite separately. Preserve the verified-cache and relation/lifecycle regression matrix.

Target no query regression. Investigate an aggregate query-time increase beyond 3%, or an individual query increase beyond 10% and 0.5 ms. Require a demonstrated regression to be fixed or explicitly reported as a failed safeguard; do not average it away with unrelated fast queries. Repeat borderline cases with enough samples to distinguish timing noise.

Repeat predecessor addition/replacement/deletion and bounded/full compaction benchmarks. These optimizations must not restore expensive native reconstruction on mutation or introduce catalog-wide work. Work-bound and source-free correctness checks remain hard gates regardless of timings.

Target end and warm memory at or below native. At minimum, F must not materially regress against R, and every residual above N must be attributed. Report archive capacity, native structures, compatibility data, scratch and old pinned owners separately. A larger index than source bytes is not itself a correctness error, and process RSS is not the exact sum of live allocations. Nevertheless, the earlier large unexplained increase is not acceptable evidence of a completed memory repair.

A speed gain obtained by retaining another full index, warming every compatibility cache, increasing unbounded worker buffers or hiding peak memory fails this task. Report construction, query and memory outcomes separately; do not label one as passing because another improved.

### 10.4 Required evidence files

Under the evidence directory retain `revisions.json`, `build-samples.tsv`, `build-summary.tsv`, `allocation-summary.tsv`, `metadata-ownership.md`, `query-results/`, `lifecycle-results/`, `profiles/` and `implementation-report.md`. Use the existing benchmark output format where available and add a conversion script only when needed.

`build-summary.tsv` must identify entry path, alphabet, workers, N/R/F commits, sample counts, medians, paired ratio, confidence bounds, end/peak RSS and gate result. Distinguish not run, inconclusive, failed and passed. Keep raw profiler output and exact commands; do not include source document contents or unnecessary local secrets.

## 11. Implementation phases and final delivery

| Phase | Work and checkpoint | Suggested commit |
| --- | --- | --- |
| 1 | Freeze predecessor revision; reproduce ownership rebase defect; establish coordinator/worker and N/R evidence | `test(core): establish archived construction performance baselines` |
| 2 | C11 bitmap-only validation and single completion boundary; P20 and corruption/domain regressions | `perf(core): eliminate redundant posting validation work` |
| 3 | C10 component-preserving catalog rebase and parallel shard construction; P19 | `perf(core): reuse catalog components during final publication` |
| 4 | C12 borrowed publication checks and batched diagnostics; P21 | `perf(core): avoid file materialization during segment publication` |
| 5 | C01 ordered archive source; P01–P04 | `perf(core): serialize posting trees from ordered entries` |
| 6 | C02 direct conversion, C05 configuration-only sorting; separately measure scratch lookup; P05–P07/P11 | `perf(core): eliminate repeated key and sort payload allocations` |
| 7 | C03 first-batch adoption and C04 simple in-place merging; optional field-slot experiment; P08–P10 | `perf(core): reuse batch and field aggregation storage` |
| 8 | C06/C07/C08 metadata and relation requirements; C13 component-local inference pruning; P12–P16/P22 | `perf(core): limit metadata and inference reconstruction` |
| 9 | C15 lightweight shells and C14 duplicate-ID experiment; P23/P24; retain only measured improvements | `perf(core): reduce construction coordinator overhead` |
| 10 | P17/P18, predecessor coverage, C09 attribution and justified section 9 experiments | `test(core): verify optimized construction and lifecycle invariants` |
| 11 | Complete matched N/R/F matrix and gate analysis | `bench(core): compare archived construction against native performance` |
| 12 | Report passed, failed and rejected experimental items; push reviewable branch and leave merge to user | `docs: report archived construction performance results` |

Keep each change attributable. Once relevant tests pass, do not repeatedly run the entire workspace suite after documentation-only edits. Run the full required checks on the final implementation and repeat affected checks after subsequent code changes.

The final report must provide a C01–C15 status table, P01–P24 test mapping, retained predecessor coverage, measured time/allocation change per phase, and the complete native comparison matrix. Describe every remaining native structure and why it remains. State clearly whether every native construction gate passed, which query/memory safeguards passed, and what blocks any failure.

If performance remains below target, retain the useful tested changes and report the largest measured residual, its owner/call path, the evidence, experiments attempted and the next concrete change required. Do not mark the document done, merge master or start persistence work. If all gates pass, move the task document to done with the final report, commit, push and leave one integration branch for the user to merge.

## 12. Source references

Additional anchors from committed repair `45a81a85be5f8a122a3272ce3369ef2ed81632d8`:

- [Validator using an owned-key adapter](https://github.com/Simon-Martens/objekte/blob/45a81a85be5f8a122a3272ce3369ef2ed81632d8/crates/core/src/posting.rs#L2123)
- [Dictionary visitation and existing bitmap-only visitor](https://github.com/Simon-Martens/objekte/blob/45a81a85be5f8a122a3272ce3369ef2ed81632d8/crates/core/src/posting.rs#L1116)
- [Repeated segment finish/freeze/validation](https://github.com/Simon-Martens/objekte/blob/45a81a85be5f8a122a3272ce3369ef2ed81632d8/crates/core/src/mutation_segment.rs#L545)
- [Completion boundary repeats statistics/validation](https://github.com/Simon-Martens/objekte/blob/45a81a85be5f8a122a3272ce3369ef2ed81632d8/crates/core/src/native_segment.rs#L346)
- [Serial catalog construction](https://github.com/Simon-Martens/objekte/blob/45a81a85be5f8a122a3272ce3369ef2ed81632d8/crates/core/src/lifecycle_catalog.rs#L719)
- [Whole-catalog rebase and ownership reset](https://github.com/Simon-Martens/objekte/blob/45a81a85be5f8a122a3272ce3369ef2ed81632d8/crates/core/src/lifecycle_catalog.rs#L869)
- [Publication materializes file entries](https://github.com/Simon-Martens/objekte/blob/45a81a85be5f8a122a3272ce3369ef2ed81632d8/crates/core/src/lifecycle.rs#L2042)
- [Diagnostic merge reconstructs a complete segment file](https://github.com/Simon-Martens/objekte/blob/45a81a85be5f8a122a3272ce3369ef2ed81632d8/crates/core/src/segment_archive.rs#L566)
- [Cross-segment duplicate-ID probes](https://github.com/Simon-Martens/objekte/blob/45a81a85be5f8a122a3272ce3369ef2ed81632d8/crates/core/src/lifecycle.rs#L991)
- [Inference pruning rewrites metadata and thaws discarded values](https://github.com/Simon-Martens/objekte/blob/45a81a85be5f8a122a3272ce3369ef2ed81632d8/crates/core/src/lifecycle.rs#L2247)
- [Initial empty shell](https://github.com/Simon-Martens/objekte/blob/45a81a85be5f8a122a3272ce3369ef2ed81632d8/crates/core/src/repository_incremental.rs#L1530)
- [Final empty shell](https://github.com/Simon-Martens/objekte/blob/45a81a85be5f8a122a3272ce3369ef2ed81632d8/crates/core/src/repository_incremental.rs#L1948)

Earlier anchors for findings unchanged by the second inspection:

These references are review anchors. The agent must work against the completed predecessor and inspect changes since these revisions.

- [Posting freeze and extra ordinal tree](https://github.com/Simon-Martens/objekte/blob/1999c49fdf156853bc85253e3dc0f8a85e1155fc/crates/core/src/posting.rs#L1190)
- [Stored scalar key conversion](https://github.com/Simon-Martens/objekte/blob/1999c49fdf156853bc85253e3dc0f8a85e1155fc/crates/core/src/posting.rs#L498)
- [Value insertion](https://github.com/Simon-Martens/objekte/blob/1999c49fdf156853bc85253e3dc0f8a85e1155fc/crates/core/src/posting.rs#L2046)
- [Shifted posting merge](https://github.com/Simon-Martens/objekte/blob/1999c49fdf156853bc85253e3dc0f8a85e1155fc/crates/core/src/posting.rs#L1756)
- [Per-batch field-container reconstruction](https://github.com/Simon-Martens/objekte/blob/1999c49fdf156853bc85253e3dc0f8a85e1155fc/crates/core/src/posting.rs#L2894)
- [Empty aggregate and batch admission](https://github.com/Simon-Martens/objekte/blob/1999c49fdf156853bc85253e3dc0f8a85e1155fc/crates/core/src/mutation_segment.rs#L507)
- [Sort cloning and late relation reconstruction](https://github.com/Simon-Martens/objekte/blob/1999c49fdf156853bc85253e3dc0f8a85e1155fc/crates/core/src/repository_incremental.rs#L1989)
- [Segment metadata serialization](https://github.com/Simon-Martens/objekte/blob/1999c49fdf156853bc85253e3dc0f8a85e1155fc/crates/core/src/segment_archive.rs#L238)
- [Global collection compatibility reconstruction](https://github.com/Simon-Martens/objekte/blob/1999c49fdf156853bc85253e3dc0f8a85e1155fc/crates/core/src/repository_incremental.rs#L1098)
- [Public collection state fields](https://github.com/Simon-Martens/objekte/blob/1999c49fdf156853bc85253e3dc0f8a85e1155fc/crates/core/src/repository.rs#L146)
- [rkyv 0.8.18 archived B-tree API](https://docs.rs/rkyv/0.8.18/rkyv/collections/btree_map/struct.ArchivedBTreeMap.html#method.serialize_from_ordered_iter)
- [rkyv 0.8.18 bulk construction implementation](https://docs.rs/rkyv/0.8.18/src/rkyv/collections/btree/map/mod.rs.html)
- [Rust BTreeMap lookup and borrowed-key ordering requirements](https://doc.rust-lang.org/std/collections/struct.BTreeMap.html#method.get_mut)

**Required result:** one isolated, tested integration branch with all avoidable construction work addressed and an honest measured verdict against the native baseline. Faster native performance is the objective; a promise or a partial benchmark is not evidence.

## Implementation report — performance acceptance pending

Task: `rkyv-construction-performance`

Date: 2026-11-17

Starting revision: `e4a7471ffe777a9a4992dff34d7070edf372e873`

Implementation head:
`65cb66dc5b` (`perf(core): share custom text payloads`)

Integration branch:
`integration/rkyv-memory-index-repair`

Integration worktree:
`/home/simon/source/objekte-integration-rkyv-memory-index-repair`

### Verdict

The construction-work repairs and required correctness coverage are complete.
The native-performance gate is not complete. The document therefore remains in
`tasks/` rather than being moved to `done/`, and the branch has not been merged
into `master`.

### Implemented construction repairs

- Posting dictionaries serialize directly from checked owned ordered entries;
  the temporary native ordinal B-tree and its key clones are gone.
- Empty posting dictionaries share immutable archives by key type.
- Scalar insertion converts `Value` directly to the stored representation.
  The reusable-scratch experiment was rejected after a 16.9% all-unique
  regression.
- The first batch is adopted by moving compatible ownership. Later field maps
  merge in place without drained/sorted/reconstructed work vectors.
- Sort compatibility headers are configuration-only with zero-capacity payload
  vectors.
- Record diagnostics move into archive rows. Record lookup serialization uses a
  borrowed ordered reference vector instead of an owned native string map.
  `RecordId` stores `Arc<String>` so the final row freeze can move a unique ID.
- Catalog rebase reuses unchanged path and ownership shards, rebuilds only
  changed metadata shards, and serializes shards through the build pool.
- Initial publication uses borrowed file validation and one batched metadata
  edit per touched ID shard. Healthy files create no native file/record cache.
- Validation is bitmap-only and fused into one completed-component pass.
  Sealed unchanged segments bypass repeat freeze/validation.
- Relation reverse dictionaries are frozen and validated before publication.
- Raw text, fallback text, custom text and sorting use immutable shared owners.
  Inference-only pruning changes only value dictionaries and sort state.
- A bounded construction-only duplicate-ID registry replaces repeated
  cross-segment probes and is released before final publication.
- Empty repository shells use a lightweight configured-state constructor with
  no construction pool or repeated empty finalization.

### Construction result

Seven fresh-process samples per revision and configuration:

| Alphabet | Workers | Native ms | Predecessor ms | Follow-up ms | Follow-up/native |
| --- | ---: | ---: | ---: | ---: | ---: |
| ASCII | 1 | 3,671.0 | 4,177.1 | 4,016.4 | 1.094x |
| ASCII | 8 | 808.0 | 1,192.5 | 987.6 | 1.234x |
| ASCII | 32 | 649.4 | 1,073.4 | 832.1 | 1.287x |
| Unicode | 1 | 4,393.9 | 5,047.7 | 4,775.5 | 1.085x |
| Unicode | 8 | 930.8 | 1,345.5 | 1,099.2 | 1.173x |
| Unicode | 32 | 702.9 | 1,140.4 | 897.6 | 1.270x |

The follow-up improves predecessor 32-worker construction by 19–22% and
8-worker construction by about 17%. Every native construction gate remains
failed.

### Query, memory and validation

All 150 query cases completed through direct and progressive paths for both
alphabets with zero semantic digest mismatches. Aggregate query time remains
1.054x native direct and 1.047x native progressive. Focused three-repetition
Q119 Unicode retries remain above the individual query safeguard.

End and peak RSS improved in every construction cell versus the predecessor.
On 32-worker ASCII, end RSS fell from 839.6 to 721.6 MiB and peak RSS from
902.2 to 784.8 MiB. Native remains lower at 575.2/615.6 MiB.

The final workspace suite, core library suite, compaction API test,
all-target workspace check, detailed-profiling all-target check and diff check
all passed. The deterministic five-seed lifecycle matrix passed with fresh
rebuild/model comparison after every operation.

### Remaining blocker

Segment finalization still dominates the remaining gap. On the critical
contents shard, bigram packing alone takes roughly 186 ms; required archive
validation, row/lookup construction and compatibility metadata add serial work
inside that shard. The next bounded optimization is to overlap bigram packing
with posting, lookup and row-archive finalization while retaining the existing
scheduler and archive ownership model.

The complete C01–C15 status, P01–P24 mapping, raw benchmark matrix, query
results, memory accounting and residual analysis are in
[`implementation-report.md`](../generated/rkyv-construction-performance/implementation-report.md).
