# Objekte stable-index prototype repair specification

Date: 16 November 2026  
Task identifier: ic-stable-index-prototype-repair  
Status: implementation assignment  
Reviewed prototype: 530f4037f01bcd38f5027042e54676b543fdd230  
Source branch: integration/ic-stable-index-prototype  
Native baseline: c54486f893a57fac0b1b80ed967ab4f5a5fd26c9  
Required integration branch: integration/ic-stable-index-prototype-repair  
Required integration worktree: ../objekte-integration-ic-stable-index-prototype-repair

## 1. Assignment and interpretation

Repair the existing ic-stable-structures prototype. Correct its incomplete behavior, remove demonstrated avoidable work, and produce an honest comparison against both the original prototype and native master.

This is a continuation of the selected storage experiment, not permission to choose another storage engine. Use ic-stable-structures 0.7.2, independent RAM-backed components, compressed Roaring postings, explicit persistence, and independently openable component files.

The original specification remains the functional contract except where this document explicitly changes implementation details, compatibility versions, or stopping rules. Read it at docs/ai/done/objekte_ic_stable_index_prototype_specification.md on the source branch. Treat its appended completion report as historical evidence, not proof that every requirement was met.

The user has authorized repairing the gaps described here. Routine refactoring, specialized key types, a revised prototype file version, additional regression tests, and the benchmark work below need no additional approval.

Deliver one reviewed integration branch. Do not merge into master, replace the production backend, or change the default CLI or desktop behavior. A working prototype that remains slower than native is an acceptable measured outcome. An incomplete or incorrect prototype is not an acceptable completed repair.

### 1.1 Preserve these decisions

- One logical index has one owner, one independent memory, and one independently interpretable saved file.
- Separate file catalog, path lookup, per-collection record catalog, identity lookup, and live/domain components.
- One writer per tree; independent trees can build or update on different workers.
- Fresh and reopened structures operate in owned RAM. Do not introduce FileMemory, mmap, automatic writeback, background synchronization, or temporary index-payload files during construction.
- Save only on explicit save_index, save_as, or save_snapshot.
- Loading reads the requested component image and reopens its existing tree. No entry reinsertion, rebuilding of all dictionaries, or reading every source file.
- Keep Roaring compressed postings. Do not replace every posting membership with an individual tree row.
- Keep the native backend and feature-off behavior unchanged.
- Source-read-only FTS verification remains distinct from candidate generation.
- Inference, live MVCC, automatic eviction, physical compaction, slot reuse, general hostile-file parsing, and GUI integration remain outside this repair.
- Do not introduce segments, change the one-file-per-logical-index model, fork the crate, write private B-tree layouts, or add a permanent full native index cache to obtain better numbers.

### 1.2 Workflow

Read AGENTS.md and docs/ai/README.md before editing. Inspect existing worktrees and preserve user changes.

Create the repair integration branch from the inspected prototype, not from master. Keep the original prototype branch and evidence intact. If that branch has advanced, record its new head, compare it with the reviewed commit, and determine which findings already have fixes.

Use sibling worktrees under the existing repository's parent; on the user's machine this is normally /home/simon/source. Do not use /tmp. Keep the original checkout on its existing main branch.

Keep pristine baseline checkouts/executables for:
1. Native master at the recorded baseline.
2. The unmodified prototype at the reviewed commit.
3. The repaired candidate.

Push after each commit. Do not rewrite published history. Follow the repository task-document and report lifecycle. A single agent may implement and supervise. Optional delegation must use disjoint subtask worktrees after interfaces are established; only the supervisor integrates changes. Do not split the mutation transaction boundary or component-format ownership across independent implementations.

## 2. Evidence and required dispositions

The following observations are from source inspection and committed measurements. They are not new performance runs.

### 2.1 Recorded performance

Five P=8 Musenalm ASCII runs report:

| Measurement | Native | Original prototype |
| --- | ---: | ---: |
| Median source build | 1.699 s | 4.959 s |
| Resident RSS in the reported comparison | 397.7 MB | 763.2 MB |
| Peak RSS in the reported comparison | 410.6 MB | 880.0 MB |
| Stable component payload length | Not applicable | 408.9 MB |

The reported candidate stage values include approximately 0.327 s discovery, 0.859 s extraction/admission, 0.452 s native finalization, and 3.179 s stable construction. Remeasure; do not assume the report's stage summaries are medians, because the campaign driver takes some fields from the final sample.

The candidate-1 component trace shows:
- 1,323 components, including 574 with zero entries.
- Component jobs span 2.956 s, within a 3.156 s stable-component stage.
- Summed component job durations are 8.268 s. This is overlapping worker elapsed time, not process CPU time.
- One worker executes all first 82 component jobs sequentially and is occupied for approximately 2.956 s.
- That worker builds the file catalog, path lookup, record catalogs, and identity catalogs. The large jobs include approximately 0.658 s files, 0.886 s paths, 0.546 s the largest record catalog, and 0.758 s the largest identity catalog.
- Every other worker has finished by approximately 1.287 s. Most of the tail therefore uses only one worker.
- Normal and skip gram jobs together account for approximately 0.235 s of summed job durations; configured sorting accounts for approximately 4.314 s. These sums must not be interpreted as additive wall-clock stages.

A simple eight-worker rescheduling calculation holding those recorded job durations fixed gives a 1.277 s makespan instead of 2.956 s. The implied 1.679 s saving is an optimistic trace calculation, not a benchmark or acceptance promise. Different concurrency changes cache, allocation, and memory-bandwidth costs. It justifies testing scheduling first.

### 2.2 Findings register

Every row must receive a code/test/evidence disposition in the final report.

