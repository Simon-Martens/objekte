# Objekte native segment construction repair specification

Status: implementation specification for the local coding agent.

Task identifier: `native-segment-construction-repair`

Prepared: 2026-11-16

Repository: `Simon-Martens/objekte`

Reviewed source branch: `integration/native-segment-construction`

Reviewed source commit: `9c61fbc5d5510ae1a2713ffba160d2155742abfe`

Original master baseline: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`

Repair integration branch: `integration/native-segment-construction-repair`

Repair worktree: `/home/simon/source/objekte-integration-native-segment-construction-repair`

Suggested task path: `docs/ai/tasks/objekte_native_segment_construction_repair_specification.md`

## 1. Assignment and required result

Repair the native segment implementation while preserving its useful construction and query performance improvements. Complete the architecture promised by the native segment construction plan. Fix all findings in this specification, rather than applying isolated conditions that make the existing benchmark pass.

The required final implementation has one authoritative snapshot model, segment-qualified record addresses, bounded construction drafts, independent native field components, truthful counters, and regression tests that exercise multiple segments and lifecycle changes. Both blocking Repository construction and progressive Index construction must produce the same logical contents and use the same query machinery.

This document is deliberately prescriptive. Follow the decisions below unless inspection proves an existing equivalent implementation already satisfies them. When an implementation detail must differ, record the reason and evidence. Do not silently omit a requirement or mark a task complete because the benchmark suite passes.

The findings were established by source inspection. The review did not execute Rust tests. Your first implementation work must turn the correctness findings into executable regressions on the reviewed branch. If a proposed reproduction does not reach the affected path, adjust the fixture or options and prove which path executes. Do not assume that failure to trigger a particular optimization disproves the address or ownership defect.

### 1.1 Scope

Retain Rust standard-library BTreeMap and HashMap, Roaring bitmaps, packed bigram construction buffers, the existing sorting representations, and Rayon. Preserve adapters, normalization, field bindings, validation, raw fallback, exact source verification, lifecycle semantics, and high-level query APIs.

Do not add a database, persistence format, mmap implementation, allocator framework, async runtime, serialization layer, or new dependency. Do not import the redb, RocksDB, or ic-stable-structures experiments. This task concerns native in-memory segmentation.

Do not weaken admission policies, disable useful acceleration globally, or replace index execution with full scans to obtain correctness. A temporary conservative fallback is permitted while repairing a broken optimization, but the final implementation must restore the useful path on a correct snapshot model and meet the performance requirements.

### 1.2 What to preserve

- Keep the existing bounded parallel extraction pipeline and move-based BatchContribution ownership.
- Keep actual segment aggregation and finalization on workers.
- Keep deterministic planned ordering independent of completion order.
- Keep immutable Arc-owned payloads with independently versioned liveness.
- Keep the existing logical-query benchmark and exact result comparison.
- Keep source-free compaction and pinned-view behavior.
- Keep the default policy of 16,384 target records and 16,384 files unless final measurements justify a documented change.

### 1.3 Priority order

1. Wrong-result defects and snapshot consistency.
2. Reclaimable ownership and a single authoritative read model.
3. Real segmentation of framed files and common mutation policy.
4. Removal of duplicate inference and compatibility payloads.
5. Independent field construction and accurate instrumentation.
6. Performance validation and removal of temporary migration code.

Correctness must not depend on the verified cache being enabled, on a collection fitting into one segment, on a particular worker count, or on using only the blocking entry point.

## 2. Repository workflow and branch handling

Read `AGENTS.md`, `docs/ai/README.md`, the original native construction plan, and its completion report before changing code. Preserve the original checkout, its branch, its index, and all user changes.

Create the repair integration branch and sibling worktree from the current remote `integration/native-segment-construction` branch. The reviewed commit above is the known starting point. Fetch first and record the actual source commit. If the source branch has advanced, inspect the intervening commits and retain existing repairs. Do not reset it to the reviewed commit. Do not start from master, because that would discard the implementation being repaired.

If the repair branch or worktree already exists, inspect and resume it. Do not overwrite it. The final repair branch must include the original native segmentation work plus these repairs so that the user has one branch to merge into master.

All implementation, integration and validation happen in the repair worktree. Do not merge into, commit onto, or push master. Push each implementation commit according to the repository workflow. A failed push is not permission to rewrite history or change credentials.

Use temporary notes under the repository's task memory directory. Preserve a phase checklist, failing regression commands, design decisions, and exact benchmark revisions. At completion, move this task to `docs/ai/done/`, append the implementation report, commit and push on the repair branch, and retain that integration worktree for the user.

One agent may perform the entire task. If the supervisor uses subagents, first commit the address and snapshot interfaces. Then separate construction, query integration, and tests/measurement ownership. Do not let independent agents invent competing snapshot or record-address types. Subagents are optional, not a prerequisite for completion.

## 3. Findings and repair map

The source locations below refer to the reviewed commit. Function names are the durable navigation points; line numbers will move.

| Repair | Finding | Main implementation areas | Required evidence |
| --- | --- | --- | --- |
| R01 | Relation optimizations combine local slots from different segments | `run.rs`, `relation_semijoin.rs`, `relation_query.rs`, `relation_target_cache.rs` | Repeated-slot relation regressions, including lifecycle changes |
| R02 | Inferred sorting reads original repository state and current publication state together | `run.rs`, `sort_binding.rs`, `sorting.rs`, cache keys | Add/replace/delete/compact sorted queries match explicit expectations and reference execution |
| R03 | Constant-left metadata predicates incorrectly reuse one record's result | `executor.rs`, metadata expression handling | Mixed record/file RHS comparisons within one segment |
| R04 | Initial payloads remain owned by the adopted repository after retirement | `repository.rs`, `lifecycle.rs`, `native_segment.rs`, `lifecycle_catalog.rs` | Weak-reference reclamation and pinned-view tests |
| R05 | Large framed files bypass boundaries; mutation uses default construction policy | `repository_incremental.rs`, `build.rs`, `mutation_segment.rs`, lifecycle publication | A file producing more than 2P segments; atomic success/failure and common policy |
| R06 | Collection-wide inference builders duplicate membership and perform serial aggregation | `inference_build.rs`, `inference.rs`, construction coordination | Global decision parity with no second global posting builder |
| R07 | Flat compatibility fields, sorting copies, and global relation payloads remain | `repository.rs`, `index_catalog.rs`, sorting/relation binding, public callers | One read model, no fake empty index fields, no copied sorting payload |
| R08 | Component construction remains broad serial stages with limited pairwise joins | `mutation_segment.rs`, posting/bigram/sort builders | Actual overlap between independent field components |
| R09 | Counters are adjusted from result shape; some tests lost their useful assertions | `repository.rs`, `run.rs`, work counters, regression tests | Measured operation counters and restored equivalent assertions |
| R10 | Snapshot routing, identities and compaction must be coherent after the above repairs | catalog, lifecycle, compaction and selection/caching code | Invariant checks and lifecycle matrix |
| R11 | Existing tests insufficiently force small multi-segment cases | existing core test modules | Required regression matrix in section 15 |
| R12 | Performance and completion evidence must cover the repaired architecture | existing benchmarks and task report | Comparable recorded results and complete acceptance checklist |

## 4. Nonnegotiable invariants

### 4.1 Local slots are local

A RecordSlot identifies a record only within its owning immutable segment. Two segments may both contain slot zero. They are different records even when their files, values or relation targets are otherwise similar.

Every address crossing a segment boundary must carry `(CollectionId, SegmentId, RecordSlot)`. A bare bitmap is permitted only where the owning segment is explicit in the surrounding value or function parameters. Do not create a collection-wide bitmap by unioning segment-local slots.

Do not use `FileId` as a substitute for a record address. A file may contain multiple records and may span multiple segments. Do not use a segment's vector position as persistent identity.

### 4.2 One query observes one snapshot

The segment list, liveness, file catalog, identity routing, inference admission, relation routing, statistics used for exactness, and revision must come from one pinned immutable snapshot. Immutable schema and source configuration may be shared separately.

An optimization must not inspect `repo.segments` to choose a path and then consume another `segments` argument. The repaired API must make that mismatch difficult to express, rather than relying on comments at every call site.

### 4.3 Payload ownership is reclaimable

Only snapshots, active construction jobs, bounded caches with an explicit ownership policy, and deliberate borrowed/pinned references may retain segment payloads. Repository configuration and runtime services must not retain an initial snapshot forever.

After an obsolete segment has been removed from the latest snapshot and all actual old views have been dropped, its Arc must be reclaimable. Compaction must not require destroying the whole Index to release it.

### 4.4 Publication is the consistency boundary

A publication installs payloads, liveness, file state, routing, counts, and admission metadata together. Replacement of one physical file may retire records in several segments and add records in several new segments. No reader may observe half of that replacement.

### 4.5 Construction parallelism does not change semantics

For the same input, policy and adapter behavior, changing worker count or worker completion order must not change segment boundaries, logical identities, inference decisions, query ordering or diagnostics. Publication revisions may differ between distinct opening modes; do not compare opaque revision numbers as logical content.

## 5. Required ownership and read interfaces

Implement this common foundation before completing R01, R02 and R07. The names below are recommended precise names; reuse an existing equivalent type rather than introducing duplicates. The relationships and responsibilities are mandatory.

### 5.1 Separate runtime from published data

Use a structure equivalent to:

```rust
struct RepositoryRuntime {
    // Immutable definitions, source registry, configuration identity,
    // worker pool ownership, and bounded query caches.
    // No current or initial segment list.
}

