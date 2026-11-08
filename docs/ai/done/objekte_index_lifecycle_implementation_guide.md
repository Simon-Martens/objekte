# Objekte index lifecycle implementation guide

Date: 2026-11-08  
Task identifier: `index-lifecycle`  
Proposed task filename: `docs/ai/tasks/objekte_index_lifecycle_implementation_guide.md`  
Integration branch: `integration/index-lifecycle`

## 1. Assignment and intended result

Implement runtime index maintenance for the current Objekte library in `crates/core/`, with the necessary migrations in its CLI, TUI, and desktop callers. The library must support adding, moving, deleting, and updating indexed files; continue serving searches during background reconciliation and maintenance; and expose explicit schema and index-cache persistence operations.

The normal update strategy is to mark previous record slots inactive and append replacement records. Additionally, support physical removal of index contributions and slot-preserving updates when caller-supplied old content is verified against the content originally indexed. An invalid old-content hint must select the normal strategy without damaging the index.

The library indexes files. It does not edit, rename, create, or delete source files. Application code owns source-file manipulation. Writing an index cache or repository configuration is separate from writing source data.

This document is an implementation assignment for the agent when the user instructs it to execute the task. Preparing this guide does not itself implement the changes.

Deliver one tested, pushed integration branch for the user to merge. Do not merge into `master`.

### Completion priorities

Implement the basic mutation and publication mechanism before the optional-data optimization. This sequencing is intended to obtain a useful, measurable implementation early; it does not remove later phases from the assignment.

All phases in this guide are required for full completion, including verified in-place maintenance, compaction, explicit persistence, repository convenience opening, and caller migration. If work stops at an intermediate milestone, report it as partial.

## 2. Repository baseline and mandatory inspection

This guide was grounded in `master` commit:

`6dc2ef194bc81bb8e4897b7652635bc7c7cb1cbb`

Use the user-designated implementation base, or the established current main branch if none is designated. Record the actual base commit. Do not reset a newer checkout to this reference commit.

Read the current root `AGENTS.md`, `docs/ai/README.md`, and instructions applying to modified directories. Inspect current code before assigning ownership: the implementation may have advanced since this guide was written.

| Area | Observed baseline | Required development |
| --- | --- | --- |
| `inventory.rs` | Separate `FileId`, `RecordSlot`, per-file record mapping, and immutable collection generations | Active/deleted state, revision tokens, efficient incremental ownership, and content fingerprints |
| `repository.rs` | A repository publishes immutable collections; ordinary open may use a default snapshot directory and synchronously check freshness | Explicit constructors and a concurrent publication owner |
| `storage.rs` | Explicit snapshot saving, checked restoration, configuration compatibility, and synchronous `sources_match` | Persist mutation state; restore a usable index before filesystem reconciliation |
| `bigram.rs` | Appendable normal and skip-bigram postings using Roaring bitmaps | Consistent active filtering and verified removal/replacement |
| `posting.rs`, `structural.rs`, `inference*` | Typed and structural postings and inferred indexing | Correct mutation of every index family and coverage declaration |
| `sorting.rs` | Stored keys and whole-collection ordering arrays | Incremental updates without reconstructing whole orders per record |
| `verified.rs`, `prepared.rs` | Caches scoped by repository ownership and immutable state | Explicit publication-revision scoping for mutation |
| `source.rs`, `source_access.rs` | Revision-checked source ranges and bounded query reads | Typed unavailable-source handling during reconciliation |
| `relation*`, `materialization.rs`, `streaming.rs` | Relations, result materialization, and streaming | One snapshot per execution, active filtering, and honest completeness reporting |

The current `docs/future-whole-file-replacement.md` explicitly defers runtime updates. This assignment supersedes that deferral. Update that document or replace its deferred description with the implemented contract. Do not retain contradictory documentation.

The baseline already depends on `sha2`. Prefer its existing SHA-256 implementation for content fingerprints; a new hashing dependency is not required.

## 3. Decisions fixed by this specification

1. Filesystem contents remain the authoritative source data.
2. Mutations change only the index and its persistence state.
3. A physical file is the default mutation unit. It may own zero, one, or many records in one or more collections.
4. A file mutation publishes all affected collection memberships together.
5. Deletion normally marks records inactive. Replacement normally appends new slots.
6. Queries must use a single published index view throughout execution.
7. A normal cache load automatically schedules background metadata reconciliation and returns without waiting for that scan.
8. The ordinary library API has no implicit schema paths, cache paths, default-cache discovery, or automatic persistence.
9. `open_repository` is the explicit convenience entry point allowed to discover schemas and caches inside its specified repository.
10. Repository convenience mode may enable automatic cache flushing through an explicit `sync_cache` option. Its default is disabled.
11. Repeated automatic reconciliation is opt-in. The initial scan associated with cache loading is automatic.
12. Reconciliation compares modification time and size, without routinely opening unchanged file contents.
13. Metadata comparison is a fast change detector, not a proof of content equality.
14. Old-content verification uses a stored digest of the original raw bytes; index membership is insufficient.
15. Compaction, rebuilding, reconciliation, and flushing are separate operations.
16. Publication must not copy or rebuild the whole repository for an ordinary single-file mutation.
17. Use existing source readers, extractors, schema handling, validation, and raw-fallback behavior. Do not introduce a second extraction implementation.

## 4. Git workflow, agent ownership, and delivery

Use the workflow in `docs/ai/README.md`. The following task-specific instructions make the intended organization concrete.

### 4.1 Worktrees

For an original checkout at `/home/simon/source/objekte`:

| Role | Branch | Worktree |
| --- | --- | --- |
| Supervisor/integrator | `integration/index-lifecycle` | `/home/simon/source/objekte-integration-index-lifecycle` |
| Publication and inventory | `task/index-lifecycle/publication` | `/home/simon/source/objekte-task-index-lifecycle-publication` |
| Index mutations | `task/index-lifecycle/mutation` | `/home/simon/source/objekte-task-index-lifecycle-mutation` |
| Background reconciliation | `task/index-lifecycle/reconciliation` | `/home/simon/source/objekte-task-index-lifecycle-reconciliation` |
| Persistence and callers | `task/index-lifecycle/persistence` | `/home/simon/source/objekte-task-index-lifecycle-persistence` |
| Independent validation | `task/index-lifecycle/validation` | `/home/simon/source/objekte-task-index-lifecycle-validation` |

All worktrees are siblings of the original checkout under `/home/simon/source`; never put them in `/tmp`. If the actual original path differs, preserve the user's worktree-location rule and record the resolved paths. Reuse an existing task worktree only after confirming it belongs to this task; otherwise choose a descriptive unused suffix.

Keep the original checkout on `master`. Preserve its staged and unstaged changes. Perform implementation, integration, validation, and commits in the task worktrees.

### 4.2 Delegation and dependency order

Use a supervising agent and subagents for genuinely separable work. Do not delegate five agents to edit `repository.rs` simultaneously.

The supervisor first fixes the public contracts, publication representation, error model, and ownership boundaries. Merge that foundation before dependent agents begin modifying production mutation paths.

| Agent | Ownership | Dependency and handoff |
| --- | --- | --- |
| Publication | Snapshot owner, revisions, active records, inventory interfaces | Establish interfaces first; supervisor owns shared integration changes |
| Mutation | Complete file replacement, moves, index-family maintenance, verified in-place strategy | Starts from merged publication contracts |
| Reconciliation | Inventory scan, scheduling, progress, retry, cancellation | Uses mutation API; must not create an independent writer |
| Persistence | Explicit load/save, repository convenience mode, schema resolution, caller migrations | May develop API migration in parallel after contracts are fixed |
| Validation | Independent reference tests, concurrency tests, benchmark fixtures and reports | Can prepare baseline measurements early; tests final merged behavior |