| ID | Finding at the reviewed commit | Required response |
| --- | --- | --- |
| R01 | Identity-sorted component jobs enter an unconstrained Rayon parallel iterator; the trace leaves expensive catalogs in one sequential partition. | Independent schedulable component jobs, demonstrated catalog overlap, balanced execution order. |
| R02 | source_build finalizes native CollectionState values and passes borrowed files/collections into stable construction. All native indexes remain alive during conversion. | Consume owned construction state and release each native index as it converts. |
| R03 | The coordinator calls CollectionAdmission::admit_profiled, which merges all field, text, relation, and sort contributions serially. | Separate identity/metadata admission from independent index-owner merges. |
| R04 | SortBuilder::finish constructs native ascending/descending orders, but build_sort ignores them and inserts by record-slot order. | Avoid creating discarded native orders; measure the selected stable insertion strategy. |
| R05 | Every component uses one unbounded ComponentKey enum with bincode serialization, even fixed-width IDs, grams, and state keys. | Role-specific stored key types and correct bounds; preserve semantic ordering. |
| R06 | live_domain clones its cached bitmap on every call, including small point queries. | Reuse the decoded live domain without full bitmap cloning. |
| R07 | next_file_id scans the full file tree; append/delete repeatedly decode and encode entire domain bitmaps per record. | Efficient high-water lookup and grouped mutation. |
| R08 | append_file/replace_file update catalogs, identities, and domains but never install new typed/text/sort/relation facts. | Complete prepared-fact lifecycle with semantic comparison after updates. |
| R09 | delete_file_by_path subtracts cumulative deleted.len() from each collection; zero-record paths return before removing the file/path. | Per-collection counts and physical-file lookup independent of record count. |
| R10 | Move/append collision checks use returned record lists, losing the distinction between absent paths and existing zero-record paths. Move checks every collection's debug identity-policy string. | Typed policy and full path entries; validate only affected collection memberships and path-dependent behavior. |
| R11 | Semantic posting APIs ignore stored completeness/indexing flags and can interpret unsupported or incomplete indexes as empty answers. | Explicit restriction availability and coverage checks. |
| R12 | Invalidation sets usable=false, but ordinary tree/query paths do not enforce require_usable; many partial mutation errors are not invalidated. | Central mutation failure boundary and enforced unusable state. |
| R13 | Standalone saves/copies write directly to the final destination. | Atomic publication through a sibling temporary file, with failure tests. |
| R14 | Component writing hashes the payload before writing and again while writing. | One streaming payload-hash pass per save. |
| R15 | Saved metadata and dirty flags are changed before snapshot publication; empty placeholder trees can remain exposed on restoration failures. | Stage publication metadata; restore ownership or mark failed; never expose an empty substitute. |
| R16 | The descriptor does not fully describe text representation/alphabet; configuration hashing relies on debug formatting and omits relevant settings. Coverage/high-water provenance is not maintained by lifecycle updates. | Complete, versioned, deterministic semantic descriptors and coherent update metadata. |
| R17 | Envelope component IDs narrow from u64 to u32 with an unchecked cast; manifest/dependency checks are incomplete. | Checked decoding and consistent descriptor/dependency validation. |
| R18 | Build encode/insert counts are inferred from final entry counts; absent timings/read metrics are emitted as zero. Memory lacks capacities and ownership accounting. | Observed counters, explicit unavailable values, and truthful memory attribution. |
| R19 | Primitive timing measures isolated calls without matched native distributions and can include first live-domain loading; errors become zero members. | Fair resident/first-load benchmarks with failures propagated. |
| R20 | Lifecycle benchmark checks counts/storage growth without verifying new search facts; composite sorts remain rejected. | Semantic lifecycle oracle and configured composite-sort support within the existing prototype model. |

Relevant locations are listed in section 14. Line numbers refer to the reviewed revision and will move.

## 3. First repair: component scheduling

Implement this as an isolated change before large ownership or codec changes, so its effect can be measured separately.

1. Preserve deterministic component identities and numeric IDs. Assign them in canonical descriptor order before scheduling.
2. Represent each component conversion as an independently schedulable job. Start with into_par_iter().with_max_len(1) before map, or use one scoped Rayon task per component.
3. After assigning IDs, order execution by a deterministic work estimate, largest first. Use available entry counts, payload/membership estimates, and role; do not scan or encode every posting to estimate work. Do not depend on machine-specific historical timing to define correctness.
4. Preserve result ordering by component ID when producing manifests and reports, regardless of completion order.
5. Reuse one global P-worker pool for construction. Do not retain one source pool while constructing a second P-worker pool. Snapshot I/O must also respect the configured budget, using at most min(4, P) active I/O jobs.
6. Do not add locks around a shared tree. A worker owns the complete tree for its job.
7. Record start/end/worker/role/count/estimated-work for every component in diagnostic mode.

Required evidence:
- Compare the original prototype and scheduling-only candidate using identical P=8 settings and fresh processes.
- Show independent large catalogs overlap, not merely that two tiny field jobs overlap.
- Report the component makespan, sum of job durations, largest job, last completion per worker, average active workers, and intervals with queued work but idle capacity.
- Do not require impossible full utilization after only one indivisible job remains. Report that legitimate tail separately.
- Verify semantic equality and component ID stability at P=1, P=2, and P=min(8, available_parallelism).

Do not assert that adding a Rayon call proves concurrency. The trace is the evidence.

## 4. Owned construction and independent accumulation

### 4.1 Required construction pipeline

Retain native extraction and packed contributions. Change ownership and routing, not the source parser or logical semantics.

    discovery and prepared descriptors
        -> bounded parallel batch extraction
        -> deterministic metadata admission and slot-base assignment
        -> move fragments to independent index owners
        -> parallel native fragment merge
        -> consume each completed owner into one stable component
        -> publish ready prototype

The coordinator may process record/file metadata to preserve identities and diagnostics. It must not traverse all posting memberships or perform every field merge.

Add a narrow consuming decomposition method for BatchContribution in crates/core/src/build.rs. Keep its fields encapsulated behind crate-local types rather than making everything public. It must move:
- File metadata, record metadata, fingerprints, diagnostics, and accounting.
- Native typed/custom posting fragments.
- Packed raw/fallback/custom BigramContribution values.
- Logical and empty-array domain fragments.
- Sort-key vectors.
- Relation contributions.

Do not use metadata_copy or clone the full batch to route fragments.

Extract the existing metadata/diagnostic admission logic from CollectionAdmission::admit_profiled into a reusable operation. Preserve:
- First and subsequent ranges of one physical file.
- Deterministic record order and collection-local slot bases.
- Diagnostic remapping and file validity.
- Duplicate logical identities.
- One physical file selected by multiple collections.
- File fingerprints collected during the actual reads.

Native feature-off behavior must continue using the same semantics.

### 4.2 Bounded waves

Use the original specification's simple bounded-wave model:
- One pool of P workers.
- At most 2P preparation jobs in a wave.
- Configurable 64 MiB estimated preparation target.
- A single oversized-job exception, explicitly reported.
- Existing per-job framing/source bounds remain in effect.
- The bound covers transient preparation, not the complete index.

Extract a wave in parallel, admit metadata in deterministic job order, move fragments into owner lists, merge independent owners in parallel, then release the wave. Do not enqueue the entire corpus or block producers and consumers against each other in the same bounded pool.

