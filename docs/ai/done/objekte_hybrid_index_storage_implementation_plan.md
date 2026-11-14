# Objekte hybrid index storage implementation plan

Date: 2026-11-13  
Task identifier: `hybrid-index-storage`  
Status: implementation specification; no implementation performed by the author  
Observed master: `9c6c9e1af37c1f2439e85fbc678588db2c848c74`  
Required integration branch: `integration/hybrid-index-storage`  
Required integration worktree: `/home/simon/source/objekte-integration-hybrid-index-storage`

Completed allocation/query follow-up: [specification and implementation report](https://github.com/Simon-Martens/objekte/blob/integration/hybrid-index-storage/docs/ai/done/objekte_hybrid_index_allocation_and_query_performance_specification.md). The broader hybrid storage task remains open.

## 1. Objective and controlling decisions

Implement a common searchable index representation that can be owned entirely in memory or accessed from application-owned immutable index files. Preserve Roaring postings, current query semantics, incremental availability, updates, diagnostics, and the existing source-file backing store.

The user-facing storage choices, shared by CLI, TUI and desktop, are:

1. **Memory only — default:** build and use the index in memory; do not load, create or synchronize index files.
2. **System temporary folder:** reuse a valid index there, otherwise create it; synchronize subsequent index changes.
3. **Repository folder:** reuse or create indexes inside the data repository's `.objekte` directory; synchronize changes.
4. **Custom folder — advanced:** reuse or create indexes beneath a selected location; synchronize changes.

Every disk-synchronized choice additionally offers **Keep entire index in memory**. This controls residency, not persistence: the index remains synchronized to its selected location.

Loading deprecated snapshot formats is prohibited. Recognized deprecated index files encountered in the selected storage scope must be removed and a new-format index created when disk synchronization is requested. Do not retain a compatibility decoder or migrate old snapshot contents.

Keep all implementation, experiments, documentation and benchmarks on the task's integration/subtask branches. **Do not merge or push to master.** The user reviews the final integration branch after correctness, approximately equivalent resident-query performance and faster loading have been demonstrated.

The database-style representation changes are explicitly part of this task. Routine format and implementation decisions within the constraints below do not require another approval. Do not expand this into a general-purpose database, SQL engine, replacement source store, or unrelated query-language redesign.

## 2. Evidence and source baseline

Master was inspected at `9c6c9e1af37c1f2439e85fbc678588db2c848c74`, which merges the snapshot loading/admission profiling work. Before implementation, fetch and inspect current master, record its exact SHA, and review any intervening changes. Start from current master unless the user explicitly selects another base. Never reset the original checkout or overwrite user changes.

Relevant existing implementation:

| Area | Starting locations |
| --- | --- |
| Field dictionaries and Roaring postings | `crates/core/src/posting.rs` |
| Normal and skip-1 bigram postings | `crates/core/src/bigram.rs` |
| Sort columns | `crates/core/src/sorting.rs` |
| Capability selection and execution | `crates/core/src/index_catalog.rs`, `executor.rs`, `run.rs` |
| Incremental construction and segment admission | `repository_incremental.rs`, `mutation_segment.rs`, `build.rs` |
| Persistence and lifecycle codecs | `storage.rs`, `lifecycle_segment_codec.rs` |
| Updates, pinned views and configured repositories | `lifecycle.rs`, `lifecycle_repository.rs`, `lifecycle_caller.rs` |
| Shared consumer arguments and opening | `crates/cli/src/arguments.rs`, `loader.rs`, `execution.rs` |
| Desktop opening, refresh and completion | `crates/desktop/src/backend/open.rs`, `controller/runtime.rs`, `controller/filter_validation.rs` |
| Existing profiling evidence | `docs/ai/generated/snapshot_loading_admission_profiling_results.md` |

The reviewed representation uses `HashMap<u64, RoaringBitmap>` for each bigram family and `BTreeMap<ScalarKey, RoaringBitmap>` for ordered values. Temporary packed bigram construction blocks are not the completed query representation. The pinned Roaring dependency was 0.11.5.

The archived workload reported about 979 ms construction and 684 ms restore, with 277 ms field-posting admission, 151 ms bigram finalization and 84 ms final sorting. These have different timer boundaries and are not additive savings estimates. They are historical context, not a substitute for a matched baseline on the implementation machine.

Construction performance and storage performance are separate outcomes. Retaining the existing mutable builders and posting merges can retain the approximately 277 ms admission cost even after completed indexes use a different representation. Encoding blocks, descriptors and posting payloads can add sealing work. The primary architectural opportunities are opening, persistence and memory management; faster construction must be demonstrated independently. The query-parity, construction and loading thresholds below are acceptance targets, not established capabilities or promised improvements.

Check whether collection-wide inference copying/merging is still attributed to coordinator bookkeeping. If so, correct that narrowly and establish a common instrumented baseline before using inference component timings. Do not claim the inference-disabled measurements establish inference-enabled costs.

## 3. Repository workflow and supervision

Read `AGENTS.md`, `docs/ai/README.md` and applicable nested instructions before starting.

- Keep the original `/home/simon/source/objekte` checkout on master and preserve unrelated local, staged and untracked files.
- Work in sibling worktrees under `/home/simon/source`; never use `/tmp` for task worktrees.
- Use one supervising integrator. Delegate independent components where useful, with explicit ownership and shared interfaces agreed first.
- Subtask branches: `task/hybrid-index-storage/<component>`.
- Subtask worktrees: `/home/simon/source/objekte-task-hybrid-index-storage-<component>`.
- Only the integrator writes to the integration worktree.
- Push immediately after each implementation, integration and report commit. Do not rewrite published history or force-push.
- Use `docs/ai/memory/hybrid-index-storage/` for uncommitted task notes, following the repository workflow.
- Keep this specification and user amendments intact. Follow the repository's rules for initially untracked or modified task documents.
- A failed acceptance gate is not a completed task. Preserve the branch and report the concrete failure; do not relax thresholds silently.
- Even after every gate passes, leave master untouched.

Suggested component ownership after the common contracts are committed:

| Component | Responsibility | Dependency |
| --- | --- | --- |
| `segment-format` | Format, checked readers, ordered dictionaries, posting descriptors | Shared contracts |
| `posting-execution` | Roaring access, bigram lookup, range execution, sorting integration | Reader contract |
| `storage-lifecycle` | Locations, manifests, locking, updates, synchronization, deprecated-file cleanup | Format/identity contract |
| `consumer-integration` | Shared CLI policy, TUI and desktop controls, progress and errors | Storage policy contract |
| `validation-performance` | Differential correctness, workload execution, memory/load/query measurements | Baseline first; implementation as available |

The supervisor owns cross-component decisions and integration. Tests and benchmarks must exercise the combined result, not just individual branches.

## 4. Storage and residency policy

Use one core policy model. Consumers must not implement competing load/save decisions.

Conceptually:

- Storage location: `MemoryOnly`, `SystemTemporary`, `RepositoryDirectory`, `CustomDirectory(PathBuf)`.
- Residency: `OnDemand` or `FullyResident`.
- Resolved storage: canonical repository identity, configuration identity, actual managed directory, and supported format.
- Observed state: construction/coverage, source reconciliation, residency preparation, current generation, durable generation, synchronization status and last error.

### 4.1 Required behavior matrix

| Storage choice | Existing index | Missing/stale configuration | Index changes | Residency |
| --- | --- | --- | --- | --- |
| Memory only | Ignore index stores | Build from sources | Remain in memory | Fully resident |
| System temporary | Open matching current format | Build and store | Automatically synchronize | On demand by default; optional full |
| Repository folder | Open matching current format | Build and store | Automatically synchronize | On demand by default; optional full |
| Custom folder | Open matching current format | Build and store | Automatically synchronize | On demand by default; optional full |

A valid store with changed source files uses normal incremental reconciliation; do not rebuild everything merely because a source revision changed. An incompatible schema, extractor or index configuration can require a new generation or configuration namespace.

Memory-only is the default for plain folders **and configured repositories**, in all three clients. Existing configured-repository convenience behavior must not silently open an old cache. Reading repository configuration, including `.objekte/collections`, remains allowed and necessary. Memory-only does not mean ignoring schemas or collection definitions.

Do not probe, create or clean index caches solely because a memory-only session was opened. Explicitly saved per-repository application preferences may select another mode; a fresh/unconfigured choice defaults to memory. No automatic mode change because a cache happens to exist.

### 4.2 Locations and identity

Use application-owned namespaces beneath each selected base directory:

- Temporary: the platform's actual system temporary directory, in a private per-user Objekte directory, then a repository/configuration namespace.
- Repository: `<data-root>/.objekte/indexes/<configuration-id>/`.
- Custom: `<selected-base>/objekte-indexes/<repository-id>/<configuration-id>/`.

These are proposed public path conventions; settle them in the first contract commit and use them consistently. The custom selector chooses a base folder, not an arbitrary existing file to overwrite. Display the resolved managed path in diagnostics and advanced settings.

Repository identity must distinguish different data roots and normalized source selections. Configuration identity must cover schemas, collection selection, source/format/extractor identities and versions, field/index declarations, inference policy, alphabet, normalization/case-folding, collation and other semantic options. Do not key the store only by a repository basename or Git remote. Store enough checked identity metadata to detect collisions/mismatches.

Do not include source modification times in the configuration identity: ordinary edits belong to reconciliation, not a new full cache namespace.

Use platform temp-directory APIs. Do not use a random new temporary folder every launch; repeated opens must find the same store. Temporary indexes may disappear after reboot or system cleanup; absence triggers rebuild. A temp directory may reside on tmpfs: report its actual filesystem in benchmarks and do not present tmpfs results as physical-disk results.

Reject or safely resolve unsafe store layouts before writes. Use private ownership/permissions for temporary storage and do not follow unexpected symlinks during cleanup. Exclude every managed index location from source discovery, including a custom folder nested inside the data root. Preserve ordinary source selection semantics.

### 4.3 Fully resident means owned index data

The full-residency option must ensure all index data needed for subsequent queries is available in application-owned memory before reporting `FullyResidentReady`.

- Read required segment sections into owned immutable buffers or an equally complete resident representation.
- Use the same logical format/readers as on-demand access.
- Do not implement the promise solely through mmap prefetch, page touching or operating-system cache advice.
- Normal OS swapping is outside this promise; do not require privileged memory locking or claim physical pages can never be reclaimed.
- Source contents are not automatically retained. FTS verification and result payload requests may still read source files.
- Future committed updates must also become resident before joining a generation advertised as fully resident.
- Keep disk synchronization active.
- Do not permanently retain a complete mapping plus a duplicate owned copy plus a complete decoded object graph.
- If allocation fails, return an explicit error or keep the existing usable session with a visible failure. Do not silently change the requested residency mode.

A full-residency startup may expose earlier provisional progress, but its readiness timer includes all required reads, validation and conversion. Query benchmarks must start after that milestone.

## 5. Searchable immutable segment format

### 5.1 Required sections

Use one versioned format for owned-memory and file-backed segments. A small manifest identifies the active generation and its immutable segment files.

A segment contains independently addressable sections:

1. Header and section directory.
2. Field paths, field capabilities, coverage and statistics.
3. Text bindings, alphabet and normalization identities.
4. Ordered scalar and array-length dictionaries.
5. Structural predicate references.
6. Normal and skip-1 bigram dictionaries.
7. Individually addressable Roaring posting payloads.
8. Record slots, source locators and necessary record classes.
9. Sort data and required relation/identifier lookup data.
10. Integrity information.

Define fixed endianness, explicit integer encodings, checked offsets/lengths and format bounds. Persist offsets/identifiers, never native pointers or Rust object layouts. Use 64-bit file offsets; justify narrower identifiers against segment limits. Specify empty sections, oversized keys and segment rollover.

Metadata should be compact and addressable. Preserve the previous persistence repairs: no triplication of file metadata, no persisted redundant hash maps, and no repository-sized empty-base offset tables. Store distinct historical file revisions only when they are genuinely needed. Reconstruct only small indispensable accelerators; do not quietly rebuild every full lookup table at startup.

Do not leave an unprofiled, large eagerly deserialized remainder that defeats the opening-time objective. Phase the work by family, but include all material remaining load costs in the final assessment.

### 5.2 Shared reader and ownership

Introduce a small shared abstraction over owned segment bytes and mapped segment bytes, with checked section access and lifetime ownership. Use a supported mapping dependency where required. Existing workspace dependencies are preferred; small established dependencies for mapping, locking or platform directory handling are within this implementation scope. Record their necessity. Adding a general database engine or changing the Roaring implementation is outside this task's default design.

Query capability selection, exactness and fallback rules remain common. Avoid parallel resident and disk query engines with diverging semantics. Choose representation-specific readers once at useful boundaries; avoid adding dynamic dispatch, locks or map lookups for every record or bitmap word.

Construction can retain mutable builders and current BTreeMaps initially. Sealing must produce the searchable representation and release redundant builder state. Memory-only construction must not write a temporary index file as an implementation shortcut.

Keep source construction single-pass: read, parse/validate and extract each input through the established traversal, producing typed postings and bigrams together. Encoding or sealing must not reopen/reparse sources. Map only application-owned immutable index files; continue using the existing safe readers for editable source files.

Do not serialize every segment into a second retained collection of buffers before writing an outer object. Stream output and release construction intermediates at bounded segment boundaries.

### 5.3 Roaring postings

Retain standard portable 32-bit Roaring payloads and segment-local record slots as the initial format.

A posting descriptor contains its payload offset, byte length and cardinality, with any additional flags justified and measured. Pack small postings; never allocate one filesystem page per small bitmap. Reuse a descriptor when an existing posting proves exactly the same structural fact.

Do not wrap the entire posting region in a compression stream that must be decompressed in full. Roaring already provides container compression. Additional compression must be independently addressable and is not required for the first implementation.

Provide:

- Cardinality and presence lookup without decoding the payload.
- Checked materialization when an operation needs an owned bitmap.
- Efficient intersection with a resident candidate bitmap.
- Union/range iteration and difference/complement operations with bounded temporary ownership.
- Optional bounded decoded-posting cache where measurements justify it.
- A stable empty-posting representation that is distinct from unavailable coverage.

Roaring 0.11.5 exposes `intersection_with_serialized_unchecked`. It is a candidate optimization, not permission to accept unchecked file contents. Establish the required validation invariant before using unchecked operations. Do not assume all operations are available as zero-copy views.

Prefer library-supported operations and safe readers. A bespoke bitmap implementation is not required. Measure allocation, bytes touched and warm-query cost before adopting specialized container readers.

### 5.4 Ordered field dictionaries

Replace the completed segment's allocated `BTreeMap<ScalarKey, RoaringBitmap>` with sorted leaf blocks containing encoded typed keys and posting descriptors. Use a sparse directory of first keys and block offsets; allow additional directory levels for large dictionaries.

This is a bulk-loaded ordered block structure. Preserve:

- Equality and all supported ordered comparisons.
- Efficient lower/upper bound seeks and sequential range traversal.
- Exact `Number` semantics, including arbitrary-precision values where supported.
- Type distinctions, IDs, booleans, dates, datetimes and Unix timestamps.
- Existing scalar string comparison semantics.
- Multivalued path behavior, coverage and completeness guarantees.
- Planner estimates without unioning/materializing the postings.

Do not use textual numeric ordering. Raw byte ordering is valid only for encodings explicitly proven order-preserving; otherwise compare decoded keys using the established comparator.

Use array-length dictionaries with the same access model. Structural facts use a small tagged reference table. Preserve `:exists`, `:?`, `:string`, `:number`, `:array`, `:object`, `:null`, null/missing distinctions, empty selections, mixed occurrences, and zero/false semantics. Restrict index coverage to admitted/indexed fields as today.

Start with a measured target leaf size, such as 16 KiB, rather than treating that number as a requirement. Define handling for keys larger than the target size. Small dictionaries should use a compact single-block representation.

#### Lookup within leaf blocks

The sparse directory selects a leaf block; the format must also support efficient equality and lower/upper-bound lookup inside that block. Do not require decoding every preceding variable-length key for each lookup in a large block.

In the Phase 0 format contract, specify entry offsets, restart points, or an equivalent mechanism that locates entries without an unbounded sequential decode from the block start. Define the search method, maximum local scan or restart interval, and treatment of oversized keys. Direct offsets with binary search and bounded scans between restart points are possible designs; choose and measure the appropriate one. A compact linear scan is acceptable for explicitly bounded small dictionaries. Sequential traversal remains appropriate after a range start has been located.

Preserve established typed comparison semantics. For encodings that are not order-preserving, decode only the comparison data required at each search step and avoid allocating owned key objects repeatedly where borrowing or reusable scratch storage suffices. Account for the offset/restart metadata in stored and resident size.

Add focused equality and bound-seek benchmarks for small and large dictionaries, exact numbers, long strings and oversized keys. Include misses and first/middle/last-block positions. Record key comparisons, entries/bytes decoded and allocation counts alongside latency. Establish these measurements in Phase 1 and include their results in the final report.

### 5.5 Bigram dictionaries

Keep ordinary and skip-1 families distinct for each text binding.

- ASCII has the currently supported 95 x 95 pair domain. Consider direct lookup through compact descriptor ordinals only when occupancy justifies it.
- Small/sparse ASCII bindings use a compact sparse representation.
- Unicode uses sorted observed 64-bit keys with a block directory. The current implementation encodes Unicode scalar pairs; do not replace it with a lossy 256 x 256 space.
- Preserve normalization/version identity, separate text bindings, logical/fallback domains, segment boundaries and current unsupported-character behavior.
- Store per-posting cardinality so planning and rarest-first intersection do not decode every posting.
- A missing gram means an empty candidate set only under proven complete coverage of that binding.
- Preserve candidate verification against the appropriate source representation.

Do not expand fixed ASCII lookup tables across every tiny field index without measuring the accumulated overhead.

### 5.6 Sorting and other lookups

Keep dedicated sort columns. Persist collation keys and slot permutations in addressable sections; do not regenerate all collation keys at open.

Range-filter string ordering and locale-aware result sorting are separate contracts. Preserve ascending/descending rules, null/missing placement, multivalued selection and deterministic ties.

For multiple segments, use existing semantics or ordered per-segment streams merged with the same comparator and tie-breaker. Apply liveness and the query candidate restriction before returning records. Do not duplicate old/new record versions or re-sort all records merely to provide the first page.

Account for relation lookups, logical IDs, record locators and file metadata. Preserve inspection and navigation APIs. If a small accelerator is rebuilt, measure its time and retained size explicitly.

## 6. Validation, opening and synchronization

### 6.1 Opening and integrity

Opening validates the manifest, repository/configuration identity, format, directory bounds and necessary capability metadata before exposing the generation.

Validate independently accessed payloads before they can establish a positive or negative query result. Use checked arithmetic, bounds, key ordering, bitmap invariants, record domains and checksums at appropriate granularity. Cache successful validation only against immutable segment identity and the exact bytes covered.

A checksum alone is not proof of semantic validity. Do not trust a serialized `all logical` claim to bypass domain checks without checked supporting record/domain evidence. Preserve mixed logical/fallback behavior.

On-demand opening must not touch every payload solely to compute a whole-file checksum. Design region-level integrity checks and any optional full verification separately. Full residency must include its required validation in its reported preparation time. Corrupt/unreadable data is an error/rebuild condition, never an empty posting.

Make partial construction coverage, complete snapshot coverage, source freshness and full residency separately observable. A complete stored generation may answer complete queries without every page being resident. Source reconciliation status must not be disguised as complete current-source freshness.

Do not add per-query filesystem freshness scans or repeat source validation to compensate for storage changes. Use the existing explicit reconciliation/generation model; validation of stored index bytes is a separate responsibility.

#### Validation boundaries and reusable evidence

The Phase 0 design document must define the following invariant-to-validation mapping before implementing lazy readers. State the exact bytes and metadata involved, the validating operation, the lifetime of successful validation and failure behavior.

| Invariant | Required validation boundary |
| --- | --- |
| Manifest identity, format, section bounds and generation completeness | Check before exposing the generation or relying on its capabilities. |
| Directory ordering, routing boundaries, binding identity and declared coverage | Establish the metadata needed for the selected route before using it to choose or exclude blocks, or to conclude that a key/gram is absent. Define how boundary summaries are bound to the corresponding block contents. |
| Leaf-block framing, entry offsets/restart points, typed key ordering and descriptor bounds | Check before trusting a block's search result; reuse successful checks for the same immutable bytes. Specify how ordering across adjacent blocks is established. |
| Posting encoding, record-slot bounds and advertised cardinality | Check before the payload or its descriptor can determine a result or an exact count. Planner-only estimates must remain explicitly estimates. |
| Logical/fallback record domains | Establish supporting record/class/domain evidence at the appropriate segment boundary and share it across relevant postings. A stored flag or cardinality alone is insufficient. Check restricted-domain membership where required. |
| Successful validation | Associate evidence with immutable segment identity, section/block/payload identity and the exact byte range checked. Reuse it across queries and pinned views without treating unvalidated bytes as checked. |

Negative answers need explicit treatment. A missing-key result, empty-posting result or excluded block can affect correctness without decoding a posting. Validating only payloads that happen to be accessed does not establish the correctness of the routing or coverage metadata that caused other payloads to be skipped.

For every relevant invariant, distinguish evidence established during construction from checks performed at open or first access. Document the trust/integrity mechanism by which construction-established evidence remains applicable to stored bytes; checksums do not independently establish semantic validity. If the chosen design requires an eager traversal to prove an invariant, record that traversal and include its cost in opening/full-residency measurements. Do not describe it as eliminated or hide it in the first query.

Publish successful validation evidence only after all corresponding checks pass. Queries sharing immutable data must not repeat whole-block, whole-posting or whole-domain validation once its evidence remains valid. Define what happens to validation evidence when a decoded cache entry is evicted and when a replacement segment is published. No per-query source validation or filesystem freshness scan is introduced.

Extend format tests to cover malformed routing boundaries, inconsistent coverage/cardinality, unordered keys within/across blocks, and corrupt logical/fallback evidence. Exercise absent-key and empty-result queries as well as successful lookups; corruption must not silently become a negative result.

### 6.2 Durable generation publication

Use immutable segment files and an atomic manifest publication protocol:

1. Prepare new segments and generation metadata.
2. Finish writes and required file flushes.
3. Publish the new manifest atomically with the platform-appropriate durability steps.
4. Advance the durable-generation marker only after successful publication.
5. Reclaim obsolete application-owned files only when no active reader/open operation requires them.

Use one writer per store with cross-process locking. Coordinate manifest reads, opening segment handles and reclamation so a reader cannot observe a manifest whose files have just disappeared. Support pinned query generations and platform-specific deletion behavior. Do not truncate or overwrite an actively mapped segment.

A crash must leave either the previous complete durable generation or the new one, never a partly committed mixture. Incomplete orphan files can be removed safely under the store lock. A general transaction log for source files is not required: the index remains reconstructible.

### 6.3 Source updates and disk synchronization

Retain the existing add/move/delete/update API and source reconciliation rules.

- Adds create new indexed records.
- Deletes retire old slots through generation-level liveness.
- Content changes retire the prior record version and introduce the replacement atomically.
- Moves update file-location metadata where content/index facts remain reusable.
- Multi-record files, intrinsic IDs, fallback records and historical source locators remain correct.
- Query/result caches invalidate by the appropriate generation/revision dependencies.

Publish in-memory query generations promptly. In disk-synchronized modes, automatically persist completed changes in bounded, coalesced batches. Track current versus durable generation. Avoid a full snapshot rewrite after every record edit.

Expose an explicit flush/synchronize operation at the shared library boundary. Finite CLI commands in disk modes must finish required construction/reconciliation and flush the resulting committed generation before reporting successful completion; do not abandon the save because the requested result page was already printed. TUI/GUI synchronize during operation and flush on orderly close. Cancellation and persistence failure must remain observable; partial initial construction must not be saved as a complete index.

A valid previous generation can remain readable while a replacement is built. For an initial build, preserving progressive query availability does not require persisting every tiny publication. Persist a complete first generation unless partial checkpoints have an explicit, tested coverage contract.

### 6.4 Compaction and resource control

Provide or adapt bounded compaction so repeated small updates do not cause unbounded segment fan-out.

Define measurable triggers based on segment count, size and retired-record ratio. Compact incrementally within a bounded work/memory budget. Do not force a full compaction on every startup, every edit, or every CLI invocation.

Test foreground queries during compaction, pinned generations, cancellation and disk publication failures. Record write amplification, temporary disk usage, peak memory and query-tail impact.

Keep source reads/parses out of compaction when index data suffices. This must not become a second source-indexing pass.

## 7. Deprecated formats and safe cleanup

The reviewed old storage implementation uses magic `OBJSNAP1`, with snapshot versions 27 and 28. Audit the current branch for every supported historical filename/version and loader entry point before removal.

Required policy:

- New production readers accept only the new format.
- Remove or make unreachable old payload decoders, compatibility fallbacks and format upgrades.
- Header recognition solely for rejecting/deleting known deprecated files is allowed.
- Remove old write paths from production consumers too.
- Old files are never loaded to recover schema, configuration, records or postings.
- Rebuild from source data and current configuration.

Before deleting a candidate, establish that it is an application-owned deprecated index in the selected managed storage scope, using known filenames/location rules and format/header evidence. A suffix alone is insufficient. Do not recursively delete `.objekte`.

**Preserve `.objekte/collections`, schemas, configuration, saved queries, source files, unrelated files, other repositories' stores and active-reader generations.** Inspect both formerly supported alphabet snapshot names and any explicit old target paths where relevant.

Acquire the relevant writer lock before cleanup/replacement. Do not follow an untrusted symlink to another location. Unknown files and newer unsupported formats are not proven deprecated; leave them intact and report a specific error. A copied store for another root must not be overwritten silently.

In memory-only mode, do not scan stores just to find obsolete files. If no disk index was requested, no replacement index is created. The requested automatic deletion applies when a selected store is inspected and recognized deprecated files are found.

A cleanup failure must be reported and must not fall back to loading the obsolete format. Do not ask for confirmation for the narrowly defined deletion the user has authorized. Document exactly what was removed and rebuilt.

## 8. CLI, TUI and desktop behavior

### 8.1 Shared options and CLI

Use one resolver for configured repositories and ordinary directory opens. Proposed canonical flags:

```bash
obj --path /data/repository
obj --path /data/repository --index-storage temporary
obj --path /data/repository --index-storage repository
obj --path /data/repository --index-storage custom --index-directory /data/indexes
obj --path /data/repository --index-storage repository --index-residency full
obj --path /data/repository --index-storage temporary --index-residency on-demand --tui
```

- `--index-storage memory|temporary|repository|custom`; default `memory`.
- `--index-directory PATH` requires custom mode.
- `--index-residency on-demand|full`; disk modes default to on-demand.
- Memory mode always uses full residency. An explicit on-demand request with memory mode is invalid; an explicit full request is harmless.
- Validate contradictions before creating/deleting files.
- Replace the old independent `--load-index`/`--write-index` mode model. Give a clear usage error and replacement guidance for removed flags; do not silently reinterpret an old filename as a custom directory.
- Preserve schema, inference, alphabet, recursive selection, query and output options.
- Machine-readable output stays on stdout; progress, storage diagnostics and rebuild notices go to the established diagnostic/stderr channel.
- Do not call a disk-synchronized command successful if required persistence failed.

Library callers receive equivalent typed policy options; they should not need CLI argument structures.

### 8.2 TUI and desktop

Expose the same four storage choices through conventional existing menus/open dialogs:

- A standard storage-choice dropdown or radio group.
- A custom folder chooser under the advanced/custom choice.
- A checkbox labeled **Keep entire index in memory** for disk modes.
- Readable effective-location and synchronization status.
- Existing defaults and recent-repository preferences handled consistently.

The TUI accepts the CLI flags and provides corresponding settings in its existing interaction model. Do not invent an unrelated navigation pattern. Desktop direct-launch arguments, open-folder, configured-repository, recent-item and reopen flows must all use the shared policy.

Changing storage/residency for an open repository should perform a controlled reopen/preparation. Keep the old view usable until the replacement is ready where feasible; flush required pending writes before abandoning a disk-synchronized session. Do not silently change the setting after an error.

Show the user-relevant states: opening/building, indexing progress, preparing full residency, reconciling changes, synchronizing, synchronized, and failed synchronization. Avoid exposing low-level codec terminology in normal UI controls.

Preserve early catalogue/file display, provisional batch results, bounded viewport payload materialization, final query refresh, inferred-field completion refresh, pagination, and export completeness rules.

Native desktop smoke tests must exercise the new controls and actual reopen behavior. Core tests alone do not establish GUI integration.

## 9. Correctness and failure tests

Use existing test infrastructure and differential evaluation. Add meaningful tests for the new storage contracts, not tests that merely repeat implementation details.

Required coverage:

1. **Queries:** equality/ranges across every supported key type; exact numbers; Unicode and case behavior; structural predicates; missing/null/empty selections; mixed and multivalued paths; array selectors and source fallback; ASCII/Unicode ordinary and skip-1 grams; multiple text bindings; FTS/regex verification; Boolean combinations; relations; sorting, stable ties and pagination.
2. **Mode equivalence:** memory-only, on-demand stored, fully resident stored and reopen yield identical counts, record identities, ordered results and applicable source-access counters.
3. **Lifecycle:** add/move/delete/update, multiple records per file, invalid-to-valid and valid-to-fallback transitions, record-ID changes, schema/inference changes, concurrent pinned queries, publication and cache invalidation.
4. **Persistence:** interrupted segment writes, interrupted manifest replacement, disk full/permissions, locking conflicts, concurrent readers/writers, orphan cleanup, cancellation, flush failure and retry.
5. **Format safety:** truncation, offsets/length overflow, malformed dictionaries, corrupt checksums, unordered bitmap values, wrong record domains, duplicate identities and unsupported newer versions.
6. **Cleanup scope:** deprecated files are rejected/deleted/rebuilt in selected disk modes; configuration, `.objekte/collections`, unrelated files, symlink targets and other repositories are preserved; memory-only performs no index writes/cleanup.
7. **Locations:** repeated temp reuse, missing temp store, same basename at different roots, changed configuration, relative/custom paths, custom store inside source root, read-only destinations.
8. **Residency:** after full readiness, a representative query suite performs zero index-file reads; source payload/verification reads are counted separately. Future updates and compaction preserve the residency promise.
9. **Consumer behavior:** defaults, invalid option combinations, all opening paths, TUI settings, desktop native controls, progress/error presentation and bounded result materialization.
10. **Compaction:** results and ordering remain identical, retired records vanish, active readers remain valid, segment count and resource use are bounded.

Run the complete current query campaign. Preserve its original cases and add targeted cases; five selected digests are not enough for this representation change. Do not weaken assertions or change expected results to accommodate a regression.

## 10. Performance design and acceptance gates

### 10.1 Measurement matrix

Freeze a baseline before changing representation. Compare exact release commits with matching data, configuration, worker count and machine settings. Rotate paired execution order. Record environment, filesystem, binary revisions, fixture hashes and commands.

Required configurations:

- Baseline master memory build/query and snapshot restore/query.
- New memory-only build/query.
- New disk-synchronized on-demand opening/query.
- New disk-synchronized fully resident opening/query.
- Warm on-demand access after the workload's index working set is resident.
- Constrained-memory on-demand access with a dataset/working set larger than the available index cache.
- Update, flush and compaction workloads.
- Inference disabled and enabled; ASCII and Unicode.
- Ordinary and configured repositories; small metadata-rich fixtures and the main benchmark corpus.

Measure separately:

- Discovery/catalogue availability.
- First published result and complete construction.
- Store opening and first complete query against the stored generation.
- Full-residency readiness.
- Source reconciliation start/completion.
- Query planning, dictionary lookup, bitmap access/operations and source evaluation where existing hooks permit.
- Complete save/flush duration and durable-generation lag.
- Update publication, compaction and foreground-query latency.
- Bytes read/written, major/minor faults, descriptors opened and posting/container decoding.
- Allocations and retained/peak memory for construction, opening, full residency, saving and compaction.

Disable verified query-result caching for primary index-performance comparisons. Warm source and index pages separately where possible. Run a secondary campaign with the normal result cache enabled, but do not use it to hide a slower index reader.

Distinguish warm filesystem-cache startup from cold-index startup and from cold-source verification. Do not clear host-wide caches on a shared machine without authorization. Use an isolated test process/container or documented pressure method; disclose limitations. An unexplained OS cache state is not a cold result.

Do not equate fast manifest opening with fast useful startup. Include opening plus the first complete query and, for full mode, every preload/validation cost.

Report construction and storage effects independently. Preserve the existing component admission measurements and distinguish field-posting merging, record admission, bigram finalization, final sorting, new dictionary/posting encoding, file writing/flush and reopen work. Nested component timers must not be added twice.

Any encoding required to make the memory-only representation queryable belongs to its construction/sealing interval. Do not move that work outside the measured completion boundary or onto the first query to meet the construction threshold. Report additional build/encoding work even when it pays for a faster later reopen.

Compare memory-only construction against baseline memory construction, and stored opening against baseline restore under matched cache conditions. Do not credit a loading improvement as a construction improvement. If a bulk-construction change becomes necessary, isolate its revision and measurements from the storage-representation change, then measure the combined result; follow the existing scope constraints for such changes.

### 10.2 Working acceptance thresholds

These operationalize the user's requirement for approximate resident-query parity and faster loading. Tighten them when measurements support it; do not loosen them silently.

| Area | Acceptance criterion |
| --- | --- |
| Correctness | Exact result and ordering equivalence; no unexplained loss of index coverage or extra source fallback |
| Memory-only resident queries | Geometric mean latency ratio <= 1.05 versus baseline; investigate every reproducible individual regression exceeding both 10% and 0.2 ms |
| Fully resident stored queries | Same resident-query gate against baseline and against new memory-only |
| Warm on-demand queries | Same aggregate 5% target once required index pages are resident; report cache/decode overhead separately |
| Query tails | Representative per-query p95 <= 1.10 of baseline beyond timer noise; do not hide regressions in an across-query average |
| Opening stored generation | Target >= 20% lower matched median than baseline restore, with improvement in opening plus first complete query; no omitted validation or reconciliation work disguised as elimination |
| Fully resident readiness | Target >= 10% lower matched median than baseline restore; include all population and required validation |
| Memory-only construction | Median <= 1.05 of baseline and no material readiness regression |
| Retained and peak memory | No >10% increase in equivalent full-resident states or build/save peaks without explicit user review; identify mapped versus anonymous memory and allocator retention |
| Stored bytes | No >10% unexplained increase for equivalent indexed content; account for metadata, liveness and temporary compaction files |
| Updates/compaction | No unbounded segment growth, per-edit full snapshot rewrite, or material foreground-query regression |

Use at least seven paired opening/build/flush trials and enough query repetitions to measure medians and tails reliably (at least 30 measured repetitions for representative tail cases). For very short queries, use a documented batch procedure that prevents the query-result cache from satisfying the work. Retain all samples and report variability.

Fully resident and warmed on-demand guarantees must be measured independently. Constrained-memory cold access is expected to pay I/O costs; report that tradeoff instead of applying the warm parity threshold indiscriminately.

If full residency is faster only because required work moved to the first query, the opening gate has failed. If parity requires optional decoded-posting caching, include its entire memory cost and warm-up work.

Evaluate each gate independently. Faster loading does not compensate for a failed construction or resident-query gate. Conversely, unchanged construction time is an acceptable outcome when it satisfies its existing threshold and the storage objectives pass. Report absolute milliseconds and ratios, including any new sealing/encoding overhead; do not assume that replacing the completed B-tree representation removes the measured builder/admission bottleneck.

Do not mark complete when a gate fails. Optimize within scope, then report the measured tradeoff and leave the integration branch for review if it cannot be resolved.

## 11. Implementation phases and commit boundaries

### Phase 0 — Baseline and contract

- Inspect current master/instructions and existing mode defaults.
- Establish the exact benchmark base and full query campaign.
- Record existing deprecated filenames/formats and safe deletion scope.
- Commit the storage/residency types, state definitions and a concise format/identity design document.
- Establish validation and source-freshness boundaries before implementing lazy access.
- Specify the within-block lookup mechanism and the invariant-to-validation mapping required by Sections 5.4 and 6.1.
- Correct remaining inference timer attribution if needed for this campaign.

Suggested commit: `Define hybrid index storage contracts and baseline measurements`.

### Phase 1 — Common segment reader and ordered postings

- Implement owned/mapped ownership and checked sections.
- Implement portable posting payloads/descriptors and sorted scalar/length blocks.
- Integrate with the existing capability/execution interfaces.
- Preserve mutable builders initially; finish memory-only execution through the common reader.
- Differentially test comparisons, structural predicates, coverage and sorting interactions.
- Run a focused resident-query benchmark before expanding the codec.
- Include within-block equality/bound-seek latency, decoding and allocations for exact-number and long-string dictionaries.

Suggested commits: `Add checked immutable segment readers`; `Store ordered postings in searchable blocks`.

### Phase 2 — Bigrams, sorting and remaining material lookup state

- Implement numeric gram dictionaries and common Roaring payload access.
- Preserve ordinary/skip and binding/domain separation.
- Make sort and required locator/relation data addressable.
- Remove duplicated retained construction representations.
- Measure representative full scans, selective queries, broad ranges and sorted pagination.

Suggested commits: `Store bigram postings in addressable segment sections`; `Integrate sort and record lookup data with segment readers`.

### Phase 3 — Locations, manifests and full residency

- Implement the three disk destinations and repository/configuration namespacing.
- Implement locking, atomic publication, checked opening and ownership-safe cleanup.
- Implement owned full-residency preparation and accurate readiness reporting.
- Add corrupt-store/failure tests and index-read counters.
- Validate no full-segment encode/decode buffer accumulation.

Suggested commits: `Add managed index locations and atomic generation storage`; `Add on-demand and fully resident opening policies`.

### Phase 4 — Updates, synchronization and compaction

- Connect existing lifecycle mutations and reconciliation to stored generations.
- Add coalesced automatic synchronization and explicit flush.
- Preserve reader pinning, cache invalidation and late inference publication.
- Implement/adapt bounded compaction and safe reclamation.
- Test concurrent queries, crashes and disk failures.

Suggested commit: `Synchronize incremental index generations and compact segments`.

### Phase 5 — Consumers and deprecated-format removal

- Replace competing CLI/GUI cache decisions with the shared policy.
- Implement the four choices and full-residency option in CLI, TUI and desktop.
- Remove old-format loading/writing and obsolete load/save flags.
- Add narrowly scoped recognized-file deletion and rebuild behavior.
- Update help, user documentation and examples.
- Run native desktop and TUI interaction smoke tests.

Suggested commits: `Expose consistent index storage policies in all clients`; `Remove deprecated snapshot loading and rebuild obsolete stores`.

### Phase 6 — Full validation and performance repair

- Run differential and full workspace tests, consumer builds, Clippy and appropriate formatting checks.
- Execute the complete performance matrix and profile any failed thresholds.
- Optimize measured representation/access costs only. Bulk-construction algorithm changes are allowed only if needed and evaluated independently; do not mix unrelated speculative optimizations into the comparison.
- Repeat affected checks after changes. Archive exact accepted commit results.
- Report construction/admission/encoding effects separately from opening, persistence and residency effects; apply each acceptance gate independently.
- Leave all work on the integration branch.

Suggested commits: `Validate hybrid storage correctness and failure handling`; `Report hybrid index storage performance and acceptance results`.

## 12. Deliverables and completion report

Commit and push:

1. Implementation, shared APIs, tests and consumer integration.
2. A versioned format/readiness/synchronization design document under `docs/development/`.
3. Updated CLI/TUI/desktop user documentation.
4. `docs/ai/generated/hybrid_index_storage_results.md`.
5. Raw trials and reproduction instructions under `bench/results/hybrid-index-storage/`, following existing repository conventions. Large fixtures/binaries may remain ignored, with hashes and exact regeneration commands.
6. The completed task specification and appended implementation report in `docs/ai/done/`, following repository document-state rules.

The result report must include a mode-by-mode behavior table, baseline/final SHAs, implementation/subtask commits, correctness totals, complete query campaign coverage, latency/size/memory comparisons, full synchronization and compaction costs, deleted-format policy, native UI evidence and every unmet criterion.

State explicitly:

- Which queries access serialized postings directly and which materialize/cache them.
- What is resident at each readiness milestone.
- Whether any large lookup structure is still rebuilt at startup.
- Which checks are eager and which occur on access.
- How within-block searches locate keys, their measured decode/allocation costs, and how negative results are covered by validation evidence.
- How source freshness differs from stored-index completeness.
- Whether deprecated loaders are truly absent from production paths.
- Whether performance gates pass without the result cache.
- Whether construction improved, remained equivalent or regressed, separately from loading gains, with admission and new sealing/encoding costs identified.
- That master has not been modified or merged.

Keep the integration branch/worktree for the user. Remove subordinate worktrees and temporary notes only under the existing workflow after successful integration, preservation and push.

## 13. Reference material

Repository baseline:
- https://github.com/Simon-Martens/objekte/tree/9c6c9e1af37c1f2439e85fbc678588db2c848c74
- https://github.com/Simon-Martens/objekte/blob/9c6c9e1af37c1f2439e85fbc678588db2c848c74/docs/ai/README.md
- https://github.com/Simon-Martens/objekte/blob/9c6c9e1af37c1f2439e85fbc678588db2c848c74/docs/ai/generated/snapshot_loading_admission_profiling_results.md

Format and implementation references:
- https://github.com/RoaringBitmap/RoaringFormatSpec
- https://docs.rs/roaring/0.11.5/roaring/bitmap/struct.RoaringBitmap.html
- https://lucene.apache.org/core/9_8_0/core/org/apache/lucene/index/package-summary.html
- https://rocksdb.org/blog/2017/05/12/partitioned-index-filter.html

These references inform the design. They do not require adoption of Lucene, RocksDB, a native-layout frozen bitmap format, or a new database dependency.


## Implementation report

- **Task identifier and closure date:** `hybrid-index-storage`, 2026-11-14.
- **Baseline and integration:** Original reviewed master `9c6c9e1af37c1f2439e85fbc678588db2c848c74`; actual implementation baseline `da8fde68472b8646a63a05b3b3654c58de877301`. Integration branch `integration/hybrid-index-storage`, worktree `/home/simon/source/objekte-integration-hybrid-index-storage`. The original checkout remains on `master`.
- **Implemented:** Shared memory-only and managed storage policies, CLI/TUI/desktop selection, immutable checked segment readers with ordered field and bigram dictionaries and Roaring postings, generation publication, source updates, reconciliation, compaction, status reporting, and deprecated-loader removal. Later repair, generation, and query follow-ups improved constructed-reader ownership, validation, maintenance, and exact query routing while preserving result semantics. Representative implementation commits include `bfca5ba9d2`, `083fc1e6b0`, `e6475f6c5`, `679c83b7d6`, `947d66e10b`, `5342a54a08`, and the measured implementation checkpoint `389375b06d`; repairs and follow-ups continue through `e15b3f5c4b`, `ca183a1ed8`, `8ca2851ab8`, and `293a10498c`.
- **Subtasks and branches:** CLI, desktop, storage, codec, and query subtask branches were integrated into this single integration branch. The final query branch was `task/hybrid-index-storage/query-scanner-plans`; its worktree was removed after its merge and push. The integration worktree is retained for review.
- **Validation:** The combined integration revision `74cab22411` passed `cargo test --workspace --locked --quiet` and `cargo clippy --workspace --all-targets --locked --quiet` (Clippy warnings). CLI and desktop tests are included in the workspace run. Rustfmt and the repository spacing check report existing broad formatting drift; no repository-wide reformat was included. Correctness, corruption, mutation, pinned-reader, and store-generation tests are documented in the linked reports.
- **Benchmarks:** The established `bench_musenalm --root examples/musenalm --extended --incremental --profiles ascii-off,unicode-off --repetitions 5 --verified-cache-bytes 0 --prepared-cache-bytes 16777216 --query-workers 32` campaign and focused paired build/open/flush scripts were run in the reports below. The final query campaign preserved all 440 case/profile digests and reduced memory-mode median geomeans to 0.789 ASCII and 0.747 Unicode versus its `8ca2851ab8` baseline. Earlier original-baseline construction and opening ratios, exact commands, hashes, fixtures, raw samples, and differing durability semantics are in [the repair checkpoint](../../../bench/results/hybrid-index-storage/repair-20261113/final-e15b/REPORT.md), [the allocation follow-up](../generated/hybrid-index-allocation-query-performance-20261114.md), [the generation checkpoint](../../../bench/results/hybrid-index-storage/maintenance-20261114/REPORT.md), and [the final query campaign](../../../bench/results/hybrid-index-storage/query-optimization-20261115/REPORT.md).
- **User-approved deviation and remaining work:** On 2026-11-14 the user instructed us to ignore further performance evidence, move these completed work records into `done/`, and transfer outstanding implementation/validation work to issues. Numeric performance targets and missing performance measurements are therefore not claimed as passed. Remaining work is tracked in [bounded base compaction](../issues/feat/hybrid-index-storage-base-compaction-budget.md), [store failure validation](../issues/feat/hybrid-store-failure-validation-coverage.md), and [fresh disk sealing](../issues/perf/hybrid-index-storage-fresh-disk-sealing-path.md). The separate, unimplemented `objekte_direct_snapshot_access_specification.md` stays in `tasks/`; this report does not close it.
