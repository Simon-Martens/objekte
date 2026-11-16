# Objekte native segment construction implementation plan

Status: implementation specification for a local coding agent. No implementation is included.
Task identifier: native-segment-construction
Prepared: 2026-11-16
Inspected master: c54486f893a57fac0b1b80ed967ab4f5a5fd26c9
Integration branch: integration/native-segment-construction
Integration worktree: /home/simon/source/objekte-integration-native-segment-construction

## 1. Required outcome

Make native segments the ordinary unit of index ownership, construction, publication and query execution. Each segment contains the indexes for a bounded group of records. Within the segment, each field index remains an independent native component.

Build several segments concurrently. Also allow independent components within a segment to merge and finalize concurrently. Keep the existing standard-library BTreeMap and HashMap implementations, Roaring bitmaps, packed bigram construction buffers, sorting representations and existing Rayon dependency.

This is a native architecture refactor. It is not an ic-stable-structures, RocksDB, redb, rkyv or Tantivy implementation. Do not introduce any new dependency, storage engine, allocator framework, serialization format, memory mapping, disk cache or automatic persistence. Do not modify the separate ic-stable prototype specification or import its implementation branch.

The agent must implement this plan in the Objekte repository, verify correctness and performance, and leave one reviewed integration branch for the user to merge. Reading or receiving the document alone does not start implementation; start when the user assigns it.

The essential result is two-dimensional construction:

| Record partition | Structured author index | Structured year index | Title bigram index | Description bigram index |
| --- | --- | --- | --- | --- |
| Segment A | Independent component A1 | Independent component A2 | Independent component A3 | Independent component A4 |
| Segment B | Independent component B1 | Independent component B2 | Independent component B3 | Independent component B4 |

This table is a logical ownership and scheduling model. It does not require a matrix allocation, one thread per component, a trait object per posting, or a file per component.

## 2. Constraints and priorities

1. Preserve current query semantics and native representations.
2. Remove collection-wide posting aggregation from the production construction path.
3. Make aggregate merging and finalization execute in worker jobs, rather than serial coordinator callbacks.
4. Share the same segmentation policy between ordinary repository construction, progressive construction, later changes and compaction.
5. Keep all record IDs inside postings local to their segment.
6. Keep component lookup borrowed and inexpensive. Do not rebuild a collection-wide index to answer a query.
7. Preserve the existing source adapters, extraction, validation, malformed-file fallback, fingerprints and file revision rules.
8. Preserve existing high-level search, count, streaming, sorting, lookup and lifecycle behavior.
9. Keep memory-only operation. This task adds no file writing or loading.
10. Do not add adaptive size prediction or exact B-tree heap accounting. A simple deterministic rule is required for this implementation.

Speed takes priority over elaborate infrastructure. Reuse current structures and algorithms. A small concrete interface is preferable to a general storage-backend framework.

## 3. Repository workflow

Read the current AGENTS.md and docs/ai/README.md before implementation. Preserve uncommitted user files and the original checkout's branch and index.

Use a sibling integration worktree. Start from the current master available when implementation begins, and record its exact commit. The inspected commit above is evidence, not permission to reset a newer master.

If the configured remote has newer master work, fetch and use that baseline without modifying the original checkout. Do not merge an experimental storage branch. Preserve and review any already-existing task branch rather than overwriting it.

Use the task identifier consistently in branch names, notes and reports. Push immediately after each commit as required by repository workflow. Do not merge into or push updates to master.

One agent may implement and supervise the entire task. If the user uses subagents, delegate only after the segment ownership and read interfaces are agreed; independent ownership areas are construction scheduling, query integration, and validation. Integrate their work on the task integration branch.

The suggested eventual task location is docs/ai/tasks/objekte_native_segment_construction_implementation_plan.md. Follow the repository's rules for untracked task documents. On completion, move the task to docs/ai/done/, append the implementation report, commit and push it on the integration branch.

## 4. Existing implementation: reuse and correct it

The following observations are grounded in the inspected master.

| Code area | Existing role | Required treatment |
| --- | --- | --- |
| crates/core/src/build.rs | Parallel preparation, BatchContribution, packed component contributions and serial CollectionAdmission | Retain extraction and bounded source scheduling; separate lightweight planning from heavy aggregation |
| crates/core/src/mutation_segment.rs | SegmentAdmission, CollectionSegment, IndexedCollectionSegment, SegmentAggregate, consolidation and compaction | Reuse native payloads; make ordinary initial construction use them directly; move general segment definitions out of the mutation-specific module |
| crates/core/src/repository_incremental.rs | Progressive construction into segments, with eager aggregation during ordered admission | Replace the eager serial aggregate with bounded segment jobs |
| crates/core/src/repository.rs | Ordinary Repository::open builds collection-wide native indexes | Route ordinary construction through the same segmented builder |
| crates/core/src/lifecycle.rs | PublishedSegment, separate ActiveRecords, immutable publications and segmented views | Reuse publication and liveness concepts; eliminate the production base-versus-delta split |
| crates/core/src/lifecycle_partition.rs | Converts a completed flat base into small partitions | Remove this conversion from normal startup; retain only helpers still needed by compaction or tests |
| crates/core/src/index_catalog.rs | Borrowed field access; IndexCatalog::collection and IndexCatalog::segment already exist | Extend and consolidate this interface, rather than creating a competing index lookup layer |
| crates/core/src/executor.rs | Segment filter bounds, FTS candidates and native planning | Use the segment catalog and local active domain consistently |
| crates/core/src/run.rs | Both flat and segmented execution, merging, sorting, streaming and direct lookup | Reuse segmented execution and retire duplicate flat production paths |
| crates/core/src/relation_query.rs | MergedRelationView and segment-qualified record references | Preserve cross-segment relation behavior and remove the special flat-base address case |
| crates/core/src/bigram.rs | Packed construction buffers; normal and skip HashMap<u64, RoaringBitmap> outputs | Preserve representations and text semantics |
| crates/core/src/lifecycle_catalog.rs | Revision-aware file catalog, currently tied to a base Repository | Keep catalog semantics; prevent the catalog from permanently retaining obsolete segment payloads |