Within an owner, merge fragments in their assigned slot order using existing merge_shifted and packed bigram methods. Do not expand grams into per-record heap lists or reconstruct per-record reverse gram indexes.

Permit an initial owner to contain one field's closely related native state if needed to preserve existing merge methods. Split its completed value/length/state maps by ownership into separate stable jobs. The saved structures remain separate components. Parallelizing independent fields is mandatory; fine-grained parallel mutation within a single field is unnecessary.

### 4.3 Consume completed state

The primary from_sources path must no longer pass a complete borrowed collection graph into stable conversion.

Use owned job data:
- Move completed native maps and bitmaps into conversion jobs.
- Move keys where possible and release each posting immediately after its encoded bytes have been inserted.
- Do not clone file/record structures merely to wrap them for serialization; use borrowed serialization views or move ownership.
- Release the completed owner and its backing maps before unrelated conversions finish.
- Build path entries with dense FileId-indexed temporary vectors when the IDs are dense. Avoid a temporary B-tree plus a second duplicate-path B-tree unless a measured or semantic need remains.
- Reuse existing identity groupings where possible rather than building another complete native identity map.

Shared read-only metadata may remain temporarily available to catalog and sort consumers where necessary. Its ownership, size, and last release must be explicit. This exception does not permit retaining all native posting/text/sort indexes until the entire stable build finishes.

A from_repository convenience adapter may remain for tests, but keep it distinct from the measured source-build path. Its borrowed conversion must not become the performance path again.

### 4.4 Avoid duplicate sorting work

The current native sort finalizer builds order vectors that stable construction ignores. Introduce a narrow way to consume extracted sort keys and configuration before native order vectors are constructed.

Default repair:
- Generate native semantic/collation keys once.
- Build required stable sort components directly from those keys.
- Preserve native null/missing placement and logical RecordId tie-breaking, followed by deterministic slot tie-breaking.
- Retain separate ascending and descending components where their ordering contracts differ.
- Implement configured composite sorts by the native constituent comparisons, directions, placement rules, and tie-breaking. One configured composite order has its own component.
- Do not replace numeric ordering with serialized-byte order.

An alternative using native sorted slot sequences as the stable insertion order may be benchmarked, but include the sorting time. Retain only the faster correct complete strategy. Ordinary sorted insert calls are not a dedicated bulk loader.

Do not optimize by dropping configured sorts or changing schema registrations.

### 4.5 Lower-priority construction cleanup

After the main changes:
- Cache canonical descriptor sort keys rather than allocating debug strings for every sort comparison.
- Remove tree extraction/reload solely to read its length/capacity. A small owned Memory wrapper may expose these statistics safely if the crate does not offer access.
- Record both memory length and Vec capacity. Do not call shrink_to_fit across all components by default; measure its copy/allocation cost separately if proposed.
- Empty configured components remain distinguishable from unsupported components and independently saveable. Do not silently omit them to reduce memory.
- Reuse encoding scratch only when ownership permits. Do not replace a moved byte buffer with an additional copy solely to claim scratch reuse.

## 5. Specialized storage types and versioned codecs

Replace the universal stored ComponentKey with role-specific key types. An API-level enum may remain for dispatch and raw inspection, but must not force every underlying tree to store all role tags or use unbounded key metadata.

Use a typed component-storage enum, with variants holding actual ic-stable-structures BTreeMap instances. Each variant owns its own IndexMemory. Keep one small dispatch layer for operations rather than duplicating algorithms across public APIs.

| Component | Stored key | Bound |
| --- | --- | --- |
| File catalog | FileId/u32 wrapper | Fixed 4 bytes |
| Record catalog | RecordSlot/u32 wrapper | Fixed 4 bytes |
| Domain/state/relation-state postings | Explicit state-code wrapper | Fixed width |
| Normal/skip grams | Existing u64 gram identity | Fixed 8 bytes |
| Relation occurrence | Source slot plus occurrence ordinal | Fixed 8 bytes |
| Path lookup | Lossless platform-tagged path bytes | Unbounded |
| Logical identity lookup | Exact identity representation | Unbounded where required |
| Typed values | Semantic ScalarKey wrapper | Unbounded where required |
| Length values | Exact Number wrapper | Follow actual representation; do not assume every number is fixed-width |
| Sort/composite order | Native semantic keys and exact ordering context | Unbounded where required |
| Relation targets | Target collection plus exact target identity | Unbounded where required |

Requirements:
- Preserve native Ord/Eq for exact numbers, text, identities, and sort semantics.
- Integer byte order does not have to be order-preserving when the tree compares decoded keys; decoded comparison must be correct.
- Implement explicit compact fixed-width codecs. Do not serialize the universal enum with bincode for numeric IDs or grams.
- Keep StoredBytes move/borrow behavior for variable payloads. Use an actual zero-sized value type for sort entries where appropriate.
- Never invent a maximum string or bitmap size to claim bounded storage.
- A single type must have one true Storable bound. Do not vary a type's bound by runtime descriptor.
- Do not replace ordering-aware sort keys with byte comparisons unless a complete ordering-preservation proof and tests exist.
- Validate sort direction/options, scalar kind, state-code domain, and relation-target identity against the descriptor in public raw mutations. Checking only an enum tag is insufficient.
- Keep raw byte access explicitly raw. A safe semantic mutation must validate payload type/encoding; do not allow malformed metadata to enter through an ordinary typed interface.

Version policy:
- Use an explicit new key codec revision and descriptor revision. Change the outer/manifest version if its interpretation changes.
- Old prototype files may be rejected with a clear incompatibility error; migration is not required.
- Never open an old image as a new typed tree merely because the crate's internal tree version is unchanged.
- Dispatch loading only after envelope and descriptor type/version validation.
- Retain exact crate pinning and direct memory-image loading.

Measure this change independently from scheduling. Faster key encoding is plausible but its contribution is not established by the old trace. Do not promise native parity.

## 6. Query correctness and avoidable query work

### 6.1 Coverage is part of the answer contract

Repository-semantic APIs must distinguish:
- A complete supported index that proves no matching slots.
- An incomplete index that cannot safely restrict the result.
- A missing/unsupported component.
- A corrupted or failed component.

Use an explicit result enum or equivalent existing convention. A suitable shape is NoRestriction versus Complete(RoaringBitmap), with real errors for unsupported requests where the API promises a complete answer.

Check the relevant predicate-specific flags:
- values_indexed and value_complete for typed value restriction.
- length_complete for length restriction.
- empty-string completeness where applicable.
- The exact supported structural state and covered logical domain.
- Named custom-binding coverage and text representation completeness.