Compaction and rebuild coordination belong to the supervisor with the publication and mutation owners. Assign one owner for each shared file. Agents submit interface changes to the supervisor rather than independently redesigning them.

Create subordinate branches from a recorded integration commit. Push immediately after every commit, including merges and final reports. The supervisor reviews and merges all subordinate work into the single integration branch.

Do not force-push, reset user work, or push to `master`.

### 4.3 Task document lifecycle

Preserve the source task document and user edits according to the repository workflow. An uncommitted task specification stays uncommitted while work is in progress.

At completion, reconcile its latest contents, move it to `docs/ai/done/` in the integration worktree, append the implementation report, and commit and push that completed document even if the original was untracked. Do not modify the original checkout's Git index.

Use `docs/ai/memory/index-lifecycle/` in the original checkout for ignored shared notes. Remove only this task's obsolete notes after integration and final reporting. Keep the integration worktree for the user.

## 5. Data model and invariants

### 5.1 Distinguish identities and versions

Retain the distinction between:

- `FileId`: repository-local identity of a physical source entry.
- `RecordId`: logical record identity within a collection.
- `RecordSlot`: internal storage address for one record incarnation.
- `SourceRevision`: observed modification time and size.
- `FileVersion`: internal mutation token changed when content, path, or membership changes.
- `IndexRevision`: monotonic publication sequence of the complete index view.

Names may be adapted to existing types, but do not conflate these concepts.

Modification time is not the concurrency token. Distinct accepted mutations can have the same modification time and size. Protect against an old handle referring to a newly reused identity or slot.

Public record locators must be generation-qualified. A stale locator must either resolve through a stable logical identity under an explicitly current-read API or return a typed stale-locator result. It must never silently address a different record after compaction.

### 5.2 File and record state

Maintain an authoritative active-record set for each collection, using Roaring bitmaps or an equivalently compact representation consistent with the chosen storage design.

Store at least the following logical information:

| State | Required contents |
| --- | --- |
| File entry | Identity, path, source revision, file version, memberships, source status, content fingerprint when available |
| Record entry | Slot, logical ID, owning file, source range and revision, record class, diagnostics |
| Current lookup | Active path-to-file and logical-ID-to-record mappings |
| File ownership | All active slots belonging to a file in each collection |
| Published view | Index revision, file inventory, collections, configuration identity, and cache scope |

Keep deletion state separate from `FileStatus::Broken` or `Unreadable`. A malformed file can remain searchable through existing raw fallback. A deleted record is excluded from every route.

Do not reuse retired slots while any retained view or handle could confuse them with a replacement. Dense slot reuse is allowed only through a new compaction generation with explicit locator invalidation.

### 5.3 Required invariants

- At most one active record exists for each collection-local logical ID.
- Active file paths have unambiguous ownership.
- Every active record maps to an active file membership and the matching source revision.
- A published view never combines new file metadata with old record ranges.
- Records from one file mutation cannot become visible independently across affected collections.
- Inactive slots never participate in current lookup, eligible source reads, exact counts, final pagination, or relation results.
- Typed postings, text indexes, coverage metadata, and sorting correspond to the same published revision.
- An extension cannot claim complete index coverage over newly added records it did not index.
- A committed no-op does not needlessly invalidate query caches or mark persistence dirty.
- A failed transaction publishes no partial index state.
- Cache flushing serializes one coherent published revision.

Physical obsolete postings may remain after logical deletion. Validation must distinguish allocated slot capacity from live-record cardinality; existing dense-inventory assumptions must be updated accordingly.

## 6. Publication and concurrency architecture

### 6.1 Recommended control structure

Introduce a shared repository/index handle owning a published immutable view and a mutation coordinator.

A minimal publication mechanism can use `RwLock<Arc<PublishedIndex>>`: queries acquire the lock only long enough to clone the current `Arc`, then execute without holding that lock. A commit replaces the root pointer after preparing and validating its state. An existing equivalent mechanism is acceptable.

Do not hold a repository read lock throughout a query and then make mutations wait for all query work. Do not hold the publication lock during filesystem I/O, extraction, compression, sorting, serialization, or destruction of a large retired view.

The mutation coordinator serializes commit order. File preparation may run in parallel with bounded workers and memory admission.

### 6.2 Avoid complete copies on mutation

Wrapping the entire repository in an `Arc` is insufficient if each update invokes `Arc::make_mut` and clones all indexes.

Choose and document a representation that shares unaffected state and limits changed work to touched files, affected index structures, and bounded publication overhead. Practical candidates are shared chunks/partitions or an immutable base with bounded additions and replacement overlays.

Before committing to the representation, demonstrate a single-file update on a representative collection and measure allocation/copying and latency. Do not build a new general database framework. Use the smallest representation that meets the contracts and performance requirements.

If an incremental overlay is chosen, specify how queries combine it, how ordering is merged, how its memory and partition count are bounded, and how maintenance folds it into a new base. Unbounded one-record segments are unacceptable.

The exact representation is an implementation decision. The invariant against collection-sized work for every small mutation is mandatory.

### 6.3 Mutation transaction

Every mutation follows this sequence:

1. Resolve its target and capture the current file version, configuration identity, and relevant view.
2. Prepare extraction and index changes privately. Read source content only as required for this file.
3. Submit prepared work to the coordinator.
4. Revalidate the target version, path ownership, configuration, and logical-ID constraints against the current view.
5. Rebase the prepared changes onto unrelated commits where safe; otherwise retry or return a typed conflict.
6. Publish all related changes once and advance `IndexRevision`.
7. Notify observers and mark the published revision dirty for persistence, if applicable.

Never publish a prepared replacement of an old root over a newer root: it would discard unrelated commits.

A delete followed by an add at the same path must not let delayed preparation for the original file overwrite the new file. Version/identity checks must prevent this ABA case.

### 6.4 Query snapshot semantics

Each search, count, relation operation, and stream pins one view. Parallel workers inherit it. No worker independently reloads the latest root halfway through execution.

Searches beginning after a deletion commit cannot return the deleted record. Searches already using an older view may finish against it. This is the default consistency contract; do not add per-result global synchronization to make deletion retroactive.

An index snapshot does not preserve the corresponding source files. Old-view source reads must still obey source-revision validation and unavailable-source behavior.

Separate search calls used for pagination normally see the current view. If the existing API offers pinned sessions or cursors, bind those to a view and its lifetime. Do not silently promise stable pagination across mutations without such a mechanism.

### 6.5 Maintenance publication

Rebuild and compaction capture a starting revision. Mutations committed after that capture must survive maintenance publication.

Use a bounded ordered change record, rebasing of affected partitions, or another explicitly demonstrated scheme. If the required retained changes exceed a configured budget, cancel/restart maintenance while retaining the current searchable view. Do not silently lose updates, keep an unbounded replay history, or pause mutation commits for an entire rebuild.

Maintenance cancellation and preparation failure leave the current view usable.

## 7. Public API contracts

The following is a conceptual surface, not a requirement to copy these Rust signatures verbatim:

```rust
Index::build(configuration, build_options)
Index::load_cache(cache_path, configuration, runtime_options)
Repository::open_repository(repository_path, repository_options)

index.add(path, add_options)
index.move_file(file_handle, destination_path, move_options)
index.delete(file_handle, delete_options)
index.update(file_handle, update_input, update_options)

index.reconcile(reconcile_options)
index.start_auto_reconcile(schedule)
index.stop_auto_reconcile()

index.compact(compaction_options)
index.rebuild(rebuild_options)
index.flush_cache(cache_path)
index.status()
index.shutdown(shutdown_options)

Schema::load(schema_path)
```

Existing naming conventions may justify `remove_file` or `load_snapshot` instead. Use one documented terminology consistently, and make it explicit that delete/move affect index state only.

### 7.1 Inputs and results

An update accepts the current source path or a complete supplied replacement byte stream/buffer with its declared source revision. Old bytes are a separate optional hint.

Conceptual options:

```rust
struct UpdateOptions<'a> {
    expected_version: Option<FileVersion>,
    old_content: Option<&'a [u8]>,
    strategy: UpdateStrategy,
}

enum UpdateStrategy {
    AppendReplacement,
    PreferInPlace,
}
```

Deletion also accepts optional old content and an expected version. A direct in-place attempt may be exposed for callers that need an explicit capability result, but the normal API must provide the safe fallback.

Without supplied old content, use the normal strategy. With supplied old content, the convenience default may prefer verified in-place maintenance; retain an explicit option to force appended replacement for comparison or caller policy. Document the chosen default consistently across update and delete.

Report the actual outcome, including new revision, old/new record counts, affected collections, selected strategy, and a structured fallback reason when relevant. Examples of fallback reasons are missing fingerprint, old-content mismatch, unsupported reversible extraction, or incompatible record layout.

Do not report a fallback as an error when the requested logical mutation succeeded.

Return typed errors for revision conflict, unknown identity, destination collision, duplicate active record ID, incompatible configuration, resource limits, I/O failure, corruption, and cancellation. Reuse the repository's established error conventions where possible.

### 7.2 Supplied new bytes and source ownership

Supplying bytes does not authorize the library to write them to the source path. They must represent the source revision the caller wants indexed.

Prefer the path-based update for ordinary callers: the library reads the actual current file. The supplied-content variant avoids a redundant read when the caller already possesses those bytes; document the caller's obligation to associate them with the correct file revision.

Do not retain every supplied source payload permanently to make that variant appear consistent. A later materialization still reads the source file and may report a revision mismatch. This API is not a virtual in-memory source store.

### 7.3 Index-only deletion and reconciliation

Removing a file from the index while leaving it on disk is not a durable exclusion rule. A later filesystem reconciliation may add it again.

Document the expected application sequence: the external application changes its source files, then informs the index. Persistent exclusions belong in discovery configuration. Do not add an implicit exclusion list to make index deletion permanent.

An explicit deletion of an already inactive known identity may return an idempotent no-op. Unknown or stale handles must not resolve to unrelated current files.

### 7.4 Background task handles

Long-running operations return observable task handles or equivalent status subscriptions with progress, cancellation, completion, errors, and a wait operation.

Background cache reconciliation must be observable even though the load operation returns before it completes. Repeated scheduling should coalesce work for the same file instead of growing an unbounded queue.

Expose shutdown semantics: stop scheduling, cancel or finish pending maintenance according to options, and join workers. No detached worker may keep an unwanted repository alive indefinitely. An explicit shutdown flush is allowed; ordinary core-handle destruction must not introduce implicit persistence.

## 8. Normal mutation behavior

### 8.1 Add

Resolve eligible collections using the explicit configuration. Extract the file once per required reader/configuration binding, and share physical reads where the existing architecture permits it.

Create its inventory entry, frame all records, apply schema validation and existing fallback behavior, construct every required index contribution, verify identities, and publish.

An already active destination is not silently overwritten. Report an existing entry or use an explicitly documented update operation. Duplicate requests with a provably identical target version may be no-ops.

A valid file with zero records is not a deleted file.

### 8.2 Delete

Use the file-to-record mapping to deactivate every owned record in every collection. This operation does not require the old source bytes or opening the source file.

Remove active path and logical-ID lookup entries, update active membership/statistics, and ensure relations and cached searches cannot expose the removed records.

Old postings remain allocated in the normal path. Do not eagerly scan every posting list to remove this file.

### 8.3 Update through appended replacement

Prepare the complete replacement before committing the transition. Atomically retire the old slots and publish the new slots, index contributions, metadata, and ownership.

Support one-to-many, many-to-one, many-to-zero, and zero-to-many record transitions. Reframe all byte offsets for a changed multi-record file.

Preserve intrinsic logical IDs where supplied by the reader. Follow the established identity policy for filename-, path-, revision-, or offset-derived IDs; do not invent identity preservation that the source format cannot support.

Distinguish a fatal preparation failure from an accepted broken-file outcome. A fatal failure publishes nothing. Existing raw-fallback outcomes may publish broken-but-searchable records, consistently with fresh indexing.

If the source file has already changed when preparation fails, retaining the previous index view does not make its source bytes current. Report failure and let source-read freshness handling prevent unsafe materialization.

### 8.4 Move

The caller has already performed any physical rename. A move changes index routing and metadata.

Use the inexpensive path-only route only when content and extraction semantics remain compatible. Update the active path map and all derived path/filename metadata, sort keys, identities, diagnostics, and relation effects that actually depend on the path.

A move across collection boundaries must recompute membership and use the new collection's source/schema rules. A filename extension can change the selected reader. If indexed data depends on the path, invoke the needed reindexing.

Reject destination collisions; do not silently merge or replace another indexed file. A move combined with a content change must use a replacement, not reuse old offsets and postings.

The initial background scanner need not recognize renames. Delete-and-add is correct when a reliable move notification is unavailable. Do not infer a move from equal modification time and size alone.

## 9. Verified in-place maintenance

This is a required second implementation stage, following a correct normal mutation path.

### 9.1 Fingerprint contract

Store a SHA-256 digest of the exact raw file bytes used by the indexing pass. The digest is a practical strong equality check, not mathematical proof against collisions. Do not substitute a noncryptographic short hash, modification time, parsed-field subset, or bigram membership test.

Associate the digest with the indexed file version and indexing configuration identity. Configuration identity must cover the relevant source registration/version, schema, text normalization/extraction, identity rules, and other settings affecting contributions.

Compute the digest while bytes are already being read for indexing. Avoid an additional file read, an additional full-file allocation, or hashing the same physical file separately for every collection. For streaming and parallel readers, design a bounded method to obtain the complete ordered byte digest. If the current reader cannot provide that without disproportionate work, record fingerprint unavailability and use the normal update path for that format until the support is implemented.

A whole-file digest is sufficient for the required whole-file API. Do not add per-record digests throughout the repository unless needed for a separately justified record-level optimization.

Older caches lacking fingerprints remain searchable if otherwise supported, but cannot enable the verified strategy. Never hash the current filesystem file and assign that digest retrospectively to potentially different cached content.

### 9.2 Atomic validation and fallback

Given current indexed version V, supplied old bytes O, and prepared replacement N:

1. Hash O and reconstruct its old contributions using the recorded compatible configuration, old path metadata, and old record framing.
2. Prepare N through the normal extraction pipeline.
3. At commit, verify that the target is still the intended file incarnation and V is current.
4. Compare the digest of O with V's recorded digest and verify removal capability/layout.
5. If eligible, atomically publish the state with old contributions removed and new contributions installed.
6. If the old bytes or removal capability are unsuitable but the target version is still valid, use the normal tombstone-and-append strategy.
7. If the target version changed concurrently, return/retry a revision conflict; do not execute fallback against an unintended newer target.

