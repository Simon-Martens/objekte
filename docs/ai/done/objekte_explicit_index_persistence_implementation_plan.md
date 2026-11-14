# Objekte Explicit Index Persistence Implementation Plan

Date: 2026-11-14  
Task identifier: `explicit-index-persistence`  
Implementation branch: `integration/explicit-index-persistence`  
Status: plan for agent execution; no merge or implementation is performed by creating this document.

## 1. Required outcome

First integrate the reviewed hybrid storage and direct snapshot work into `master`. Then simplify the library, CLI, TUI, and desktop application around three separate operations:

1. **Build:** construct a searchable index in memory. This is the default and does not consult or write saved indexes.
2. **Load:** explicitly attempt to open a saved index at one selected location. Persisted index data is always accessed on demand through the existing checked snapshot readers.
3. **Flush:** explicitly write the current complete index publication to one selected location. This saves a generation; it does not enable automatic persistence or change the current index's backing representation.

The three disk locations are the system temporary directory, the repository's `.objekte` directory, and a custom base directory. A location is an argument to loading or flushing, not a session-wide policy that silently combines both operations.

Remove the optional fully resident disk-loading mode and its public API, preparation paths, client controls, options, and active documentation. Preserve native in-memory construction, owned construction buffers, decoded posting caches, and bounded query scratch allocations. They are necessary implementation mechanisms, not the fully resident loading mode being removed.

This plan supersedes previous requirements for automatic index synchronization, flushing on ordinary exit or repository replacement, and selectable disk residency. Source-file reconciliation remains a separate operation and must continue to update the live in-memory index without automatically saving it.

## 2. Baseline and repository workflow

### Reviewed branch state

The remote branches observed while preparing this plan are:

| Branch | Observed commit |
| --- | --- |
| `master` | `da8fde68472b8646a63a05b3b3654c58de877301` |
| `integration/hybrid-index-storage` | `dd3f8feb19df68dd5a5a3ada0d8c99ca91336524` |
| `integration/direct-snapshot-access` | `0c6365145822fffc4631b41bac3a9a436116fa51` |

The reviewed direct-snapshot branch contains the reviewed hybrid branch and was 52 commits ahead of it. Re-fetch and establish actual ancestry before execution; the user may already have merged these branches. Do not reset newer work to these reference commits.

Read `AGENTS.md`, `docs/ai/README.md`, the assigned task, and current task notes before changing the repository. Preserve staged changes, untracked specifications, and unrelated work. Use task worktrees under `/home/simon/source`, with the original checkout remaining on `master`. Push immediately after every task or integration commit, as required by the repository workflow. Do not force-push or delete unrelated worktrees or branches.

### Phase A: integrate the reviewed work into master

This is the first implementation phase. Once the user assigns this plan for execution, this phase implements the requested merge into `master`; it must not be replaced with a proposal to merge later.

1. Fetch the configured remote and record the current local and remote heads and working-tree state.
2. Establish whether the current hybrid branch already contains the direct-snapshot branch. If not, fast-forward it when ancestry permits. If newer independent work has caused divergence, integrate it in the hybrid worktree and resolve conflicts there, preserving both sets of changes.
3. Confirm that the final hybrid result includes the direct snapshot readers, frontend storage controls, and warning cleanup reviewed above.
4. Validate the combined result with the workspace tests and normal workspace checks. If combining previously separate changes, also run the existing focused storage/query smoke checks. Retain the approved benchmark report; do not reopen accepted small latency trade-offs as merge blockers.
5. Merge the reviewed integration result into `master`, preferably by fast-forward. If `master` has diverged, prepare and validate the combined result in a task worktree before advancing `master`.
6. Push `master` normally and record its resulting commit. Updating the original checkout by this authorized merge is allowed; implementation edits and new development commits still belong in task worktrees. If user changes prevent a safe merge, preserve them and report the concrete obstruction instead of stashing, resetting, or discarding them automatically.

Do not combine the subsequent API simplification with this initial merge. The reviewed implementation must first exist on `master` as a recoverable baseline.

### Phase B: implement the simplified contract