A field can have complete length facts and incomplete values; do not require every family to be complete for any one family to be usable. Absence/complement operations use the proper live logical/fallback domain.

Raw posting APIs may return physical postings regardless of coverage. Keep their naming and documentation explicit. Do not implement unsupported semantic predicates by returning empty bitmaps or silently scanning all source records.

### 6.2 Live-domain reuse

Keep one accounted decoded live-domain cache per accessed collection. Return a borrowed view or a shared immutable handle; do not clone the entire bitmap for every point query.

Arrange borrows cleanly, for example using an Arc<RoaringBitmap> cache and owned/borrowed component access. Update or invalidate it once after a successful batch. Intersect an already owned result in place where possible.

A caller explicitly requesting an owned copy of all live slots may pay that cost. Ordinary typed, gram, relation, and sort queries must not pay it unnecessarily.

Do not add a permanent decoded cache of every posting.

### 6.3 Text, ranges, sorts, and relations

- Persist the text alphabet and normalization/extractor identity/version. text_candidates must obtain them from the selected component or reject mismatching caller options.
- Prepare normal/skip query grams once; retain native normalization and segment semantics.
- Restrict candidates through liveness and the appropriate representation domain.
- Traversal must decode only visited values. Never reconstruct an entire native map to answer a range.
- Avoid key cloning where the iterator supports consuming an entry; otherwise document unavoidable crate behavior.
- Preserve pagination, exact sort ties, missing/null placement, and composite semantics.
- Filter outgoing source records and backlinks through authoritative liveness. Specify whether target resolution is derived from current identity catalogs; avoid a stale stored resolved flag after target deletion or addition.
- FTS candidates remain candidates. If reporting verified query results, use the native verifier and identical source I/O on both benchmark sides.

## 7. Complete and efficient file lifecycle

### 7.1 Prepared mutation unit

Introduce one prepared file-change representation containing:
- Physical-file metadata and expected existing state.
- All affected collection memberships.
- Local record metadata and diagnostics.
- New typed, length, structural, raw/fallback/custom text, sort/composite, relation occurrence/target/state facts.
- Slot mapping information and coverage updates.
- Required key/value encodings or validated preparation data.

Preparation must complete before retiring old slots. Reuse native extraction and compact contributions; do not add per-record reverse gram storage to every index build.

The public fixture-level add/replace operation consumes prepared facts and updates every configured family. A metadata-only helper may exist privately, but must not be presented or benchmarked as complete replacement.

For a physical file belonging to multiple collections, prepare and replace all affected collections or explicitly expose a collection-scoped operation that leaves other memberships untouched. Do not delete every membership and rebuild only one.

### 7.2 Add and replace

- Allocate non-reused FileIds and collection-local RecordSlots with checked arithmetic.
- Use a stored high-water value or the ordered maximum key to determine the next FileId. Do not scan all file entries for each append.
- Detect collision through full PathCatalogEntry lookup, including an entry with no records.
- Append record metadata, identity groups, all new indexes, and live/domain facts consistently.
- For replacement, old slots become dead and new slots receive all facts even when the file contents are unchanged.
- Test an unchanged replacement: the semantic query results must be unchanged except for permitted slot/incarnation differences.
- A failed preparation leaves the old state unchanged.
- An unexpected application failure after mutation begins makes the owner unusable unless the operation has fully restored the pre-change state. Rollback is not required.

### 7.3 Group updates by component and key

For one prepared batch:
1. Group additions/removals by component and posting key.
2. Read/decode each touched bitmap once.
3. Apply all changes.
4. Encode and replace once, or remove once if appropriate.

Apply this to live/all/logical/fallback/empty-array domains and identity postings, as well as typed/text/state/relation postings. Do not repeatedly serialize a collection-wide domain bitmap for every record.

Keep changes for high-frequency postings compact. Measure bytes read, bytes encoded, and bytes submitted to stable storage. One-member changes to huge postings still have real amplification; do not claim constant-cost mutation.

Move encoded metadata into the tree instead of cloning it from an intermediate list. Do not add unconditional no-op writes.

### 7.4 Delete

Look up the physical PathCatalogEntry first, preserving its FileId even if refs is empty.

For each affected collection:
- Count only records newly retired in that collection.
- Mark records dead and update live/domain counts consistently.
- Update identity lookup according to the existing logical-deletion contract.
- Use one grouped domain update for that collection.
- Return fully qualified RecordRef values where records from multiple collections are returned.

Never subtract the global cumulative deleted vector length from each collection. Avoid saturating arithmetic concealing invalid counts; detect an invariant violation.

Remove the path mapping and mark the physical file dead even for zero-record files. Repeated deletion is a well-defined no-op. Dead posting memberships may remain physically stored, but every semantic API must filter them.

### 7.5 Move

Use typed persisted identity/selection policies, not Debug strings or contains("Generated").

Validate only the file's actual affected memberships. Determine whether paths affect collection selection, extraction, generated identity, validation, sorting, or indexed fields. A truly path-independent rename updates file/path catalogs while preserving slots and facts. Otherwise use the prepared replacement path or return an explicit unsupported-operation error before mutation.

Handle:
- Destination occupied by a zero-record file.
- Destination occupied by a nonempty file.
- Source equals destination.
- Dead or absent source.
- Membership changes.
- Non-UTF-8 native paths where supported.

Do not overwrite another path's mapping.

### 7.6 Metadata after mutation

Maintain collection live counts, slot high-water values, coverage counts, component revisions, and detached/origin metadata consistently.

Separate immutable slot namespace identity from a component's build coverage and revision. Do not stamp an old unchanged component as complete for newly appended records. Do not force every unchanged component image to be rewritten merely to carry a global generation number.

A manifest binds a coherent selection of component revisions. A standalone descriptor must truthfully describe its own coverage and dependencies. Raw detached mutation must not continue claiming validated repository completeness; mark that semantic claim unavailable when appropriate.

Keep small metadata required for updates in RAM or descriptors. Do not add a full second record/search backend.

## 8. Failure boundaries and persistence repair

### 8.1 Unusable state

Store the first invalidation reason. Enforce the unusable state before every semantic query, mutation, save, load-for-query, and raw operation through an invalid repository owner. Read-only diagnostic/status inspection may remain available and must identify failure.

Centralize the mutation application boundary. If any error or caught unwind occurs after the first change, mark the owner unusable. Do not leave this only in replace_file. Include append, delete, move, and batched posting updates.