Important current limitations:

- Repository::open_with_sources calls build_fresh, which builds whole-collection structures.
- The progressive path prepares batches concurrently, but its admission callback still performs index merges into SegmentAggregate.
- CollectionSegment::into_indexed_profiled merges batches and finalizes several index families sequentially.
- The native Musenalm build benchmark uses Repository directly. Improving only Index construction would leave that important path unchanged.
- IndexCatalog already provides the main borrowed field access seam.
- Current 2,048-record partitions, 132,000-record progressive batches and mutation defaults are different policies. Do not preserve those differences under new names.

## 5. One simple segmentation rule

### 5.1 Default policy

Introduce one SegmentPolicy in the core library. It has two public or crate-level configuration fields, according to the existing options conventions:

    target_records: NonZeroUsize
    max_files: NonZeroUsize

Default both to 16_384.

Expose the policy through RepositoryOptions, with the same option used by all construction entry points. Update in-repository struct literals and defaults. Provide a benchmark option --segment-records; this changes target_records and, unless explicitly overridden in internal tests, max_files to the same value.

Reject values exceeding the supported local-slot range. Use checked arithmetic. Tests may use tiny limits such as 2, 3 or 7.

The record target is a soft boundary at completed contribution-batch boundaries. The file limit prevents an unbounded segment consisting of zero-record files. Neither is an exact memory limit.

Do not estimate a B-tree's size for every insertion. Existing source-job and prepared-input admission limits remain separate scheduling controls.

### 5.2 Exact boundary algorithm

Process contributions for each collection in the established deterministic source/job order.

For an incoming completed contribution:

1. Determine its record count and the number of distinct physical files it would add to the current draft. A later range of an already-seen file does not count as a new file.
2. If the draft is nonempty and admitting the contribution would exceed either policy limit, seal the existing draft before admitting it.
3. Move the contribution into the next draft. Record its segment-local base offset; do not merge its postings at this step.
4. If the draft has reached a limit, seal it at that contribution boundary.
5. At end of input, seal the final nonempty draft.
6. An explicit lifecycle publication request may seal a smaller draft. There is no minimum segment size and no requirement to wait until 16,384 records exist.

Do not split already-built posting maps merely to reach exactly 16,384 records. A single indivisible contribution larger than a target is accepted as an oversized segment and reported. Subsequent normal contributions start a new segment.

Ordinary extraction batches should remain bounded by the existing preparation machinery. Oversized contribution handling must not become a way to place an entire ordinary collection in one segment.

Deterministic ordering refers to planning, not worker completion. For the same input, policy and adapter behavior, changing the worker count must not change the segment boundaries or logical result order.

### 5.3 Files with multiple records

A physical file can contribute records to several segments. Existing framed-file chunks can be assigned to separate drafts. Do not force every JSONL file into a single segment.

Preserve whole-file validation and revision semantics:

- Segments containing records from an unfinished framed file remain private until that file's required validation and fingerprint work completes.
- If the adapter's existing failure behavior invalidates earlier logical records, discard their private contributions and publish the established fallback outcome.
- Initial publication and replacement publication must not expose a mixture of invalidated and replacement records.
- Track all segment memberships of a file.
- Distinguish source-file framing flags from segment membership. A later framed chunk can be the first contribution from that FileId to a new segment. Create that segment's file membership using the existing file identity/revision metadata; do not require a second source-level first_range event or reread the file. Rewrite diagnostic/local-slot references using the destination segment's mapping.
- Count physical source reads and fingerprints once according to the existing adapter semantics, not once per field or resulting segment.
- Segment source-byte diagnostics are accounting data, not a reason to count the complete file size again for every chunk.

For adapters that emit one indivisible large record or contribution, retain the existing oversized-input exception and report it.

### 5.4 Use the same policy throughout

Apply this policy to:

- blocking initial construction;
- progressive initial construction;
- batches of additions and replacements;
- rebuilding;
- compaction output;
- merging small segments.

Remove the first-versus-later record-size distinction. If the UI needs an early searchable publication, use an explicit request to seal the current draft; do not reintroduce hidden per-path segment sizes.

Do not repartition successfully constructed segments into 2,048-record pieces during adoption, publication or a query.

## 6. Ownership and module structure

### 6.1 Canonical native segment

Use IndexedCollectionSegment as the initial payload model. Move its general definition and basic accessors into native_segment.rs, or equivalently rename the existing module with minimal mechanical changes. Keep mutation-specific functions in their existing lifecycle modules.

A segment owns:

- immutable segment identity and collection identity;
- its ordered record metadata and segment-local RecordSlot values;
- file membership and record ranges;
- declared and inferred field postings;
- named/custom postings;
- raw, fallback and configured field-text bigram components;
- presence, null, coverage, empty-array and other structural facts;
- sorting columns and composite sorting data;
- relation contributions retaining logical target identifiers;
- once-computed diagnostics and accounting.

Keep existing native typed fields. Do not put every component into a universal byte-value map.

A small immutable component descriptor may identify collection, segment, field or text binding, and component kind. This is for inspection, scheduling and future persistence boundaries; it does not introduce serialized files.

### 6.2 Liveness and identity

Reuse PublishedSegment: an Arc-owned index payload plus a separately versioned ActiveRecords mask.

Use a named SegmentId, preferably wrapping the existing monotonically assigned ordinal. Do not use a segment's position in a vector as a persistent identity.