Create `integration/explicit-index-persistence` from the resulting `master`, with worktree `/home/simon/source/objekte-integration-explicit-index-persistence`. Perform all subsequent implementation there or in subordinate worktrees. Deliver one tested integration branch for review; the initial authorization to merge the reviewed snapshot work is not an instruction to merge the later API changes into `master` automatically.

One supervisor owns the public API, persistence contract, combined validation, and completion report. After those interfaces are committed, separable CLI/TUI and desktop changes may be delegated to agents on `task/explicit-index-persistence/cli-tui` and `task/explicit-index-persistence/desktop`. A core/storage agent may use `task/explicit-index-persistence/core`. Each agent owns a separate worktree, pushes each commit, and returns tested commits to the supervisor. Do not have multiple agents modify the public API independently.

## 3. Operation contract

| User or library operation | Read a saved index? | Create a durable generation? | Result |
| --- | --- | --- | --- |
| Ordinary build/open from sources | No | No | Searchable in-memory index |
| Explicit load from a selected target | Only that target | No | On-demand readers over the saved generation |
| Explicit load with no saved generation | Only that target | No | Library reports absence; caller may build in memory |
| Explicit flush to a selected target | Inspect destination state as needed | Yes, unless identical snapshot is already current there | Current index remains usable with its existing backing |
| Source reconciliation, add/update/delete, or compaction | No implicit search for stores | No | Publish new live in-memory state |
| Ordinary close, switch repository, or process shutdown | No implicit load | No | Release resources; do not save automatically |

The no-store-access rule does not prohibit reading repository schemas, source files, or ordinary application preferences. It prohibits probing saved index manifests and generations unless a load or flush operation requested that target.

### Explicit loading

- Search exactly the requested target and repository/configuration namespace. Do not search temporary, repository, and custom locations in sequence.
- Open using the existing direct snapshot tables and on-demand segment mappings. Do not introduce owned copies of entire persisted segments or eager prefaulting that recreates full residency under another name.
- A missing namespace or missing committed generation is a normal not-found result. A damaged manifest, permission error, incompatible configuration, or I/O failure is a typed error, not a cache miss.
- Loading must not build an index internally, retire an old store, write a replacement, run garbage collection, or enable automatic synchronization. Move the existing load-and-rebuild recovery behavior to explicit caller decisions.
- Loading must not write validation stamps as a hidden persistence operation. It may consume existing valid stamps. Produce or refresh stamps during an explicit flush, using the existing trustworthy validation boundaries; do not weaken checks to avoid writes.
- A valid saved index must be loadable from a read-only store. Preserve reader pinning and file-handle/lock protection without requiring new generation, namespace, or sidecar creation merely to read it.
- Source reconciliation may begin after snapshot search becomes available, according to the established caller behavior. Reconciliation must not turn into a disk write. Keep snapshot readiness and source freshness distinct.

### Explicit flushing

- The caller supplies the destination on every flush call. Remembering a previous destination can prefill a user dialog, but cannot authorize another write.
- Capture one immutable, complete index publication and persist exactly that publication. Reject flushing an incomplete initial build with a typed state error; clients can wait for completion and then issue their already requested flush once.
- Do not hold the publication lock throughout serialization or disk I/O. Concurrent queries keep using pinned views. Updates published during the flush are not included retroactively.
- Return the publication revision actually saved. If the live index advances during the operation, report that newer in-memory changes remain unsaved.
- Preserve the existing fast sealed-section persistence and direct lookup encoding. A memory-built index stays memory-backed after flushing. A loaded index keeps its current mappings and in-memory mutation overlay; saving to another target must not reopen or rebuild it.
- Flush is an explicit snapshot save, not a request to discover or parse sources again. Clients may wait for source reconciliation before invoking it when their operation promises a refreshed snapshot.
- A failure or cancellation before commit keeps the previous saved generation and the live index usable. After the atomic commit point, report the committed outcome accurately even if a late cancellation arrives.

## 4. Library API

Keep the existing `Index::build` and registered-source/incremental variants as the default memory-only construction entry points. Replace the storage-policy interface with a disk-target type and separate load/flush methods. The following is a contract sketch, not a requirement to duplicate existing builder types:

```rust
pub enum IndexStorageTarget {
    SystemTemporary,
    RepositoryDirectory,
    CustomDirectory(PathBuf),
}

impl Index {
    // Existing default construction contract remains memory-only.
    pub fn build(/* root, definitions, options */) -> Result<Self, IndexError>;

    // Ok(None) means no committed saved index at the selected target.
    // This method never falls back to construction or persistence.
    pub fn try_load(
        /* root, definitions, options, registered sources, */
        target: &IndexStorageTarget,
    ) -> Result<Option<Self>, IndexError>;

    // Snapshot the current complete publication and save it explicitly.
    pub fn flush_to(
        &self,
        target: &IndexStorageTarget,
        /* existing cancellation/progress arguments where appropriate */
    ) -> Result<IndexFlushReport, IndexError>;
}
```

Use normal explicit parameters or the existing source registry builder rather than the comments in this sketch. Preserve builtin-source convenience methods and custom adapter support. Do not require a storage destination to construct an index.

`IndexFlushReport` must identify the saved in-memory revision, destination, committed store generation, whether the operation was unchanged, and relevant work/error information. Distinguish the current publication revision from the durable store generation; their numbers are not interchangeable.

Remove `IndexResidency` rather than leaving a one-value residency selector. Remove `IndexStoragePolicy` as a combined load/build/autosave contract. Replace or remove `open_with_storage`, `start_incremental_build_with_storage`, public storage attachment, `set_sync_storage`, and their shutdown notifications. Reuse internal persistence helpers where useful, but no public entry point may silently restore the removed automatic behavior. Audit the legacy cache methods and `Repository` entry points so they cannot bypass the new default; existing test-only historical helpers need not become new production APIs.

Loaded-origin information and last-flush information are distinct. Record both without making either an automatic write destination. Preserve bounded caches and immutable backing ownership so pinning, lazy access, and concurrent queries remain safe.

### Location and identity resolution

Retain the established managed namespaces:

| Target | Destination rule |
| --- | --- |
| System temporary | Existing per-user Objekte namespace beneath the operating system temporary directory |
| Repository | Managed namespace under `<repository>/.objekte/indexes` |
| Custom | Existing managed Objekte namespace beneath the explicitly selected custom base directory |

Use a stable temporary namespace so an explicit later load can find it while the OS retains it. Do not replace this with a new random directory on each operation. Temporary storage is not guaranteed to survive OS cleanup.

A custom path is a base directory, not a raw segment filename. Resolve relative paths once against the invocation's working directory. Preserve existing custom-base validation and unrelated files. Memory construction does not create any of these directories; explicit flush creates only the destination's owned descendants.

**Configuration identity must not depend on residency or whether construction happened before a destination was selected.** The current policy prepares source exclusions before deriving the store identity. Audit that coupling carefully: build-in-memory → flush-to-repository → explicit-load-from-repository must resolve the same logical configuration and stored index.

Separate logical source/schema/adapter identity from physical destination resolution. Apply established managed-namespace discovery exclusions consistently across build and load. Never exclude an arbitrary custom base containing real source documents, change the indexed collection set during flush, or read saved indexes just to decide how a default build works. Preserve schema files, saved queries, and application preferences under `.objekte`.

## 5. Existing destination and generation semantics

The default overwrite behavior is **atomic publication of a newer complete generation**. This satisfies replacement without rewriting files that active mmap readers may still use.

1. Resolve and verify the selected managed namespace and repository/configuration identity.
2. Observe its current committed generation using the existing writer coordination protocol.
3. Choose a new store generation from the destination's current sequence, not from the index's in-memory revision. Handle overflow explicitly.
4. Write or safely reuse the immutable segments, catalog, domains, filters, and validation information for the publication being saved.
5. Commit the manifest atomically with the expected previous generation. Honor the established durability ordering and writer-lock protocol.
6. Retain old files for pinned readers; reclaim only through the existing ownership-checked retirement mechanism.

### Cases that must work