Prevalidation should prevent routine errors after application starts. Do not catch process aborts/OOM or claim rollback guarantees that do not exist.

Do not expose a fresh empty placeholder tree when restoring an extracted memory image fails. Install Failed/Unavailable, retain diagnostic context, and reject subsequent operations.

### 8.2 Standalone atomic publication

Use one writer shared by save_index, StandaloneIndex::save_as, generation components, and verified unloaded copies.

Standalone publication:
1. Reject an existing destination.
2. Reserve a unique sibling temporary file with create_new.
3. Write and verify envelope, descriptor, and complete payload.
4. Flush/sync and close file handles.
5. Publish by rename on the same filesystem.
6. Sync the parent directory where supported.
7. Only then update source_path/saved metadata.

Concurrent publishers to the same destination remain unsupported under the original prototype contract. Do not claim portable atomic no-replace semantics from an existence check plus rename. Existing files must be preserved in the supported single-publisher case.

On failure:
- The prior saved file remains unchanged.
- The requested final path does not expose an unfinished file.
- The RAM tree is restored, or the handle becomes explicitly failed.
- Temporary files are removed on ordinary error paths where possible.
- Crash leftovers are not accepted as published indexes by automatic discovery; there is no automatic discovery requirement.

Test both loaded and unloaded-copy paths, including copied/renamed standalone inputs without any neighboring manifest.

### 8.3 Generation publication and dirty state

Treat snapshot publication metadata transactionally:
- Keep new ComponentFileInfo values, checksums, paths, and intended clean states in temporary publication results.
- Do not replace previous saved_info or clear dirty flags as each staging chunk finishes.
- Restore all extracted memory ownership before returning from any failure.
- Commit generation/source paths and clean states only after publication succeeds.
- Preserve usable loaded/unloaded components after an ordinary I/O failure.
- A component restoration failure must explicitly fail the affected owner; no empty substitute.
- After a rename followed by a directory-sync failure, report that the generation may be visible but durability was not confirmed. Do not claim it was never published or delete a completed generation blindly.

Sync created component subdirectories as well as the manifest/staging directory where directory durability requires it. Keep platform limitations explicit.

Do not load unchanged unloaded components to save another component. Copy and verify their existing image bytes.

### 8.4 Eliminate duplicate hashing and redundant file opens

For saving, compute the payload digest once while streaming the bytes; place the result in the final envelope. Exclusive immutable access to the payload already prevents concurrent mutation. A prior full payload hash plus a second write-time hash is unnecessary.

For opening, parse the header/descriptor and read the payload through one file handle where practical. Read/hash directly into the final owned payload allocation, then move it into IndexMemory. Never allocate a full second payload copy.

Use checked sizing and fallible reservation where available. Do not introduce unsafe uninitialized-buffer handling solely to remove zero-fill without proof and measurement.

### 8.5 Descriptor and manifest consistency

Add or validate the information required to interpret a component independently:
- Full collection/field/binding identity.
- Text alphabet, normal/skip role, extractor/normalization versions and relevant options.
- Exact scalar/number codec and ordering identity.
- Sort direction, placement, locale, collation version, tie-breaking, and composite constituents.
- Relation declaration/cardinality and target namespace.
- Key/value storage variant and bound identity.
- Coverage state, slot namespace, component revision, and dependency identity.
- Detached provenance for standalone modifications.

Use canonical structured encoding for configuration identities. Include every collection's relevant schema, registered operations, normalization, sort overrides/composites, relation definitions, identity and source-selection settings. Do not hash Debug text, random HashMap iteration, or only the first collection's schema fingerprint.

Persist sufficient semantic configuration to perform checked reopen/mutation, or reject operations whose custom extractor implementation is unavailable. A missing implementation must not silently use a different builtin.

Validate:
- Checked u64-to-u32 component IDs and all slot/length arithmetic.
- Supported descriptor versions and key/value variants.
- Unique IDs, paths, logical component identities, and required catalogs.
- Correct collection membership and expected component roles.
- Dependency existence, role, collection/namespace, and descriptor consistency.
- live_records <= slot_high_water and sensible coverage bounds.
- Embedded descriptor/envelope/manifest agreement.
- No duplicate definitions silently overwriting component lookup entries.
- Rejection of incomplete staging directories.

Do not demand that every unchanged component share one global origin revision; the manifest supplies coherence. Do not use checksums as authentication or claim full structural validation of adversarial, validly-checksummed tree images.

## 9. Required correctness tests

Add focused regressions for the actual failures, not tests that merely mirror new functions. Preserve existing tests. A passing build-state oracle is not evidence of post-mutation correctness.

### 9.1 Semantic oracle

For a small deterministic corpus:
1. Build native and stable independently.
2. Compare complete semantic memberships, metadata, sort order, and relation occurrences.
3. Apply a prepared change to stable and the equivalent source change to the corpus.
4. Rebuild the native oracle from the modified source.
5. Compare through stable public semantic APIs.
6. Save/reopen stable without source access and compare index/catalog primitives again.

RecordSlots legitimately differ after append-only replacement. Compare stable logical identities together with physical path, record range, collection, and occurrence identity; also compare exact slot-based internal invariants separately. Duplicate logical IDs must not collapse during oracle normalization.

### 9.2 Mandatory cases

| Area | Required cases |
| --- | --- |
| Replacement | Unchanged contents; changed typed value; changed length/state; changed raw/custom text; changed sort/composite key; changed relation target; changed record count. |
| Shared files | One file in two collections with unequal record counts; delete/replace; verify each live count, qualified returned refs, and unrelated records. |
| Empty files | Existing zero-record path; deletion; append/move collision; empty-to-nonempty and nonempty-to-empty replacement. |
| Identity | Duplicate logical IDs across files, multiple records per file, generated/path-dependent policies, FileId and slot overflow rejection. |
| Coverage | Unindexed values, incomplete custom binding, complete lengths with incomplete values, unsupported structural state, true complete-empty result. |
| Text | ASCII and Unicode; raw/fallback/custom separation; normalization/version mismatch; both gram kinds; segment boundaries; source verifier where claimed. |
| Sorting | Ascending/descending, equal keys/IDs, missing/null/unavailable, exact decimals and large integers, locale behavior, configured composites. |
| Relations | Duplicate occurrences, position vectors, dangling target, target addition/deletion, source deletion, backlinks and source liveness. |
| Mutation batching | 1/100/1,000 records; singleton/common/dense postings; one decode/encode cycle per changed component/key. |
| Failure | Failure before first change preserves old state; failure after first change blocks reads and saves; restoration failure never exposes an empty tree. |
| Standalone | Fresh save; copy/rename to empty directory; open/mutate/save_as/reopen; no manifest; detached semantics; wrong key/value role. |
| Snapshot | Old generation unchanged; mixed loaded/unloaded save; unrelated payloads remain unread; copy failure; hash mismatch; descriptor/dependency swaps. |
| Publication | Faults after file creation, partial payload, flush/sync, pre-rename, post-rename/parent sync; retry; no incomplete final file. |
| Portability | Lossless native paths; foreign platform rejection; compile/smoke on supported hosts where available. |
| Determinism | P=1/P=2/P>2: identical logical identities, slot assignment, descriptors, results; physical byte equality is optional. |

