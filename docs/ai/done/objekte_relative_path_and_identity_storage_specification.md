# Agent specification: root-relative paths and shared record identity storage

Task identifier: `relative-path-and-identity-storage`  
Repository: `Simon-Martens/objekte`  
Decision date: 12 November 2026

## 1. Objective and scope

Implement exactly two changes:

1. Store and return source-file paths relative to the repository root. Remove retained per-file absolute paths and avoid duplicate ownership of identical relative-path data.
2. Remove duplicate ownership of logical record-ID strings between record entries and identity lookup structures.

The user explicitly requests root-relative output, so this includes the library, CLI, TUI and desktop consumers. It is an intentional change to the meaning of returned source-file path fields and `@file.path`; document it consistently.

Do not compact collection-membership vectors, diagnostics, timestamps, record ranges or other file metadata. Do not redesign relation edges, position vectors, sorting storage, postings or bigrams. Do not implement a global string-interning framework, FFF-style chunk compression, a path trie, custom allocation machinery or a new indexing policy. Adapt affected call sites where necessary, without expanding the task into those optimizations.

This task authorizes its bounded ownership changes, the path contract change, necessary snapshot/API adaptations, and focused measurement. Ordinary implementation choices within that scope do not need another approval.

## 2. Base revision and delivery workflow

The reviewed master revision is `737cfb7745ff15915804553ab583af260ac0f2e4`. Inspect current remote master when starting and record the actual baseline. Implement missing work against that baseline; do not overwrite improvements already present.

The separate `index-construction-selected-merge` task may advance master with metadata concurrency, input-buffer reuse, direct logical JSON deserialization and admission diagnostics. Preserve whichever of those changes have landed. This specification does not authorize executing that task, selecting its experiments, or merging the staged-index-construction branch.

Read applicable `AGENTS.md` files and `docs/ai/README.md`. Use one supervising implementer for this bounded task; parallel implementation agents are unnecessary.

- Integration branch: `integration/relative-path-and-identity-storage`.
- Integration worktree: `/home/simon/source/objekte-integration-relative-path-and-identity-storage`, a sibling of the original checkout.
- Keep the original checkout on `master`. Perform implementation, tests, measurements and commits in the integration worktree.
- Push immediately after every commit. Preserve unrelated edits and uncommitted task documents according to repository policy.
- Deliver the tested integration branch. Do not merge into or push changes to master without a separate user command for this task. Authorization granted to another task does not transfer here.

## 3. Required path contract

### Repository root is the reference point

The reference point is the repository root, never the current working directory or a collection's subdirectory. For repository root `/data/archive` and collection source `contents`, the source file `/data/archive/contents/123.json` is represented as `contents/123.json`.

Keep the root once in repository/open configuration. Construct an absolute operating-system path temporarily when reading, writing, revealing or opening a source file. Do not retain that constructed path in each indexed entry, result cache, record tab or serialized result.

Paths must remain valid contained relative paths under the existing containment and symlink policy. Do not weaken validation, allow absolute components to replace the root during joining, or introduce per-query filesystem canonicalization or freshness checks. Preserve existing separator, Unicode and non-UTF-8 behavior through the established native-path and `path_repr::query_path` representations. Do not introduce lossy conversion or a different escaping scheme.

### Query and returned values

Apply the contract to:

- Library search/list/streaming results and their source-file path fields, including `SearchHit.file_path` and equivalent projections.
- The value observed by `@file.path`, its indexed predicates, scan fallback, sorting, returned metadata and highlights where applicable.
- CLI structured output, printed source-file paths, TUI rows, desktop rows/tabs, copy/export operations and machine-readable result serialization.
- Lifecycle result projections and newly added, updated or moved files, not only fresh builds.

Indexed and scan evaluation must agree. Compare `@file.path` with the root-relative query representation. An absolute-looking string literal is an ordinary query literal; do not silently strip the repository root from it or keep a hidden legacy absolute-path comparison index. Update examples and tests that depended on absolute `@file.path` values. Do not rewrite unrelated field values or user queries automatically.

Keep existing result field names unless a rename is unavoidable. Keep repository context outside the per-hit path; consumers that need to open a file resolve the returned path against the owning repository's root. Verify this works when launched from another working directory and with multiple open repositories.

Absolute paths supplied to repository-open configuration or existing file-operation input APIs may still be accepted under their existing contract. Normalize them to the contained root-relative representation at the boundary. The stored/result path contract does not require breaking valid input APIs.

This concerns engine-owned source-file paths. Do not modify absolute path strings contained in user documents, unrelated application settings or external links. Do not claim that an entire snapshot or process can contain no absolute paths: the single root/configuration context remains legitimate.

### Storage changes

Inspect `FileInventory`, `FileEntry`, path lookup/range helpers and `FileCatalog` base/delta state. At the reviewed revision, the inventory owns a relative path in `FileEntry`, clones it into `by_path`, and builds an absolute string for `absolute_paths`.

Required changes:

1. Remove retained absolute-path keys from the base inventory and lifecycle deltas; rename affected fields/helpers to describe their relative semantics.
2. Retain efficient exact lookup and ordered path predicates. An ordered relative-path index may contain file IDs or shared path keys. Do not replace selective lookups with a full file scan.
3. Share identical native relative-path storage between file entries and lookup keys. Prefer a standard immutable shared representation or an existing repository storage abstraction. Do not use self-referential borrowed keys or unchecked raw pointers.
4. Where native paths and escaped query keys genuinely differ, preserve both necessary representations or compute one without retaining it. Count them separately. Do not claim deduplication merely because two buffers contain equivalent text in different encodings. Avoid unnecessary duplicate payloads for ordinary paths where the representation permits sharing.
5. Preserve stable file IDs, deterministic discovery/admission order, overlapping collections, byte-range provenance and file-to-record mappings.

Deletion, moves and updates must preserve existing published views. Share immutable payloads; a move must publish a new relative path and update current lookup entries without mutating a path used by an older view. Keep existing tombstone and cache invalidation behavior.

## 4. Required record-ID ownership change

### Numeric slots and logical IDs are different

`FileId` and `RecordSlot` are numeric handles. At the reviewed revision, logical `RecordId` wraps `String`, and finalization builds `id_lookup` using `r.id.clone()`. IDs that look numeric still have string identity semantics.

Preserve exact IDs, including `"001"` versus `"1"`, arbitrary textual intrinsic IDs, generated/fallback IDs, namespaces, duplicate detection, ordering and serialization. Do not convert IDs to integers or replace user-visible IDs with internal slots.

### Bounded implementation

Use one immutable payload for a logical record ID shared by its record entry and retained identity lookup keys. Prefer a shared string representation such as `Arc<str>` behind `RecordId`, or a comparably small existing abstraction. A process-global interner and a new ID-dictionary architecture are unnecessary for removing this clone.

Keep content-based equality, ordering, hashing and borrowed string lookup. Pointer identity must not determine record equality. Preserve convenient string access at API boundaries and serialize the same external string values. If necessary, enabling an existing serialization dependency's shared-pointer support is within scope; do not upgrade unrelated dependencies or add a new interning package.

Apply the ownership rule to fresh construction, snapshot restore and retained lifecycle identity lookup structures. A decoder that allocates a separate string for both the record and its lookup key has not completed this fix. Rebuild lookup keys from shared record IDs during restore, or otherwise restore the intended sharing.

Do not expand this into deduplicating every string in posting dictionaries or relation edges. Relation consumers may require mechanical adaptation to the revised ID accessors, but relation representation changes are excluded. Likewise, owned strings created when serializing a result are distinct from duplicated resident index storage.

## 5. Persistence and integration

Audit library metadata projection, source access, snapshot encode/decode, lifecycle mutation/restore, CLI/TUI formatting and desktop source-opening actions. Search current code rather than assuming all consumers are in the files named above.

- Keep native path access correct; temporary absolute paths belong at the operating-system boundary.
- Update cached/materialized metadata and configured `@file.path` sort keys to the new relative contract. Removing the inventory's absolute-path map alone is insufficient.
- Update documentation and output fixtures together. Describe the intentional path behavior change; ID values and other query semantics remain unchanged.
- Preserve existing snapshot compatibility where straightforward, rebuilding derived path/ID indexes and sharing on load. If stored derived absolute-path values or representation changes make compatibility unsafe, bump the relevant snapshot/version discriminator and follow the existing incompatible-cache rebuild/error policy. Never silently load an index with mixed absolute and relative semantics. Explicitly test the chosen migration or rejection behavior, including required-load mode.
- Do not add search-time revalidation, source metadata checks, global index reconstruction on each mutation, or a new cache layer.

## 6. Focused correctness validation

Extend existing tests for meaningful gaps; avoid duplicating covered cases. Verify:

1. Fresh-build indexed results, forced scans, list results and snapshot-restored results return the same root-relative source path. Include two directories containing the same filename and a nested collection.
2. `@file.path` equality, ordered comparisons and supported text predicates use the same relative value in indexed and scan execution; path sorting retains deterministic ties. Absolute-looking literals receive no implicit conversion.
3. Moving, deleting, adding and updating a file update current path lookup and output correctly; previous retained views remain consistent. Include records sharing a multi-record source file and overlapping collections where already supported.
4. Native unusual paths retain existing reversible behavior, including percent/backslash characters and invalid UTF-8 on supported platforms. Existing containment checks still reject escaping paths.
5. CLI/TUI/desktop display or export relative paths while source open/reveal/read actions resolve against the correct root, independently of the process working directory.
6. String ID equality, ordering, duplicate rejection, relation resolution, generated IDs and `"001"`/`"1"` behavior remain unchanged. Verify shared ownership of the resident entry/lookup ID payload after build and restore using an implementation-appropriate focused test or allocation measurement.

Run the workspace suite, Clippy and relevant native desktop checks using repository-documented commands. Run the existing query suite; all non-path results must match baseline. For changed path queries/output, assert the explicit new semantics rather than counting an intentional root-prefix difference as an unexplained mismatch.

## 7. Simple memory and performance measurements