| Destination state | Required behavior |
| --- | --- |
| No committed index | Create the first complete generation on explicit flush |
| Existing compatible index, freshly built index being flushed | Publish the newly built snapshot as a newer generation; records absent from the new build disappear from current results |
| Existing compatible index, loaded index changed in memory | Publish the current live publication, reusing unchanged immutable segments where valid |
| Identical publication already current at this destination | Permit an unchanged result after verifying destination identity and generation |
| Load from A, flush to B | Save a self-contained generation in B; retain A and the current reader backing |
| Another writer commits concurrently | Return a typed conflict or use a documented safe retry before committing; never silently overwrite an unexpected generation |
| Valid owned old-format artifact or incompatible configuration | Loading returns a typed incompatibility; explicit flush may replace recognized obsolete artifacts using the managed retirement path or create the correct configuration namespace |
| Corrupt or ownership-ambiguous existing artifact | Preserve it and report a typed error unless the existing format/ownership evidence is sufficient for the managed replacement path; do not delete arbitrary files |

New generation does **not** mean unioning the existing saved records with the new index. The flushed publication is the complete authoritative snapshot. Do not resurrect deletions or duplicate records from a previous build.

A newly built index may start at revision 1 while the target store is already at generation 20. That must be a supported replacement, producing a later durable generation. Likewise, two independent in-memory builds with the same revision number are not the same snapshot. An unchanged shortcut needs publication/lineage identity, destination/store identity, and a verified current generation, not equality of numeric revisions alone.

For an index loaded from the same target, retain its expected durable generation and detect a stale-writer conflict if another process has since replaced it. For a fresh build explicitly flushed over an existing store, inspect the destination as part of that requested replacement and protect the resulting commit against intervening writers.

When copying to another target, reuse or copy immutable sections with the existing file-reference translation. The new target must remain loadable after the source target is removed. Do not persist borrowed process addresses or external catalog references. Do not rewrite an existing mapped segment in place.

## 6. CLI contract

Replace the combined `--index-storage`, `--index-directory`, and `--index-residency` interface with operation-specific options:

| Option | Meaning |
| --- | --- |
| `--load-index temporary\|repository\|custom` | Explicitly attempt to load from this location, always on demand |
| `--load-index-directory DIR` | Custom base for the load operation; valid only with `--load-index custom` |
| `--flush-index temporary\|repository\|custom` | Explicitly save one completed publication to this location |
| `--flush-index-directory DIR` | Custom base for the flush operation; valid only with `--flush-index custom` |

Loading and flushing are independent and can name different locations. The two custom-directory arguments must not be conflated. The earlier historical `--load-index` spelling took different inputs; update its rejection/migration code deliberately so the new enumerated-location contract is unambiguous.

Examples:

```sh
# Build in memory and query. Existing saved indexes are ignored.
obj --path ./data --fts Goethe

# Build in memory and explicitly save, without a query.
obj --path ./data --flush-index repository

# Explicitly attempt to load; do not save on exit.
obj --path ./data --load-index repository --fts Goethe

# Load one location and explicitly save the resulting publication elsewhere.
obj --path ./data --load-index temporary \
    --flush-index custom --flush-index-directory /mnt/index-storage --fts Goethe

# Use independent custom locations.
obj --path ./data --load-index custom --load-index-directory /mnt/index-source \
    --flush-index custom --flush-index-directory /mnt/index-destination

# Open a saved index in the TUI. Later saves require an explicit action.
obj --path ./data --load-index repository --tui
```

### Execution rules

- Without `--load-index`, use normal memory construction even if a saved index exists.
- With `--load-index`, call the explicit library loader. On clean absence, report that no saved generation was found and build in memory. This fallback is a client decision and does not imply a flush. Other load failures remain errors; advise the user to omit the load option to rebuild intentionally.
- Without `--flush-index`, do not save during opening, reconciliation, query completion, ordinary exit, Ctrl-C, or repository replacement.
- A finite command with `--flush-index` performs one flush after initial construction and any reconciliation it promises to complete, before query output and successful terminal completion. This makes the requested save independent of a downstream consumer closing its query-output pipe. A flush failure produces a nonzero exit and no false saved/success status.
- With both flags, load if available, otherwise build in memory, then perform the explicitly requested flush once. The destination option does not itself authorize loading from that destination.
- An explicit load or flush permits an index-only invocation without a query. Bare `obj` can retain its current missing-query behavior; it must not gain unsolicited disk operations.
- `--tui --flush-index ...` is a one-time save request after startup construction/reconciliation completes. Dispatch it in a worker, show progress, and never treat it as a persistent autosave setting.
- Reject removed storage/residency options with specific migration guidance. Do not silently accept `full`, ignore it, or retain a hidden full-residency alias.
- Update help, completion generation, CLI parser tests, example scripts, and benchmark argument plumbing. Finite CLI error/exit codes and query-result semantics remain consistent.