struct NativeIndexSnapshot {
    revision: IndexRevision,
    // One collection segment group per collection.
    // Catalog, routing, liveness, admission metadata and current statistics.
}

pub struct Repository {
    runtime: Arc<RepositoryRuntime>,
    snapshot: Arc<NativeIndexSnapshot>,
}

struct IndexInner {
    runtime: Arc<RepositoryRuntime>,
    // Existing synchronization around Arc<NativeIndexSnapshot>.
}
```

This is an ownership sketch, not a requirement to replace every module or lock. Adapt `PublishedIndex` into the snapshot role if that is the smallest coherent implementation. Retain the existing publication synchronization rather than adding a new concurrency mechanism.

An IndexView owns the same runtime and a pinned snapshot. Repository search and IndexView search call shared functions with those two inputs. `Index::from_repository` consumes the Repository and transfers/shares those exact Arcs. It must not retain a Repository containing a second initial segment list.

Do not implement this by cloning the Repository, clearing fields in one copy, or moving every index through a temporary flat collection. Use ownership decomposition. Adoption should be constant time in payload size, aside from small root bookkeeping.

FileCatalog may retain immutable inventory metadata and versioned changes. It must not own Repository, Index, or a segment-owning snapshot. Keeping file identity metadata is acceptable; keeping obsolete field payloads through that metadata is not.

### 5.2 Define one qualified address

Introduce or consolidate:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
struct SegmentRecordAddress {
    collection: CollectionId,
    segment: SegmentId,
    slot: RecordSlot,
}
```

Reuse the named SegmentId but give it one allocation authority per index lineage. The simplest decision is a checked monotonically increasing u64 allocated in deterministic coordinator planning order. Do not pack collection and local counters into overlapping bits without a checked range. Collection identity is already carried separately.

Remove the duplicate independently mutable segment identities currently carried by both payload and publication, or make one constructor assign both from the same value and assert equality everywhere a payload is published. Prefer one authoritative identity. Compaction creates new identities; changing only liveness preserves the immutable payload identity.

A lookup from SegmentId to group position belongs to the snapshot. Query-local cursors may retain a vector position after resolving it against that pinned snapshot. Such positions must not escape into caches, selections, deferred hits, or later revisions.

### 5.3 Keep compact routing, not duplicate indexes

The snapshot must resolve:

- `(CollectionId, RecordId)` to the active qualified record address.
- Relative path to FileId using the existing path catalog.
- FileId and collection to all segment memberships and local record ranges/slots.
- SegmentId to its payload and liveness entry.

Retain or extend existing catalogs. Do not introduce an unrelated routing framework. Update routing only for affected publications; do not rebuild it for every query. Sharing immutable strings and unchanged maps is allowed. If publication currently copies routing maps, measure the cost and reuse existing persistent/chunked catalog facilities where applicable; do not introduce a new persistent-container dependency.

The identity lookup is allowed to contain one address per record. It must not duplicate every RecordEntry, field value, sort key, or posting. A route is metadata, not a second record store.

### 5.4 Borrow through one read view

Extend the existing IndexCatalog and NativeSnapshotRef/SegmentReadView concepts. The query-facing view should supply the runtime metadata plus one snapshot, and resolve a qualified address to borrowed record, file and segment references.

Pass this view to relation evaluation, inferred sorting, direct lookup, exact selection, posting values, streaming, analysis and reference execution. Avoid long argument lists containing independent `repo`, `segments`, `base_active`, `catalog`, and `revision` values that can disagree.

Compile/bind the query once per selected collection. Reuse bound paths, normalization, collation and sort specifications across segments. Do not recursively call the public search API merely to reuse an internal stage when that would bind and plan the same query again.

## 6. R01 repair relation membership and source-dependent execution

### 6.1 Defect to remove

At the reviewed revision, `try_semijoin_relation_page` inserts each segment's `record.slot.0` into a shared bitmap, calls `fts_witnesses`, and then checks those unqualified slots in every segment. `segment_target_candidates` and `segment_fts_target_candidates` perform similar unions, while `verify_target_records` addresses the flat collection record array.

The streaming source-dependent target prefetch also assembles unqualified candidates and calls a helper that reads flat collection relation state. Audit and repair that call, even if the returned prefetch value is currently discarded. Unused speculative reads must not charge resources, fail a valid query, or reopen stale files.

### 6.2 Required candidate representation

For collections of candidates, use a segment-keyed native container such as:

```rust
struct SegmentRecordSet {
    // Key includes collection identity when the set spans collections.
    members: BTreeMap<(CollectionId, SegmentId), RoaringBitmap>,
}
```

Use the same representation for relation input and target candidate sets where compact membership is needed. An equivalent vector of explicitly identified segment sets is acceptable when its ordering is owned by the pinned snapshot. Do not convert the whole candidate set into a HashSet of allocated record strings.

Keep bitmap intersection, union and difference local to each key. `len()` sums local cardinalities. Empty local sets may be omitted. A missing segment key means no selected records in that segment, not unknown coverage. Represent unknown candidate coverage separately using the existing may/must model.

### 6.3 Relation traversal and semijoin algorithm

1. Evaluate source predicates against each source segment's active domain.
2. Resolve forward relation edges through logical target IDs using the snapshot's ID routing.
3. Accumulate target addresses into segment-qualified target sets.
4. Apply target postings or bigram filters in each target segment; preserve fallback and incomplete coverage.
5. Verify each required target address using its own source locator and revision.
6. Map verified targets back to qualified source addresses through segment-local reverse contributions or correct snapshot routing.
7. Intersect the witnesses with the original source candidate set and current active domain.
8. Merge globally ordered results using logical tie-breaking, never local slot order across segments.