Any record address that leaves a segment must include:

    collection ID
    segment ID
    segment-local slot

A bare RecordSlot is valid only alongside its segment. This applies to caches, relation addresses, direct lookup, result ordering and deferred record access.

Segment IDs are assigned at deterministic planning/publication positions, not in the order workers happen to finish. Compaction creates new segment IDs and updates affected routing. Old pinned views continue to resolve their old addresses.

### 6.3 One snapshot model for Repository and Index

Refactor toward one native snapshot containing the collection segment lists, catalog/routing information, liveness and revision identity. Reuse the existing PublishedIndex model instead of retaining a second independently populated flat index model.

Repository::open remains a blocking high-level entry point. It waits for the segmented builder and returns a static searchable view of the resulting native snapshot.

Index uses the same snapshot representation for lifecycle updates. Index::from_repository adopts/shares the completed segment payloads; it must not copy every posting, replay all records or call partition_base to manufacture segments afterward.

Separate immutable repository metadata/runtime configuration from index payload ownership where necessary. In particular:

- schemas, sources, collection definitions and text-binding descriptions can be shared independently;
- FileCatalog must not hold a base Repository that permanently owns superseded segment payloads;
- snapshot ownership must be acyclic;
- after compaction, obsolete segments should be retained only by actual old views or other deliberate references.

Do not solve API compatibility by keeping a duplicate collection-wide posting map.

Keep high-level Repository and Index query methods compatible. Low-level public CollectionState fields that assumed one whole-collection map may need replacement by segment iteration/accessors. Update all in-repository callers and document those source-level changes explicitly; do not silently return empty maps from compatibility fields.

## 7. Construction pipeline

Use these stages:

1. Discover files and prepare immutable collection/adapter configuration.
2. Read, parse and extract in bounded parallel jobs using existing adapters.
3. Route owned BatchContribution objects into deterministic SegmentDraft objects.
4. Build and finalize complete segment jobs on the shared worker pool.
5. Validate each completed segment once.
6. Publish complete segments and their routing/catalog changes in a deterministic order.

### 7.1 Lightweight routing

Introduce SegmentDraft and SegmentBuildJob, or adapt CollectionSegment to those roles.

The routing coordinator may:

- inspect batch counters;
- choose a segment boundary;
- assign local base offsets;
- move vectors/maps and contribution ownership;
- assemble small descriptors;
- submit jobs and install completed results.

It must not iterate through every posting key or bitmap member to aggregate an index.

In particular, remove ordinary coordinator calls that perform collection-wide or segment-wide merge_shifted work. Those operations belong to segment/component jobs.

The existing eager SegmentAggregate in the progressive callback must not remain the serial construction path behind a new scheduler name.

### 7.2 Reuse extraction; do not parse per field

BatchContribution already contains native posting contributions and packed gram contributions. Keep that efficient preparation output initially.

The first implementation does not need to redesign all extraction into column arrays. Move prepared component contributions into segment jobs. This gives parallel segment aggregation and field-level reduction without parsing source records again.

A segment job groups contributions by component using ownership transfer. It may walk the list of fields/components to distribute their owned maps, but it must not clone full maps, expand bitmaps into record vectors, or retain a second corpus-sized copy.

Borrow or share immutable schema and source descriptors. Do not retain a complete parsed Value tree for the entire collection to support field tasks.

### 7.3 Independent component jobs

Within a segment, reduce and finalize these independently where their current implementation permits:

- each declared field PostingIndex, including all of that field's structural facts;
- each named/custom posting component;
- each raw, fallback or field-text BigramContribution;
- each independent sort column;
- relation contributions, partitioned by existing relation declaration;
- inference components after their required classification decisions are available.

Metadata and local-slot mapping must be fixed before these tasks begin.

Composite sort structures run after their required columns are available. Do not block pool workers on child tasks via ordinary mutex/channel waits; use existing Rayon join/scoped work or a coordinator-controlled dependency stage.

Do not create a thread for every field. A shared pool executes component jobs. Combine very small component jobs into a serial loop if task overhead dominates; preserve the component ownership boundaries. Use a documented simple threshold based on contribution/item counts, not per-entry timing.

Parallelizing final bitmap conversion alone is insufficient. The potentially expensive native posting aggregation must also run outside the coordinator.

### 7.4 Scheduling and bounded work

Use the existing resolved build-worker count P and existing Rayon facilities. Do not create one pool per segment, per field or per source file.

Permit up to 2 × P segment build jobs in the planned/queued/running/completed-but-not-consumed window, with a minimum of one. Apply checked/saturating arithmetic for this scheduling limit. Bound prepared extraction queues separately using the existing weighted admission mechanism.

A window slot remains occupied until the corresponding result is consumed or discarded. Completed results waiting behind an earlier job count against the limit.

Consuming a completed segment into a physical file's private publication group releases its build-job window slot even if whole-file validation has not finished. That finished native output remains resident and must be included in retained/private-output accounting, but it must not consume all scheduler permits and prevent the remaining chunks of the same file from reaching validation. This exception permits output-sized private staging for one unfinished large file; it does not permit unbounded queued extraction data or unfinished builders. Add a test where one file produces more than 2 × P segments.

Keep a small ordered completion buffer indexed by planned ordinal. Work may finish out of order; publication order is deterministic. Schedule earlier ordinals before filling the window with later ones.

The scheduler must work with P = 1:

- no worker waits for space in a queue that only another blocked worker can drain;
- no pool worker waits for an unscheduled earlier ordinal;
- no unbounded channels merely hide retained results;
- no blocking wait while holding publication locks;
- cancellation releases private work and drains/joins required jobs safely.

Submission and publication can be coordinated outside the worker pool. Use nonblocking result delivery into a logically bounded window or an equivalent existing mechanism. The window bounds task count; it is not an exact RSS guarantee.