All validation occurs before modifying published state. Stage the complete removal and addition privately; never partially clear postings and then discover an incompatible custom index.

For a delete, verified physical removal has no appended replacement. If verification fails, commit only the normal tombstone deletion, subject to the same target-version checks.

### 9.3 Slot preservation and physical reclamation

For a compatible one-record file, preserve its slot and replace all relevant contributions. For multi-record files, slot preservation requires a reliable mapping between old and new records and refreshed ranges. Falling back when record membership/layout changes is acceptable and must be observable.

Do not claim that clearing postings immediately releases every byte of memory. Container capacity and retired immutable views may remain until compaction or reader release. Report removal strategy separately from memory reclaimed.

Readers pinned to the previous view retain their previous logical index state. Slot-preserving updates must therefore use unpublished changes/shared structure replacement, not mutable writes into an old reader's postings.

### 9.4 Reversible extraction and extensions

Old content can reconstruct old contributions only when the relevant reader/extractor behavior is deterministic under the recorded configuration.

Built-in index families must support verified removal. Custom extensions must either use existing generic removable contributions or declare a narrowly defined removal/reconstruction capability. Unversioned external state or nondeterministic extraction must disable this optimization for the affected operation.

An unsupported extension must not force unsafe partial removal. Use the normal strategy for the affected file transaction. Do not require every third-party extension to implement a new reverse API just to retain ordinary indexing support.

## 10. Correctness across all query and index routes

Audit the execution system; do not implement tombstone filtering only in the final result loop.

| Component | Mutation requirements |
| --- | --- |
| Raw, field-specific, custom, normal, skip, ASCII and Unicode bigrams | Add new contributions; exclude inactive candidates before scanning; remove old contributions only through verified removal/compaction |
| Typed scalar postings | Update numeric, boolean, string/dictionary, date/time, ID, and all supported scalar postings |
| Structural postings | Maintain presence, absence domains, null, array length, empty-array and unavailable-value semantics |
| Coverage | Distinguish active domain, logical-record domain, raw fallback, and complete/incomplete coverage |
| Sorting | Maintain scalar/composite order, null/missing placement, tie-breaking and identity order |
| Logical identity lookup | Resolve only current active records; detect duplicates before publication |
| Relations/backlinks | Filter inactive sources and targets; update outgoing/reverse contributions and resolve replacement targets consistently |
| Inference | Admit new values safely and invalidate assumptions that additions or type changes make unsound |
| Diagnostics | Preserve bounded diagnostic behavior and correct file/record ownership |
| Statistics | Keep exact live counts exact; separate allocated capacity and approximate planner statistics |
| Reference execution | Apply the same visibility and freshness contract independently of optimized candidate generation |
| Direct record lookup | Reject inactive/stale identities or locators before source access |
| Streaming | Pin one view; apply active filtering and report terminal completeness consistently |

A tombstoned source's relation must not count as a live backlink. Target replacement, disappearance, or ID change must be reflected in relation evaluation, including cached target sets.

An unavailable relation target is not automatically a proven nonmatch. Preserve unknown/unavailable semantics so that negation cannot turn an unverified target into a positive match. The same principle applies to unavailable sort keys and residual predicates.

Inference may retain conservative historical type/cardinality information after deletion when doing so only affects optimization. It must not retain an assumption that causes false negatives for new values. Widen or disable unsafe inferred restrictions until a valid replacement is available. Rebuilding can recover narrower statistics.

### 10.1 Sorting and source ownership costs

Do not insert into or reconstruct complete sorted arrays for every individual record. If existing orders are immutable vectors, use batched delta orders with a bounded merge or another measured incremental representation.

The current compact file-to-record offset table also needs attention: changing one file's record count must not require shifting the entire ownership table after every update. Preserve fast file ownership lookup using bounded changes, then consolidate during maintenance.

Maintain source-read batching. A new mutation layer must not force one open or one allocation per candidate record.

### 10.2 Cache versioning

Initially prefer coarse, correct invalidation by published index revision over a complex dependency invalidation framework.

Scope prepared plans, verified membership/order/evidence, inferred sort orders, and relation-target caches to the relevant published configuration and data revision. Old readers may retain old caches; new readers cannot consume old results.

A query cache lookup must not stat every document, hash the file inventory, or validate all cached member files. The publication revision is the invalidation boundary.

Preserve normalized query keys and ordered-result reuse for pagination within one revision. Exclude page offset/limit from complete membership/order identity where the existing cache correctly does so.

Do not insert incomplete, cancelled, truncated, or source-unavailable executions into a cache advertised as complete verified membership.

Do not destroy a large retired cache while holding the publication lock. Old views/cache entries must eventually be released and must be counted in memory measurements.

## 11. Background filesystem reconciliation

### 11.1 Cache-opening lifecycle

Ordinary cache loading has two stages:

1. Synchronously validate format, checksums, bounds, configuration compatibility, and internal structural invariants, then restore and publish a usable cached view.
2. Schedule a background filesystem reconciliation and return the handle with status indicating that reconciliation is pending/running.

Do not synchronously enumerate all source files or require `sources_match` to succeed before returning the restored handle. Internal cache validation remains mandatory before exposing the cache.

Do not serve a cache with an incompatible schema or extractor configuration as though it were merely stale filesystem state.

Provide an explicit wait method for callers requiring completion of the initial reconciliation. This is useful for batch CLI commands and tests without making the default load blocking.

If retaining a low-level deserialization function, keep it internal or clearly separate from the ordinary public cache-opening contract. Existing public callers must not accidentally bypass automatic initial reconciliation.

### 11.2 Scan algorithm

Capture an inventory view and enumerate the configured source selections. Normalize paths using the existing native path representation and discovery policy.

Compare `SourceRevision { modified_ns, size }` for equality. A decrease in modification time is a change too.

Classify observations as unchanged, added, changed, or removed. Process additions/changes through the same normal mutation API. Perform removals through the same delete API. Do not duplicate indexing logic inside the scanner.

Deduplicate physical metadata reads where overlapping collections select the same file. Keep directory traversal, queued observations, prepared content, and worker concurrency bounded. The entire source content set must not accumulate in memory.

A no-change reconciliation performs metadata work but zero source-content reads/parses, publishes no new data revision, and does not flush an unchanged cache.

### 11.3 Scan failures and concurrent mutations

Only infer removal from a successfully enumerated scope or an explicit, sufficiently resolved not-found observation. A permission error, interrupted scan, inaccessible root, or failed directory enumeration is not evidence that every child was deleted.

Track which scopes completed. Preserve entries in unverified scopes and report incomplete reconciliation with diagnostics.

Before committing scan findings, revalidate their target versions. A scan based on revision A must not delete or overwrite a record added/updated by an explicit API call at revision B.

If a file changes while being prepared, use bounded retry or reschedule it. Avoid repeated metadata probes in unchanged build loops. Use metadata already captured by discovery and a small number of targeted checks where necessary to associate an incremental read with its intended revision.

Do not redesign fresh-build extraction around continuous filesystem stability verification. Its existing documented stable-input assumption can remain. Reconciliation is eventually consistent with external modifications and cannot provide an atomic filesystem snapshot.

### 11.4 Freshness, completeness, and source access

Expose separate concepts:

- **Index revision:** which published index state the query used.
- **Reconciliation state:** pending, running, idle/completed, cancelled, or failed/partial.
- **Query completeness:** whether execution could evaluate all required candidates and materialize the requested results.

A completed scan means the last pass was processed; it does not guarantee that an external application has made no subsequent changes.