Reuse MergedRelationView's existing snapshot-aware resolution where possible. The semijoin should call the same record/edge access operations as the ordinary relation evaluator. It must not keep a second flat relation model for its fast path.

Target-first and source-restricted traversal remain supported. Their cost estimates must be based on the current snapshot. Do not compute estimates from current segments and then traverse edges from original metadata.

### 6.4 Evidence and caching

Store witnesses and evidence by qualified source address. A field matching for one record must not become evidence for every record selected from its segment. Per-segment constant evidence may be shared only when the proof actually applies to all members of that selected set.

Target value caches must include snapshot/revision identity and qualified target address where their lifetime exceeds one pinned query. Query-local caches may omit revision from individual entries only because the cache itself belongs exclusively to one pinned view.

Delete or adapt old helpers whose parameters imply collection-global slots. Do not leave them callable from production segmented paths. A test-only flat reference helper may remain if it cannot supply production proofs.

### 6.5 Required regression cases

Construct at least two source segments each containing local slot zero. Source A0 refers to a matching target, source B0 to a nonmatching target. An FTS query restricted to the related target field must return only A0. Reverse which source matches and repeat.

Construct two target segments with a match only in the second segment at local slot zero. Verify target-first and source-restricted strategies, counts, evidence, selected-field FTS and relation FILTER. Add filters selecting different local slots in each source segment.

Repeat after adding, replacing, deleting and compacting source and target records. Exercise blocking Repository, Index adopted from Repository, and progressive Index. Use Locator and Logical results, cache disabled and enabled, and streaming with source-dependent field comparisons.

Tests must assert explicit expected logical record IDs. Comparing two implementations that share the same broken candidate mapping is insufficient.

## 7. R02 repair inferred ordering and its cache

### 7.1 Correct eligibility

Every eligibility check must examine the pinned snapshot used for execution. Remove direct original-repository segment access from `inferred_order_index`, `posting_order_eligible`, `inferred_order_column`, and `try_inferred_segment_order_page`.

A single-segment optimization must obtain the actual sole eligible live segment and carry its identity through execution. It must not check that one segment is live and then assume vector element zero is that segment. Empty or fully retired preceding segments are a required test case.

Do not construct a temporary collection generation by cloning all collection metadata and replacing its record vector. Pass the segment's borrowed records and postings directly to sort-column construction.

### 7.2 Implement segment-local inferred orders

Build an inferred order for each eligible segment from that segment's complete scalar postings and local records. Use the existing exact scalar key and ICU collation behavior. Preserve null, missing, fallback, multivalue and incomplete-coverage rules.

An inferred order is allowed only when each relevant logical record has a complete sortable value or a known missing/null state according to the existing policy. Do not interpret a missing inferred posting as an empty field. Use the established computed-sort path when the complete proof is unavailable.

For multiple eligible segments, merge their local orders using the existing global ordering machinery. Compare actual sort keys and then the established logical identity tie-break. Offset and limit apply after the merge. Do not sort each segment and concatenate its page.

Do not force materialization of every matching logical record merely to merge complete numeric or boolean inferred orders. Locator pages over complete inferred postings should remain source-free. String sorting must retain current locale/options/version semantics.

### 7.3 Cache identity and lifetime

Key a cached inferred order by index lineage, SegmentId, field/binding identity, inference-policy decision/version as applicable, locale, collation version, sort options and direction. The exact key fields may reuse existing typed structures.

Choose to cache a full immutable segment order, then apply the pinned snapshot's live mask during iteration. With that choice, deletion alone does not invalidate the immutable order, but replacement and compaction create new SegmentIds. A cache entry containing a live-only order must instead include the liveness/revision identity; do not mix these designs.

Keep the cache bounded by the existing configured budget. Cache entries must not retain an obsolete entire snapshot. Store only required derived order/key data or an explicit bounded component reference. EXPLAIN without ANALYZE must not populate this cache or perform source reads.

### 7.4 Required regressions

Start with inferred numeric values 20 and 30 in one segment. Adopt the Repository into Index, add a record with value 10 in another segment, then request ascending limit one. The answer must be the new record. Repeat descending, offset one, exact count, deletion of the first segment's records, replacement, compaction and a pinned old view.

Test equal sort values in different segments, differing logical IDs, and different worker completion orders. Page boundaries must remain stable. Test one live segment after a fully retired first entry. Test missing/null/long-string/incomplete inferred coverage and the fallback sort route.

Restore assertions for zero source reads on eligible Locator queries and for cache reuse. Verify them for both construction entry points, not just a static Repository.

## 8. R03 repair metadata constant evaluation

The current segment metadata helper treats a predicate as constant whenever its left operand is CollectionName or RepositoryRoot. That is incorrect when the right operand is record- or file-dependent.

Implement a small explicit expression-dependence classification, or a narrowly scoped helper that covers the exact supported metadata expression forms. For this repair, a predicate may use the evaluate-once shortcut only if both operands and all applied transforms are known invariant across that segment's records.

Literals, collection name and repository root may be invariant. Record ID, record class, byte ranges, file path, file status, file size and timestamps must not be assumed invariant merely because they are metadata. A file field may vary within one segment. Unknown/custom/source-dependent operations conservatively require ordinary per-record evaluation.

Do not disable metadata-to-metadata comparisons. Evaluate them over each active record when needed, using the same eval_term semantics as the reference evaluator. Continue to avoid source reads for predicates resolvable from resident metadata.

Required fixture: collection `items`, two logical record IDs `items` and `other` in one segment. `@collection.name = @record.id` returns only `items`; the reversed operands produce the same membership. Reorder the records and repeat. Include inequality, NOT, AND and OR, metadata transforms, and a file-dependent RHS across two files. Assert both exact bounds and final results. For parser syntax differences, construct the equivalent public query or bound expression and retain at least one end-to-end parsed query.

## 9. R04 and R07 complete ownership and remove false compatibility

### 9.1 Remove the retained initial snapshot

Implement section 5's ownership split. Both `Index::open` and `Index::from_repository` must adopt the completed snapshot without a second repository-owned reference to its initial segments. Progressive construction must end in the same ownership shape.

Add a test that obtains Weak references to initial segment payloads, performs replacement/deletion and actual compaction or retirement, drops all test-owned strong references and old views, and verifies that `upgrade()` returns None. Keep the Index alive during that assertion.

Add the converse test: while an old view remains pinned, its payload survives and Locator queries return the old logical contents. Once that view is released, the payload becomes reclaimable. Payload reads of changed source files continue to follow existing revision/unavailability rules; an old in-memory index is not a promise to retain historical source bytes.

Do not assert reclamation solely using process RSS. Allocators can retain freed pages. Weak-reference lifetime is the correctness test; RSS is a separate performance measurement.

### 9.2 Make collection metadata honest

Replace or narrow CollectionState so it contains actual collection metadata: definition, source binding, schema/text-binding descriptions, sort specifications, inference admission/report metadata, and snapshot statistics where appropriate.

Remove public compatibility fields that expose empty or partial collection-wide postings, raw/fallback/custom bigrams, inferred indexes or sorting columns. Update all repository callers, examples, CLI, desktop, tests and benchmarks to use borrowed segment/component inspection. This is an intentional low-level source API migration, explicitly authorized by the original plan.