Include a regression demonstrating that an unchanged replacement previously lost indexed matches. Counts alone do not satisfy this test.

Use test-only deterministic fault injection for mutation and publication. Avoid shared global fault state that races unrelated tests; scope injection to the specific owner/path or serialize only affected tests.

### 9.3 Load and independence tests

- A standalone record/file/path component opens without sibling files.
- A generation member opens directly without manifest access.
- A first query loads only the required index and its actual semantic dependencies.
- All-component loading makes zero insertion calls.
- Saving one component does not read/write unrelated components.
- No source corpus is needed for stored catalog/index primitives.
- No index-payload file is created before an explicit save.
- Wrong format or tree kind cannot initialize a fresh empty tree.

## 10. Measurements and honest reporting

### 10.1 Instrumentation

Separate cheap timing-default statistics from expensive diagnostic instrumentation.

Record actual operations at their call sites. Do not set bitmap_encodes or stable_inserts equal to final entry counts and then present the equality as evidence. In diagnostic builds, wrapper counters must observe insert/get/remove, bitmap encode/decode, source reads/parses, and relevant bytes.

For unavailable metrics, emit null plus a reason. The original report's zero source reads and zero admission timing must not be repeated as measured facts when profiling was disabled.

Record:
- Discovery, preparation, metadata admission/routing, per-owner merging, sorting-key preparation, stable insertion, encoding, and final transient-state destruction.
- Full source-build wall time including all preparation and release needed for readiness.
- Process CPU time separately from overlapping worker elapsed sums.
- Worker utilization and ready-but-unscheduled jobs.
- Per-component entry count, build duration, memory length, capacity, and role.
- Posting memberships, observed encodes/decodes, and insertion counts.
- Resident and high-water RSS before validation/export/warmup.
- Owned stable bytes versus native accumulators, shared metadata, scratch, and allocator-retained memory.
- Snapshot writing/hashing/syncing, file counts, and actual bytes.
- Manifest/header-only open and first payload load separately.
- Mutation preparation, application, byte amplification, and snapshot growth.
- Current memory sizes after mutation, not stale build-time or last-saved sizes.

Native accumulators are dropped before the existing direct builder returns. Therefore, elevated readiness RSS must not automatically be called retained live native indexes. Measure live ownership, Vec capacities, and allocator retention separately. Peak overlap and resident memory are different findings.

### 10.2 Fair comparisons

Keep identical:
- Corpus and source fingerprints.
- Complete schema/registered bindings and sort/relation settings.
- Alphabet and normalization.
- Worker budget and CPU affinity.
- Release/compiler/dependency settings.
- Output consumption and query semantics.
- Filesystem-cache policy.

Build executables before timing and run them directly. Keep each executable's commit, lockfile, compiler, feature flags, and hash. Verify provenance rather than trusting directory names.

Use the actual Musenalm profile. No reduced-schema comparison may be called full-workload parity. Keep separate synthetic cases for relations/composites and other features absent from Musenalm.

Report native, unmodified prototype, and repaired candidate. Preserve old result files; store repaired results in a new task-specific directory.

### 10.3 Required campaign

1. Before repair: remeasure native and original prototype with at least three paired P=8 samples if the corpus is available.
2. Scheduling-only commit: at least three paired original/candidate samples and one full component trace.
3. After ownership/codec work: localized component measurements and a matched whole-build check. Do not run a full campaign for every minor edit.
4. Final correctness-passing candidate:
   - Exploratory P=1, P=2, and P=min(8, available_parallelism).
   - At least five alternating native/candidate fresh-process ASCII pairs at the selected matched worker count.
   - At least three Unicode pairs if the same corpus/configuration is supported. A slow ASCII result does not excuse untested Unicode correctness.
   - The original synthetic workload with 100,000 distinct scalar keys, at least one million memberships, a frequent gram, and variable catalog entries.
   - A mixed-size component fixture specifically exposing scheduling imbalance.

For final stage summaries and RSS, report medians/ranges across the same run set. Do not combine build-time medians with unexplained last-run stage/memory values.

### 10.4 Query and mutation benchmarks

Measure matched native and stable primitives for point, bounded range, length/state, FTS candidates, sorting/composites, and relations on applicable fixtures.

- Separate first component load, first dependency load, and fully resident operation.
- Preload both index and live-domain dependency before resident measurements.
- Use repeated batches to amortize timing overhead, consume results, and report median/p95 with sample count.
- Do not enumerate all values before first-load timing and call it a cold lookup.
- Do not swallow errors into zero matches.
- Compare the same result ownership/consumption. Report unavoidable stable payload decoding separately.
- Do not compare a candidate returning counts with a native operation allocating all results, or vice versa.

Mutation batches must include correct new facts. Benchmark 1/100/1,000 records, shared paths, zero-record cases, and repeated replacement. A replacement that updates only catalogs is not a mutation performance result.

### 10.5 Targets and bounded optimization

Retain the original experiment targets:

| Measurement | Target |
| --- | --- |
| Full source-build median | Native parity target; <= 1.10x is provisional near parity |
| Resident RSS | <= 1.40x native |
| Peak RSS | <= 1.60x native |
| Resident primitive families | Report individually; repeated material > 2x regression blocks general parity |
| Reopen | No reinsertion; include actual I/O/hash/dependency costs |
| Correctness | No semantic differences within declared scope |

Add a repair-success assessment independent of native parity: how much each isolated change improves or regresses the original prototype.

This document supersedes the old instruction to stop after one localized repair above 2x native. Complete all mandatory correctness, ownership, scheduling, and evidence fixes here even if the candidate remains slow.

After these bounded repairs, do not start another database rewrite or unbounded tuning campaign. Deliver one of:
- Correct prototype meeting the stated gates.
- Correct repaired prototype with measured residual performance rejection.
- Incomplete/blocked repair, with exact unmet requirements and cause.