Use existing benchmark tooling and the configured Musenalm corpus. Record baseline/candidate SHAs, source/schema fingerprints, release settings and worker counts. If master advanced, compare against the actual pre-task baseline, not historical timings from another task.

After a warm-up, run three alternating baseline/candidate fresh-process builds per alphabet, with profiling disabled. Record build time, post-build RSS, peak RSS and snapshot size. Measure representative exact/range path lookup, ID lookup, source materialization and mutation operations; use existing harnesses and enough repetitions to avoid timer noise. Investigate material regressions instead of exchanging memory for a new scan or repeated path reconstruction in hot loops.

Add a small opt-in diagnostic breakdown for the changed storage, outside timed sections:

- File table length/capacity and entry size; native relative-path payloads; path lookup containers; distinct escaped query-key payloads, if retained.
- Record table storage; unique ID payload bytes; identity lookup containers and shared references.
- Separately report estimated live ownership and process RSS. Count shared payloads once. Do not add overlapping categories or call serialized bitmap sizes a complete heap measurement.

Correct the benchmark/report label: the historical 83.68 MB `FileInventory` estimate excludes record tables and record-ID lookup storage. Record this clarification in the new report; preserve historical raw results. A full repository-wide memory-accounting redesign is out of scope.

Expected acceptance: retained absolute per-file path storage is eliminated, duplicate entry/lookup path and ID payload ownership is removed as specified, measured affected storage decreases, and important lookup/query/build behavior has no unexplained material regression. Do not promise a particular RSS reduction: allocator-retained pages can obscure live-byte savings. If ID savings are small, report them honestly and keep the implementation correspondingly simple.

## 8. Commit sequence and completion

Use a small sequence of reviewable commits, each pushed immediately:

1. Root-relative path contract, ownership, lifecycle/persistence adaptations and consumer updates, with focused tests.
2. Shared record-ID ownership across build/restore/lifecycle and its tests.
3. Measurement/reporting corrections, final validation and completed task report. Put required diagnostic code earlier if needed to measure the preceding commits.

Write `docs/ai/generated/relative_path_and_identity_storage_results.md` with the storage design, path compatibility change, validation commands/results, raw measurement locations, before/after component bytes, RSS/timing results and remaining limitations. Explain any necessary distinction between native path storage and escaped query keys.

Move this specification to `docs/ai/done/` and append the implementation report according to repository policy, preserving user edits. Commit and push the completed specification and report on the integration branch. Report the tested final commit and leave master unchanged for this task.

## Implementation report

- **Task and completion:** relative-path-and-identity-storage, 12 November 2026. Actual base commit: 64a09886c5fed963fb0f53df779c6ad6c1a008f1. Integration branch: integration/relative-path-and-identity-storage. Integration worktree: /home/simon/source/objekte-integration-relative-path-and-identity-storage. Master remains unchanged.
- **Implementation:** 7e71100040 makes source path storage and @file.path index semantics root-relative and shares ordinary path payloads; 72ba7cfd07 shares logical record-ID payloads between records and identity lookups, including restored lifecycle segments. Both commits were pushed immediately. There were no delegated subtasks or subordinate branches.
- **Validation:** cargo check --workspace --locked; cargo test --workspace --locked; cargo clippy --workspace --all-targets --locked; and cargo test --locked -p objekte-desktop --example environment_validation all passed. The extended Musenalm suite passed all 194 cases in ASCII and Unicode with identical ordered result digests and source-work counters. Tests cover path index/scan parity, unusual native paths, snapshots, lifecycle views, CLI working-directory independence, ID semantics, and shared ownership.
- **Benchmarks:** Clean release binaries from the base and 72ba7cfd07 ran one warm-up and three alternating fresh-process build pairs per alphabet, four build workers, profiling disabled. ASCII paired build time improved 1.46%; Unicode improved 3.08%. Estimated affected live storage fell 123,576,752 → 94,304,474 bytes. Snapshot sizes remained equal. Focused path/ID/source and 20-pair lifecycle mutation probes found no material operational regression. Q119 showed a repeatable +1.2 ms (+4.7%) in alternating query blocks, with unchanged planner timing, index-proven matches, and zero source reads; the user directed us to accept the benchmark and finish. Details, commands, raw evidence, and accounting limits are in [the results report](../generated/relative_path_and_identity_storage_results.md) and [raw results](../../../bench/results/relative-path-and-identity-storage/2026-11-12/).
- **Compatibility and limits:** The returned file path and @file.path value intentionally use the root-relative spelling; absolute-looking query literals are not rewritten. Snapshot versions 23/24 reject older derived path indexes in required-load mode or rebuild in IfValid mode. Escaped native paths can retain a second reversible query key. Live-byte estimates omit allocator overhead and unrelated indexes, and process RSS need not equal those estimates. The historical 83.68 MB FileInventory estimate excludes record tables and record-ID lookups. No other approved deviations or open in-scope failures remain.
- **Cleanup:** The temporary baseline instrumentation was preserved as a patch with raw evidence and removed from its control worktree. The integration worktree is retained for review; the task's temporary memory is removed after this completion commit is pushed.