While reconciling, metadata/index-only results may reflect cached state and new matches may be missing. Expose this freshness information without inventing current-content guarantees.

If source access finds a missing file, a revision mismatch, or an unusable range, the availability-oriented search path skips the unavailable result, records a bounded diagnostic/incompleteness indicator, and requests reconciliation of that file. Do not interpret new bytes through obsolete offsets.

Preserve an explicit strict failure option for callers needing it. An API called `count_exact` must not return a partial count as exact: return a completeness error or require the caller to use a result type that distinguishes an exact count from a lower bound.

For sorted or paginated results, filter inactive records before applying offset/limit. If source unavailability prevents filling or establishing an exact page, report that fact; do not manufacture null keys or silently declare the page complete.

Handle unavailable source data consistently in direct lookup, source verification, relation evaluation, sorting, materialization, and streaming. Avoid a blanket exception handler that hides corruption or programming errors as ordinary stale files.

### 11.5 Repeated operation

Provide start/stop methods for periodic reconciliation with an explicit interval and observable status. Permit only one full scan per index at a time and coalesce redundant requests.

Filesystem watchers are optional future scheduling accelerators, not required for this task. Correctness must not depend on their delivery. Do not add platform-specific watchers solely to satisfy automatic diffing.

Report useful counters: scanned files, unchanged, added, updated, removed, skipped/error files, bytes indexed, duration, and remaining work when known.

## 12. Explicit schemas, cache persistence, and repository convenience

### 12.1 Explicit core mode

- Building from supplied configuration does not search for schemas or caches.
- Loading a schema reads only the requested path.
- Loading a cache requires an explicit cache location and compatible explicit configuration, including source roots/registrations.
- Cache loading automatically reconciles in the background.
- Searching, mutating, compacting, rebuilding, or dropping an ordinary core handle does not automatically write a cache.
- Flushing writes only to the specified destination.
- Missing, corrupt, or incompatible explicitly requested caches produce a structured error. Do not silently select a different cache or rebuild unless the caller chose that behavior.

The API may accept relative paths, but must document their resolution base and resolve them once. Never search parent directories, consult undocumented current-directory conventions, or substitute `.objekte` when an explicit path is absent.

Separate persistence options from query caches: the existing prepared/verified caches are memory caches, not files to auto-discover.

### 12.2 Repository convenience mode

`open_repository(repository_path, options)` is the only library entry point that automatically discovers repository configuration, schemas, and caches.

Reuse the established repository layout, including existing `.objekte/collections/` schema conventions, rather than introducing a competing layout.

The convenience layer resolves concrete paths and definitions, then calls the explicit primitives. It must not create a second indexing engine or separate update semantics.

Defaults:

- Compatible cache present: restore, return searchable state, reconcile in the background.
- Cache absent: use the normal fresh-build path and report that it is building.
- Corrupt/incompatible cache: report the cause; rebuilding is allowed only through an explicit convenience-mode policy.
- `sync_cache = false`: no automatic writes.
- `sync_cache = true`: automatically persist dirty revisions to the resolved repository cache destination using bounded/coalesced scheduling.

When there is no usable existing index, full results cannot be available before an initial build. Do not promise instant search completeness for that case.

Manual schema and cache choices remain usable independently. A manual flush to another path must not silently retarget future automatic writes. Replacing the schema/configuration requires compatible reconstruction, not changing schema pointers underneath existing postings.

### 12.3 Flush correctness and durability

Flush one pinned published view. Serialize all necessary inventory, active state, ownership, revisions, fingerprints, index contributions, compatibility identifiers, and diagnostics.

Reuse the existing checksummed/validated snapshot format where possible. Bump model/snapshot versions when required. An old reader must not accept a new layout accidentally. Do not attempt an elaborate backward converter unless justified by existing requirements; report incompatible caches clearly.

Write to a uniquely created temporary file in the destination directory, finish serialization, synchronize as required by the durability contract, atomically replace the destination, and synchronize the parent directory where required/supported. Clean up only temporary files owned by this attempt.

Reject a cache destination that resolves to an indexed source file or an active schema/configuration file; a persistence operation must not overwrite source data through a path alias. Exclude explicitly configured cache destinations and their owned temporary files from source discovery when they lie inside a source root. This exclusion derives from the selected persistence target, not from implicit cache-path discovery. Test that automatic flushing cannot cause the index to ingest its own output repeatedly.

Atomic replacement provides protection against partial files. Document any platform limitations separately from the claimed durability guarantee.

Serialize flush publication per destination so an older concurrent flush cannot overwrite a newer one after it. Reject or clearly document unsupported concurrent writers from different processes; do not imply an in-process lock coordinates external processes.

Return the saved `IndexRevision`. If revision B was committed while revision A was being written, successful saving of A must not mark B clean. With automatic persistence enabled, schedule another flush.

Coalesce rapid changes, serialize snapshot writers, bound queued work, and expose failures for retry. A failed automatic flush must not disable search or erase the last usable cache. Do not persist after every individual appended record.

Do not serialize short-lived query caches or raw content buffers merely because the index now has background workers.

## 13. Compaction and full rebuilding

### 13.1 Compaction

Expose an explicit compaction operation that removes retired records and their obsolete index contributions. It must reclaim substantive obsolete storage, not just clear a deletion flag.

Construct a replacement from active index state wherever possible without reading unchanged source files. Compaction is not filesystem reconciliation.

A dense compaction may renumber record slots. If so, remap all postings, logical/structural domains, sort orders, file ownership, identity maps, relations, and diagnostic references, and publish a new generation. Invalidate incompatible locators and caches.

Preserve intrinsic IDs and valid current generated identities according to their documented policy. Do not silently change an externally visible generated identity merely because its physical storage slot moved.

Report records/slots before and after, retired contributions removed, estimated memory/disk size, elapsed time, temporary peak memory, and delayed reclamation due to pinned readers.

Use bounded work and cancellation. If retaining the old and new representation would exceed a memory limit, report the limit or use bounded partition compaction; do not claim that background work makes its memory cost free.

Automatic compaction is not required. If a bounded delta design needs internal consolidation, document its threshold and cost without coupling it to automatic disk persistence.

### 13.2 Rebuild

Expose explicit full index reconstruction using the current explicit configuration, or an explicitly supplied replacement configuration.

Use the established build/extraction machinery. Keep the existing published view searchable while constructing the replacement. Publish all rebuilt components together after validation.

Preserve committed mutations that occurred during the rebuild using the coordination scheme in section 6. Replaying a mutation must not accidentally execute an obsolete filesystem action; replay index intent/prepared state or re-resolve the current source as appropriate.

Rebuild failure/cancellation leaves the old view installed. Source-file changes may still make some old-view materializations unavailable; report that limitation.

Rebuilding does not imply flushing. In repository mode with `sync_cache` enabled, the committed new revision becomes eligible for the ordinary coalesced flush mechanism.

## 14. Implementation phases and suggested commits

Keep commits reviewable and buildable. The listed subjects are examples; split further when necessary. Push each commit immediately.

