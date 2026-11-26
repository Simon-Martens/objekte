# Objekte implementation specification for interchangeable index storage

**Assignment:** `index-storage-backends`  
**Integration branch:** `integration/index-storage-backends`  
**Reference revision:** `cf7253d048c9d6fe5d2a7e9b47441b2207cce932` on `master`  
**Deliverable:** One production memory backend, a backend-neutral read and write boundary used throughout the workspace, and a small independent test backend.

## 1. Required result

Move the physical in-memory indexes, segments, segment catalog, and file catalog into `crates/core/src/index/memory/`. A future `index/disk/` implementation must be able to build, publish, query, update, and compact the same logical index with different data structures. A backend must also be able to read existing indexed information in bounded batches so that a future conversion can proceed in either direction without opening the original source files.

This document supersedes the earlier query-read-interface assignment. **Construction, ingestion, lifecycle, maintenance, query execution, inspection, and workspace clients all use the interfaces.** The physical algorithms implementing an interface can use their own private representation directly. There is one planner, one publication coordinator, and one set of public search semantics.

Deliver the memory backend in this assignment. Production disk storage, a disk format, mmap, cache eviction, save/load commands and a user-facing conversion operation are separate future assignments. Do not create an empty `disk/` module. The test backend provides the evidence that these later additions are possible.

## 2. Starting points in the current implementation

| Area to inspect | What the agent must replace |
|---|---|
| `index/{posting,bigram,sorting,relation,segment}`, `inference` | Concrete B-trees, bitmaps, sort columns, relation tables, projections, and sealed segment layouts. Separate logical keys, policies, reports, and comparison rules from the owning containers. Include default, inferred, named/custom, raw/fallback, and per-field indexes. |
| `construction/{batch,segment_admission,segment_jobs,repository_state}`, `ingestion/publication.rs` | Workers already build `PostingCollection`, `BigramContribution`, sort-key maps, and relation contributions. Introducing an interface only at the final seal would still force every writer to construct the memory representation. Cover worker accumulation, ordered batch admission, and direct and file-based ingestion. |
| `repository/{mod,state,inspection}.rs`, `index/segment/mod.rs` | The base collection owns physical index fields and its own ID table. `SegmentReadView` still exposes `&IndexedCollectionSegment`. Both base and segmented search must use the new contracts. |
| `lifecycle/publication.rs`, `inventory/{mod,scalar}.rs`, `lifecycle/catalog.rs`, `files/mod.rs` | `PublishedIndex` retains segment vectors, liveness, record routes, file routes, file versions, and catalog state. File lookup combines a base inventory with later revisions. These are first-class storage responsibilities. The public `files::FileCatalog` configures the optional file service. |
| `query_access`, `query_execution/{restrictions,runner}`, `query/sort_binding.rs`, `query_results/{ranking,cache}` | Preserve the provider offer/application design. Remove requirements for a native posting reference, Roaring input, complete sort vector, concrete segment backing, or repository field access in generic code. |
| `lifecycle/{initial,mutation,reconciliation,maintenance}`, `index/segment/{compaction,consolidation,projection}` | Preparation, verified replacement, retirement, route updates, base partitioning, and compaction reach directly into indexed data. Preserve their scheduling and policy. Make their physical reads and writes replaceable. |
| CLI, TUI, desktop, loaders, examples, benchmarks, tests | Find direct consumers, including benchmarks that construct a native posting collection or file inventory and public inspection APIs that return concrete references. Migrate them in the same branch. |