Do not preserve source compatibility by returning empty maps, copying the first segment, constructing a global index on accessor calls, or using a misleading field with a different meaning. Preserve high-level search, count, get_record, posting values, selection, streaming and lifecycle APIs.

Any global record enumeration API should iterate segment-qualified records from a pinned snapshot. If a caller needs a materialized vector, make that an explicit caller operation. Avoid permanently retaining both segment records and a second flat CollectionGeneration containing copies of every record.

### 9.3 Sort binding metadata must not own sort data

Remove `collection.sorting = first.index.sorting.clone()`.

Introduce or reuse compact SortSpecification metadata containing field/binding identity, logical type, locale, options, collation version and configured composite definitions. Bind query sort terms to those descriptions, then borrow each segment's actual column when executing.

Composite definitions are shared metadata; their sorted local slot arrays belong to segments. A missing segment column requires a correct fallback or an explicit coverage decision, not accidental access to the first segment's keys.

### 9.4 Eliminate duplicate global relation payloads

Keep relation declarations and logical-target resolution metadata shared. Keep forward and reverse posting contributions segment-local. The snapshot's ID and file routing may be global.

Remove finalization that clones and shifts all segment relation contributions into another collection-wide relation index. Update semijoin, backlinks, relation sort and validation to traverse the snapshot's local contributions. For reverse lookup, probing the relevant relation component in each segment is a valid first implementation; do not duplicate every reverse member globally to hide the migration.

Optional routing summaries may say which segments contain a relation target, if measured necessary. Such summaries must contain segment IDs, not copies of the complete record-membership lists. They are maintained at publication, not rebuilt per query.

### 9.5 Remove the production base-versus-delta split

All records in an ordinary completed index live in segments. Remove the flat production search branch and production base_active address case once callers have migrated. Do not keep unreachable flat construction machinery solely because an empty metadata Repository was convenient to initialize.

An empty snapshot has no segments and produces empty valid results through the same machinery. It must not switch to a different engine based on `has_native_segments()`.

Keep a simple correctness reference evaluator that enumerates active records from the pinned snapshot and evaluates predicates without optimized candidate proofs. It may share adapters and expression semantics; it must not share the optimized relation candidate bug it is intended to detect.

This is a focused separation of configuration, snapshot and execution. Do not undertake unrelated formatting changes or split modules arbitrarily by line count. Extract cohesive relation/sort/stream execution helpers where that removes duplication and makes ownership explicit.

## 10. R05 real segmentation of framed files and mutation batches

### 10.1 One boundary rule

Use RepositoryOptions::segment_policy everywhere. Defaults remain 16,384 records and 16,384 distinct files. Validate supported local-slot limits at every public construction entry point, including progressive opening.

At each completed contribution boundary:

1. Count incoming records and distinct files not already present in the draft.
2. If a nonempty draft would exceed either target, seal it before adding the contribution.
3. Move the contribution into the draft and assign its local base offset.
4. Seal after adding if either target has been reached.
5. Seal remaining nonempty drafts at end-of-input or an explicit publication request.

An indivisible contribution larger than a target may occupy an otherwise empty segment. Report that exception. Do not treat an entire framed file as indivisible when its adapter already yields bounded contributions. A later source chunk may be the first membership of that file in a new segment even though first_range is false.

Count files by identity, not by first_range flags. Source-level framing flags and destination membership are different facts. Preserve local diagnostics and slot remapping for every chunk.

### 10.2 Private file publication groups

For a large framed file, use an explicit construction state equivalent to:

```text
Reading -> Validated -> Published
Reading -> Invalidated -> FallbackPrepared -> Published
Reading/Validated -> Cancelled or Failed
```

At the start of a large file, seal any preceding ordinary draft. Keep the large file's drafts and completed segments in its own private publication group. This avoids complicated rollback of unrelated files while preserving parallel segment finalization within the large file.

As bounded contributions arrive, apply the common boundary rule and submit sealed drafts immediately. The file's completed segment payloads may finish before its final validation/fingerprint result. Hold those completed payloads privately. They must not appear in any searchable snapshot yet.

After successful file completion, wait for all required segment jobs, validate the group, finalize shared file metadata, and install all its segments and routes atomically. A progressive callback observes one consistent group publication even if the group contains several segments. Report publication count separately from segment count.

For malformed inputs, preserve the adapter's existing distinction between a recoverable per-record fallback and failure invalidating the entire physical file. Do not turn every malformed JSONL line into a whole-file rollback. If the adapter invalidates earlier records, discard all private output from that file attempt and publish only the established fallback outcome. No provisional ID routes, inference observations or diagnostics from discarded records may leak into the final snapshot.

### 10.3 Bounded scheduling without deadlock

Let P be the resolved build-worker count and W = max(1, 2P). Maintain W as the bound on queued, running and completed-but-not-consumed segment jobs. Keep existing extraction job and byte allowances separately bounded.

Use deterministic planned ordinals and an ordered completion buffer. A completion waiting behind an earlier ordinal still occupies a job slot. Once a completed result is consumed into the file's private group, release its scheduler slot even though the group is not publishable yet.

This release is essential: one file producing more than W segments must not fill all permits and prevent later chunks from reaching validation. The private completed native output can grow with the final index for that file; it is real retained output, not unbounded queued work. Account for it separately.

Coordinators may wait for worker results outside the Rayon worker pool. Component tasks must use Rayon join/scope/parallel iteration rather than blocking on bounded channels whose consumers need those same workers. Never hold publication locks while waiting for extraction or finalization.

On error/cancellation, stop submitting new work, signal cancellation to cooperative stages, and drain/join already dispatched work so its memory is released. Suppress publication of cancelled private groups. Do not detach work that continues retaining a large corpus after the caller reports cancellation. Return ordinary worker failures through the existing error mechanism; do not convert expected malformed input into a worker panic.

### 10.4 Shared-file status and diagnostics

A physical file can belong to several collections. Preserve the existing semantics of merged file status and diagnostic references. Final file-state publication must not require cloning every posting and bigram in affected segments.

Prefer catalog-owned file metadata with stable references from segment records. Keep derived status-dependent sorting coherent: rebuild only the affected sort component or represent the dependency as a snapshot metadata order, using the existing ordering semantics. Do not deep-clone an entire IndexedCollectionSegment just to update FileStatus.

If diagnostic references need remapping, finalize the mapping before exposing that publication and store it in the appropriate metadata layer. Remove per-hit searches over every segment that exist only to repair mismatched diagnostic numbering after query execution.

### 10.5 Mutation and compaction

Replace `SegmentAdmission::new` in production mutation paths with construction using the configured policy. PreparedCollectionBuild may contain several bounded contributions; route those contributions through the same segment planner rather than forcing them all into one segment.

Change mutation preparation outputs from an implicit one-segment-per-collection assumption to an explicit list of segments grouped by the physical-file transaction. Update callers accordingly. An explicit add/replace request may publish a smaller final segment; it need not wait for 16,384 records.

Replacement retires all old memberships and installs all new memberships in one publication. Move changes path/catalog data without rewriting field payloads. Delete retires every membership. Duplicate active IDs are checked against the prospective snapshot after the replaced identities are retired.

Compaction and small-segment consolidation use both record and file limits. The maintenance work budget remains separate from output size policy. Do not restore the hidden 2,048-record repartitioning step or accept a merged segment exceeding max_files merely because its record count fits.

## 11. R06 inference without a duplicate global posting builder

### 11.1 Required separation