| Phase | Main work | Completion gate | Suggested commit subject |
| --- | --- | --- | --- |
| 0 | Inspect current code; record base, contracts, ownership and benchmarks | Known baseline and explicit implementation decisions | `Document index lifecycle implementation contracts` |
| 1 | Published view owner, revisions, inventory/active state and handles | Snapshot visibility and stale-handle tests pass | `Introduce versioned index publication and active record state` |
| 2 | Apply eligibility through queries, lookup, relations, sorting, counts and caches | Retired records cannot escape through any route | `Enforce record visibility across query execution` |
| 3 | Complete file add/delete/update/move using normal strategy | Multi-record and cross-collection atomicity passes | `Implement atomic file index mutations` |
| 4 | Background reconciliation, status, typed stale-source handling, periodic scheduling | Cached search runs while scan is paused; scan errors are safe | `Reconcile source files without blocking cached search` |
| 5 | Explicit schema/cache API and repository convenience wrapper | No implicit I/O in core mode; caller policies explicit | `Make index persistence and repository discovery explicit` |
| 6 | Persist active state/revisions; safe coalesced automatic flushing | Restart and interrupted-save scenarios pass | `Persist versioned index state and coordinate cache flushing` |
| 7 | Fingerprints and verified removal/slot-preserving update | Correct content succeeds; stale hints safely fall back | `Support verified in-place index maintenance` |
| 8 | Compaction and background rebuild coordination | Dense remapping and concurrent mutation survival pass | `Compact and rebuild indexes while serving queries` |
| 9 | Finish CLI/TUI/desktop migration and public documentation | Existing callers build and exercise intended policies | `Integrate explicit index lifecycle controls in clients` |
| 10 | Combined regression/performance validation and completion report | Full scope validated and pushed; user can merge one branch | `Report index lifecycle implementation and validation` |

Fingerprint calculation can be introduced earlier to avoid rebuilding fixtures before phase 7. API migration work may proceed in parallel after phase 0/1 contracts settle. Merge dependencies in the correct order.

Phases 1–4 provide the first useful milestone, but are not full completion. Do not use the phase order to postpone essential visibility, cache invalidation, or conflict protection until after ordinary mutations are exposed.

Migrate user-facing callers deliberately:

- Keep any current-directory/schema/cache convenience in the CLI/TUI/desktop layer or through explicit repository opening.
- Preserve explicit schema/index path flags.
- Make cache-writing intent explicit.
- Let interactive callers show reconciliation progress and serve the restored view.
- Let batch callers explicitly wait for reconciliation when current results are required.
- Avoid unrelated interface redesign or new file-editing features.

## 15. Validation matrix

Use small controlled fixtures and deterministic synchronization barriers for correctness tests. Do not rely on sleeps to make races happen. Run existing independent reference comparisons, and add differential checks against a fresh index built from the final fixture state.

The table is a behavioral checklist, not a requirement for one test function per row.

| Area | Required scenarios and assertions |
| --- | --- |
| Basic mutation | Add; delete without old bytes; update without old bytes; unchanged update/no-op; explicit move; destination conflict |
| Multi-record files | One-to-many, many-to-one, many-to-zero, zero-to-many; shifted byte ranges; changed record IDs |
| Shared physical files | Multiple collections observe one atomic file transition; no mixed metadata/ranges |
| Active eligibility | FTS/raw/regex/filter-only, NOT, presence/absence, arrays, direct lookup, count, sort, pagination and stream exclude retired slots |
| Old source access | Deleted slots cause no current-view source opens; stale ranges are never used against replacement bytes |
| Move semantics | Path sort/filter, path-based IDs, raw-fallback IDs, extension/reader changes, collection boundary, same path no-op |
| Old content verification | Exact old bytes; wrong bytes with identical bigram sets; incomplete old bytes; correct bytes but stale target token; missing digest; incompatible extraction |
| In-place update | Slot retained when eligible; obsolete postings removed across all families; old readers remain consistent |
| Delete optimization | Verified removal succeeds; mismatch falls back to tombstone; no replacement record appended |
| Conflicts | Update/update; update/delete; delete/add at same path; move/update; duplicate active logical ID; prepared changes after configuration replacement |
| Reconciliation | Added/removed/changed/unchanged; older timestamps; size-only difference; external rename as delete/add |
| Incomplete scan | Permission-denied directory, inaccessible root, cancellation, partial enumeration and per-file read failure do not produce mass deletion |
| Scan concurrency | Explicit mutation after scan capture survives; stale scan observation cannot overwrite or delete it |
| Metadata limitation | Same-size content change with restored mtime is not promised to be detected; explicit update still works |
| Search availability | Restore cache, pause reconciliation before enumeration completes, and successfully search the restored view |
| Query completeness | Missing sources, relation targets and sort inputs produce documented incomplete/strict outcomes; NOT does not convert unknown to true |
| Query cache | Warm membership/order cache invalidates on add/update/delete/move; unchanged scan preserves scope; no per-document stat loop on cache hit |
| Inference | New conflicting scalar type/cardinality, null/absence changes, mixed arrays, and retired evidence never cause unsafe candidate elimination |
| Source errors | Broken format, schema validation failure, raw fallback, unreadable source, fatal extension error and duplicate IDs follow documented outcomes |
| Persistence | Explicit load/flush; no implicit discovery/writes; fingerprint/active-state round trip; schema mismatch; corrupt/truncated cache; source/configuration destination collision; own-cache discovery exclusion |
| Flush races | A saved older revision cannot mark a newer one clean or replace a newer completed flush; I/O failure preserves previous cache |
| Repository mode | Schema/cache discovery only under explicit root; sync_cache on/off; manual override unaffected; missing-cache behavior documented |
| Compaction | Dense remap across all index families; stale locator rejection; old snapshot still works within source limits; logical results unchanged |
| Rebuild | Failure/cancellation retains old view; concurrent committed mutations survive; configuration change is atomic |
| Resource bounds | Queue admission, worker limits, pinned-view memory, repeated scheduling, cancellation and shutdown release behavior |
| Extension behavior | Deterministic reversible and unsupported/nondeterministic extensions select safe strategies |
| Alphabets | ASCII and Unicode with normal/skip grams, including case-sensitive verification and raw fallback |
| Regression | Existing query, relation, serialization, identity, inference, sorting, CLI/TUI/desktop tests remain valid or are intentionally migrated |

Use an independent fresh rebuild as the correctness oracle only after filesystem mutations are complete and background reconciliation has settled. Compare logical IDs, order, payloads where applicable, status, and counts; slot numbers are not expected to match after append or compaction.

Snapshot-format tests must validate corrupted liveness/ownership mappings, not merely successful round trips.

The baseline documents these commands; confirm their current applicability:

```bash
cargo fmt --all --check
cargo test --workspace --all-targets
```

Use current repository-specific acceptance and benchmark commands rather than inventing replacement suites. Rust 1.99 is declared at the inspected baseline. Record build prerequisites and any unavailable GUI/native dependencies. Missing required validation is an explicit limitation, not a passing result.

## 16. Performance evaluation and limits

Speed is a primary requirement. Measure on comparable release builds, the same hardware and dataset, and controlled filesystem-cache conditions.

Record the actual baseline and integration commits, compiler, worker counts, query/cache settings, dataset size, record/file count, source format, and repeated-run distributions. Do not interpret a single small timing difference as an improvement.

Use the existing Musenalm benchmark and its current query suite. Discover the actual case count at execution time rather than hardcoding a historical 150/152 count. Preserve the full established comparison suite.

Also include a multi-record fixture and a sufficiently large available collection to expose collection-sized update work. If the large dataset is unavailable, state the missing coverage.