Measure peak concurrent segment aggregation/finalization jobs. At P >= 2 and with sufficiently large ready work, more than one segment must actually execute concurrently.

## 8. Bigram handling

Preserve:

- normal and skip-1 gram generation;
- the configured ASCII/Unicode representation;
- case folding and normalization;
- separate text-binding coverage;
- raw-fallback behavior;
- packed intermediate blocks;
- final HashMap<u64, RoaringBitmap> components;
- exact source verification after candidate filtering.

All bitmaps use the owning segment's local slot domain.

Generate normal and skip grams in the existing shared traversal. They may finalize concurrently using the existing implementation. Never reread a field merely to produce the second gram family.

Do not convert gram membership into a B-tree entry for every gram-record pair. Do not add one bitmap covering the complete collection as a hidden query shortcut.

Construction diagnostics may reuse vector-capacity counters. Collect full diagnostic traversals at bounded batch/segment points, not after every record. Clearly distinguish allocated-buffer estimates, serialized-size estimates and process RSS.

An absent gram proves an empty candidate set only when that component provides complete applicable coverage. An absent or incomplete field index requires the established fallback/verification path.

## 9. Collection-wide facts without collection-wide postings

Some decisions are broader than one segment: duplicate logical IDs, inference admission, collection statistics, relation resolution and field bindings.

Keep these as metadata/routing responsibilities. They do not justify retaining whole-collection posting payloads.

### 9.1 Inference

Preserve current collection-wide inference and admission semantics. Do not independently admit a low-cardinality string index in every segment when the entire collection would reject that field.

Use lightweight mergeable inference observations. Exact distinct-value decisions may require the existing bounded key sets or an ordered merge over completed segment dictionaries; they must not clone all posting bitmaps into a second global InferenceBuilder.

Incomplete or delayed inferred components must be represented as unavailable/partial, and queries must retain the correct fallback. Collection summary finalization must not change record IDs or silently rewrite all segments.

Document the chosen reuse of the existing InferenceBuilder and global summary interfaces before implementing this part. The nonnegotiable acceptance criteria are unchanged inference results and no retained global duplicate posting index.

### 9.2 ID and file routing

Reuse or extend existing catalogs to resolve:

- logical record ID to active segment-qualified record address;
- file path to FileId;
- FileId to all segment memberships/record ranges.

A compact global native routing table is allowed. It contains identifiers and addresses, not copies of field indexes. Avoid rebuilding it for each query.

Check duplicate active IDs across segments during publication. If a batch replaces records, retire its old incarnation and install new routes in the same publication. Treat cross-segment file membership as normal.

Prefer existing shared/owned string representations. Do not clone every path into every field component.

## 10. Query interfaces and execution

### 10.1 Borrowed access

Extend the existing IndexCatalog and segment accessors. Do not introduce a new generic storage-backend trait.

Provide a concrete borrowed segment read view that supplies:

- SegmentId and collection definition;
- local record count;
- active and logical domains;
- IndexCatalog for field/text lookup;
- record and file membership access;
- sorting and relation component access.

The intended usage is:

    for segment in snapshot.collection_segments(collection_id) {
        let catalog = segment.index_catalog(collection_definition);
        let candidates = evaluate_in_segment(query, catalog, segment.active_records());
        collect_segment_matches(segment, candidates);
    }

This is interface intent, not a demand to allocate a new catalog or candidate set for every term. Reuse query preparation and existing borrowed catalog methods.

Bind schema paths and text bindings once per query/collection where possible. Do not repeat schema compilation, collation setup, normalization or path-string allocation for each segment.

Metadata, payloads and live masks borrowed by a query must belong to the same pinned snapshot.

### 10.2 One production execution path

Route Repository search and IndexView search through the same segment-aware evaluation machinery.

Reuse the existing segmented implementations for:

- exact count;
- ordinary search and global pagination;
- sorted results and computed sort;
- streaming and cancellation;
- direct ID/path lookup;
- relation traversal and backlinks;
- explain and explain-analyze;
- verified-result caching.

Remove flat-base execution from the normal path after migration. A temporary compatibility adapter is acceptable during intermediate commits, but final completion must not leave a separate unsegmented native path as the default CLI/benchmark implementation.

A single-segment collection must work without building a fake collection-wide aggregate. Optimize it through the same read interfaces if needed.

### 10.3 Correct local set operations

AND, OR and NOT are evaluated within a segment's slot domain. NOT is relative to the applicable live/eligible domain, preserving current missing/null/raw semantics.

Never union local bitmap slot 7 from different segments as if it identified one record. Convert matches to segment-qualified addresses before crossing a segment boundary.

Use local statistics for segment planning and shared collection metadata for semantics. A field missing from one segment does not imply that the field is absent from the entire collection.

Sorting and pagination are global: merge segment iterators or bounded top-k results, then apply the requested global offset/limit. Do not apply offset separately per segment. Preserve existing stable logical tie-breaking; do not let worker completion order or SegmentId become a new visible sort order.

Preserve file-scoped versus record-scoped results when one file spans several segments. Counts, deduplication, evidence and source access must follow the current scope semantics.

### 10.4 Caches and relations

Key cached record addresses with snapshot/revision and segment identity as required. Compaction and slot reuse must never make an old cache entry address a different record.

Reuse MergedRelationView with the unified segment representation. Forward and reverse resolution must work across segments and collections, including after replacement and compaction.

Do not rebuild a whole-corpus relation index at every query. Do not resolve relation target identifiers solely within the source segment.

Move-only operations use the current catalog path without rebuilding unchanged posting payloads.

## 11. Lifecycle and compaction

Reuse the current separate active masks and immutable publication model.