The existing `batch.inference_copy()` clones a builder containing many record-membership bitmaps and provisional value maps. The coordinator then merges that clone into a global builder, while workers merge the originals into segments. Remove this duplication.

Separate two responsibilities:

- Segment-local inference data: candidate posting memberships, coverage, structural facts and provisional per-field native data.
- Collection-wide inference observations and decisions: counts, types, bounded distinct-key summaries, completeness/budget state, and admission results.

The collection summary must not contain record-membership bitmaps or values mapped to record-ID sets. Bounded sets of distinct values used to make exact policy decisions are allowed. They are different from a second posting map.

### 11.2 Concrete implementation route

Refactor `inference_build.rs` so a completed extraction batch can produce a compact owned InferenceObservationSummary while retaining its original local candidate data for the segment builder. Generate this summary on an extraction/worker path from the existing observation state; do not parse the source again or walk the same Value tree twice.

Summary generation may borrow local bitmaps to calculate cardinalities and intersections. Do not clone those bitmaps into the summary. Use checked/saturating arithmetic according to existing counter semantics and preserve bounded policy behavior.

The summary contains, as required by the current policy:

- Eligible, observed and unavailable record counts; completeness and omission state.
- Per-path kind information, nesting/fractional-number flags and occurrence counts.
- Per-record category counts computed after within-record deduplication.
- Bounded exact distinct-key sets and their byte charges; explicit overflow/truncation flags.
- Coverage and null/empty-selection counts needed for reports.
- Deterministic logical budget charges and policy version.
- Ancestor-empty-array information needed to classify paths discovered in later batches.

Preserve exact set relationships when computing counts. For example, a record can contain both null and non-null selected values; exact-null and null-occurrence are different. Do not sum overlapping category counts as if they were disjoint.

Late-discovered nested paths need special handling. Within a batch, reduce the existing ancestor and path bitmaps into cardinalities of the required unions/intersections. Retain sparse ancestor-path counts and per-path overlap counts, not their record memberships, in the summary. When a path was entirely absent from a batch, ancestor-empty counts can still contribute to its final empty-selection report. Add a direct comparison test against the old inference evaluator for this case before removing that evaluator from production.

Merge summary objects deterministically in input order. Ordinary summary merging may run in an ordered worker reduction if needed; the routing callback should primarily move summaries and contributions. Never reintroduce per-key posting aggregation on the coordinator under a summary name.

### 11.3 One global admission policy

Refactor the admission decision logic into a common function operating on observations, so segment-local finalization cannot accidentally apply a different threshold policy. Preserve declared-field precedence, explicit opt-out, type compatibility, completeness, distinct-value limits, repeated-string thresholds, short-string limits and structural-fact behavior after value rejection.

Represent admission by component capability, not one Boolean for an entire field. The current implementation can retain short-string, exact-empty-string, presence or array-length information even when a general value dictionary is rejected. A report status of Built does not prove that every value component is admitted or complete. Tests for low-cardinality rejection must inspect the particular general-value capability and its coverage; choose fixture values/policy limits that actually exercise that path instead of accidentally taking the independent short-string path.

Do not admit low-cardinality value indexes independently in every segment when the collection-wide policy rejects that field. Conversely, a field that reaches its minimum occurrence threshold only across multiple segments must be eligible globally even if each segment is individually below that threshold.

Retain bounded segment-local candidates sufficient to realize the globally admitted components. Local finalization must not throw away a candidate solely because that segment has too few occurrences. Local resource overflow is represented as incomplete coverage; it must not become a false exact proof.

Apply global admission through immutable snapshot metadata and component availability. For progressive construction, inferred value acceleration remains unavailable until the corresponding collection-level decision is finalized; declared indexes and ordinary FTS remain searchable. This is an explicit implementation decision to avoid prematurely claiming a global policy result. Report inference-ready timing separately. Preserve any existing stronger safe behavior only if it has a clear monotonic proof and equivalent tests.

After a decision, publish admission metadata and any component installation coherently. Do not mutate payloads visible to old snapshots. Use owned unfinished candidates or independently Arc-owned inference components to avoid cloning unrelated raw/field postings. Old views retain their previous admission state.

Rejected candidate payloads must be released once no pending decision or pinned view needs them. Maintain structural accelerators that the existing policy allows even when value dictionaries are rejected.

### 11.4 Budgets and lifecycle

Keep logical admission-policy accounting distinct from actual retained-memory estimates. Preserve current policy decisions at documented thresholds; do not substitute Roaring serialized size or allocator RSS for a policy charge. Preserve deterministic batch-order decisions for budget exhaustion. Compare the new summary decision against the old implementation in tests, including an exact-boundary case and one unit above it.

Account for live extraction batches, active segment jobs, retained provisional inference components and private file groups. No per-segment copy of a repository-wide allowance may silently multiply the intended budget by the number of segments.

A failed private file group contributes no final observations. Stage its summaries with the group and merge only accepted contributions. Retained observation summaries should be removable or recomputable from immutable per-segment summaries when lifecycle changes require a new report; do not permanently append deleted observations to a supposed current report.

Queries use the pinned snapshot's admission and local coverage. A field losing complete coverage after mutation uses the existing safe fallback until a new complete decision/component is available. Exact query results must not depend on report refresh timing.

### 11.5 Acceptance

`batch.inference_copy()` has no production caller and is removed if unused. No production collection-global InferenceBuilder aggregates membership bitmaps. The completed snapshot has no hidden global inferred posting copy. AutoIndex and ReportOnly results, explicit opt-outs and threshold decisions match the reference policy on the regression matrix. Report memory estimates identify actual retained segment data separately from compact observations.

## 12. R08 independent field component construction

### 12.1 Replace sequential families with owned component jobs

The current `admit_parallel` launches several consecutive pairwise joins. The custom-field loops inside them remain serial, and finalization runs each bigram component sequentially. Replace that structure with a concrete component job plan.

After segment-local record/file mappings are fixed, transpose owned batch contributions into component inputs. A component input contains an ordered list of owned contributions plus their segment-local base offsets. Moving a BTreeMap or bitmap-owning object transfers its ownership; cloning the map to place it into a job is forbidden.

Independent units are:

- One declared field PostingIndex, including that field's presence/null/length/structural facts.
- One named/custom posting binding.
- One raw, fallback or field-text bigram component.
- One independent sort column.
- One relation declaration's contribution.
- One inferred field component after the required policy information is available.

Use concrete types or a small private enum for job descriptions. Do not create a generic storage-backend trait, dynamic per-record dispatch, or a universal byte-valued component map.

### 12.2 Reduction and finalization

Each job reduces its ordered inputs into exactly one local output, finishes its statistics, and validates its own coverage/domain invariants. Preserve merge semantics and deterministic error selection. Independent jobs may run concurrently; contributions to the same component remain in their defined logical order where that affects diagnostics or admission.

Bigram jobs retain packed buffers until the existing native map construction stage. Generate normal and skip grams in the existing shared extraction traversal. Reuse its internal Rayon join if useful; never reread a source field to obtain a second gram family.

Sort columns finish independently. Composite orders run only after all constituent columns are available. Segment assembly runs after component results are collected and performs cross-component checks without repeating every component's full validation.

Return component results by ownership. Do not use a shared mutex around one aggregate for every posting insertion. Do not block a worker on a child job that can only run after that worker releases a queue permit.

### 12.3 Scheduling policy

Use the existing build pool. With one worker, execute component reductions serially. With multiple workers and at least two nonempty components, use Rayon parallel iteration or scoped work over component jobs.