A number below a threshold is not enough if correctness, coverage, ownership, or evidence is missing. Do not rename thresholds after seeing results. Do not claim the estimated 1.679 s scheduling saving was achieved without a rerun.

## 11. Implementation phases and commits

Keep changes reviewable. Each phase has a stopping condition.

| Phase | Deliverable | Verification before proceeding | Suggested commit |
| --- | --- | --- | --- |
| 0 | Provenance, findings checklist, baseline commands, regression fixtures | Reproduce identified semantic failures; retain original measurements | Record stable-index repair baseline and regression cases |
| 1 | Independent component scheduling and trace reporting | Scheduling-only comparison, ID determinism, catalog overlap | Schedule independent stable-index components across workers |
| 2 | Complete prepared-file lifecycle, batching, coverage and invalidation | Unchanged/changed replacement oracle, shared/empty paths, failure tests | Repair stable-index mutation and query semantics |
| 3 | Atomic standalone/generation publication and metadata validation | Fault/retry tests, source-free standalone and lazy generation proofs | Repair stable-index publication and descriptor consistency |
| 4 | Typed component storage and versioned codecs | All storage roles round-trip/mutate; exact numeric/sort ordering; old-version rejection | Specialize stable-index keys and storage codecs |
| 5 | Owned batch routing, parallel owner merge, consumed conversion, no discarded sort orders | No complete borrowed native graph on primary path; ownership and stage evidence | Consume native contributions into independent stable indexes |
| 6 | Query-copy removal, remaining measured local cleanup, composite support if not already completed | Full semantic fixtures and matched primitive checks | Complete stable-index query and construction repairs |
| 7 | Final workspace checks, matched benchmark campaign, findings dispositions | No unreported functional gap; correct classification | Report stable-index repair correctness and performance |

Regression tests may be introduced with the relevant fix if committing deliberately failing tests would violate repository workflow. Before fixing, record a focused reproduction against the original revision.

Do not postpone lifecycle correctness to the final report. Do not mark a phase complete because a missing requirement was moved into an issue document.

If using subagents, establish mutation/storage interfaces first. Suitable disjoint work after that point is construction scheduling/ownership, persistence/codecs, and test/benchmark harness. The supervisor must resolve shared interfaces and perform the complete validation.

## 12. Required checks and outputs

Run repository-required formatting/lint checks and the following against the integrated repair:

    cargo check --workspace --all-targets
    cargo test --workspace
    cargo check -p objekte-search-core --all-targets --features ic-stable-prototype
    cargo test -p objekte-search-core --features ic-stable-prototype
    cargo check -p objekte-search-core --all-targets --features ic-stable-prototype,detailed-profiling
    cargo test --workspace --features objekte-search-core/ic-stable-prototype

Run feature-off performance checks for any shared native helper refactor. Do not make native master slower merely to improve the candidate/native ratio.

Run supported-platform compile/smoke checks where infrastructure permits. Report unavailable hosts/targets accurately; source portability is not executed platform evidence.

If the corpus or benchmark host is unavailable, complete independent code/test work and report performance as unverified. Never replace missing corpus measurements with invented values.

Store reports under:
- docs/ai/generated/ic-stable-index-prototype-repair/
- bench/results/ic-stable-index-prototype-repair/

Required outputs:
- repair-report.md: outcome, remaining limitations, and concise explanation of gains/losses.
- findings-disposition.md: R01-R20, code locations, regression test, measured result or verified disposition.
- campaign-manifest.json: all source/executable/environment/configuration identities.
- comparison.json: native/original/repaired timings, ratios, uncertainty, gate classification.
- Per-run build.json, memory.json, components.json, persistence.json, primitives.json, mutations.json, validation.json.
- Scheduling-only trace and final trace.
- Stage/operation counters with unavailable values represented explicitly.
- Schema/corpus identity and exact commands.
- Completion report appended to the task document per repository workflow.

Do not commit source corpus copies, binaries, or generated index images. Do not overwrite the original experiment's evidence.

## 13. Final acceptance checklist

The supervising agent must answer each item explicitly:

1. Is each logical index still independent, RAM-backed, and individually saveable/openable?
2. Is ic-stable-structures the actual mutable storage after both build and reopen?
3. Do expensive catalog jobs actually run concurrently when workers are available?
4. Does the primary builder move fragments and consume native indexes instead of retaining a complete borrowed native graph?
5. Is field/index merge work outside the serial metadata coordinator?
6. Are discarded native sort orders and universal stored key tagging removed?
7. Does an unchanged file replacement preserve all semantic query results?
8. Are changed values, text, sort/composite keys, and relations correctly represented after replacement and reopen?
9. Do shared/empty-file paths, collisions, per-collection counts, and checked IDs behave correctly?
10. Are incomplete coverage and unsupported predicates distinguishable from complete-empty results?
11. Do queries reuse live domains without copying whole cached bitmaps?
12. Are mutation writes grouped by component/key, with observed counters?
13. Can no partial mutation be queried or saved as valid?
14. Are standalone files and generations published atomically within the documented platform contract?
15. Do save failures preserve prior files, tree ownership, and honest dirty/provenance state?
16. Can descriptors independently identify text/ordering/key codecs and validate dependencies?
17. Are load operations free of reinsertion and unrelated payload reads?
18. Do the benchmark boundaries include all readiness work and correct lifecycle facts?
19. Are measured improvements separated from estimates and unverified claims?
20. Is the final result correctly classified, with master unchanged and the repair branch pushed?

## 14. Source references

Reviewed code and evidence are pinned to 530f4037f01bcd38f5027042e54676b543fdd230.