Revise structured status to identify build versus explicit load, loaded origin, current publication revision, last saved publication, destination/store generation, and flush outcome. Remove full-residency readiness and automatic-synchronization semantics. If documented NDJSON fields are removed or change meaning, version that protocol change explicitly and update its tests and consumers; do not silently repurpose an existing generation field.

## 7. Desktop and TUI behavior

Use conventional operations and labels. The application-facing action can say **Save index…**, with documentation explaining that it invokes the library flush operation. Keep source-file saving, query bookmarks, and index persistence visibly distinct.

### Desktop

- Ordinary repository opening defaults to **Build in memory**.
- The opening dialog may offer **Try loading saved index** with Temporary, Repository, or Custom selection. The default must not be changed by a remembered previous disk mode. Remembered paths may prefill the explicit operation only.
- Provide **Open saved index…** and **Save index…** actions for an active repository. Both use the existing dialog/menu components and the three destination choices. They replace **Change storage**, **Synchronize now**, and **Retry synchronization** semantics.
- Explicit saved-index opening can fall back to a memory build on clean absence, with visible status. Failure or cancellation must preserve the previous session until a replacement is ready. Opening another index must not first flush the old one automatically.
- Saving runs in a background worker, preserves current tabs/search/results, and displays the destination and saved generation after commit. It does not reopen the repository or change a native memory index into a disk-backed session.
- Disable saving during incomplete initial construction, or queue only a clearly explicit one-time request. If source reconciliation or later updates are active, show the actual saved revision and whether newer in-memory changes remain.
- A failed save offers **Retry save**, which is a new explicit action. Do not retry automatically in a loop.
- Remove **Keep entire index in memory**, residency selectors, full-residency progress, and associated preferences. Existing persisted preference values must not cause a panic or revive the old mode; migrate or discard the obsolete fields safely.
- Ordinary close and repository switching do not save the derived index and need no compulsory unsaved-index prompt. If an explicit save is already running, finish it or allow cancellation through the normal operation UI; do not start another save on close.

### TUI

- Keep Ctrl+K as the familiar entry to index operations. Replace the persistent storage-mode overlay with **Open saved index…** and **Save index…**, each selecting Temporary, Repository, or Custom.
- Remove the residency checkbox, **Apply and reopen**, and automatic synchronization controls. Custom paths belong to the selected action; editing one must not issue an operation.
- Saving uses a worker and leaves the current search, filter, selected collection, tabs/record context where applicable, and result view intact. Explicit loading preserves the current session until successful replacement.
- Ctrl+Q closes without issuing a new flush. Resolve an already running explicit save through the operation lifecycle and report its outcome; do not resurrect the old exit-flush behavior.
- Share labels, target resolution, status mapping, and error guidance with CLI/desktop adapters rather than inventing a separate storage model.

### Status model

Show source readiness/freshness separately from index saving. Useful fields are **Built in memory** or **Loaded from …**, current publication revision, **Not saved** / **Saved revision …** / **Changes since last save**, last save destination, save progress, and typed save errors.

A successful save does not imply that an unfinished filesystem check has completed. A fully cached mmap remains on-demand operation; do not infer or display a user-selectable resident mode from RSS or page-cache state.

## 8. Implementation sequence and ownership

| Step | Commit scope | Dependency and verification |
| --- | --- | --- |
| 1 | Merge reviewed hybrid/direct-snapshot work into `master` | Phase A; preserve the approved implementation as the base |
| 2 | Define explicit load/flush API and status types | Create the new integration worktree; agree interfaces before client delegation |
| 3 | Separate target resolution, snapshot identity, and store generation sequencing | Tests for memory-build → flush → load and replacement of an existing higher generation |
| 4 | Implement on-demand-only explicit loading and snapshot flushing | No automatic source build/write on load; no implicit write destination; atomic publication and pinned readers |
| 5 | Remove full-residency and automatic persistence paths | Audit shutdown, reconciliation, update, compaction, storage attachment, validation stamps, and recovery hooks |
| 6 | Migrate finite CLI and TUI | Independent load/flush targets, one-shot startup save, no exit save; shared status and argument tests |
| 7 | Migrate desktop | Explicit dialogs/actions, preference migration, background save and safe replacement |
| 8 | Update documentation, examples, protocol, and benchmarks | Only memory construction and on-demand loading remain as execution cases |
| 9 | Validate and report the combined implementation | Relevant benchmarks, workspace checks, native UI and PTY verification, final task report and push |