[Construction batch](https://github.com/Simon-Martens/objekte/blob/cf7253d048/crates/core/src/construction/batch.rs) · [Base collection](https://github.com/Simon-Martens/objekte/blob/cf7253d048/crates/core/src/repository/state.rs) · [Publication and routes](https://github.com/Simon-Martens/objekte/blob/cf7253d048/crates/core/src/lifecycle/publication.rs) · [File catalog](https://github.com/Simon-Martens/objekte/blob/cf7253d048/crates/core/src/lifecycle/catalog.rs) · [Query access](https://github.com/Simon-Martens/objekte/blob/cf7253d048/crates/core/src/query_access/contract.rs) · [Compaction](https://github.com/Simon-Martens/objekte/blob/cf7253d048/crates/core/src/index/segment/compaction.rs)

## 3. Module ownership and dispatch

Use the following layout under `crates/core/src/index/`. The file names identify ownership. A file may be split into children as it grows.

| Location | Owns |
|---|---|
| `storage/mod.rs`, `storage/publication.rs` | `StorageFactory`, the memory default selection, immutable published storage handles, candidate-publication assembly, shared error and bounded-batch conventions. |
| `storage/segment.rs`, `storage/segment_catalog.rs`, `storage/file_catalog.rs` | Segment read, build and maintenance contracts. Separate read and edit contracts for each catalog. Logical descriptors, identities, versioned handles and cursor contracts. |
| `storage/component.rs`, `storage/transfer.rs` | Component descriptors, component builders and sealed handles, ordered traversal, and the logical block source/sink used by maintenance and a future conversion. Keep predicate offers and proof semantics in the existing `query_access` contract. |
| `memory/{posting,bigram,sorting,relation,segment}/` | Concrete readers, writers, worker accumulators, projection and merge algorithms, private caches, concrete inference indexes and fast paths. The current simple relation implementation may begin as `memory/relation.rs`. |
| `memory/segment_catalog.rs`, `memory/file_catalog.rs`, `memory/mod.rs` | Native directories, routes, masks, path and metadata indexes, copy-on-write updates, memory factory and wiring. Larger catalog files may become modules. |
| Existing neutral modules | Keep record IDs, file identities, field paths, schema, extraction, inference policies, bigram alphabet, collation options, segment sizing policy, query semantics and public file registration independent of the backend. Preserve established import paths for logical types with narrow re-exports where useful. |

Implement the contracts as internal Rust traits with opaque, cheaply cloneable handles for *sealed* data. Use `Arc<dyn … + Send + Sync>` at shared ownership boundaries, borrow handles during a request, and use an owned worker-local builder at write boundaries. Concrete private types and statically dispatched loops remain available inside each backend. Dispatch at component, segment, batch, or cursor-refill granularity. Reuse an existing interface when it already supplies the operation. Do not create one trait per struct or a second predicate-provider registry.

Select the default factory once at the repository/index creation boundary. Pass the same selection through every open/build path, progressive jobs, ingestion, mutations, `empty_like`, rebuild, and compaction. Provide an internal injection point used by conformance tests. A segment refers to sealed component **handles**. A published root holds separate segment-catalog and optional file-service handles. Backend choice can vary by component, segment, segment catalog, and file catalog. A composition factory can select the target independently for each. Neither a backend-kind switch in generic code nor an `as_memory()` escape hatch satisfies this assignment.

## 4. Read and write contracts

Use these responsibilities and method shapes. Preserve existing application-level names where appropriate. Operations that can access backend data return a typed result; use `Result<Option<T>, E>` when absence is a valid result. An immutable logical descriptor or counter already captured on the handle can be infallible. Batch limits include both item count and bytes. Define the existing oversized-single-item/error behavior explicitly. Make EOF, cancellation, and failure distinguishable.

| Contract | Required operations and invariants |
|---|---|
| **StorageFactory** | Create empty stores and worker writers, start segment builds, create editors from pinned catalog revisions, choose component builders, and assemble sealed handles. It must be possible to build directly into the selected representation from the first worker batch. Thread-pool sizing and scheduling remain in core. |
| **Published storage** | Pin a mutually consistent revision, collection/base descriptors, segment directory, liveness, file state, schema/configuration identity and routing. Resolve services through this revision. A pin may retain a small root and backing resource handles. It cannot require loading every page. |
| **SegmentCatalogReader / SegmentCatalogEditor** | List a collection’s segment descriptors incrementally, resolve a segment ID, route `(collection, record ID)` to a live address, find participating segments for a file, read live membership/counts, and handle base records. An editor stages installation, replacement, retirement, consolidation, masks and route deltas and seals a successor. Preserve uniqueness rules, stable identities and local-slot domains. No always-resident shadow directory is permitted in `Repository` or `PublishedIndex`. |
| **FileCatalogReader / FileCatalogEditor** | Lookup by file ID or exact repository-relative path; use existing path-range and file scalar capabilities for size, modified time, created time and unknown creation time. Enumerate in bounded batches. Preserve versions, fingerprints, configuration identity, status, diagnostics, registered collections, and file ↔ record provenance. Stage discover/register, append, move, change, retire, rebase, and construction completion. Route file-to-record membership through the segment catalog and relevant segments where those own the membership. Avoid a second global copy of the mapping. |
| **SegmentReader / SegmentBuilder** | Read descriptors, counts, live headers, ID/slot lookup, bounded file membership, and requested locators or projections. Enumerate available components and obtain their read handles. Builders accept worker contributions, preserve input order and deterministic slot assignment, attach component handles, validate, and seal an immutable segment. Keep the current distinction between file ranges, retained values, and versioned external record readers behind record access. |
| **Component readers / builders** | Support declared, inferred, and named postings, structural facts, normal/skip/folded text candidates as configured, stored and inferred ordering, and forward/reverse relations. Builders receive logical contributions with local slots and completeness information. Readers expose supported operations, descriptors, estimates, and bounded logical traversal for maintenance. Allow several index families to offer access for the same predicate. |
| **Ordered access** | Bind an opaque order handle for available sort specifications. Open a fallible bounded cursor in the requested direction; return comparable keys and slot addresses for cross-segment merging. Support selected-slot key reads for ranking and preserve scalar/composite ordering, ICU settings, missing/null placement, direction and deterministic ties. A native backend may borrow existing columns privately. The caller cannot require a complete order vector. |
| **Membership and predicate access** | Retain `IndexAccessProvider::propose/apply`, `DomainId`, the restriction/proof model and cost-based choice. Adapt membership inputs/results to support borrowed or guarded encoded data, bounded iteration, containment, cardinality, intersection/union/difference and exactness. Continue using native Roaring operations internally and query-owned bitmaps for real computed intermediates. Provider storage and planner scratch have independent representations. |

### Minimum Rust call shapes

Use the following shapes when wiring the contracts. The identifiers for argument records below denote logical inputs. Reuse existing ID, policy, revision and error types. Keep the traits object-safe at runtime selection points. A worker builder is `Send` and exclusively owned; sealing consumes it. Readers are shared. An editor is derived from the pinned root it will replace.

```rust
trait SegmentBuilder: Send {
    fn admit(&mut self, part: PreparedContribution, slots: SlotAssignment)
        -> StorageResult<()>;
    fn attach_component(&mut self, id: ComponentId, data: ComponentHandle)
        -> StorageResult<()>;
    fn seal(self: Box<Self>) -> StorageResult<SealedSegment>;
}

trait SegmentCatalogEditor: Send {
    fn apply(&mut self, changes: SegmentDirectoryDelta<'_>) -> StorageResult<()>;
    fn seal(self: Box<Self>) -> StorageResult<SegmentCatalogHandle>;
}

trait FileCatalogEditor: Send {
    fn apply(&mut self, changes: FileCatalogDelta<'_>) -> StorageResult<()>;
    fn seal(self: Box<Self>) -> StorageResult<FileCatalogHandle>;
}

trait OrderedCursor: Send {
    fn read_batch(&mut self, limit: BatchLimit, output: &mut OrderBatch)
        -> StorageResult<BatchStatus>;
}
```

`ComponentId` identifies a logical family plus its field/binding/representation within the segment. `PreparedContribution` carries transferable staged ownership. `SealedSegment` contains a segment handle and bounded or streamable routing contributions. `OrderBatch` is reusable scratch or a guarded batch; it need not own a vector of decoded keys. `BatchStatus` distinguishes progress from completion. Forbid zero-progress refill loops. Use a checked shared error for storage failures with context on the operation, segment, component or file. Map it once at existing query, ingestion and lifecycle boundaries; never turn an error into absence with `.ok()`.

Keep backend selection distinct from index selection. For example, the **storage factory** chooses where the declared posting and sort order for `title` are built. Later, the **query planner** compares a posting restriction with an ordered traversal using the offers those components expose. The planner must never select “memory” or “disk” to answer a predicate.

### Ownership, lifetimes and observable behavior

- Pin the repository instance, collection, revision, and base/segment domain. A local slot alone has no cross-segment meaning. Carry domain information through offers, membership operations, cursors, relations, caches and stale-handle checks.
- A batch may borrow native storage, own a bounded buffer, or carry a backend page guard. Make the lifetime explicit. A cursor can release its last batch when advancing. Do not return an unguarded reference that assumes a mapped page stays resident for the entire publication. Keep pinned views usable after later commits or compaction.
- Continue to enforce `proven ⊆ matches ⊆ possible ⊆ effective live input`. Partial coverage preserves residual verification. An absent optional index may change the plan. An I/O or integrity error midway through an advertised operation must surface as an error; it cannot produce an empty result or a successful short stream. Preserve cancellation, budgets and available-source search semantics.
- `files::FileCatalog` stays optional for programmatic indexes. Direct records retain indexed record-ID lookup without a file service. Catalog paths are relative to the registered root. Multiple records per file, shared physical files across collections, path-only associations, optional ranges and unavailable creation times remain valid. Storage placement is independent of record origin.
- Build and publication state are separate. Unsealed writer state is private. A sealed component may be staged without becoming visible. Only the revision installation described below changes the published view. Do not expose backend keys, offsets, serialized bytes, lock guards or filesystem paths as logical record/segment identity.

## 5. Implement the complete write path

### Initial construction and ingestion

1. Keep source discovery, reader scheduling, framing, validation, identity derivation, schema/inference policy, and extraction where they belong. Send discoveries to a file-catalog editor as bounded batches. Workers request logical file metadata through the catalog or a bounded read-only job descriptor. Eliminate accesses such as `inventory.files[file_id]` in generic scheduling code.
2. A worker receives a writer from the chosen factory. Feed it bounded batches of the already-extracted record headers, logical field facts, text contributions, relation targets and metadata needed by the configured families. A borrowed projection may live for the synchronous consumption of a batch. Consume owned buffers where the current implementation transfers ownership. Do not clone entire records, flatten all index families into one event vector, emit one boxed message per value, or parse the source a second time to satisfy a writer.
3. Preserve worker-local index construction. The native writer owns the existing local posting, bigram, sort and relation accumulators. At the worker/coordinator boundary carry an opaque prepared contribution, its logical record count, file boundaries, and the small statistics needed for scheduling. Preserve fast native adoption/merge behind the writer. A writer targeting another layout can accumulate that layout directly. No generic producer may instantiate a native accumulator as a prerequisite.
4. Ordered admission reserves the current slot range, checks limits, and hands the contribution to its segment builder. Maintain existing segment sealing policies, file-boundary rules, progressive publication, queue/backpressure limits, and multi-worker concurrency. Both programmatic-batch ingestion and managed-file ingestion use this route. Preserve a path for zero-record files, failed records, and files shared across collections.
5. Move physical inference dictionary and posting construction into the memory implementation. Keep the decision about whether a field is admissible for indexing, the observable inference report, and schema interpretation reusable. Routine collection or segment construction must never build a second complete representation to cross the new boundary.

### Publication, mutation and maintenance

Use the current lifecycle coordinator as the sole visibility point. Keep its lock and revision rules. The following is the required sequence for each existing atomic publication unit, including one progressive publication.

1. Capture the base revision and relevant file/record version preconditions. Create unpublished editors and segment builders from that revision. Use the configured factories for target stores.
2. Prepare and seal changed components, segments, file entries, masks, and routes off the publication lock wherever the current coordinator permits. Reuse untouched components and catalog structure. Carry only touched deltas across the preparation boundary. For operations that change file links and record locations together, build a single candidate root containing both changes.
3. At installation, revalidate preconditions against the current revision, apply the existing conflict/retry semantics and atomically install the fully prepared root. Keep expensive I/O, full index validation, decoding and compaction outside the installation critical section. Publish receipts, completed-file status and visible revision only on success at the established points.
4. Failure, cancellation or a stale base before installation leaves the old root and its routes intact. Release unpublished preparations on abort. Retries cannot install the same records twice or resurrect retired IDs. A concurrent reader observes one complete revision. This specifies visibility within the process; durable crash recovery belongs to the eventual disk implementation.
5. Cover file add/move/delete/replace, linked direct records, verified slot-preserving replacement, reconciliation, rebuild, active-only compaction, base partitioning and segment consolidation. An unchanged-file move must retain its current index reuse. Compaction may deliberately remap local slots and must translate all affected indexes, masks, relations and routes consistently. Generic maintenance chooses *what* to compact. Backends implement movement of indexed data via the interfaces.

### Bounded transfer between representations

Give current source-free maintenance a logical block **source** and **sink**. These are the foundation for future memory-to-disk and disk-to-memory conversion. Use them for at least one real maintenance path and for an independent-representation test. Make the transfer version an internal contract version if needed. A disk file format is outside this assignment.

- The source describes component family, field or binding identity, configuration, representation version, coverage/proof characteristics and logical slot domain. Then it supplies typed blocks. Examples include a posting key with a chunk of sorted local slots, a gram plus posting slots, an ordered run of sort keys and slots, relation endpoints, and record headers or file rows. A single high-frequency key must be splittable across batches. Retained record content can have multiple bounded parts under its existing size limit.
- Provide corresponding bounded catalog traversal and writer inputs sufficient to preserve file IDs, retired holes or allocation high-water marks, file versions, segment identities, record addresses, memberships and liveness. Include owning programmatic retained data or external-reader version descriptors as appropriate. Preserve file revision/range associations. A future conversion cannot require rebuilding from original documents or calling a live record reader.
- Distinguish identity-preserving transfer from compaction. Identity transfer preserves logical IDs, slot/domain layout, order and diagnostics. Compaction explicitly supplies a slot mapping and may omit retired slots according to policy. A transfer must preserve absence, null, coverage, exact versus candidate semantics, text representation, relation bindings, and sort/collation compatibility. Reject mismatched descriptors before publication.
- Stream ordered chunks with explicit completion and checked errors. The native backend may retain an internal optimized projection, move, or shared-handle path when representations agree. The generic path must work with two backends that share no physical types. No production conversion command or whole-index copy on normal build/open is required.

## 6. Migrate the consumers and close bypasses

| Consumer | Required change |
|---|---|
| Planner, predicate access, execution and reference evaluation | Put native provider implementations and native index catalog adapters in `index/memory/`. Keep offer selection, logical predicates and independent source evaluation neutral. Explain, streaming and ordinary queries consume the same operations. Route record-ID lookup and relation semijoins through catalog/segment/provider handles. |
| Ranking, sorting, cached results, payload delivery | Replace bound references to `SortColumn`, native variant matches, and indexing into `keys`, `ascending`, or `descending`. Use order cursors, selected-slot projections, and neutral record access. Preserve early page termination, reuse of verified results, cache budgets, and batched source reads. |
| File service and repository inspection | Adapt `file_catalog()`, `file_by_path`, catalog rows, file metadata filters, forward/reverse file membership, direct lookups, field-value inspection, relations, statistics, and diagnostics. Service methods may collect the page or vector their public contract explicitly requests. Lower layers remain bounded. Status polling must retain its cheap counters. |
| CLI/TUI/desktop, loaders, examples and benchmarks | Keep user-visible query syntax, behavior, formats, and flags. Replace access to `CollectionState.postings`, file inventory vectors or native relation tables with neutral descriptors/operations. Move a genuinely physical microbenchmark into the memory backend’s own test/benchmark surface or exercise an equivalent backend operation. Keep its measurements meaningful. |

**Public Rust API decision.** Preserve behavior and call signatures wherever a signature already expresses a logical operation. An API returning a borrowed native map, `&FileInventory`, `&RelationIndex`, or `&RoaringBitmap` for a stored posting exposes the representation and must be replaced. Migrate all in-tree callers and list each unavoidable signature change with its replacement in the report. Keep public configuration names such as `BigramAlphabet` and `SegmentPolicy` usable at their established paths. For a borrowed row, use a guarded view or an owned bounded result. For an existing infallible lookup that must perform fallible backend work, migrate to a checked lookup/cursor and update callers. Preserve the distinction between absent and failed. A compatibility wrapper is acceptable only when it preserves that distinction and does not reconstruct a resident mirror.

**Dependency rule.** `index/memory` depends on the contracts; generic production code depends on the contracts. The single default-factory construction point may name the memory factory. Backend-specific unit tests may inspect their own implementation. Enforce privacy and add a small repeatable boundary check for forbidden imports, re-exports and native types in neutral signatures. Also inspect inferred types and field access through `Repository`, `CollectionState`, or a catalog handle. An import-only check misses those bypasses. Ordinary logical values and query-owned temporary maps/bitmaps are valid outside a backend.

## 7. Implementation order and checkpoints

Start from the current authorized integration base. Incorporate any intervening completed read-interface work and record the starting SHA. Carry out the stages on one integration branch. Each checkpoint should compile before moving to the next.

| Stage | Work | Exit evidence |
|---|---|---|
| 1. Baseline and surface inventory | Run representative release reads and writes. Map every native construction, read, mutation, maintenance and public inspection entrance, including external crate consumers. Record current tests and public physical signatures. | A before table with caller, owning object, intended contract, and benchmark baseline. |
| 2. Separate layout | Move physical families under `index/memory`; extract neutral identities, configuration and comparison behavior. Introduce the storage factory and sealed handles. Temporary import aliases are acceptable inside the integration branch. | Memory still builds and queries correctly. Move-only changes are distinguishable from semantic ones. |
| 3. Establish catalogs and publication | Implement memory file/segment readers and editors. Route the immutable root, liveness, record routes, file versions, base collections, and public file service through these contracts. Introduce the independent catalog fixture here. | Pinned lookup, move/retire and failed-install tests pass; two revisions can coexist. |
| 4. Establish writers | Wire worker accumulation, initial build, direct and managed ingestion, update and sealing through factories/builders. Keep native parallel construction and buffer reuse. | The independent writer builds a queryable fixture through normal ingestion, and producer instrumentation records the selected writer. |
| 5. Complete reads | Migrate predicate, ordered, relation, metadata, source projection, direct lookup, inspection and all delivery paths. Migrate workspace clients. | The same production entry points work across backends, including one segment containing components with different backings. |
| 6. Complete maintenance and transfer | Route compaction, verified replacement, consolidation, rebuild and catalog rebasing through readers/editors and bounded transfer. Remove transitional native aliases and duplicate state. | Cross-representation maintenance succeeds with original source reads disabled. Dependency checks pass. |
| 7. Verify and report | Run the acceptance matrix, workspace checks and release comparisons. Fix regressions and report the remaining documented API changes. | One reproducible integration report and a branch ready for user review. |

## 8. Conformance and failure tests

Implement a deliberately small second backend under test configuration. Use ordered tables or short blocks, a non-native membership encoding, cursors with short batches, and independent segment and file directories. Implement bounded writes for the fixtures. A wrapper around memory structs for those tested operations cannot demonstrate independence. Reuse the current sorted-table provider fixture where it is genuinely independent. Fixtures can support a modest schema and advertise the subset accurately.

| Case | Required observation |
|---|---|
| Independent construction | Build with the memory factory and the test factory through normal entry points. Cover retained direct records, versioned-reader records and managed multi-record files across fixtures. Verify the independent path constructs its own segments and catalog rows without first building memory indexes. |
| Mixed placement | Run a real query across one native and one independent segment. In a separate case use a native posting and an independently stored sort order in **the same segment**. Swap the segment catalog and file catalog independently of those components. Use the same planner, executor and publication path. |
| Query equivalence | Compare ordered IDs, exact totals, payloads, match evidence and diagnostics against memory and known answers or the independent reference evaluator. Cover exact and candidate-only predicates, a false-positive bigram, partial coverage, a relation crossing segments, an ordered page tied across segments, and streaming. |
| File and identity behavior | Check record-ID lookup without file registration, file ID and exact relative path, two equal basenames in different folders, multiple records per file, shared files across collections, reverse links, metadata ranges and unavailable creation time. Check old and new pinned views after move, replacement and retirement. |
| Producers and maintenance | Compare initial build, bounded group ingestion, mutation and source-free compaction. Transfer memory → test → memory on a small fixture using logical blocks. Compare descriptors, record/file IDs, order, liveness, coverage and query results. For compaction deliberately change local slots and verify routes and results. Prove that the source loader was never called during transfer. |
| Failure and concurrency | Inject failures after a successful write batch, during catalog finalization, and after a cursor emits a batch. Verify a failed publication exposes neither new records nor an orphan catalog row. A retry succeeds once. Check simultaneous writers under current conflict rules, cancellation, old-view reads during compaction, and an active stream across a revision change. |
| Bounded work | Count components opened, cursor refills, peak live batches, copied/decoded rows and source reads in the fixture. Verify planning can decline an unused index without reading its contents. A small ordered page under a provably complete order stops early; a high-frequency posting is consumed by blocks. Lookups by ID/path do not scan every segment/file. |

Use current integration tests to exercise all supported families on the production backend. Write narrowly targeted tests where the current suite cannot detect storage leakage, torn publications, unwanted materialization or a bypassed producer.

## 9. Native performance and completion evidence

1. Run `cargo fmt --all -- --check`, `cargo check --workspace --all-targets` and `cargo test --workspace`. Run relevant feature-gated and release tests as specified by the repository. Compile all examples and benchmark callers affected by public API changes.
2. Compare clean release builds of the recorded base and final code on the same machine, dataset, worker counts and settings. Use the current Musenalm quick query comparison plus focused repeated cases for exact postings, bigram verification, cross-segment relations, a small sorted page, direct lookup and file-path lookup. Include cold query/result caches and warmed repetitions as applicable. Use the existing build, managed-file/record-ingestion and lifecycle mutation/compaction harnesses for the producer side. [Benchmark procedures](https://github.com/Simon-Martens/objekte/blob/cf7253d048/bench/README.md)
3. Report median build and ingestion throughput, mutation/compaction elapsed time, query latency, peak RSS and available work counters. Include the existing fast parallel worker configuration as well as a single-worker comparison for the build. Run at least three paired measurements of the focused operations and repeat noisy material differences. Reuse fixtures and build artifacts between cases when the harness allows it. Existing project regression gates apply. A reproducible slowdown from mandatory copies, per-hit dispatch, whole-index reconstruction, or poorer parallelism requires correction before merge. Report raw values and any unavailable measurement; a correctness-only pass cannot establish performance.
4. Attach a coverage table mapping each producer and consumer to its final interface and conformance test. Include exact commands, base/final SHAs, the API migration table and any limitations of the test backend. Show which concrete types remain accessible outside `index/memory/` and justify any logical public types with historical paths. Confirm the production backend can be replaced at the factory and catalog/component wiring points without edits to generic consumer or producer logic.
5. Follow `docs/ai/README.md` for the sibling worktree, integration branch, pushes, and completed task report. Submit the integration branch for review. Implementation of the production disk backend and the user’s merge to `master` are subsequent decisions.


## Implementation report

| Item | Value |
| --- | --- |
| Task | `index-storage-backends` |
| Base revision | `cf7253d048c9d6fe5d2a7e9b47441b2207cce932` |
| Integration branch | `integration/index-storage-backends` |
| Implementation and performance comparison revision | `c760904257c05cb50ccc50959021a78408378e12` |
| Integration worktree | `../objekte-integration-index-storage-backends` |
| Completion date | 2026-11-26 |

Production uses one memory factory selected at open or build. It owns the physical indexes, worker accumulators, file catalog, segment directory and sealed segments. The publication coordinator installs a catalog revision, placements, masks and routes together. Generic producers and consumers use the storage contracts. The selected segment can carry an independently stored order alongside native postings. A flat collection base uses the same reader contracts with its own pinned slot domain.

The independent test backend uses ordered tables, sorted slot membership and short owned batches. Its ordinary writers cover numeric score, file-range records, owned retained records and versioned external-reader origins. Imported fixtures also cover ID dictionaries, adjacent and skip grams, whole-record literal candidates, relation adjacency and backlinks, and scalar order. The fixture declines capabilities beyond that schema and text policy. Native coverage tests exercise the broader field families. The complete alternate round trip uses file-range segments. Retained bodies, liveness, direct write versions and external-reader descriptors have separate bounded record-transfer coverage. The interchange is internal and the task introduces no persistent format.

The native target accepts independently represented file segments with declared, named and inferred dictionaries, raw and field grams, relations, and declared scalar and composite orders when the source descriptors provide the required coverage. The source-free dense path validates and remaps slots, retains record/file identities and provenance, translates short-string coverage, and refreshes file-path order keys after a path change. Identity transfer retains the source layout and coverage. Native maintenance keeps its compatible-layout path and can share unchanged payloads.

### Integration checkpoints

All implementation commits are on the integration branch. Use `git log --reverse --oneline cf7253d048c9d6fe5d2a7e9b47441b2207cce932..c760904257c05cb50ccc50959021a78408378e12` for the full checkpoint history.

| Checkpoint | Commits |
| --- | --- |
| Physical module split and initial catalog dispatch | `9257ec6cad`, `372b51b0b6`, `d4ace10dbc` |
| Worker dispatch, immutable admission and selected order placement | `83ad4c199e`, `a199520ee8`, `8ba8bfb166` |
| Scoped query providers and opaque sealed segments | `8a19d68fcd`, `9da1b09c74`, `ef8a9769e9` |
| Independent ingestion, directory ownership and typed retained blocks | `f96a695d71`, `8358a8f2f4`, `2f3d8e48ec` |
| Source-free alternate file transfer and base storage reads | `c488f2ae4f`, `9df0030801`, `78bdadd89f` |
| Source scan and compiler-enforced storage privacy | `be1ba8c55a`, `5007c9cdf3` |
| Candidate text, relation transfer, declared order and inference coverage | `4eb8148ab3`, `214f6e561c`, `f85db221ad`, `d46ea0ac65` |
| Checked slot delivery for verified cache hits | `c760904257` |

### Producer and consumer coverage

| Path | Interface and evidence |
| --- | --- |
| Initial discovery, framing, worker preparation and ordered admission | `FileDiscoveryBuilder`, `StorageFactory::prepare_file_records`, opaque prepared contributions and `SegmentBuilder`. Independent initial managed file fixtures build and query through the ordinary scheduler, compare worker/admission counts, and cover zero-record files, late failures, ordered visibility and retry. |
| Managed file ingestion and mutation | Selected file editor and segment/record directory deltas, installed together. The independent catalog and segment fixtures inject failures during sealing and lookup and check that a retry installs one result. Workspace integration tests cover shared files, exact relative paths, versions, original-byte verification, move/relink, retirement and reconciliation. |
| Programmatic records | Selected direct preparation and sealing with optional file registration and retained or versioned source handles. `independent_and_native_segments_share_the_real_query_and_ingestion_path` covers old and new routes, payload and evidence parity, source-reader versions and read-free compaction. |
| Predicate planning, text, relations and reference evaluation | Scoped `IndexAccessProvider`, checked slot sets and `RelationReader` over selected segments. `file_segment_round_trip_and_compaction_transfer_logical_blocks` checks cross-segment forward/backlink query equivalence and candidate-only folded grams with false positives. `partial_score_coverage_and_stream_hold_the_original_revision` checks residual evaluation and a stream spanning publication. |
| Sorting, ranking, delivery and caches | `OrderColumnReader`, batched keys, ordered slots, retained blocks and record locators. Native posting and independent order share a sealed segment in the component placement tests. Short cursor tests check early page termination and an error after an earlier batch. The alternate round trip validates a tied, ordered two-row page. The verified-cache test counts zero segment ID lookups during a warm request. |
| Inspection and workspace clients | Checked file rows, segment summaries, declared paths, typed facet cursors, relation pages and captured counters. CLI, TUI, desktop, loaders and examples compile against the neutral APIs. The source-scan example also checks the selected catalog. |
| Compaction, partitioning, replacement, consolidation and transfer | Selected maintenance interfaces validate slot/layout identity. Catalog and component transfer have explicit progress, item/byte bounds and checked cancellation. The alternate round trip compares a canonical file index signature and query results with the source directory physically detached while moving the data. Separate fixtures exercise frequent posting keys, retained body chunks, partial inferred string coverage, write versions, masks and bounded file rows. |

### Rust API migration

| Previous API or surface | Replacement |
| --- | --- |
| `Repository::files()`, public `FileInventory`, `CollectionBuilder`, `CollectionGeneration`, `FileRecordIndex` | `Repository::repository_root()`, `IndexBuilder`, `IndexView::file_catalog()`, checked catalog rows and file/record membership operations. |
| Native `CollectionState` generation, dictionaries, raw text, orders, masks, route tables and relation fields | `declared_posting_paths()`, checked `base_inferred_postings()`, base logical counters, `posting_values()`, relation and graph inspection, and search APIs. Inferred summaries own their paths and key bounds. |
| `Repository::relation_index()` and `backlink_relation()` | Checked relation and backlink inspection through the existing relation page and graph APIs. |
| Physical segment read views and borrowed component collections | Logical `segment_summaries()` and checked `segment_components()`. `NativeSegmentSummary` and component descriptor names remain compatibility names for logical diagnostics. |
| Infallible `FileCatalog::file_by_path()` and `files_without_creation_time()` | `Result`-returning methods, plus bounded checked enumeration where applicable. |
| `IndexView` file status, handles, path lookups, catalog row, versions, observation and signature | Checked results that distinguish absence from failure. Paths and diagnostics may borrow pinned storage or own decoded values through `Cow`. `catalogue_rows()`, `file_paths()` and `file_paths_for_collection()` yield checked entries. |
| `SourceScan::file_paths()` and `file_paths_for_collection()` | Iterators of `Result<Cow<Path>, String>`. The benchmark consumes every result. |
| Public physical bigram and sort containers | Memory owns the concrete types privately. Historical logical paths still export `BigramAlphabet`, `SegmentPolicy`, `SegmentId`, `ScalarKey`, `SelectionSummary`, sort configuration and comparable keys, and relation endpoint values. |

Production Rust module visibility prevents sibling code from importing the memory backend. The default factory construction in the storage module is the selection point. Compile-fail doctests check the former public memory module and concrete bigram and inventory imports, plus the old inferred dictionary field access through a collection. Generic query code only retains bitmaps and maps for request-local computation. The unit build gives backend-specific fixtures access to internal layouts.

### Validation

- `cargo fmt --all -- --check`
- `cargo check --locked --workspace --all-targets`
- `cargo test --locked --workspace`
- `cargo test --locked --workspace --features objekte-search-core/test-scan-instrumentation`
- `cargo build --locked --release -p objekte-cli --examples` from a clean archive of the implementation revision.
- `cargo test --locked --release -p objekte-search-core --lib transfer` from the same clean archive.
- `cargo test --locked --release -p objekte-search-core --lib verified_cache_reads_pinned_slots_without_repeating_id_lookups` from the same archive.

The default and scan-instrumented suites each passed with 697 core tests and 16 ignored, along with the workspace clients and five compile-fail doctests. All six focused release transfer tests and the verified-cache regression passed. A separate release run of the source-scan example returned 888 matches across 148,486 records in seven collections.

The existing detailed-profiling admission test reports zero collection entries where it expects two on both clean revisions. This limits the available per-collection timing breakdown. [Detailed build profiling issue](../issues/bug/detailed-build-profiling-collection-admission-totals.md)

### Release comparisons

The paired runs used Linux x86-64, Rust and Cargo 1.100.0, and a 32-thread Intel Core i9-13900K. Both release trees were clean archives at the listed revisions. Musenalm has 148,486 indexed records in seven collections. The build tests used ASCII and Unicode at one worker and at auto (32 workers). The order of the two revisions alternated between pairs. Query profile `ascii-off` disables the verified-result cache. `ascii-warm` warms a 64 MiB verified-result cache before the measured run. The query measurements used the same dataset and options in both revisions.

Run the examples from release builds of each revision with the same `ROOT` and separate output paths. Repeat the focused commands for three pairs.

```sh
cargo build --locked --release -p objekte-cli --examples
bench_musenalm --root "$ROOT" --profiles ascii-off --repetitions 1 --result-dir "$QUICK"
bench_musenalm --root "$ROOT" --cases Q020,Q032,Q035,Q070,Q141,Q145,Q149,Q150 \
  --profiles ascii-off,ascii-warm --repetitions 1 --result-dir "$FOCUSED"
bench_build_musenalm --root "$ROOT" --alphabet "$ALPHABET" --build-workers "$WORKERS"
bench_record_ingestion_gate probe "$INGESTION_FIXTURE"
bench_managed_file_scale "$FRESH_JSONL_ROOT" jsonl "$WORKERS"
OBJEKTE_BENCH_MUTATE_COPY=1 bench_lifecycle_mutations small "$FRESH_SMALL_ROOT"
OBJEKTE_BENCH_MUTATE_COPY=1 bench_lifecycle_mutations stress "$DISPOSABLE_MUSENALM_COPY"
bench_query_execution_addendum "$ROOT"
```

Both versions produced the same 150 Musenalm quick-query result digests and the same canonical build signatures for each alphabet and worker count. Parentheses contain the raw values for each run in paired order.

| Build metric | Base median (three runs) | Integration median (three runs) |
| --- | ---: | ---: |
| ascii, auto, build ms | 665.74 (664.19, 706.96, 665.74) | 631.46 (647.60, 631.46, 621.72) |
| ascii, auto, peak RSS MiB | 643.16 (643.88, 643.16, 629.82) | 612.89 (609.66, 612.89, 620.16) |
| ascii, 1, build ms | 3,427.75 (3,427.75, 3,411.30, 3,461.62) | 3,394.06 (3,410.37, 3,394.06, 3,393.73) |
| ascii, 1, peak RSS MiB | 502.00 (501.27, 502.27, 502.00) | 471.22 (471.20, 471.76, 471.22) |
| unicode, auto, build ms | 742.83 (719.41, 743.59, 742.83) | 708.29 (708.29, 690.93, 731.13) |
| unicode, auto, peak RSS MiB | 667.23 (667.23, 670.98, 664.79) | 627.73 (629.66, 625.75, 627.73) |
| unicode, 1, build ms | 4,236.59 (4,197.99, 4,236.59, 4,293.51) | 4,152.15 (4,143.74, 4,234.67, 4,152.15) |
| unicode, 1, peak RSS MiB | 509.74 (510.41, 509.61, 509.74) | 480.05 (480.05, 479.44, 485.20) |

All runs opened the source 148,486 times and read 193,297,230 source bytes.

| Read/write metric | Base median (three runs) | Integration median (three runs) |
| --- | ---: | ---: |
| 10,000 direct lookups, ms | 1.846 (2.837, 1.817, 1.846) | 1.567 (1.551, 1.580, 1.567) |
| 1,024 residual source verifications, ms | 0.437 (0.437, 0.458, 0.422) | 0.441 (0.441, 0.469, 0.441) |
| 8,020 file-path lookups, ms | 0.708 (0.724, 0.703, 0.708) | 0.545 (0.545, 0.552, 0.534) |
| Managed ingest at 1 worker(s), records/s | 418,742 (423,059, 418,742, 418,068) | 454,345 (435,981, 454,345, 454,634) |
| Managed ingest at 1 worker(s), peak RSS MiB | 55.33 (55.33, 55.34, 55.32) | 40.90 (40.96, 40.89, 40.90) |
| Managed ingest at 8 worker(s), records/s | 1,606,870 (1,576,238, 1,606,870, 1,763,016) | 2,383,745 (2,383,745, 2,405,993, 1,922,059) |
| Managed ingest at 8 worker(s), peak RSS MiB | 56.56 (56.56, 56.71, 56.56) | 42.94 (42.94, 42.96, 42.71) |

The direct probe has 16,384 records and measures seven repetitions with 5,000 hits and 5,000 misses per repetition. Its exact predicate returns 1,024 matches with zero source opens. The residual case reads and verifies 1,024 records. The independent file-path probe makes 4,010 successful and 4,010 missing exact relative-path lookups against the agents collection, repeated 15 times inside each run. Use the same calls to `FileCatalog::file_by_path()` and propagate checked errors on the integration revision. Every managed JSONL run indexes 100,000 records from one 30,000,000-byte file with one source open.

| Focused Musenalm query | Base median µs (three runs) | Integration median µs (three runs) |
| --- | ---: | ---: |
| Q020 cache off | 1,015.4 (1,015.4, 1,070.5, 975.5) | 993.6 (984.8, 993.6, 997.2) |
| Q020 warm | 24.8 (25.6, 24.8, 24.6) | 16.2 (15.7, 16.2, 16.8) |
| Q032 cache off | 36.8 (36.8, 36.9, 36.2) | 36.3 (37.7, 36.3, 34.8) |
| Q032 warm | 3.5 (3.8, 3.5, 3.5) | 3.2 (3.1, 3.2, 3.2) |
| Q035 cache off | 46.0 (46.0, 49.3, 45.3) | 48.7 (49.1, 47.5, 48.7) |
| Q035 warm | 10.8 (10.5, 11.4, 10.8) | 10.2 (10.2, 10.1, 10.7) |
| Q070 cache off | 115.0 (115.0, 124.5, 113.9) | 103.1 (109.5, 103.1, 102.3) |
| Q070 warm | 1.8 (1.8, 1.8, 1.8) | 1.8 (1.8, 1.7, 1.9) |
| Q141 cache off | 1,847.3 (1,847.3, 1,896.9, 1,787.2) | 1,923.5 (1,923.5, 1,927.0, 1,920.2) |
| Q141 warm | 33.5 (33.5, 33.5, 33.8) | 18.3 (17.5, 18.3, 18.5) |
| Q145 cache off | 35,759.2 (35,759.2, 34,752.1, 36,085.3) | 38,302.9 (36,711.0, 39,299.2, 38,302.9) |
| Q145 warm | 560.8 (560.8, 569.9, 527.5) | 40.3 (40.3, 38.5, 44.9) |
| Q149 cache off | 1,080.4 (1,338.3, 1,080.4, 1,057.3) | 999.4 (999.4, 1,196.9, 872.4) |
| Q149 warm | 22.9 (22.9, 22.3, 23.4) | 16.8 (17.5, 16.8, 16.1) |
| Q150 cache off | 1,499.3 (1,499.3, 1,494.3, 1,600.6) | 1,532.0 (1,532.0, 1,497.3, 1,587.1) |
| Q150 warm | 41.4 (41.4, 43.5, 39.8) | 27.8 (27.6, 27.8, 30.7) |

| Cache-off query | Candidates, base → integration | Source opens, base → integration |
| --- | ---: | ---: |
| Q020 | 285 → 285 | 285 → 285 |
| Q032 | 7 → 7 | 0 → 0 |
| Q035 | 124 → 124 | 0 → 0 |
| Q070 | 1 → 1 | 1 → 1 |
| Q141 | 982 → 982 | 982 → 982 |
| Q145 | 44373 → 44373 | 44373 → 44373 |
| Q149 | 126 → 126 | 126 → 126 |
| Q150 | 457 → 457 | 457 → 457 |

| Lifecycle operation | Base median ms (three runs) | Integration median ms (three runs) |
| --- | ---: | ---: |
| lifecycle / add | 0.272 (0.272, 0.268, 0.293) | 0.304 (0.311, 0.295, 0.304) |
| lifecycle / update_append | 0.193 (0.207, 0.193, 0.193) | 0.182 (0.179, 0.209, 0.182) |
| lifecycle / update_verified | 0.158 (0.219, 0.158, 0.141) | 0.167 (0.208, 0.167, 0.138) |
| lifecycle / move | 0.030 (0.029, 0.042, 0.030) | 0.031 (0.047, 0.031, 0.026) |
| lifecycle / delete_verified | 0.024 (0.023, 0.036, 0.024) | 0.024 (0.039, 0.024, 0.023) |
| lifecycle / reconcile_no_change | 0.113 (0.111, 0.130, 0.113) | 0.127 (0.140, 0.125, 0.127) |
| stress / update_verified_base | 146.037 (146.037, 148.169, 145.311) | 146.929 (146.929, 134.095, 147.005) |
| stress / update_verified_untouched_base | 261.526 (261.219, 275.015, 261.526) | 269.675 (276.094, 269.675, 266.670) |
| stress / batch_small_8 | 4.476 (3.554, 4.632, 4.476) | 6.058 (3.703, 6.071, 6.058) |
| stress / batch_large_128 | 81.332 (64.547, 94.524, 81.332) | 68.514 (68.514, 69.279, 61.929) |
| stress / repeat_update_32 | 504.453 (504.453, 685.434, 488.020) | 611.991 (606.261, 611.991, 643.537) |
| stress / reconcile_with_query | 1,982.418 (1,945.679, 1,982.418, 2,013.425) | 360.562 (370.535, 360.562, 360.050) |
| stress / compact | 186.902 (185.730, 186.902, 193.249) | 188.927 (188.927, 188.973, 186.783) |

- Stress compaction work for base. `retired_slots_removed=145;remaining=0;records_examined=16412;input_bytes_budgeted=28386610;temporary_bytes_estimated=93562774;slots_after=148486;probe_samples=88;probe_p50_ms=0.0438;probe_p95_ms=0.0941;probe_p99_ms=0.0963;probe_max_ms=0.0989;probe_errors=0` / `retired_slots_removed=145;remaining=0;records_examined=16412;input_bytes_budgeted=28386610;temporary_bytes_estimated=93562774;slots_after=148486;probe_samples=89;probe_p50_ms=0.0432;probe_p95_ms=0.0884;probe_p99_ms=0.0977;probe_max_ms=0.1096;probe_errors=0` / `retired_slots_removed=145;remaining=0;records_examined=16412;input_bytes_budgeted=28386610;temporary_bytes_estimated=93562774;slots_after=148486;probe_samples=93;probe_p50_ms=0.0317;probe_p95_ms=0.0650;probe_p99_ms=0.0719;probe_max_ms=0.0724;probe_errors=0`
- Stress compaction work for checkpoint. `retired_slots_removed=145;remaining=0;records_examined=16412;input_bytes_budgeted=28386610;temporary_bytes_estimated=93562774;slots_after=148486;probe_samples=89;probe_p50_ms=0.0620;probe_p95_ms=0.0999;probe_p99_ms=0.1147;probe_max_ms=0.1252;probe_errors=0` / `retired_slots_removed=145;remaining=0;records_examined=16412;input_bytes_budgeted=28386610;temporary_bytes_estimated=93562774;slots_after=148486;probe_samples=89;probe_p50_ms=0.0613;probe_p95_ms=0.0955;probe_p99_ms=0.1034;probe_max_ms=0.1094;probe_errors=0` / `retired_slots_removed=145;remaining=0;records_examined=16412;input_bytes_budgeted=28386610;temporary_bytes_estimated=93562774;slots_after=148486;probe_samples=88;probe_p50_ms=0.0621;probe_p95_ms=0.0971;probe_p99_ms=0.1076;probe_max_ms=0.1094;probe_errors=0`

| Query execution addendum | Base median µs (three runs) | Integration median µs (three runs) |
| --- | ---: | ---: |
| multi-field-relation-fts / median | 26.2 (26.2, 21.9, 56.2) | 23.8 (26.6, 22.0, 23.8) |
| stored-residual-count-off / first rows | 33.8 (40.9, 33.8, 32.5) | 50.0 (72.8, 33.7, 50.0) |
| stored-residual-count-on / first rows | 26.0 (27.2, 26.0, 26.0) | 26.4 (31.3, 26.4, 26.3) |
| q141-count-on / first rows | 13,222.5 (13,222.5, 13,140.5, 13,373.5) | 12,983.4 (13,115.1, 12,831.9, 12,983.4) |

The relation probe returns 3 hits and reads 4 target records in both revisions. The stored-order probes record latency to the first rows with and without a complete count. The stress fixture uses a fresh file copy of Musenalm for each process. Each run removes 145 retired slots while preserving 148,486 live records.

### Follow-up measurements

Across two comparison passes, the initial storage revision showed warm-cache overhead on several queries. The final checkpoint uses checked pinned slots for cached delivery and avoids whole-result bookkeeping when provider validation and observation capture are inactive. Warm Q145 improved from a median of 560.8 µs to 40.3 µs in the final comparison.

Cache-off Q145 was repeated in three additional alternating pairs with `bench_musenalm --root "$ROOT" --cases Q145 --profiles ascii-off --repetitions 7 --result-dir "$Q145_REPEAT"`. The median of the three within-run medians was 35.431 ms on the base and 35.979 ms on the integration revision. Both versions opened 44,373 candidate files.

| Q145 cache-off repeat | Pair 1 ms | Pair 2 ms | Pair 3 ms |
| --- | --- | --- | --- |
| base | 35.163 [35.755, 36.263, 34.949, 35.143, 35.163, 35.874, 34.759] | 35.679 [36.284, 35.056, 34.678, 35.67, 35.679, 35.913, 35.814] | 35.431 [36.159, 33.927, 36.081, 35.476, 35.431, 35.116, 35.182] |
| checkpoint | 35.979 [36.805, 36.574, 34.835, 35.979, 35.87, 36.587, 35.589] | 36.654 [37.268, 35.78, 35.347, 36.792, 38.776, 36.319, 36.654] | 35.726 [35.726, 35.561, 35.63, 35.052, 37.938, 36.672, 36.225] |

The stored-residual first-row benchmark was repeated for three additional pairs. Every count-off sample in both revisions opened and verified one record and emitted one row. Count-on samples and the full Musenalm ordered page had similar medians. Count-off first-row times varied across overlapping ranges, with a higher integration median.

- base stored-residual-count-off first rows in µs. Median 38.7. Samples 40.9, 33.8, 32.5, 74.4, 36.5, 43.9.
- base stored-residual-count-on first rows in µs. Median 26.6. Samples 27.2, 26.0, 26.0, 31.6, 26.6, 26.6.
- checkpoint stored-residual-count-off first rows in µs. Median 60.5. Samples 72.8, 33.7, 50.0, 40.2, 70.9, 72.0.
- checkpoint stored-residual-count-on first rows in µs. Median 27.4. Samples 31.3, 26.4, 26.3, 26.3, 28.5, 46.9.

The eight-file stress append sample had a higher integration median in both comparison passes. In the final comparison the absolute difference was 1.58 ms over eight updates. Both versions use the same append strategy, with staged catalog and directory edits confined to the touched ranges. The larger 128-file stress batch was quicker in the final comparison. The repeated single-file stress run varied between passes. The benchmarks establish the observed timing differences, and the available counters do not isolate their attribution.

Raw logs, manifests, and digests from the final comparisons are in `/tmp/objekte-storage-paired-c7609042` on the benchmark host. The earlier cache measurements are in `/tmp/objekte-storage-paired-d46ea0ac`.