| Measurement | Purpose |
| --- | --- |
| Fresh build with and without digest calculation where practical | Quantify fingerprint CPU/read/allocation overhead |
| Cache decode to first searchable handle | Demonstrate that source inventory scanning moved off the opening path |
| Initial reconciliation duration and metadata operations | Quantify background startup work |
| No-change reconciliation source opens/parses | Must be zero content reads/parses for unchanged files |
| Single add/update/delete/move at increasing collection sizes | Expose hidden whole-collection copies, sorting, ownership shifts, and cache destruction |
| Small and large mutation batches | Compare throughput, temporary memory, and publication overhead |
| Repeated updates to the same files | Measure retired storage growth and compaction benefit |
| Verified in-place versus append replacement | Establish whether the optimization improves cost and storage for relevant workloads |
| Warm repeated query and pagination latency | Protect Objekte's main performance objective |
| Queries during reconciliation/compaction/rebuild/flush | Measure availability and tail latency under maintenance |
| Flush bytes, duration, and frequency | Ensure auto-flush coalesces changes rather than serializing per record |
| Peak RSS, allocations/copies, retained views and preparation buffers | Identify excessive transient memory |
| Publication lock duration and waiting | Detect stalls hidden by background execution |
| Post-compaction query speed and index size | Demonstrate physical benefit and preserved search behavior |

Report median and tail latency where sample size supports it. Distinguish cache invalidation costs from steady-state query costs. Do not repeatedly rebuild or broaden benchmark coverage without a new change or unresolved concern.

Mandatory performance properties:

- Ordinary deletion does not parse the source or traverse every posting list.
- A small update does not rebuild every collection or resort every record.
- No query cache hit checks metadata for every stored/cached record.
- Unchanged-file reconciliation does not hash or parse unchanged content.
- Background preparation, queued work, and automatic persistence are bounded.
- Publication holds no lock across source reads, snapshot serialization, or full-index destruction.
- Repeated updates cannot create unlimited query-merge overhead without a documented consolidation mechanism.
- Performance reports include the temporary old-plus-new view cost.

Do not invent a fixed percentage speedup or a universal latency budget without measurements. If the selected representation requires a substantial slowdown or memory increase, report it with a concrete reproduction and design alternative before declaring completion. Routine representation adjustments needed to meet this assignment are authorized; do not turn each such choice into an approval request.

## 17. Explicit exclusions and design restraint

Do not implement source-file editing, a filesystem transaction manager, a durable write-ahead log, distributed coordination, platform-specific watcher infrastructure, a general-purpose storage engine, or a complete reverse-index framework beyond what this task requires.

An in-memory bounded change record for maintenance coordination is within scope. It is not a durable recovery log.

Do not add automatic full-content hashing to every scan or search. The content digest is acquired from indexing reads and checked against caller-supplied old bytes.

Do not treat every old snapshot as a materialized copy of source content. Do not store all source payloads merely to avoid stale-source handling.

Do not silently drop the verified in-place API, auto-reconciliation methods, or caller migrations because the normal update path works. If a format or extension requires a supported fallback, document it precisely and test it.

## 18. Acceptance and final implementation report

The task is complete when:

- All four file-index operations are available and atomic across affected collections.
- Normal deletion/update uses active-state retirement and appended replacements.
- Verified old data enables a working physical-removal/slot-preserving path where supported, with safe fallback.
- Revision conflicts cannot overwrite newer data.
- No inactive record escapes through optimized, reference, cached, relation, sorted, streamed, or direct-lookup execution.
- Cache loading makes restored search available before background inventory comparison completes.
- Reconciliation is observable, cancellable, repeatable, and safe under errors/concurrent mutations.
- Core schema/cache paths and persistence are explicit.
- Repository convenience discovery and optional sync_cache work without changing manual API semantics.
- Compaction reclaims obsolete state and rebuilding reconstructs it while preserving concurrent accepted updates.
- Current callers use the intended policies and compile.
- Required tests and performance comparisons have been run and their outcomes are recorded.
- All implementation and report commits are pushed on the integration branch.
- The original checkout remains on master, with unrelated user state preserved.

Append an `Implementation report` section to the completed task document containing:

1. Actual base commit, integration branch, worktree, completion date, and contributing branches.
2. Public API summary and migration examples for explicit mode and repository convenience mode.
3. Publication representation and why measurements support it.
4. Mutation behavior, old-content verification, fallbacks, and conflict policy.
5. Reconciliation/freshness/completeness guarantees and source-data limitations.
6. Cache versioning, persistence compatibility, flush durability, and automatic scheduling.
7. Compaction/rebuild behavior and temporary memory cost.
8. Test commands/results and benchmark comparisons, including regressions or unavailable coverage.
9. Approved deviations and specific remaining limitations.
10. Final committed document/report paths, pushed implementation commits, and cleanup status.

Place longer benchmark reports under `docs/ai/generated/` or the established benchmark results directory as appropriate, and link them from the completion report. A separate report does not replace the required appended report.

Retain the integration branch and worktree. Remove subordinate worktrees only after their changes are merged and pushed and all necessary untracked files are preserved. Never force removal.

## 19. Grounding references

These links describe the inspected baseline, not a guarantee that the implementation base remains unchanged.