Keep intermediate commits coherent. Internal adapters can bridge client migration temporarily, but remove production adapters that retain the old combined behavior before completion. Do not implement unrelated issue reports as part of this task.

Inspect these current areas first:

- Core: `index_storage.rs`, `lifecycle.rs`, `lifecycle_storage.rs`, `lifecycle_open.rs`, `lifecycle_catalog.rs`, `managed_store.rs`, `segment_format.rs`, and repository opening wrappers.
- Client adapters: CLI argument/preparation code, `crates/cli/src/execution.rs`, `storage_view.rs`, and `tui/`; desktop `backend/open.rs`, controller `runtime.rs`, UI opening dialogs/panels, and preferences.
- Documentation: `docs/cli/obj.md`, `docs/tui.md`, storage/direct-snapshot development documentation, generated help/completions, and benchmark examples/scripts.

Use `rg` to inventory both callers and side effects. Search at least for `FullyResident`, `IndexResidency`, `PreparingFullResidency`, `FullyResidentReady`, `fully-resident`, `index-residency`, `keep_entire_index_in_memory`, `open_with_storage`, `attach_storage`, `set_sync_storage`, `notify_sync_storage`, `flush_storage`, `set_sync_cache`, and client close/reopen flush hooks. Historical benchmark archives and completed task records remain historical evidence and need not be rewritten; active documentation, APIs, help, and production behavior must reflect the new contract.

## 9. Verification and performance

### Behavioral tests

Tests must verify observable behavior rather than merely match enum variants or implementation structure.

1. Put a recognizable saved index in each location. A default build/query still reads current sources and does not probe/load/write those stores.
2. Explicit load consults only the selected target. A saved index elsewhere does not change an absent result.
3. Missing load returns absence; corruption, incompatibility, permissions, and resource failures retain distinct typed outcomes. No failure triggers an implicit replacement.
4. Load a valid current snapshot from a read-only store, run queries, and verify that manifests, segments, catalogs, and validation sidecars were not changed.
5. Build in memory, flush, then load successfully from each target with identical result digests, including custom source adapters, inference, relations, sorting, fallback records, and multiple records per file.
6. Verify that target selection does not change logical configuration identity, source selection, or record counts. A custom base containing unrelated source files must not hide those files.
7. Create a destination with a high durable generation, then flush a fresh low-revision index. The new snapshot becomes current, absent old records stay absent, and identities are not duplicated.
8. Flush two independent indexes with equal numeric revisions. The second must not be mistaken for an unchanged save of the first.
9. Change a loaded index and reconcile sources without flushing. The saved generation stays unchanged. An explicit later flush persists the chosen publication.
10. Flush while later updates publish. The receipt identifies the captured revision and status still reports newer unsaved changes.
11. Exercise simultaneous writers and a stale loaded writer; verify conflict handling and preservation of the previously committed generation.
12. Query a pinned old reader while a new generation is committed. Never modify or reclaim its backing files prematurely.
13. Load from A and save to B, then remove A in a disposable fixture. B remains independently loadable, and the active session was not reopened by saving.
14. Inject failure/cancellation before manifest commit. The old generation loads successfully and the in-memory index remains usable. Check post-commit outcome reporting separately.
15. Test load/flush combinations, separate custom directories, invalid removed options, index-only commands, and the one-shot `--tui --flush-index` case.
16. Test ordinary CLI exit, TUI quit, desktop close, repository replacement, reconciliation completion, and compaction for absence of automatic generation writes.
17. Test desktop preference migration, missing-index fallback, save failure/retry, and safe replacement while the previous session remains active.

Use existing storage fault fixtures and semantic differential tests. Preserve corruption checks and immutable-file ownership protection; this is not permission to weaken them for faster loading.

### Required final checks