Start with one task per component, not one task per key, record or batch. If benchmarks show task overhead on tiny components, group them using one documented threshold based on existing item/contribution counts. Any threshold change must retain independent ownership and permit a deterministic test to force concurrent heavy components. Do not add adaptive heap prediction.

Do not artificially serialize fields because several segment jobs are also active. Rayon provides shared scheduling; the planner should expose independent work without creating extra pools.

### 12.4 Instrumentation and tests

Keep separate active/peak counters for actual segment work and actual component reduction work. Test synchronization must be placed inside the relevant reduction, not before a function that might later run serially. A counter of parent tasks parked in nested joins is not proof of simultaneous useful component work.

With a single segment containing at least two substantial independent fields, P >= 2 must allow their reducers to enter a deterministic overlap gate. With multiple segments, verify segment overlap separately. Use a timeout/watchdog for failure diagnosis so a broken scheduling test cannot hang the whole suite indefinitely. Do not use sleeps as the success criterion.

Verify one-worker completion, out-of-order completion, cancellation, and stable logical signatures. Measure complete-build performance before and after this phase; do not assume more scheduled tasks automatically improve time.

## 13. R09 truthful counters and useful tests

### 13.1 Remove result-shaped accounting

Remove production adjustments that infer work from returned rows. Specifically replace the behavior in `stamp_native_result` that invents a file open when parses occurred, and the behavior in `stamp_native_query_result` that counts returned raw-fallback hits as exact posting matches.

Also audit `try_lifecycle_ordered_page`: it currently performs a page execution followed by a count execution, replaces the page counters with count counters, and then reconstructs some work using returned hits. Record and combine actual work instead. Prefer one prepared execution with page delivery and optional exact counting; if separate internal passes are temporarily necessary, reuse preparation and account for both passes explicitly.

Counts of matches and counts of work are different. Returning ten rows does not imply ten file opens or ten posting proofs. A cached or buffered record may be parsed without a new file open in that phase. Do not manufacture an open to make counters appear plausible.

### 13.2 Counter ownership

Assign counters to the operation that performs the work:

| Counter class | Measurement source |
| --- | --- |
| Physical file opens and bytes read | Source access/read session |
| Record parses and verification calls | Actual preparation/scanner calls |
| Relation target reads and edge visits | Relation access/traversal |
| Posting proof matches | Proven local membership before page truncation, according to existing documented semantics |
| Returned records | Final delivered page/all-match output |
| Distinct files | Deduplicated FileIds within the documented accounting scope |
| Source failures and count completeness | Actual failed access and execution semantics |
| Component and segment timings | Timed worker stages, identified as overlapping CPU/work intervals |

When merging worker counters, sum additive events and union distinct identities. Preserve the existing distinction between primary and relation-target accounting. Do not add counters twice because a target cache is reused. If current public counter definitions are ambiguous, document the definitions in the existing counter type and test them; do not silently change them.

### 13.3 Restore equivalent assertions

Review all tests changed by the native segment branch, especially `inference.test.rs` and executor/streaming tests. Restore assertions for source-free inferred sorting, posting access, cache reuse, and planning behavior where those remain required semantics or performance properties.

When a specific old flat-planner name is obsolete, replace the assertion with a meaningful segmented property: chosen access kind, complete local proof, actual zero reads, or bounded candidate work. `assert!(!explain.is_empty())` is not an equivalent replacement for an access-path test.

Do not merely rename a test whose advertised property is no longer checked. Explain the migration in the test and assert the new equivalent contract. Do not mark new repair regressions ignored to make workspace checks pass.

EXPLAIN reports the path that would be used without performing source reads or building expensive order caches. EXPLAIN ANALYZE reports actual execution. Do not repair traces after execution by guessing from the query syntax or output shape.

## 14. R10 lifecycle, publication and stable identity audit

After the shared model is in place, audit every consumer of RecordSlot and every use of original Repository segment/catalog state. Classify each as strictly local, snapshot-qualified, test-only reference, or incorrect. Record remaining exceptional uses with their invariant.

Inspect at least direct ID/path lookup, exact selection, posting-values enumeration, verified membership cache, inferred sort cache, relation target caches, streaming deferred materialization, source availability, evidence, compaction, moves and deletion.

ExactSelection must belong to an index lineage and snapshot revision. Its segment membership must be keyed by stable segment identity or remain explicitly bound to that exact pinned snapshot. Reject use with an incompatible revision instead of interpreting vector positions against a new layout.

For every publication:

1. Check the expected revision and configuration.
2. Prepare a private next snapshot using the current one.
3. Retire affected memberships and routes.
4. Validate duplicate active logical IDs against the prospective state.
5. Install new segments, file state, routes, admission metadata and statistics.
6. Allocate new segment identities where payload contents changed.
7. Validate cheap cross-root consistency checks.
8. Install once under the existing publication mechanism.

Expensive construction and validation occur outside the publication lock. Do not publish a partially updated catalog and repair its segment list afterward.

Queries pinned before the publication continue to use their old liveness and routing. Caches may reuse immutable segment computations only when their key and coverage permit it; verified whole-query membership must include the correct snapshot identity.

Keep final logical ordering independent of segment creation and compaction order. Cross-collection ordering uses the existing collection/record identity rules. File-scope queries deduplicate a physical file spanning several segments. Exact count must not count a file once per segment.

## 15. R11 required regression matrix

Use existing test modules and fixture utilities. Small fixtures should be deterministic and cheap. A tiny target alone does not guarantee tiny segments because boundaries occur between prepared contributions: the existing file batch may contain hundreds of records. Tests must explicitly construct multiple contributions, use a test-only extraction batch limit, or verify that the public fixture actually produced the intended segments. Do not accidentally run a multi-segment test with one oversized batch.

Test-only controls must not change production policy or create a second implementation. Prefer a shared internal builder with injected limits. Assert segment count, local slot collisions, and the active snapshot shape before checking the behavior under test.