- [Repository workflow](https://github.com/Simon-Martens/objekte/blob/6dc2ef194bc81bb8e4897b7652635bc7c7cb1cbb/docs/ai/README.md)
- [Repository ownership and opening](https://github.com/Simon-Martens/objekte/blob/6dc2ef194bc81bb8e4897b7652635bc7c7cb1cbb/crates/core/src/repository.rs)
- [File and record inventories](https://github.com/Simon-Martens/objekte/blob/6dc2ef194bc81bb8e4897b7652635bc7c7cb1cbb/crates/core/src/inventory.rs)
- [Snapshot storage and source comparison](https://github.com/Simon-Martens/objekte/blob/6dc2ef194bc81bb8e4897b7652635bc7c7cb1cbb/crates/core/src/storage.rs)
- [Verified query cache](https://github.com/Simon-Martens/objekte/blob/6dc2ef194bc81bb8e4897b7652635bc7c7cb1cbb/crates/core/src/verified.rs)
- [Sorting representation](https://github.com/Simon-Martens/objekte/blob/6dc2ef194bc81bb8e4897b7652635bc7c7cb1cbb/crates/core/src/sorting.rs)
- [Existing whole-file replacement boundary](https://github.com/Simon-Martens/objekte/blob/6dc2ef194bc81bb8e4897b7652635bc7c7cb1cbb/docs/future-whole-file-replacement.md)
- [Core documentation and validation commands](https://github.com/Simon-Martens/objekte/blob/6dc2ef194bc81bb8e4897b7652635bc7c7cb1cbb/docs/search-core.md)


## Implementation report

**Task:** `index-lifecycle`
**Completed:** 2026-11-08
**Base:** `9a94bc90d0d80ff7d14f44e457acf8787212d5ab` on `master`
**Integration:** `integration/index-lifecycle` at `/home/simon/source/objekte-integration-index-lifecycle`
**Contributing subtask branches:** `task/index-lifecycle/{validation,publication,relation-identity,persistence,configuration,query-visibility,mutation-preparation,mutation-segments,segment-queries,cache-format,cache-guard,cli-configuration,live-inspection}`. Their reviewed commits were merged into the integration history; all implementation and merge commits through `55e450a02e` were pushed. The original checkout stayed on master, and unrelated untracked user files were preserved.

### Public API and caller migration

Explicit mode uses `Index::build(root, definitions, options)` or `Index::load_cache(root, definitions, options, cache_path)`. The latter validates a cache synchronously, returns a searchable handle, and starts metadata reconciliation in the background. Callers can `index.view()?.search(&query)`, wait on `index.wait_for_reconciliation()`, and explicitly save with `index.flush_cache(cache_path)`. `Index::protect_configuration_files(&paths)` guards schema/configuration files when a caller uses explicit paths. `Schema::load`/the schema adapter load the specified file without repository discovery.

Convenience mode uses `Index::open_repository(root, RepositoryOpenOptions { ... })`. It discovers `.objekte/collections` only under that root, selects one alphabet-specific cache, and supports explicit missing/invalid cache policies and optional `sync_cache`. The coalesced writer saves dirty revisions automatically only in that mode. `Index::open_configured_repository` avoids parsing an already loaded configuration twice.

Versioned file APIs are `add_file`, `update_file`, `update_file_from_bytes`, `update_file_with_options`, `move_file`, `delete_file`, and `delete_file_with_old_content`. Source bytes and filesystem operations remain the application's responsibility. A returned `FileHandle` carries instance, file ID, and version; stale/foreign handles and destination collisions return typed errors. CLI batch queries now use the live lifecycle adapter and wait after explicit cache loads where current results are required. TUI and desktop load a restored view without waiting for reconciliation. Explicit desktop load+write schedules a one-shot flush after a successful background reconciliation and surfaces save errors.

### Publication and mutation design

Each query pins an immutable `IndexView` and one monotonic `IndexRevision`. The root shares the unchanged repository base and uses bounded indexed delta segments, chunked file catalog/version data, and per-collection active-record domains. A file mutation prepares outside the writer lock and publishes all affected collections atomically after revision/file-token checks. Normal update retires old slots and appends replacement contributions; normal delete retires slots without reading the old source. A concurrent commit makes a stale preparation conflict rather than replacing a newer root. A bounded 64-segment policy consolidates active contributions before query merge overhead can grow without limit. Old pinned views remain readable within source-revision limits.

Fresh indexing captures complete SHA-256 digests during existing source reads. Caches persist those digests. Supplied complete old bytes are checked against the digest, file token, reader layout, and configuration before verified maintenance. Eligible isolated one-record deltas preserve their local slot in a new immutable shard; other eligible deltas and base files physically remove old contributions in a private staged root. Base removal uses source-free compaction with a 262,144-record-per-collection budget. A missing digest, stale or mismatched old bytes, incompatible layout, or budget limit selects the normal tombstone/append fallback and reports the reason. No mutation writes source files.

### Reconciliation, query completeness, and persistence

A loaded cache is searchable before inventory comparison. Reconciliation is observable, cancellable, repeatable, and coalesces duplicate requests. Failed or unenumerated scopes do not cause mass deletion. Explicit source-read failures request a coalesced reconciliation pass. Periodic scheduling can be started/stopped independently; stopping ticks does not cancel an in-flight pass. Unchanged files are compared using metadata without content hashing or parsing. This deliberately does not detect a same-size edit with restored modification time; callers can explicitly update that file.

`IndexView::search` and stream summaries expose bounded typed source diagnostics when missing, changed, or unusable bytes prevent complete evaluation; `count` is then absent. `search_strict` and `count_exact` reject incomplete results. `get_record_available` distinguishes absent IDs from unavailable source bytes. Active domains apply to optimized, reference, relation, sorted, streamed, cached, posting, and direct-lookup routes. A completed scan describes the last processed pass, not an atomic filesystem snapshot.

Legacy static snapshots are version 16 and lifecycle snapshots version 17. Cache decode checks checksums, bounds, compatibility, liveness, ownership, and segment structures before publication. Explicit flush serializes a pinned revision to a guarded destination with an atomic replacement and parent-directory sync; older completion cannot mark a newer revision clean. The selected cache is excluded from source discovery. Ordinary search, mutation, compaction, rebuild, and handle destruction do not implicitly persist. Repository-mode automatic saves are coalesced with a maximum one-second latency; failures preserve the last usable cache and are observable.

`compact()` reclaims retired base/delta contributions without rereading unchanged sources and invalidates stale physical locators by publishing a new revision. `rebuild()` reconstructs from the current explicit configuration while readers use the old root; bounded CAS retries preserve concurrent committed mutations. `start_compaction()` and `start_rebuild()` expose progress, cancellation, completion, and wait handles. Shutdown stops scheduling, cancels and joins active work. Maintenance never implies a cache write. The old-plus-new representation temporarily raises memory use: the measured large-fixture process peaked around 1.1 GiB versus roughly 0.5 GiB after fresh build. Pinned readers may delay reclamation.

### Validation and performance

`cargo test --workspace --all-targets --quiet` passed on `55e450a02e`: 429 core tests passed, 4 ignored; CLI 75 unit and 32 process tests passed (1 process ignored); desktop 83 and TUI 49 passed; all other workspace targets passed. A controlled mutation sequence was compared with an independent fresh index, including ordered IDs, payloads, status, and exact count. Cache corruption/ownership, source failure, stale view, concurrent revision, and caller policy tests also passed. The committed `bench_lifecycle_musenalm` and `bench_lifecycle_mutations` examples preserve the benchmark procedure.

The full established Musenalm extended suite passed 194 cases × 4 profiles × 5 repetitions on the final static route (3,880 samples); the lifecycle view passed 194 cases × 2 uncached profiles × 5 repetitions (1,940 samples). All 388 lifecycle case/profile counts and returned-row counts matched static. One large active-domain planning regression, Q172, was fixed by constructing the all-live Roaring domain as a range; a targeted five-repetition release rerun reduced its median from about 2.0 ms to 0.1 ms with the exact count unchanged. Three final large-fixture lifecycle mutation runs had median fresh build 1.284 s, add 0.302 ms, append update 0.231 ms, verified update 0.377 ms, move 0.215 ms, verified delete 0.164 ms, source-free compaction 1.232 s, flush 357 ms, restore-to-search 693 ms, and no-change reconciliation wait 886 ms. Peak RSS was 1.12–1.18 million KiB. The full commands, distributions, dataset hashes, run conditions, and raw paths are in [the performance report](../generated/index-lifecycle-performance-validation.md).

`cargo fmt --all --check` fails on widespread formatting differences present before this task (913 reported diff hunks on original master); it reports 1,079 on the integration worktree. We did not blanket-format unrelated files. Individual touched files were formatted where practical. No GUI interaction was part of this core API task; desktop lifecycle UI work is a separate authorized dependent task.

**Remaining limits:** Cache files do not contain source payloads, so old pinned views can report unavailable data after external source changes. Verified base maintenance is opt-in and collection-sized under its budget; ordinary mutations stay sparse. Source freshness is eventually consistent with filesystem metadata. Warm verified-cache profiles were measured on the established static runner; the lifecycle benchmark runner measured uncached ASCII/Unicode profiles. Three stress runs on 148,486 files found that 128 normal updates took a 44.207 ms median, with no query errors during reconcile, compact, rebuild, or flush. A prior 73-second batch regression after verified base maintenance was fixed by merging small compatible segments before considering dense-shard compaction (`20c04e23cd`). A true verified base update still took a 1.279-second median and is recorded in [the performance issue](../issues/perf/verified-base-maintenance-collection-rewrite.md); ordinary appended update was 0.329 ms. Concurrent rebuild query p95 was 2.268 ms, with isolated maxima near 74 ms in two runs. Exact publication lock duration, per-operation allocator bytes, and metadata syscall counts were not instrumented; RSS and query latency were measured instead. No deviations from the authorized API behavior were requested or approved.

**Completion files and cleanup:** This report is appended to `docs/ai/done/objekte_index_lifecycle_implementation_guide.md`; the longer report is `docs/ai/generated/index-lifecycle-performance-validation.md`. At this report commit, subordinate task worktrees are retained pending the required post-push clean-worktree check and normal removal. The integration worktree and branch remain available for the dependent desktop task.