- Add: build native segment components privately, then publish.
- Delete: retire the record slots and update routing/catalog state; do not rewrite every posting.
- Replace: retire all old file memberships and publish all new memberships atomically.
- Move: preserve segment payloads when current semantics allow it; update the catalog.
- Compact: construct new native segments containing active records and publish their routing/liveness changes together.

Use SegmentPolicy for output grouping. Do not leave a maintenance path that silently restores 2,048-record partitions.

Keep maintenance work bounded and cancellable. Separate the amount of work permitted in one maintenance invocation from the target output segment size. If a per-invocation record budget is used, allow one target-sized segment or explicitly resume work; do not repeatedly split and merge the same data merely to satisfy two conflicting constants.

For the first implementation, reuse the existing small-segment count/retirement triggers with their size classification changed to the common policy. Merge only compatible segments of the same collection/configuration whose combined output fits the target, except a necessary oversized indivisible contribution.

Do not merge every initial build into one collection-wide segment. Do not automatically recompact fully live segments immediately after building them.

Old pinned readers must remain correct. Obsolete payloads must become releasable after those readers and deliberate references are dropped.

## 12. Instrumentation

Extend the existing detailed-profiling/build diagnostics, without adding a framework.

Record per collection:

- planned, completed, published and current segment counts;
- records and distinct files per segment;
- oversized contributions and their reason;
- queued/running/ready segment counts and peaks;
- peak simultaneously executing segment build jobs;
- component task counts and time by family;
- coordinator routing time;
- worker aggregation time;
- finalization/validation time;
- ordered-publication wait time;
- time to first searchable publication and complete construction;
- existing source reads, parse counts and memory observations.

Do not sum parallel task durations and label that total elapsed construction time.

Add a cheap structural diagnostic that confirms there are no collection-wide posting payloads left in the normal completed repository. This can be a test-only invariant or a narrow inspection method; do not traverse all entries on every status poll.

Expose segment/component inspection through borrowed methods so tests and future persistence work can address one component directly.

## 13. Implementation phases and commit boundaries

Complete each phase in order. Run the relevant focused checks before each commit. Push every commit.

### Phase 0 — Record the baseline

- Record master commit, machine, CPU/thread count, compiler, feature flags and dataset identity.
- Run the existing native build and query benchmarks.
- Record the blocking Repository path and progressive Index path separately.
- Record initial and final segment counts where available.
- Preserve baseline binaries/results outside the implementation timing.
- Read all affected query and lifecycle interfaces before changing ownership.

Deliverable: baseline report and an interface migration checklist. No production behavior change.

### Phase 1 — Central policy and segment identity

- Introduce SegmentPolicy and its configuration.
- Add tiny-policy tests for deterministic boundaries, empty files and oversized contributions.
- Name/reuse SegmentId and segment-qualified addresses.
- Move general segment definitions out of the mutation-specific module with minimal behavior changes.
- Add borrowed segment/component inspection.

Suggested commit subject: Define native segment policy and identity.

### Phase 2 — Share native ownership and read interfaces

- Separate repository metadata from segment payload ownership where required.
- Establish the shared snapshot/segment read model.
- Extend IndexCatalog rather than duplicating it.
- Adapt catalog ownership to avoid cycles and permanent retention of obsolete segments.
- Keep existing tests passing through temporary adapters without duplicating full indexes.

Suggested commit subject: Centralize native segment ownership and read access.

### Phase 3 — Parallel segment and component construction

- Replace eager serial aggregate admission with lightweight SegmentDraft routing.
- Submit multiple complete SegmentBuildJob values to the shared pool.
- Move posting/gram aggregation and finalization into segment/component tasks.
- Preserve extraction and packed contribution reuse.
- Implement bounded job windows, deterministic publication and cancellation.
- Demonstrate real overlap of at least two segment jobs on a controlled multi-worker fixture.
- Exercise the one-worker path before proceeding.

Suggested commit subject: Build native segments and field components in parallel.

### Phase 4 — Make blocking and progressive construction share the pipeline

- Route Repository::open and progressive startup through the new builder.
- Remove whole-collection PendingCollection payload aggregation from production.
- Make Index::from_repository adopt existing segments without reindexing/repartitioning.
- Preserve whole-file framing, validation and fallback publication.
- Complete inference and routing summary integration.
- Update startup diagnostics.

Suggested commit subject: Use segmented construction for all native entry points.

### Phase 5 — Complete query migration

- Route all Repository and IndexView query modes through the shared segment reads.
- Remove the production flat-base branch and obsolete adapters.
- Adapt direct lookup, sorting, relations, caches, counts and streaming.
- Replace in-repository use of removed CollectionState payload fields.
- Verify logical results against the baseline/reference implementation.

Suggested commit subject: Unify native query execution over segments.

### Phase 6 — Align changes and compaction

- Apply the common policy to add/replace/rebuild/compaction.
- Preserve atomic multi-segment file replacement and move semantics.
- Update routing when segment IDs change.
- Verify old-view correctness and release of unreferenced obsolete payloads.
- Remove conflicting segment-size constants from normal lifecycle paths.

Suggested commit subject: Align lifecycle maintenance with native segment policy.

### Phase 7 — Measure, repair and simplify

- Run the benchmark matrix in section 15.
- Repair regressions attributable to unnecessary copies, serial aggregation, query setup repeated per segment, over-small tasks or excessive publication work.
- Remove dead flat-path code and unused options.
- Run the combined workspace checks and produce the completion report.

Suggested commit subject: Validate and tune native segmented construction.

Do not mark a phase complete solely because newly added unit tests pass while its required production entry point still uses the old path.

## 14. Required correctness tests

Prefer focused fixtures that exercise real boundaries and existing end-to-end tests.