Run the repository's workspace tests and workspace compilation checks on the combined implementation. Run Clippy under the existing policy and check the task diff. Existing repository-wide formatting drift is not a reason to reformat unrelated files.

Perform a TUI PTY smoke and native desktop verification using the maintained GUI environment workflow: default memory build, explicit load, save to all three targets, save failure and retry, and close without save. Record commands and observations in the completion report.

### Performance scope

Use the merged direct-snapshot implementation as the immediate baseline. Its reference medians were approximately 235–245 ms for on-demand opening, 1.192 s for construction, and 316 ms for the first ASCII flush. Re-measure paired baseline and candidate on the same machine instead of treating those historical values as fixed limits.

Run one final established paired build/open/initial-flush campaign and the full ASCII/Unicode query suite with result caching disabled. Remove fully resident cases from the active harness. Check first query and first update after an explicit load; include saving to an already populated target. Record durations, source reads, result digests, and whether lookup reconstruction or an unnecessary reopen occurs.

Preserve the accepted direct-snapshot benefits. Do not introduce source scans, whole-index owned copies, full lookup reconstruction, or automatic disk work merely to simplify the interface. Small millisecond-level query changes and reasonable storage/RSS overhead are not standalone rejection criteria. Investigate material slowdowns or data-model mismatches, especially destination replacement that unexpectedly reserializes every unchanged segment. Use ordinary release measurements for timings and existing profiling only when needed to explain a regression.

## 10. Completion criteria and deliverables

The task is complete when:

- The reviewed hybrid/direct-snapshot work was merged and pushed to `master` before this API simplification began.
- Default library and client construction is memory-only and ignores saved indexes.
- Explicit loading requires one target, uses on-demand readers only, and neither creates nor rewrites a saved index.
- Explicit flushing requires one target, persists the captured complete publication, and leaves the active backing representation unchanged.
- Existing compatible destinations receive a correct newer generation; publication numbers, independent build identities, conflicts, and pinned readers are handled correctly.
- No client or library background path starts automatic index synchronization or ordinary exit-time flushing.
- Full-residency loading and its active options, controls, readiness states, and preferences are removed.
- CLI, TUI, desktop, examples, protocol documentation, and benchmark tooling agree on the operation contract.
- The combined tests, relevant performance checks, PTY/native flows, and failure behavior have been verified and reported.

Deliver `integration/explicit-index-persistence`, its worktree, the final implementation commit, and the initial `master` merge commit. Append the implementation report to the task document, move it to `docs/ai/done/`, commit and push it, and place detailed measurements under the established benchmark and `docs/ai/generated/` paths. Preserve outstanding limitations explicitly; do not claim a failed requirement is complete because it was moved to an issue.

Keep the new integration branch and worktree for review. Remove only completed subordinate worktrees and task memory according to the repository workflow. Do not merge the new API work into `master` without the user's subsequent instruction.

## 11. Source references