| Test ID | Required scenario | Essential assertion |
| --- | --- | --- |
| T01 | Two source segments both contain local slot zero; only one related target matches | Exactly the correct logical source ID, never both |
| T02 | Matching target exists only at slot zero in the second target segment | Match is neither lost nor redirected to first segment |
| T03 | Relation target-first and source-restricted strategies | Same explicit expected IDs, count and evidence |
| T04 | Relation queries after source/target add, replace, delete, move and compaction | Current view correct; old pinned view logically stable |
| T05 | Source-dependent relation FILTER through streaming | Correct answers and bounded, actually used target reads |
| T06 | Inferred sort after adding a new minimum/maximum in another segment | Global first page and count include the new record |
| T07 | Inferred sort after retiring first segment; one later live segment remains | Sole live segment selected by identity, not position zero |
| T08 | Inferred sort after compaction and cache warmup | No stale order or wrong slot interpretation |
| T09 | Equal keys across segments and collections, multiple offsets | Stable tie-breaking and no duplicate/omitted rows |
| T10 | Incomplete inferred/string coverage and raw fallback | Correct computed fallback; no false complete order |
| T11 | Constant-left metadata compared with record/file-dependent RHS | Per-record membership correct in both operand orders |
| T12 | Metadata NOT/AND/OR and transforms | Correct may/must bounds and source-free final evaluation |
| T13 | Adopt Repository, retire/compact originals, drop old views | Weak payload references expire while Index remains alive |
| T14 | Same operation with an old view retained | Old payload remains until that view is dropped |
| T15 | One JSONL file creates more than 2P segments with P=1 and P>1 | Completes, bounded job window, all records present |
| T16 | Large framed file completion out of order | Deterministic boundaries and atomic publication |
| T17 | Late adapter failure invalidates prior chunks | No invalid provisional records, routes or inference observations visible |
| T18 | Recoverable malformed JSONL line | Existing per-record fallback behavior preserved |
| T19 | Cancellation during extraction, component reduction and private staging | No partial publication; dispatched work releases retained state |
| T20 | Replace one file whose old and new versions both span segments | One revision change exposes the complete replacement |
| T21 | Custom max_files differing from target_records, including empty files | Both limits respected by build, mutation and compaction |
| T22 | Oversized indivisible record/contribution | Explicitly reported exception; next ordinary batch starts new segment |
| T23 | Global inference minimum reached only across segments | Same admission as collection-wide reference policy |
| T24 | Locally low cardinality but global threshold exceeded | Affected value capability follows global rejection; permitted structural/short-string capabilities and query correctness remain intact |
| T25 | Mixed/null/empty/nested arrays, late paths, explicit opt-out, budget boundaries | Report and admission parity without global membership copy |
| T26 | One segment, two substantial independent field components | Actual component reducer overlap at P>=2 |
| T27 | Multiple segments, gated completion order | Actual segment overlap and outstanding jobs <=2P |
| T28 | Counter fixtures with shared files, several records, cache and payload modes | Exact measured opens/parses/proofs, no result-shaped repair |
| T29 | Empty repository/collection and fully retired collection | Valid empty results through the same production engine |
| T30 | Selection/caches created before layout-changing compaction | Correct old-view use or explicit incompatibility, never silent reinterpretation |
| T31 | Normal/skip bigrams, ASCII/Unicode, field coverage and raw fallback | Exact results match reference across segment sizes |
| T32 | Shared physical file in several collections with diagnostics/status sorting | Coherent catalog and diagnostic references without deep payload cloning |

Run representative correctness cases through Repository, adopted Index and progressive Index. The full Cartesian product is unnecessary; use a table-driven harness for the critical differences. Include Locator, Bytes/Logical where relevant, count-only, pagination, OrderedRows, PageReplacements, cancellation/sink failure, and cache disabled/enabled.

Require explicit expected IDs for the core regressions and differential comparison with the unoptimized evaluator for broader expression coverage. Compare evidence per record, not merely result counts. Source-free tests should make unintended reads observable through existing instrumented adapters or carefully controlled source removal, according to the operation's semantics.

## 16. R12 performance measurements and gates

### 16.1 Preserve two baselines

Record both the original master and reviewed native branch commits. Use the reviewed native branch as the immediate performance baseline for repairs. Use master as the historical comparison. Neither a known-wrong optimized result nor a layout-dependent hash is a correctness oracle.

Build baseline and candidate executables under the same toolchain, profile, features and lockfile. Record their hashes, dataset identity, exact commands, CPU/thread settings, cache settings and segment policy. Keep raw results in the repository's existing benchmark-results location or `docs/ai/generated/` as appropriate; do not leave the only evidence in a temporary directory.

The existing report claims approximately 0.797 seconds for the default ASCII 8-worker blocking build, 15 segments and about 503.5 MB peak RSS on Musenalm. It also reports slower first publication and roughly doubled progressive median query ratios despite acceptable p95 ratios. Treat these as reported measurements to reproduce under the same local environment, not universal timing targets.

### 16.2 Required workloads

1. Existing Musenalm build benchmark, ASCII and Unicode, default policy, one worker and the same normal worker count as baseline.
2. Existing unchanged 150-case query suite, cache disabled, blocking Repository and progressive Index.
3. Adopted Index queries after a deterministic mutation sequence, including inferred sorting and relation FTS.
4. A framed/JSONL fixture large enough to generate many segments, including more than 2P segments from one file.
5. AutoIndex construction with several fields and enough segments to exercise global inference decisions.
6. One/few-segment construction with several substantial independent fields, to measure component parallelism.
7. Replace/delete/compact measurements and payload reclamation checks.

Use three warmed repetitions and report medians for the standard comparisons. Measure first-searchable publication separately from full completion and inference-ready time. Record segment count/distribution, oversized contributions, source reads/bytes, peak extraction allowance, peak outstanding segment jobs, private completed output, actual worker overlap and peak RSS.

Complete-build wall time includes finalization, validation, global decisions, routing and the final installed snapshot. Overlapping component durations are work measurements, not additive wall-clock stages.

### 16.3 Provisional acceptance gates

Compared with the reviewed native branch under the same configuration, require no unexplained complete-build slowdown above 10%, no unexplained peak-RSS increase above 10%, and no unexplained query p95 regression above 10%. Investigate every individual query regression exceeding both 20% and 1 ms. Also report median ratios and absolute differences so cheap-query overhead is visible.

These are review gates, not permission to sacrifice correctness or inflate counters. If repairing a previously wrong shortcut increases its time, show the correct workload and explain the cost. Replacing an incorrect answer with the right answer is mandatory; then optimize the correct path. A failed performance gate requires a documented cause and resolution or an explicit incomplete disposition, not a hidden benchmark exclusion.

Memory should improve after removing retained initial payloads and duplicate inference state. Do not promise a specific percentage before measurement. First-publication latency must be reported; investigate any further material regression rather than hiding it behind improved total time.

Do not run an endless benchmark sweep. Start with the default target. Repeat the 8,192/16,384/32,768 comparison only if the repair changes the size tradeoff enough to justify retuning. Keep a simple documented default.

## 17. Implementation phases and commit sequence

Every phase should leave the integration branch buildable. Add focused tests with the repair when committing; record their pre-repair failures in task notes or the final report rather than pushing a deliberately broken shared branch solely to demonstrate failure.

### Phase 0 reproduce and preserve

- Record current branch/base, read instructions and preserve baseline executables/results.
- Add or prepare T01, T02, T06, T11, T13 and T15 as minimal reproductions.
- Verify fixtures genuinely contain multiple segments where required.
- Inventory production flat-state access and changed/weakened tests.
- Write a short interface decision note implementing section 5.

Suggested commit: `Record native segment repair baseline and regression coverage`.

### Phase 1 establish snapshot ownership and addresses

- Separate runtime/configuration from snapshot ownership.
- Define one qualified record address and stable segment identity authority.
- Implement snapshot segment/ID/file routing accessors.
- Make Repository, adopted Index and progressive Index own the same shape.
- Add adoption, reclamation and pinned-view tests.

Suggested commit: `Unify native snapshot ownership and record addresses`.

### Phase 2 correct relation and metadata execution

- Migrate semijoin source/target membership and witnesses to qualified sets.
- Remove stale flat target-prefetch calls and reuse snapshot-aware access.
- Preserve relation evidence and source/target cache scope.
- Correct metadata operand-dependence classification.
- Run T01–T05 and T11–T12.

Suggested commit: `Correct segmented relation membership and metadata predicates`.

### Phase 3 correct ordering and derived caches

- Migrate inferred order eligibility and construction to the pinned view.
- Implement local inferred orders and global merging.
- Introduce metadata-only sort binding; remove first-segment sorting copies.
- Fix cache keys/lifetimes and restore source-free ordering assertions.
- Run T06–T10 and affected streaming/sorting tests.

Suggested commit: `Bind inferred ordering to native snapshot segments`.

### Phase 4 segment framed files and align mutations