1. Same input and policy, one worker versus several workers: identical planned boundaries and logical results.
2. At least three segments with repeated local slot numbers: no cross-segment bitmap/address collisions.
3. Exact equality and numeric ranges spanning several segments; values appearing in one, several or no segments.
4. Missing fields, nulls, empty arrays, structural facts and NOT under live/eligible-domain rules.
5. Normal and skip-1 bigrams, field-specific FTS, Unicode folding, case-sensitive verification and raw fallback across boundaries.
6. Multi-record file spanning multiple segments; file-scope deduplication and record-scope counts remain correct.
7. Late framed-file failure: no invalid partial logical file remains visible.
8. Global sorting, stable ties, offset/limit and streaming match the established reference results.
9. Relations, backlinks and target lookup across segment and collection boundaries.
10. Delete, add, replace and move after construction; replacement spanning several segments is one consistent publication.
11. Compaction changes addresses without changing logical results; pinned old views and cache invalidation remain correct.
12. Zero-record files, empty collections, one-record collections and oversized contributions make progress.
13. Cancellation/failure while an earlier segment is delayed and later jobs finish; no deadlock, lost error or unbounded result retention.
14. Collection-wide inference decisions match the reference when local segments have deceptively low distinct-value counts.
15. Index::from_repository shares/adopts existing segment payloads. A test should detect full reaggregation, not merely compare output.
16. Publication/status/direct lookup does not reconstruct all field indexes or routing maps.
17. A controlled scheduler test proves actual concurrent segment aggregation, not just concurrent parsing. Use deterministic test synchronization/hooks, not elapsed-time sleeps.
18. Every ordinary build path observes SegmentPolicy; no hidden 2,048/132,000 partition rule remains.

Structural hashes containing physical slot numbers or segment grouping are expected to change. Compare logical content using collection + logical record identity, field values, posting membership translated to logical IDs, query results and ordering.

Reuse existing signature code where practical, but perform any expensive canonical comparison outside timed construction. Do not add a production global index just to satisfy the old benchmark hash.

## 15. Performance validation

Use the repository's existing benchmark harnesses and dataset. Do not introduce a new benchmark dependency.

Baseline and candidate must use identical adapters, schemas, alphabets, inference settings, data revisions, result modes and source-cache conditions.

A known build command shape is:

    cargo run -p objekte-search-core --release --locked --example bench_build_musenalm -- --root DATASET_PATH --alphabet ascii --build-workers 8

Use the actual documented dataset path and supported options. Add --segment-records to the candidate benchmark as specified above; do not pass a new flag to an unmodified baseline executable.

Run:

- default 16,384-record target at 1 worker and at the machine's usual benchmark worker count;
- 8,192 and 32,768 targets at the usual worker count;
- ASCII and Unicode modes already covered by the existing benchmark suite;
- progressive and blocking entry points;
- the existing full query suite;
- a controlled larger/multi-record fixture that yields several meaningful segments.

Measure at least three warmed runs per primary configuration and report medians and spread. Compile outside measured execution. Report cold runs separately if measured.

The primary construction measurement includes discovery, parsing, segment routing, worker aggregation, finalization, required validation, metadata/routing completion and publication. Do not stop the timer before finalizing native indexes or perform a mandatory cleanup merge after the timer.

Report:

- total complete-build time and time to first searchable result;
- peak and retained memory using the existing supported measurements;
- actual segment/component counts and distribution;
- source read/parse counts;
- worker overlap and coordinator cost;
- query latency distribution and individually regressed cases;
- update and compaction timings.

The default remains 16,384 unless measured results justify selecting 8,192 or 32,768. A measured change within those choices is an authorized tuning decision; record it. Do not implement runtime adaptive sizing for this task.

Provisional review gates:

- No correctness differences.
- Median complete-build time should be within 15% of the comparable native baseline or better.
- Peak RSS should be within 25% of baseline unless a measured, explained throughput tradeoff is accepted by the user.
- Query p95 should remain within 15% of baseline. Investigate individual regressions exceeding both 20% and 1 ms.
- Report first-publication latency separately; do not hide a startup responsiveness regression behind faster complete-build time.

These are engineering review gates, not claims that exact speedups are guaranteed. If repeated measurements remain materially worse, keep the implementation unmerged, document the responsible phases and continue in-scope repairs. Do not declare completion by changing the timing boundary or removing required behavior.

## 16. Final checks and acceptance

Follow the current repository validation instructions. At the inspected revision these include:

    cargo test --workspace --locked --no-fail-fast
    cargo check --workspace --all-targets --locked
    cargo clippy --workspace --all-targets --locked
    cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked

Use any newer applicable repository instructions at implementation time. Record environmental failures honestly.

The task is complete only when:

- [ ] No new dependency or alternative storage backend was introduced.
- [ ] Native BTreeMap/HashMap/Roaring representations remain in use.
- [ ] The common segmentation policy governs all ordinary construction paths.
- [ ] Initial construction produces segments directly.
- [ ] Multiple segment aggregation/finalization jobs execute concurrently.
- [ ] Field components have independent ownership and can be scheduled independently.
- [ ] The coordinator does not merge the full posting/gram payload.
- [ ] Repository and Index query the same native segment model.
- [ ] No normal query reconstructs collection-wide indexes.
- [ ] Record addresses, routing, liveness, caches and relations are segment-correct.
- [ ] Compaction and lifecycle updates preserve semantics and consistent publication.
- [ ] No normal adoption path builds flat indexes and then repartitions them.
- [ ] The existing benchmark measures the new path, including complete finalization.
- [ ] Tests and comparable performance results are recorded.
- [ ] The integration branch and report are committed and pushed; master remains unchanged.

The implementation report must state the final policy, actual Musenalm segment counts by collection, demonstrated concurrency, changed public low-level interfaces, benchmark comparisons and remaining limitations.