- [Reviewed direct-snapshot branch](https://github.com/Simon-Martens/objekte/tree/0c6365145822fffc4631b41bac3a9a436116fa51)
- [Direct snapshot implementation and performance report](https://github.com/Simon-Martens/objekte/blob/0c6365145822fffc4631b41bac3a9a436116fa51/docs/ai/generated/direct_snapshot_access_report.md)
- [Current storage policy and location resolution](https://github.com/Simon-Martens/objekte/blob/0c6365145822fffc4631b41bac3a9a436116fa51/crates/core/src/index_storage.rs)
- [Current managed loading and persistence](https://github.com/Simon-Martens/objekte/blob/0c6365145822fffc4631b41bac3a9a436116fa51/crates/core/src/lifecycle_storage.rs)
- [Current CLI behavior](https://github.com/Simon-Martens/objekte/blob/0c6365145822fffc4631b41bac3a9a436116fa51/docs/cli/obj.md)
- [Current TUI behavior](https://github.com/Simon-Martens/objekte/blob/0c6365145822fffc4631b41bac3a9a436116fa51/docs/tui.md)
- [Repository agent workflow](https://github.com/Simon-Martens/objekte/blob/0c6365145822fffc4631b41bac3a9a436116fa51/docs/ai/README.md)

## Implementation report

- Task: `explicit-index-persistence`; completed 2026-11-14.
- Base and Phase A: reviewed hybrid/direct-snapshot result was merged and pushed to `master` at `e64a5caad59912d37148de614b79987373333a76` before Phase B began. No Phase B commit was merged into `master`.
- Phase B integration branch: `integration/explicit-index-persistence`; worktree: `/home/simon/source/objekte-integration-explicit-index-persistence`.
- Implementation commits: `4d7d5b61` (read-only explicit load), `05828120` (explicit flush and destination generations), `31b0f67e` (finite CLI), `89163825` (TUI), `6b78ba6c` (desktop), `8e2189e1` (desktop targets), `b54822f9` (remove combined client arguments), and `1df5f26274` (remove old production paths, complete examples, tests, documentation, benchmarks, and follow-up issue). No subordinate branches or agents were used.

The default build and frontend open paths now construct in memory without probing a managed store. `Index::try_load` reads only its selected target through checked on-demand snapshot readers and performs no write or recovery build. `Index::flush_to` saves one complete pinned publication to its selected destination, reports the saved revision and durable generation separately, and keeps the active backing. Replacement uses destination generation sequencing and expected-generation conflict checks. Source reconciliation updates the live index without saving it. Production full-residency selection, combined storage policy, and automatic synchronization paths were removed. The CLI, TUI, and desktop expose independent load and save controls; the NDJSON protocol is version 3. Existing historical storage helpers are test-only.

Validation on the combined branch passed `CARGO_TARGET_DIR=/home/simon/source/objekte-integration-hybrid-index-storage/target/direct-snapshot-access/integration-build cargo test --workspace --locked -- --quiet`, `cargo check --workspace --all-targets --locked`, normal `cargo clippy --workspace --all-targets --locked`, and `git diff --check`. The workspace run included core, CLI, desktop, and integration tests. Normal Clippy completed with existing repository lint warnings; a strict warnings-as-errors run is not the repository's passing baseline. The TUI PTY smoke built in memory, explicitly saved to Repository, reopened with explicit load, showed the Ctrl+K action menu, and quit without changing the manifest. Native Wayland desktop verification exercised memory build, explicit Repository, Temporary, and Custom saves, explicit Custom load, typed save failure and Retry, and close without another save. The saved Custom manifest hash was unchanged after close. Task-owned screenshots remain under ignored `target/gui-validation/desktop/` in the integration worktree.

Paired ordinary release benchmarks and raw digests are archived at [`bench/results/explicit-index-persistence/2026-11-14/README.md`](../../../bench/results/explicit-index-persistence/2026-11-14/README.md); the summary is [`docs/ai/generated/explicit_index_persistence_results.md`](../generated/explicit_index_persistence_results.md). The established `bench_musenalm --extended --incremental --storage memory --profiles ascii-off,unicode-off --repetitions 3 --query-workers 32` campaign matched all 220 ASCII and 220 Unicode query digests with zero failures and caching disabled. Candidate/baseline geometric mean latency ratios were 0.944 and 1.021 respectively; ratios of summed medians were 1.001 and 0.986. Paired total build-and-save median was 1,498.619 ms versus baseline 1,806.797 ms (0.829 ratio). Source-free open was 252.732 ms versus 249.022 ms (1.015 ratio), with essentially unchanged 248.8 MB gross allocation. An unchanged explicit save took 1.279 ms after the verified manifest fast path. First loaded query took 0.0177 ms median with zero source reads. The first verified update on a disposable 16,384-file fixture took 11.175 ms versus baseline 9.538 ms in one run; the small single-run difference is not treated as a regression. The archived baseline executable is the reviewed direct-snapshot candidate from which the Phase A merge was made; executable hashes and exact commands are recorded with the trials. Isolated initial-flush timings are not directly comparable because the prior automatic policy committed several generations while the new operation commits once.

The native retry dialog currently prefills the previously active Custom path rather than the failed save destination; the explicit retry action and session safety work. This follow-up is recorded for user review in [`docs/ai/issues/feat/explicit_save_retry_prefills_previous_path.md`](../issues/feat/explicit_save_retry_prefills_previous_path.md). No other requirement remains open. The integration branch and worktree are retained for review. There were no subordinate worktrees; temporary task memory was removed after the report push.