- [Original implementation report](https://github.com/Simon-Martens/objekte/blob/530f4037f01bcd38f5027042e54676b543fdd230/docs/ai/generated/ic-stable-index-prototype/prototype-report.md)
- [Matched comparison](https://github.com/Simon-Martens/objekte/blob/530f4037f01bcd38f5027042e54676b543fdd230/bench/results/ic-stable-index-prototype/ascii-p8-direct/comparison.json)
- [Candidate-1 component trace](https://github.com/Simon-Martens/objekte/blob/530f4037f01bcd38f5027042e54676b543fdd230/bench/results/ic-stable-index-prototype/ascii-p8-direct/candidate-1/components.json)
- [Component scheduling and borrowed construction](https://github.com/Simon-Martens/objekte/blob/530f4037f01bcd38f5027042e54676b543fdd230/crates/core/src/stable_index/build.rs#L326)
- [Source builder and serial admission](https://github.com/Simon-Martens/objekte/blob/530f4037f01bcd38f5027042e54676b543fdd230/crates/core/src/stable_index/source_build.rs#L260)
- [Native admission logic](https://github.com/Simon-Martens/objekte/blob/530f4037f01bcd38f5027042e54676b543fdd230/crates/core/src/build.rs#L2531)
- [Native sort finalization](https://github.com/Simon-Martens/objekte/blob/530f4037f01bcd38f5027042e54676b543fdd230/crates/core/src/sorting.rs#L791)
- [Storage types and bitmap encoding](https://github.com/Simon-Martens/objekte/blob/530f4037f01bcd38f5027042e54676b543fdd230/crates/core/src/stable_index/memory.rs)
- [Universal key codec, descriptors and file writer](https://github.com/Simon-Martens/objekte/blob/530f4037f01bcd38f5027042e54676b543fdd230/crates/core/src/stable_index/codec.rs)
- [Lifecycle mutation](https://github.com/Simon-Martens/objekte/blob/530f4037f01bcd38f5027042e54676b543fdd230/crates/core/src/stable_index/mutation.rs)
- [Query coverage and live filtering](https://github.com/Simon-Martens/objekte/blob/530f4037f01bcd38f5027042e54676b543fdd230/crates/core/src/stable_index/posting.rs)
- [Ownership, failed state, snapshots and live cache](https://github.com/Simon-Martens/objekte/blob/530f4037f01bcd38f5027042e54676b543fdd230/crates/core/src/stable_index/mod.rs)
- [Manifest validation](https://github.com/Simon-Martens/objekte/blob/530f4037f01bcd38f5027042e54676b543fdd230/crates/core/src/stable_index/snapshot.rs)
- [Benchmark adapter](https://github.com/Simon-Martens/objekte/blob/530f4037f01bcd38f5027042e54676b543fdd230/crates/core/examples/bench_ic_stable_index.rs)
- [Campaign driver](https://github.com/Simon-Martens/objekte/blob/530f4037f01bcd38f5027042e54676b543fdd230/bench/run_ic_stable_index_prototype.py)
- [Rayon indexed iterator job-size control](https://docs.rs/rayon/latest/rayon/iter/trait.IndexedParallelIterator.html#method.with_max_len)
- [Pinned stable B-tree implementation](https://docs.rs/crate/ic-stable-structures/0.7.2/source/src/btreemap.rs)

The scheduling estimates, prioritization, and repair architecture are engineering conclusions from this review. They are not measured performance guarantees.


## Implementation report

- Task identifier: `ic-stable-index-prototype-repair`
- Completion date: 16 November 2026
- Base for this continuation: `1022768be3d625be7acec28da38defd831b85fe1`
- Reviewed prototype: `530f4037f01bcd38f5027042e54676b543fdd230`
- Native baseline: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`
- Integration branch: `integration/ic-stable-index-prototype-repair`
- Integration worktree: `/home/simon/source/objekte-integration-ic-stable-index-prototype-repair`

### Summary

The four residual findings are fixed. The direct builder no longer constructs a
borrowed native `CollectionState` graph: completed owners are moved into owned
`OwnedCollection` values and then into component-job `JobData`, where posting,
gram, sort and relation state is released as each component job finishes.
Custom postings, raw/fallback/custom bigrams, sort keys and relation owners now
merge outside the metadata coordinator in explicit bounded extraction waves.
Unavailable detailed diagnostics serialize as `null` with reasons, actual source
reads/bytes are reported, and loaded component statistics expose live memory
length and Vec capacity. Matched native/stable primitive distributions now cover
point, range, length, state, FTS candidates, sorting and relations, with zero
result-membership mismatches in the final run.

### Implementation commits

- `956648ae3075a85fbf76d3e4ecab8e349103df84` — owned native-owner conversion,
  bounded parallel owner waves, truthful diagnostics and matched primitive
  harness.
- `5c5c5609c561` — final ASCII/Unicode/synthetic campaigns and matched primitive
  evidence.
- `608d95f76a3e` — updated repair report, findings disposition, campaign
  manifest and comparison.
- This completion-document commit records the task lifecycle move and report.

### Validation

Commands run:

```text
cargo check --workspace --all-targets
cargo test --workspace
cargo check -p objekte-search-core --all-targets --features ic-stable-prototype
cargo test -p objekte-search-core --features ic-stable-prototype
cargo check -p objekte-search-core --all-targets --features ic-stable-prototype,detailed-profiling
cargo test --workspace --features objekte-search-core/ic-stable-prototype
```

All checks passed except
`backend::open::tests::incremental_catalogue_preserves_non_utf8_file_paths` in
`objekte-desktop`. The same test fails alone in the untouched native baseline
worktree at `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`; all other workspace tests
pass.

Full Musenalm direct-build validation passed with zero semantic differences:
148,486 files/records, 152,544 posting keys, 2,814,411 posting members,
25,477 gram keys, 56,275,908 gram memberships and 566,768 sort entries.

### Measurements

- ASCII P=8, five paired fresh processes: native 1.573 s, original 4.717 s,
  repaired 3.174 s (2.02x native, 32.7% better than original).
- Unicode P=8, three paired fresh processes: native 1.878 s, original 5.323 s,
  repaired 3.661 s (1.95x native, 31.2% better than original).
- ASCII resident/peak RSS: 1.90x/1.91x native. Unicode: 1.81x/1.83x native.
- Synthetic P=1/2/8: 4.36x/3.94x/2.14x native.
- Final component trace: 1.704 s ASCII / 1.635 s Unicode makespan, 4.91/5.04
  average concurrent jobs.
- Matched primitives: 503 rows x 200 repetitions, zero result-membership
  mismatches. Stable/native ratios range from 2.02x for FTS candidates to 507x
  for configured sort pages.

### Deviations and remaining limitations

- The candidate remains above the resident and peak RSS targets because all
  completed native collections are still finalized before the stable job graph is
  published. Further reduction requires a streaming collection-to-component
  pipeline.
- Stable primitive payload decoding remains materially slower than resident
  native maps, especially sort pages.
- Composite sorts are implemented and tested on a synthetic fixture; the
  Musenalm profile has no composite registrations.
- Validation is Linux-only. Logical deletion still retains dead posting
  memberships; compaction remains outside this repair.

### Worktree cleanup

No subordinate worktrees were created. The integration worktree and branch are
retained for the user. The completed task document is committed in `done/`.