No persistent component file format is part of this task. The resulting borrowed component interfaces and explicit ownership should make later per-component persistence possible without another query architecture rewrite.

## 17. Source references

All links below identify the inspected baseline. Recheck the corresponding code if implementation starts from a newer master.

- [Native segment payload and finalization](https://github.com/Simon-Martens/objekte/blob/c54486f893a57fac0b1b80ed967ab4f5a5fd26c9/crates/core/src/mutation_segment.rs)
- [Progressive construction](https://github.com/Simon-Martens/objekte/blob/c54486f893a57fac0b1b80ed967ab4f5a5fd26c9/crates/core/src/repository_incremental.rs)
- [Ordinary repository construction](https://github.com/Simon-Martens/objekte/blob/c54486f893a57fac0b1b80ed967ab4f5a5fd26c9/crates/core/src/repository.rs)
- [Parallel preparation and admission](https://github.com/Simon-Martens/objekte/blob/c54486f893a57fac0b1b80ed967ab4f5a5fd26c9/crates/core/src/build.rs)
- [Borrowed index catalog](https://github.com/Simon-Martens/objekte/blob/c54486f893a57fac0b1b80ed967ab4f5a5fd26c9/crates/core/src/index_catalog.rs)
- [Segmented query execution](https://github.com/Simon-Martens/objekte/blob/c54486f893a57fac0b1b80ed967ab4f5a5fd26c9/crates/core/src/run.rs)
- [Lifecycle publication and active masks](https://github.com/Simon-Martens/objekte/blob/c54486f893a57fac0b1b80ed967ab4f5a5fd26c9/crates/core/src/lifecycle.rs)
- [Current base partitioning](https://github.com/Simon-Martens/objekte/blob/c54486f893a57fac0b1b80ed967ab4f5a5fd26c9/crates/core/src/lifecycle_partition.rs)
- [Bigram representation and construction](https://github.com/Simon-Martens/objekte/blob/c54486f893a57fac0b1b80ed967ab4f5a5fd26c9/crates/core/src/bigram.rs)

## Implementation report

Task identifier: `native-segment-construction`

Completed: 2026-11-16

Base commit: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`

Integration branch: `integration/native-segment-construction`

Integration worktree: `/home/simon/source/objekte-integration-native-segment-construction`

### Outcome

Native segments are now the normal ownership, construction, publication and query unit. Ordinary blocking construction and progressive construction both produce and publish the same `IndexedCollectionSegment` representation. Each segment owns its bounded record metadata, field/custom postings, raw/fallback/field-text bigrams, structural facts, sorting components and relation contributions. Record slots remain segment-local and externally identified by collection, `SegmentId` and local slot.

The default policy is `target_records = 16_384`, `max_files = 16_384`. The same `RepositoryOptions::segment_policy` reaches blocking construction, progressive construction, additions/replacements and compaction. The record target is applied only at completed contribution boundaries; an oversized indivisible contribution is accepted into an empty draft. Segment and job-count arithmetic is checked or saturating where appropriate.

The construction scheduler has a bounded `2 * P` window, deterministic finalization order, and measured worker overlap. `Index::from_repository` adopts the existing `Arc` payloads rather than rebuilding or repartitioning a flat index. Normal completed repositories no longer retain collection-wide posting/bigram payloads as the authoritative index.

### Phases and commits

| Phase | Result | Commit |
| --- | --- | --- |
| 0 | Recorded baseline binaries, environment, dataset, builds, queries and the interface migration checklist. | `7adb001500` |
| 1 | Added `SegmentPolicy`, named collection-tagged `SegmentId`, canonical segment definitions, borrowed component inspection and the shared option. | `9c59661663` |
| 2 | Centralized active domains, segment/catalog/revision ownership, removed the `FileCatalog` to `Repository` cycle, and made repository snapshots segment-readable. | `b51f93a425` |
| 3-7 | Completed bounded parallel construction, shared blocking/progressive construction, segment query migration, lifecycle/compaction alignment, dead flat-path removal, benchmarking and tuning. | `200f02be53` |

The final completion-document commit is the commit containing this report. No subtask branches were used.

### Construction and ownership

- The coordinator routes owned `BatchContribution` values to deterministic segment drafts and does not merge every posting/gram key itself.
- Segment jobs finalize native components independently. Large independent component work uses the existing Rayon pool; no dependency was added.
- Scheduler slots remain occupied through out-of-order completion. Publication consumes planned ordinals in deterministic order.
- `Repository::open_with_sources` uses the progressive segmented builder and waits for completion.
- `Index::from_repository` adopts existing segment `Arc`s. The adoption test checks pointer identity, not only equal output.
- `repository_direct.rs` and its production direct-build path were removed.
- No serialization, persistence, mmap, storage backend, new crate, or IC-stable-structures import was introduced.

### Query and lifecycle behavior

- Search, count, explain/analyze, streams, direct ID/path lookup, posting values, exact selection, relation traversal/backlinks, computed/stored/inferred sorting and verified caching use segment-aware execution.
- A 256-candidate route threshold hands verification-heavy multi-file ordered pages to the existing file-parallel scanner; one large JSONL source keeps its single-handle cursor.
- Stream fallback caching is deferred until successful delivery. Cancellation and sink failure cannot publish verified membership.
- Segment-qualified records are retained through sorting, cache entries, relations and deferred materialization.
- Add/replace/move/delete/compaction use the shared policy and publish routing/liveness changes together. Pinned old views retain their old payloads and addresses.

### Low-level interface changes

- `RepositoryOptions` now includes `pub segment_policy: SegmentPolicy`.
- `native_segment.rs` is the canonical owner of `SegmentPolicy`, `SegmentId`, segment payloads, published segments, borrowed read views, snapshots and component descriptors.
- `Repository` exposes `segment_summaries()`, `segment_components()` and borrowed segment read views for diagnostics and future persistence boundaries.
- `Repository::native_snapshot()` supplies active domains, segments, catalog and revision from one snapshot.
- The public `CollectionState` fields retained for source-level compatibility no longer represent the completed ordinary index. In a completed repository, callers must inspect `Repository` segment views rather than treating `CollectionState::postings`, `raw`, `fallback_text`, `custom_text` or `sorting` as one collection-wide payload.
- `IndexedCollectionSegment::read_view()` and component descriptors are borrowed and do not construct a global catalog.

### Validation

All commands used `--locked` and completed on the final code.

| Command | Result |
| --- | --- |
| `cargo test -p objekte-search-core --lib` | 521 passed, 16 ignored, 0 failed |
| `cargo test -p objekte-search-core --tests` | all integration tests passed |
| `cargo test --workspace --locked --no-fail-fast` | all workspace tests and doctests passed |
| `cargo check --workspace --all-targets` | passed |
| `cargo clippy --workspace --all-targets` | passed with the repository's existing warning-level diagnostics |
| `cargo check -p objekte-search-core --all-targets --features detailed-profiling` | passed |
| `git diff --check` | passed |

The scheduler overlap test `lifecycle_initial::tests::multi_worker_segment_finalization_actually_overlaps` uses deterministic synchronization. Musenalm runs also observed eight concurrent segment jobs at eight build workers and one at one worker.

### Performance

`bench_build_musenalm` ran three warmed release runs per configuration. Times below are medians.

| Configuration | Build median | Peak RSS median | Segments |
| --- | ---: | ---: | ---: |
| ASCII, default, 8 workers | 796,713,695 ns | 503,545,856 B | 15 |
| Unicode, default, 8 workers | 905,195,373 ns | 517,623,808 B | 15 |
| ASCII, default, 1 worker | 3,611,667,883 ns | 483,221,504 B | 15 |
| Unicode, default, 1 worker | 4,346,044,893 ns | 490,962,944 B | 15 |
| ASCII, 8,192 target, 8 workers | 793,542,907 ns | 549,519,360 B | 23 |
| Unicode, 8,192 target, 8 workers | 888,712,551 ns | 561,934,336 B | 23 |
| ASCII, 32,768 target, 8 workers | 856,265,304 ns | 510,341,120 B | 11 |
| Unicode, 32,768 target, 8 workers | 932,045,916 ns | 514,297,856 B | 11 |

Compared with the blocking baseline median of `910,984,168 ns`, ASCII at the default/8-worker setting was 12.5% faster. Peak RSS was 22.8% higher, within the 25% gate. All runs parsed 148,486 records with zero failed parses. ASCII and Unicode signatures were stable across runs for every segment policy.

Default Musenalm segment distribution was:

| Collection | Segments |
| --- | ---: |
| `contents` | 9 |
| `agents` | 1 |
| `entries` | 1 |
| `items` | 1 |
| `pages` | 1 |
| `places` | 1 |
| `series` | 1 |
| Total | 15 |

The progressive Index produced 15 searchable publications. ASCII completed in `629,526,674 ns` with first publication at `424,759,587 ns` and peak RSS `630,743,040 B`; Unicode completed in `662,605,346 ns`, first publication `431,452,029 ns`, peak RSS `975,933,440 B`. Complete build improved versus the progressive baseline, while first publication regressed by 44.5% ASCII and 29.2% Unicode; this startup tradeoff is reported separately rather than hidden in the complete-build result.

The unchanged 150-case suite ran three repetitions in ASCII and Unicode with caching disabled and eight query workers. Both direct Repository and progressive Index runs completed 900/900 measurements with zero failures. All 150 result digests matched baseline exactly in both alphabets and both entry points.

| Entry point | Alphabet | Median ratio | p95 ratio |
| --- | --- | ---: | ---: |
| Repository | ASCII | 0.653 | 0.903 |
| Repository | Unicode | 0.599 | 0.460 |
| Progressive Index | ASCII | 1.985 | 1.134 |
| Progressive Index | Unicode | 2.275 | 1.099 |

The progressive p95 values pass the 15% gate. Its median is elevated by small fixed segment-routing/planning costs on already-sub-millisecond queries. Individual remaining regressions above both 20% and 1 ms were investigated: ASCII `Q043`, `Q054`, `Q055`, `Q119`, `Q144`; Unicode `Q043`, `Q052`, `Q054`, `Q055`, `Q069`, `Q119`, `Q136`. These are exact/range or shallow-count cases with no source scans or with small absolute times; the larger verification-heavy `Q141`, `Q143` and `Q150` regressions were repaired by the multi-file route threshold.

Raw benchmark and query evidence is retained in `/tmp/objekte-native-segment-final.xrF8Dp/`. Final release executable SHA-256 values were:

- `bench_musenalm`: `64e8c93f5f0278e0e43c579b80f7f8bf8a37c0f3886dc9b501577c87f50691e6`
- `bench_build_musenalm`: `4b44d55ee5a372896aca582bd769714e5c3377e6726f91c79f4ce0201a1ff4a0`

### Deviations, limitations and cleanup

- No approved dependency or storage-backend deviation was made.
- First-searchable publication is slower in the progressive path even though total construction is faster. The default 16,384 soft segment target remains selected because complete-build time, RSS and query p95 remain within their review gates and smaller targets increased segment count and RSS.
- The progressive median query ratio remains above 2 because of fixed per-query segment setup on very cheap cases; p95 and exact logical results remain within the specified gate.
- Clippy reports pre-existing warning-level diagnostics in the repository; it does not fail.
- The integration worktree and branch are intentionally retained for the user. No subordinate worktrees were created. Temporary task memory is deleted after this report is pushed.