- Apply the common boundary rule to every bounded contribution.
- Implement private physical-file publication groups and permit release on consumption.
- Support multiple output segments per replacement membership.
- Align mutation and compaction record/file limits.
- Preserve diagnostics/status without full payload cloning.
- Run T15–T22 and T32, including one-worker and late-failure cases.

Suggested commit: `Build framed files as atomic groups of bounded segments`.

### Phase 5 remove duplicate inference and legacy payloads

- Separate observations, local candidates and global admission decisions.
- Eliminate inference_copy and global membership accumulation.
- Remove global relation duplicates and obsolete CollectionState payload fields.
- Migrate all public callers and remove production base/flat paths.
- Run T23–T25, empty-index cases, and the complete feature suites.

Suggested commit: `Separate inference decisions from segment payloads`.

### Phase 6 construct independent components concurrently

- Transpose owned contributions into concrete field-component jobs.
- Finish independent posting/text/sort/relation components in the shared pool.
- Preserve dependent composite ordering and single-pass extraction.
- Add real component overlap instrumentation and T26–T27.
- Compare build time and memory before further tuning.

Suggested commit: `Parallelize independent native index components`.

### Phase 7 repair diagnostics and complete validation

- Remove result-shaped counter adjustments and repeated planning where identified.
- Restore meaningful tests, complete lifecycle/cache audit, and remove temporary bridges.
- Run T28–T31 and all required checks.
- Run final comparable benchmarks and publish raw evidence and analysis.

Suggested commit: `Validate native segment execution and measured work`.

### Phase 8 completion record

- Complete the requirement/test matrix and document intentional low-level API changes.
- Move the task to done, append the report, commit and push.
- Keep the repair integration branch/worktree; do not merge master.

Suggested commit: `Document native segment repair results`.

The exact number of commits may differ, but do not place all architecture, correctness, performance and test weakening in one opaque final commit. Keep commit boundaries reviewable and report any dependency-driven changes to the sequence.

## 18. Required commands and final inspection

Use the repository's current documented commands and toolchain. At minimum run the corresponding checks already used for the native branch:

```text
cargo test -p objekte-search-core --lib --locked
cargo test -p objekte-search-core --tests --locked
cargo test --workspace --locked --no-fail-fast
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked
cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked
git diff --check
```

Run applicable existing ignored large-construction tests explicitly in release mode. List each relevant ignored test and why it was or was not executed. New small correctness regressions must run in the ordinary suite. Record existing warning-level diagnostics separately from newly introduced warnings.

Before completion, use targeted searches to inspect remaining production references to `repo.segments`, base_active, base_as_segment, partition_base, inference_copy, global inference merge_shifted, and first-segment sorting clones. A text match is an audit prompt, not an automatic failure: renamed symbols and legitimate local operations need semantic review. Document any remaining compatibility bridge and why it cannot supply conflicting production state.

Check that no new dependency or storage mode was introduced, no user file was overwritten, no result suite was silently reduced, and no performance assertion was replaced by a vacuous check.

## 19. Completion criteria and report template

The task is complete only when all of the following are true:

- R01–R12 are implemented and mapped to committed tests or measurements.
- The concrete wrong-result cases return explicit expected logical identities.
- Both entry points and adoption use one authoritative snapshot model.
- Old initial payloads are reclaimable after their actual readers release them.
- Large framed files produce bounded segments and atomically publish valid file groups.
- All producers and compaction use the configured record and file policy.
- There is no duplicate collection-wide inferred/relation posting model or false compatibility payload.
- Independent field reductions can actually overlap on the shared pool.
- Counters measure operations and tests retain meaningful performance properties.
- Required workspace checks and benchmark comparisons have completed.
- The integration branch, task completion report and raw evidence are committed and pushed.

Append a report with these sections:

1. Exact source/base/final implementation revisions and worktree.
2. Ownership and public API changes, with the final snapshot relationship described plainly.
3. R01–R12 status table and test names proving each requirement.
4. Reproduced defects and before/after behavior, including any adjusted reproduction.
5. Framed-file scheduling/publication behavior and cancellation evidence.
6. Inference policy parity and retained-memory accounting.
7. Test/check commands and outcomes, including explicitly run ignored tests.
8. Build/query/mutation performance comparison with absolute timings, ratios, RSS and first-publication latency.
9. Remaining limitations, each with its impact and exact scope.
10. Branch/worktree cleanup and user merge readiness.

Do not claim complete architecture migration while retaining an authoritative flat production path. Do not claim bounded segmentation while a framed file bypasses it. Do not claim correct identity handling while any cross-segment set contains only a local slot. If a required item cannot be completed, state that item and the concrete blocker instead of describing the branch as ready to merge.

## 20. Reviewed source references

These links preserve the evidence used for this specification. Read the current implementation before editing; fixes may move or replace these functions.

- [Relation source membership and witness application](https://github.com/Simon-Martens/objekte/blob/9c61fbc5d5510ae1a2713ffba160d2155742abfe/crates/core/src/run.rs#L6404)
- [Relation target candidates](https://github.com/Simon-Martens/objekte/blob/9c61fbc5d5510ae1a2713ffba160d2155742abfe/crates/core/src/relation_semijoin.rs#L222)
- [Inferred order snapshot mismatch](https://github.com/Simon-Martens/objekte/blob/9c61fbc5d5510ae1a2713ffba160d2155742abfe/crates/core/src/run.rs#L10868)
- [Metadata constant evaluation](https://github.com/Simon-Martens/objekte/blob/9c61fbc5d5510ae1a2713ffba160d2155742abfe/crates/core/src/executor.rs#L595)
- [Repository adoption ownership](https://github.com/Simon-Martens/objekte/blob/9c61fbc5d5510ae1a2713ffba160d2155742abfe/crates/core/src/lifecycle.rs#L552)
- [Large-file admission](https://github.com/Simon-Martens/objekte/blob/9c61fbc5d5510ae1a2713ffba160d2155742abfe/crates/core/src/repository_incremental.rs#L154)
- [Mutation admission policy](https://github.com/Simon-Martens/objekte/blob/9c61fbc5d5510ae1a2713ffba160d2155742abfe/crates/core/src/lifecycle.rs#L940)
- [Global inference duplication](https://github.com/Simon-Martens/objekte/blob/9c61fbc5d5510ae1a2713ffba160d2155742abfe/crates/core/src/repository_incremental.rs#L115)
- [Compatibility sorting copy and global relations](https://github.com/Simon-Martens/objekte/blob/9c61fbc5d5510ae1a2713ffba160d2155742abfe/crates/core/src/repository_incremental.rs#L1355)
- [Pairwise aggregation stages](https://github.com/Simon-Martens/objekte/blob/9c61fbc5d5510ae1a2713ffba160d2155742abfe/crates/core/src/mutation_segment.rs#L164)
- [Sequential component finalization](https://github.com/Simon-Martens/objekte/blob/9c61fbc5d5510ae1a2713ffba160d2155742abfe/crates/core/src/mutation_segment.rs#L561)
- [Result-shaped counter adjustments](https://github.com/Simon-Martens/objekte/blob/9c61fbc5d5510ae1a2713ffba160d2155742abfe/crates/core/src/repository.rs#L1186)
- [Changed inferred-order tests](https://github.com/Simon-Martens/objekte/blob/9c61fbc5d5510ae1a2713ffba160d2155742abfe/crates/core/src/inference.test.rs#L1783)
- [Native construction plan and implementation report](https://github.com/Simon-Martens/objekte/blob/9c61fbc5d5510ae1a2713ffba160d2155742abfe/docs/ai/done/objekte_native_segment_construction_implementation_plan.md)
