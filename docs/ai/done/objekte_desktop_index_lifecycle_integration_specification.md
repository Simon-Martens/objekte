# Objekte desktop index lifecycle integration specification

Date: 2026-11-08

Task identifier: `desktop-index-lifecycle`

Proposed task path: `docs/ai/tasks/objekte_desktop_index_lifecycle_integration_specification.md`
Integration branch: `integration/desktop-index-lifecycle`

## 1. Objective and scope

Integrate the index lifecycle APIs defined in `objekte_index_lifecycle_implementation_guide.md` into the existing native GPUI desktop application in `crates/desktop/`.

The user must be able to open a cached repository and search while file changes are checked in the background; request another check; load, save, rebuild, and compact the active index; choose explicit schema/cache inputs; and enable optional periodic checking and automatic cache persistence through ordinary desktop controls.

Existing result lists, record tabs, relation navigation, counts, exports, and asynchronous work must remain correct when the index changes. An index operation must act on the active repository handle or install its successful replacement. It must not build a separate repository and discard it while reporting success.

This is a second, separately deliverable task dependent on the core lifecycle implementation. It owns the desktop user interaction and lifecycle integration. The earlier task owns core semantics and minimal caller migration needed to expose them. Coordinate shared interfaces without duplicating the index engine.

The scope is the desktop UI. Preserve CLI/TUI compatibility where shared types change; this task does not redesign the TUI.

The application remains read-only for source files, schemas, and repository configuration. Index-cache writes, existing exports, and ordinary application preferences remain allowed. Do not add source-file create, edit, move, or delete commands merely because corresponding index APIs now exist.

### 1.1 Required user outcomes

- The first usable cached index opens without waiting for a filesystem inventory scan.
- Background index work does not disable search, scrolling, text input, or unrelated record operations.
- The user can distinguish checking source files, running a query, and saving the index.
- Results never append batches from different index revisions.
- A changed or removed record cannot silently become a different record in an existing tab.
- Explicit paths remain explicit and visible.
- Automatic checking and automatic saving are separate settings.
- Maintenance failures preserve the usable workspace and provide a local retry path.
- No routine action requires knowledge of slots, tombstones, publication generations, digests, or posting lists.

## 2. Dependencies, precedence, and baseline

Read these specifications before implementation:

1. `objekte_index_lifecycle_implementation_guide.md`: core lifecycle, persistence, concurrency, and freshness contracts.
2. `objekte_desktop_query_navigation_specification.md`: current navigation, record-tab, continuous-loading, typography, and control-placement requirements.
3. `objekte_desktop_interface_redesign.md`: broader desktop layout and opening workflow.
4. Current root `AGENTS.md`, `docs/ai/README.md`, GUI environment instructions, and applicable directory instructions.

This specification supersedes older desktop instructions only where lifecycle behavior conflicts: synchronous cache validation against all source files, index operations implemented as a discarded reopen, and stored-index controls that rely on an undocumented empty-path default.

Preserve the later navigation specification over the earlier redesign. In particular:

- Keep automatic delivery in batches of 100 and bounded virtualized projections. Do not restore page buttons, Limit/Offset controls, or the removed query Options popover.
- Keep the icon-only hamburger menu and existing compact menu style.
- Keep Source Sans 3 and Source Code Pro.
- Keep saved queries/history in the sidebar, independent retained record tabs, Fields and Links, the right-aligned Source selector, and Source-only record Copy/Export controls.
- Keep two-column fields, hidden empty values, collapsed complex arrays, and metadata above content.
- Keep the free-text filter authoritative, its builder synchronized, and validation within the existing row.
- Do not alter user query text, sorting syntax, column preferences, or tab contexts merely to report index status.

Inspected desktop baseline: `master` at `6dc2ef194bc81bb8e4897b7652635bc7c7cb1cbb`.

| Current location | Observed behavior | Integration requirement |
| --- | --- | --- |
| `backend/open.rs` | `OpenedSession` contains `Arc<Repository>` and CLI-derived opening configuration | Retain the lifecycle-capable handle and explicit resolved configuration |
| `controller/runtime.rs::start_index` | Calls `backend::open`, extracts a success message, and discards the opened session | Replace with real active-handle operations or controlled replacement installation |
| `controller/runtime.rs` | Existing opening, query, tab, validation, and output generation guards | Extend ownership with index revision and independent maintenance operation IDs |
| `ui/state.rs` | One `Busy` enum and shared status string | Represent concurrent query, reconciliation, maintenance, and persistence states independently |
| `ui/panels.rs` | Generic index-mode selection and directory input with an empty default | Replace with explicit conventional index commands and task status |
| `ui/open_dialog.rs`, `ui/model.rs` | Existing modes, schema controls, and snapshot options | Map user selections to explicit core calls and repository convenience mode |
| `ui/preferences.rs` | Existing local preferences | Store desktop lifecycle choices without editing repository configuration |

Inspect the actual implementation base before editing. The core lifecycle types may differ from this guide's conceptual names. Use their implemented contracts and record the dependency commit; do not guess APIs or bypass them.

## 3. Branches, worktrees, and agent coordination

Use one integration branch and worktree for this specification:

`integration/desktop-index-lifecycle`

`/home/simon/source/objekte-integration-desktop-index-lifecycle`

Keep the original checkout on `master`, with unrelated staged and unstaged work untouched. All task and subtask worktrees are siblings under `/home/simon/source`, never `/tmp`.

If the core lifecycle task has already been merged, branch from the current approved main-branch commit. If it is implemented only on its integration branch, base this UI task on a recorded pushed core integration commit. Report that the UI branch includes that prerequisite and the intended merge order. Do not merge the prerequisite into master yourself.

If the APIs are still being implemented, the UI agent can inspect, design, and prepare test fixtures independently. Coordinate the minimal stable adapter contract, then continue dependent work from an available implementation. Do not write a second background scanner or fake persistence implementation to bypass a missing dependency. A missing required API must be reported specifically.

Use a supervisor/integrator and bounded delegation:

| Owner | Branch suffix | Scope |
| --- | --- | --- |
| Supervisor | Integration branch | Shared state contracts, dependency alignment, active-session replacement, final integration |
| Lifecycle adapter | `task/desktop-index-lifecycle/adapter` | Core-event subscription, maintenance commands, cancellation and ownership |
| Desktop controls | `task/desktop-index-lifecycle/controls` | Opening options, index dialog, status indicator, settings |
| Query/navigation | `task/desktop-index-lifecycle/navigation` | Result revisions, continuations, tabs, counts and export ownership |
| Validation | `task/desktop-index-lifecycle/validation` | Deterministic races, interaction coverage, native evidence and performance measurements |

Subtask worktrees use `/home/simon/source/objekte-task-desktop-index-lifecycle-<suffix>`. Fix shared types and file ownership before parallel production edits. The supervisor owns changes to the large shared controller until responsibility has been separated cleanly.

Push immediately after every commit, including integration merges and the completion report. Do not force-push or commit to master.

Follow the task-document lifecycle in `docs/ai/README.md`: preserve uncommitted task documents while work is active; at completion, commit the latest task document with its appended implementation report under `docs/ai/done/` on the integration branch. Use ignored task memory, remove only completed subordinate worktrees after preserving their contents, and retain the integration worktree for the user's merge.

## 4. User-facing commands and vocabulary

Use the existing application menu. Place **Check for changes** as a directly reachable command in its repository-related group. Place the remaining operations in **Index…**, a focused nonmodal management dialog/panel that does not cover or disable the whole workspace.

Do not introduce a new permanent toolbar of maintenance buttons. Keep the primary search interface compact.

| UI label | Meaning | Backend mapping |
| --- | --- | --- |
| Check for changes | Compare configured files and update the active index | One-shot reconciliation |
| Index… | Open current index status and maintenance controls | No operation on open |
| Save index | Persist the current index to its selected save target | Explicit flush |
| Save index as… | Persist to a destination selected now | Explicit flush to that destination |
| Load index… | Load a selected compatible cache and install it as the active index | Explicit load plus automatic initial reconciliation |
| Rebuild index | Recreate index data from the currently configured source files | Background rebuild |
| Compact index | Remove obsolete index storage | Background compaction |
| Automatically check for changes | Enable/disable periodic file comparison | Start/stop periodic reconciliation |
| Automatically save index | Enable/disable coalesced cache persistence in repository convenience mode | Configure `sync_cache` through the supported core API |
| Details… | Show bounded diagnostics and operation outcomes | Existing diagnostics surface |

Use “index” in ordinary labels. “Cache” may appear in explanatory text or technical diagnostics. Do not label Save index as a source-data save, or Compact index as file deletion.

Do not expose raw `add`, `delete`, or `move_file` index calls as user file-management commands. External additions, moves, changes, and deletions arrive through reconciliation. Verified in-place maintenance is an internal optimization, not a checkbox or old-content upload form.

Do not pass a clipped Source preview or projected Fields data as the core's old-content hint. Only complete original bytes associated with the exact indexed file version qualify. The current read-only desktop normally has no reason to supply such a hint; safe core fallback is sufficient and does not justify reading every old file into UI memory.

Where an existing record Refresh command is present, distinguish refreshing the displayed record from checking the filesystem. Refresh resolves the latest indexed record. **Check for changes** first updates the index from source files. Keep these verbs and their help text consistent.

## 5. Opening data and cache handling

### 5.1 Preserve the current opening flow

Continue to use the existing Open data dialog with Folder, Browse, optional multiple-folder selection, mode detection, and collapsed Advanced settings. Ctrl+O remains Open data.

Detection stays asynchronous and inexpensive. It may inspect repository configuration locations; it must not build an index or scan all source files. A detected but malformed repository is an error, not permission to silently open it as an unrelated plain folder.

Cancel, failed validation, and late native-picker/detection results leave the active workspace unchanged.

### 5.2 Opening policies

Use these mappings. The UI may retain familiar existing labels where they remain unambiguous.

| Choice | Required behavior |
| --- | --- |
| Plain folder, default | Build in memory from explicit selected folder/configuration; no cache discovery or automatic disk writes |
| Configured repository, use repository settings | Explicitly call repository convenience opening; discover configured schemas/cache under the selected root; use a compatible cache if available |
| Build new index in memory | Ignore stored index loading and build using the chosen explicit configuration |
| Load selected index | Require a selected concrete cache location; restore it and begin background reconciliation |
| Build and save index | Explicitly build, then save once to the selected destination |
| Load or rebuild and save | Explicit policy authorizing fallback reconstruction and one save; show any restore failure/rebuild reason |

Using a compatible stored index by default for **configured repository mode** is an intentional update to the older in-memory-only desktop default. It does not enable automatic disk writes, change plain-folder behavior, or enable schema inference/Unicode indexing.

For legacy directory-based cache selections, resolve the actual filename/location in the desktop adapter and show it. Pass concrete paths to the core. An empty input is not a hidden default in explicit mode.

Repository-derived paths should be shown as resolved read-only values or clearly identified defaults, with an ordinary custom selection when supported. Inactive controls retain local drafts if useful but must not leak values into the active request.

Keep explicit schema selection, recursion, alphabet, inference, and diagnostic options compatible with existing modes. Manual choices must never be overwritten by repository detection.

### 5.3 First usable view

Opening has at least these observable stages:

| State | Workspace behavior |
| --- | --- |
| Validating/opening selected cache | Keep the existing workspace usable; if there is none, show loading state |
| Cached index ready, initial check pending/running | Install the new session and enable search immediately; show “Checking files…” |
| New index being built, no cache available | Keep previous workspace until a usable replacement is ready; otherwise show initial build progress |
| Reconciliation completed | Update freshness status and refresh the appropriate result context as specified below |
| Reconciliation failed/partial | Keep the installed cached session usable and show “Some files could not be checked” |
| Open failed before a usable index | Preserve the previous session, tabs, input drafts, and dialog choices |

Do not wait for the initial scan to finish before closing the opening dialog or enabling the new repository's search.

A successful change to a different repository closes the old repository's record tabs as the existing navigation specification requires. A failed or cancelled open does not.

Opening a replacement cache for the same source configuration is an index replacement, not a new dataset. Preserve user query definitions, table preferences, and tab identities while invalidating their old data contexts. Do not call the current unconditional `Opened` reset path for every maintenance operation.

## 6. Index management surface

The Index dialog uses conventional labelled rows, buttons, real checkboxes, a native file picker where supported, and a path-input fallback. It is resizable or scrollable at small window sizes. Closing the dialog does not cancel work.

Display concise summary information:

- Repository/source location and opening mode.
- Selected cache load location and selected save destination, when different.
- Last successful file check and whether that check was complete.
- Whether the current index has been saved to its configured destination.
- Current operation, useful progress, and last error.
- Active file/record counts when already available cheaply.
- Reclaimable obsolete storage only when the core already supplies an estimate.

Do not scan or count the corpus merely to populate this dialog. Put revision IDs, internal sizes, timing breakdowns, and strategy counters under Details.

Use normal grouped sections for source checking, saved index, and maintenance. Do not render six oversized cards or an always-visible technical dashboard.

### 6.1 Save and save destination

Save index writes the current active view. It must not rebuild or reopen the repository as a substitute.

When no save target is selected, Save index opens the destination picker and performs no write until a valid path is chosen. Show the final resolved destination. Keep existing overwrite safeguards: writing the known active index destination is an ordinary save; replacing a different existing file requires the existing explicit overwrite decision. Refuse source/schema/configuration collisions through the core safeguards.

Save index as writes a separate snapshot and does not silently change the target of future automatic saves. A separate, explicit **Change save location…** control may change the default destination within the supported mode.

Report “Index saved” only for the saved revision. If newer changes arrived during the write, display “Newer index changes are not saved yet.” Do not show an unconditional clean state.

Track successful persistence by destination as well as revision. Saving a copy to location B does not mean that the configured automatic-save location A is current.

Manual saves remain available when automatic saving is disabled.

### 6.2 Load selected index

Load index selects a cache compatible with the active source configuration. It does not infer another schema or silently change repositories to make a cache fit.

Prepare and validate the replacement in the background. Install it only on success, assign a new index-instance/revision identity, start its automatic reconciliation, and invalidate old result/record contexts. Preserve the old active index on failure or cancellation.

Handle an unsaved derived index without treating it as unsaved source data. Do not add a mandatory data-loss warning merely because a rebuildable cache is dirty. Preserve explicit operations already in progress through the operation rules below.

### 6.3 Rebuild and compact

Rebuild index uses the active configuration and leaves the current view searchable until publication. Its description says that it rereads source files. It does not silently reload changed schemas.

Compact index removes obsolete index entries. Its description says that it does not delete source files. If there is nothing to compact, return a clear no-op result.

These commands execute directly after selection; a second generic “Are you sure?” dialog is not required for reversible index maintenance. Use existing task controls for cancellation and failures.

Neither operation automatically saves unless repository automatic saving is enabled or the user explicitly chose a combined build-and-save action.

### 6.4 Schema/configuration changes

Changing schema selection uses the opening/configuration workflow and constructs a compatible replacement. It must not swap schemas under existing index postings.

Show the selected schema paths in the opening/index details. Invalid schema selection preserves the current workspace and the user's draft. The desktop does not edit schema files or repository configuration.

## 7. Automatic behavior and preference ownership

Both recurring settings are initially disabled unless the user has previously enabled them for the same repository through desktop preferences.

The automatic initial check after cache load runs even when periodic checking is disabled. The label/helper must make this clear.

| Setting | Default | Availability | Persistence |
| --- | --- | --- | --- |
| Automatically check for changes | Off | Any open source configuration supported by reconciliation | Existing application preferences, scoped to source configuration |
| Check interval | 30 seconds when enabling for the first time | Editable conventional numeric duration control; never a per-keystroke request | Same preference scope |
| Automatically save index | Off | Repository convenience sessions with a resolved writable cache destination | Existing application preferences scoped to repository/destination |
| Manual Save/Load/schema choices | Explicit | Compatible modes | Preserve explicit choices independently |

The 30-second interval is a desktop default, not a core hardcoded scheduling policy. Validate positive bounds using the core's supported interval contract. Coalesce scans; never overlap scans merely because one takes longer than the interval.

Do not build a second desktop auto-flush scheduler for explicit/manual sessions. If automatic saving is unavailable in that mode, keep the checkbox disabled with a concise explanation and keep manual saving usable.

Enabling automatic saving applies to the current repository and may schedule saving of an already dirty index. Show its destination before enabling. Do not apply it to every repository globally or edit source repository configuration to store the preference.

Disabling a recurring setting stops future scheduling. A running operation has its own cancellation control; turning off periodic checking does not pretend a current check was cancelled. Already-started flush publication follows the core's cancellation/durability rules.

A remembered destination or permission failure must be visible. Do not silently choose another directory, disable the setting without explanation, or report that all changes were saved.

## 8. Concurrent activity and progress

Replace the single authoritative busy/status model with separate state for opening, query execution, file checking, rebuild/compaction, persistence, record jobs, and existing exports/batches.

This can be implemented with a small typed activity model. It must not become a second task orchestration framework.

Use the existing fixed-height footer/status area for a compact index status indicator. Examples:

| Condition | Short status |
| --- | --- |
| Initial scan pending/running | Checking files… |
| Check completed | Files checked |
| Some scopes failed | Some files could not be checked |
| Scan cancelled | File check stopped |
| Saving | Saving index… |
| Dirty with automatic saving off | Index changes not saved |
| Automatic save failure | Index could not be saved |
| Rebuild/compaction | Rebuilding index… / Compacting index… |

“Files checked” refers to the completed scan, not a guarantee that no external program changed a file afterward. A tooltip/details view gives the time and scope.

Use distinct text/icon state, not color alone. Do not reuse the record validity dot to represent cache freshness.

Query count/loading remains visible separately. If several maintenance activities coexist, show a concise summary with a details affordance. Do not let a save-success notification overwrite a query error or an invalid-filter label.

Progress is indeterminate when total work is unknown. Show real processed counts when supplied; do not fabricate a percentage. Coalesce progress notifications to roughly 10 UI updates per second while delivering completion/error changes promptly.

The query Stop control cancels the query. An index task's Cancel control cancels that task. Neither acts as an undocumented global stop button. Closing a task dialog only hides it.

Errors are local to the relevant task, preserve the previous usable state, and offer Retry/Details where valid. Background problems should not open modal dialogs or produce one notification per failed file.

## 9. Result-list revisions and refresh policy

### 9.1 Context identity

Every executed result context carries:

- Repository session identity.
- Index-instance identity and published index revision.
- Effective query identity, resolved parameters/time, and deterministic sort order.
- Query-operation generation and continuation ownership.
- Completeness/freshness state associated with those results.

An index revision is not interchangeable with an editor generation. A progress update is not an index publication. No-op scans and saves must not restart queries.

Each request and response for page delivery, projection, exact count, completion values, and relation-derived display must carry enough identity to reject a late response from an obsolete context.

### 9.2 On index publication

Do not append new-revision rows to an existing old-revision list.

When a publication invalidates the active result context:

1. Mark that context outdated.
2. Cancel/ignore its pending continuations, count, and incompatible projection jobs.
3. Keep the existing bounded display projections and selection identity visible.
4. Stop offering more batches from that invalidated context.
5. Schedule or offer a refresh according to the policy below.
6. Release its old index/query snapshot after dependent jobs stop; do not retain complete old repositories indefinitely just to display stale rows.

Show **Results changed — Refresh** in the existing footer/status area. Old displayed counts belong to old displayed results and must be marked as such, not shown as current totals.

The first implementation may conservatively invalidate on every relevant index publication. If the core provides reliable change summaries, avoid invalidation for proven-unaffected contexts. Do not scan all results or files in the UI to establish that proof.

### 9.3 Refresh policy

Use a stable, predictable policy:

- Completing initial cache reconciliation automatically refreshes the current valid executed query once, if its context is outdated.
- Completing a user-requested Check for changes, Load index, Rebuild index, or Compact index similarly refreshes once when needed.
- Subsequent periodic or other background mutations mark the list outdated and offer Refresh, preserving the user's reading position instead of continuously rearranging the table.
- A new valid query or explicit Run always uses the latest published view.
- Do not refresh after a save, a no-change scan, or progress-only events.

Coalesce publications during a task into one completion refresh. If another valid user query has already executed against the final revision, do not issue an unnecessary duplicate.

An invalid or not-yet-validated editor draft must not be executed or overwritten. Preserve “Showing last valid results”; add outdated status for those results if necessary. Automatic refresh resumes through the normal validation/execution path when appropriate, without modifying the draft.

### 9.4 Refreshing with position preservation

A refresh creates a new executed context and replaces the list coherently. Keep the old display until a successful first/anchor page is available; show Updating results. A failure retains the old display with its outdated label and a retry action.

A lifecycle-triggered refresh preserves the executed query's resolved time/parameter values. An explicit new Run or user query edit follows the normal rule for resolving them again. This prevents a data refresh from silently changing a temporal query for an unrelated reason.

Preserve selection and viewport using canonical logical record identity and a stable visible anchor, not a stale row index. Use the core's ordered-result lookup where available to locate the anchor without materializing all preceding bodies.

If an anchor disappeared or no longer matches, use a deterministic nearby/fallback position and explain concisely. Starting at the beginning is acceptable when no efficient safe anchor lookup exists; document it and do not scan an entire corpus just to preserve scrolling.

Retain the existing 100-record delivery policy and bounded projections. Refresh must not silently fetch all previously loaded bodies.

Clicking/pressing Enter on an outdated row must not dereference its old slot. Resolve the action as refresh-then-open using the captured canonical identity and query context. If the record no longer exists or no longer belongs to the refreshed results, report that outcome instead of opening the replacement occupying its old row.

Do not add duplicate history entries for automatic refresh. User query history records semantic user executions, not every lifecycle publication.

## 10. Record tabs, links, and stale locators

### 10.1 Preserve tab identity and local state

A same-repository index update must not close all record tabs. Keep each tab's title, originating query definition, canonical record identity, Back frames, Fields/Links/Source selection, expansions, and scroll position where they remain meaningful.

Track current record freshness separately from the validity of its originating query/navigation context. Another file can change query order even when the displayed record's source version is unchanged.

Represent at least: current, refreshing, outdated, removed/unresolvable, and failed refresh. A pending state is not a valid/invalid source-data verdict.

On invalidation, cancel or ignore pending jobs tied to the old context. Keep only bounded already-loaded projections and lightweight navigation state. Do not keep a complete old repository or unbounded old result list alive for each tab.

A conservative implementation may mark all tab navigation contexts outdated on a relevant publication. Reliable file-version/change metadata may preserve verified unaffected record content, but does not by itself prove the old query ordering is still valid.

### 10.2 Visible state and Refresh record

Show a compact inline message in the inspector's existing metadata/navigation area, for example **Record may have changed — Refresh**. Use a conventional refresh icon/button with accessible name **Refresh record**.

Do not add a new record tab, a full-screen error, or a permanent field-status column.

While its origin context is outdated, disable Previous/Next and unsafe Back/relationship operations with an explanatory tooltip. Keep Close and Open query available. Fields already loaded remain readable and clearly labelled as previously loaded content.

Refresh record performs the following off the UI thread:

1. Capture the tab ID, tab operation generation, original query definition, resolved time/parameters, and canonical record identity.
2. Pin the current index view.
3. Resolve the record and recompute the originating query/navigation context where needed.
4. Publish one coherent tab update only if ownership still matches.
5. Restore compatible view state; invalidate field expansions that no longer address the same structure.

Keep resolved temporal values for the tab's original query so navigation does not silently reinterpret “now.” A user running that query anew in the main pane follows the established new-execution behavior.

If the record exists but no longer matches the original query, keep it inspectable and show **No longer in the original results**. Do not invent its old result position. Previous/Next require a valid remaining origin anchor or stay disabled. Open query can show the refreshed original query without claiming this record is a result.

If it no longer exists, retain the tab with **Record is no longer available**, its last known identity/location, and optional bounded previously loaded content. The user may close it or check/refresh again. Do not silently substitute another file with the same old row number or storage slot.

If a move changes a generated/path-derived identity, follow a reliable core-provided identity remapping only. Without one, show the unresolved state; do not guess from equal filenames, file sizes, or modification times.

### 10.3 Back, links, and Source

Back frames remain lightweight canonical identities and local view state. Following a frame after a refresh resolves it under the refreshed context and may report that the record is gone. A failed navigation preserves the current record and does not consume a successful Back frame.

Invalidate cached outgoing/backlink pages with their source/target revision. Load them lazily when Links is visible. Inactive or missing targets have an explicit unavailable state; no automatic full relation traversal is permitted to update a badge.

Source never reads through an obsolete range. It can retain a bounded prior preview with a stale label, but obtaining current complete source requires a successful backend read under the current record version.

Copy record and Export record remain available only in Source. Disable or safely refresh these full-record actions when their target is outdated; do not substitute a clipped prior preview as complete output. Ordinary selection/copy of visibly displayed text retains standard behavior.

Keep the existing metadata validity dot tied to the backend record status. A green validity dot on old content does not mean that its index/source version is current; the stale message remains separate. Unknown/unavailable status is neutral.

### 10.4 Tab job ownership

Every asynchronous tab job is tagged with session identity, index instance/revision, tab ID, and tab-operation generation. Closing or refreshing a tab invalidates its obsolete jobs.

Late replies cannot reopen a closed tab, update a different tab, install old links over new content, or copy/export from a new record selected after the action began.

Changing the main query does not retarget the tab's original query context. Changing the repository successfully does close old-session tabs, as before.

## 11. Counts, incomplete data, exports, and batches

### 11.1 Honest count and freshness presentation

Keep these states independent:

| State | User-facing treatment |
| --- | --- |
| No exact count requested | “100 loaded”; no invented total |
| Exact count running | Existing counting indicator |
| Exact count complete for current context | Show the exact total |
| Context outdated | Old count belongs to labelled old results; never present it as current |
| Source access incomplete | “Some results could not be checked” with Details; no false exact total |
| Initial cache reconciliation running | File-check status stays visible even if the cached-view query itself completed |
| Exhausted complete query | Total may be inferred from delivered membership |
| Exhausted incomplete query | Do not infer an exact total merely because no further rows were delivered |

A missing source or unavailable relation/sort value is not “zero matches.” Preserve the core's unknown/incomplete semantics and strict error behavior where requested.

Do not trigger exact counting after every background revision merely to populate the footer.

### 11.2 Exports

“All results” export continues to use the complete captured effective query, not only displayed rows or the 100-record delivery batch.

Capture query definition, resolved time/parameters, index view, destination, and operation identity when export starts. Later search edits and index updates do not retarget it or combine records from different index views.

An index snapshot cannot preserve externally changed source bytes. Follow the core's completeness/error contract; a partial or failed export must be identified as such. Never display “Export complete” for a silently shortened file. Preserve the existing output/no-overwrite and partial-file handling contract.

Starting an export from outdated or invalid main results follows the existing query-validation gate. Refresh or explicitly capture a valid latest execution context before writing; do not export the wrong effective query from an old display.

Changing the repository must not retarget an ongoing export. Reuse the established behavior for finishing/cancelling explicitly started output jobs, with their original owner retained until cleanup. Do not add source-data overwrite actions.

### 11.3 Batch queries

Keep batch-file grammar, ranges, output encoding, per-query outcomes, and cancellation compatible. Background publication must not mix contexts within one query.

Use a clearly documented batch snapshot policy. Prefer one pinned index view for an entire finite batch, within existing resource limits, so query-to-query comparisons are reproducible. If the implemented core intentionally provides per-query views, record their revisions in the batch diagnostics and do not imply a single common snapshot.

No maintenance action may cancel an export or batch merely because the current controller reuses one generic output cancellation token. Separate ownership where required.

## 12. Controller and backend integration

### 12.1 One active lifecycle handle

Extend `OpenedSession` to retain the implemented core lifecycle handle, resolved source configuration, explicit cache targets, and session identity. Queries acquire views from that handle. Do not clone or rebuild an index for a tab, a status poll, or a maintenance dialog.

Introduce a small adapter module, such as `backend/index_lifecycle.rs`, for core lifecycle commands and typed outcomes. Keep core algorithms out of the renderer.

The adapter must not implement filesystem diffing, slot validity, fingerprints, compaction, or cache compatibility itself. Those belong to the core prerequisite.

At phase 0, verify that the core exposes the status/subscription, task cancellation, snapshot acquisition, explicit flush/load, and runtime periodic/sync-cache configuration needed here. If automatic saving is currently only an opening option, coordinate a narrow runtime configuration API with the core owner; do not reopen the dataset and reset navigation whenever the user toggles it.

### 12.2 Typed actions and events

Use explicit concepts equivalent to:

| Action | Event/state changes |
| --- | --- |
| Reconcile now | Started/progress/completed/partial/failed/cancelled |
| Rebuild or compact | Maintenance task ID, progress and publication revision |
| Save selected destination | Save task ID, saved revision, target and outcome |
| Load cache | Load task ID, prepared replacement, installation result |
| Change automatic-check policy | Effective policy and scheduling outcome |
| Change automatic-save policy | Effective policy, destination and save status |
| Refresh results/tab | New execution context or explicit failure |
| Cancel task | Cancellation requested and eventual terminal outcome |

Do not encode machine state by parsing human status strings. Use typed state and format messages in the presentation layer.

Each event carries session and operation identity. Data publications also carry index instance/revision. Revisions from a newly loaded cache cannot be compared as if they belonged to the replaced instance.

Subscribe before relying on progress notifications, or subscribe and obtain a current status snapshot using the core's race-safe contract. Do not lose a fast initial completion between cache restoration and subscription. If detailed events are coalesced/dropped, resynchronize status and invalidate contexts conservatively.

### 12.3 Execution and resource ownership

Do not block the GPUI thread on file scanning, cache decoding/serialization, source reads, hashing, rebuild, compaction, or waiting for workers.

Reuse core workers and task handles. Avoid starting a new OS thread for each progress notification or each scheduled file check. The desktop may use its established bounded worker interface for blocking adapter calls.

Keep operation ownership separate for:

- Pending repository opening/replacement.
- Main query/continuations and count.
- Individual record tabs.
- Reconciliation and periodic scheduler.
- Rebuild/compaction.
- Persistence.
- Exports and batch work.

Build a compatibility table from actual core capabilities. At minimum, search remains available during maintenance; duplicate full scans coalesce; rebuild/compaction do not run simultaneously unless the core safely supports it; saves serialize per destination. Explain disabled commands locally instead of blocking the whole window.

A Load index replacement must not race with an old-handle save to the same destination or leave old automatic schedulers alive. Quiesce/cancel relevant old tasks using their documented core contracts before installation, or disable the conflicting command with a clear reason.

### 12.4 UI state and preferences

Replace the single maintenance busy/status assumption with small independent state fields. Retain normal query state rather than forcing maintenance into `Busy::Querying`.

Keep opening-dialog drafts separate from the active session's effective settings. Changing a draft destination does not redirect a currently running save.

Remember periodic/sync-cache preferences through the existing preference store. Scope them by canonical repository/source configuration and destination as appropriate. Do not persist transient operation IDs, job handles, complete index snapshots, or stale record payloads.

Do not clone `PageView`, `DetailView`, full activity histories, or repository structures on every progress repaint. Store shared immutable projections and bounded activity summaries.

### 12.5 Repository switching and shutdown

Opening a new repository is prepared while the old one remains usable. A successful swap cancels old UI subscriptions and superseded jobs, stops old periodic scheduling, and releases old-session state according to the operation policy. Late old events cannot update the new status bar or trigger a save to its destination.

An ordinary manual-mode session does not auto-save on close. An unsaved index cache is derived data, not an unsaved source document; no mandatory source-data-loss prompt is required.

For repository sessions with automatic saving enabled, request a final save through the core's explicit shutdown contract. Keep waiting work off the UI thread. If completion is slow or fails, present a focused closing state with appropriate choices such as Wait, Retry, Close without saving index, and Cancel close. Explain that source files are unaffected. Do not promise crash-proof persistence if the application is terminated forcibly.

Closing one window must not terminate a lifecycle service still owned by another window, if the application supports shared sessions. Do not expand this task into multi-window support where none exists.

## 13. Detailed user scenarios

Implement and demonstrate these end-to-end cases.

### 13.1 Open a cached configured repository

1. Select the repository in Open data.
2. Display resolved repository/schema/cache choices.
3. Restore and install the usable index.
4. Allow a query and first 100-row delivery while the filesystem check is deliberately paused.
5. Show Checking files without moving query controls or stealing focus.
6. Commit discovered changes.
7. On completion, refresh the current valid result context once if outdated.
8. Keep any open record tabs and mark/refresh their contexts safely.

### 13.2 External add, edit, rename, and delete

An external program changes source files. Check for changes, or an enabled periodic scan, updates the core index.

After explicit checking, refresh the main results once. After a periodic change, show Results changed — Refresh. A new explicit search already uses current data.

An edited tab offers Refresh record; a removed record stays visible as unavailable; a move uses reliable core identity mapping when available. No desktop file-editing action is introduced.

### 13.3 Save while changes continue

Start Save index, then publish another update before serialization completes. Show the completed saved revision accurately and retain dirty status for the newer revision.

Automatic-saving mode schedules its next coalesced write. Manual mode leaves the newer change unsaved until requested. Queries and record navigation remain responsive.

### 13.4 Load an invalid or incompatible cache

Select a corrupt or incompatible cache. Show a useful error and optionally an explicit Rebuild index action. Preserve the active workspace and the selected path for correction. Do not silently replace the schema, choose another cache, or reset tabs.

### 13.5 Rebuild or compact while reading

Start maintenance with a populated grid and open tabs. Type and run a query while maintenance is progressing. Inspect a current record. Cancel maintenance in one run and let it finish in another.

Cancellation leaves the published view usable. Successful publication refreshes results once and invalidates incompatible record/navigation contexts. It must not act as a complete dataset close/reopen.

### 13.6 Partial filesystem scan

Make one directory unreadable while another contains valid updates. Successful observations may be committed according to the core policy, while unverified scopes are preserved.

Show Some files could not be checked with bounded details and Retry. Keep search available. Do not display Files checked as if the entire source selection were verified.

## 14. Implementation phases and commits

The supervisor may split commits further. Every phase must preserve a usable application and be pushed immediately.

| Phase | Deliverable | Suggested commit subject |
| --- | --- | --- |
| 0 | Inspect prerequisites, record dependency/base, capture current GUI and define adapter ownership | Document desktop lifecycle integration contracts |
| 1 | Active lifecycle handle, typed adapter/events, revision ownership, independent cancellation | Connect desktop sessions to index lifecycle services |
| 2 | Cache-ready startup and explicit opening policy/path mapping | Open cached repositories before background reconciliation completes |
| 3 | Index dialog, check/save/load/rebuild/compact actions and independent progress | Add desktop index maintenance controls |
| 4 | Periodic checking and repository automatic saving with scoped preferences | Add explicit desktop index automation settings |
| 5 | Revision-safe result refresh, continuations, counts, tabs and links | Preserve desktop navigation across index changes |
| 6 | Export/batch ownership, source incompleteness, replacement and shutdown behavior | Handle lifecycle failures and output ownership |
| 7 | Native interaction validation, regressions, performance comparison and report | Validate desktop index lifecycle integration |

Phases 1–3 provide the first visible milestone. Revision-safe result/tab handling is required before declaring that maintenance is safe for ordinary use. Do not report full completion with nonfunctional controls or the old discarded-reopen behavior behind new labels.

This UI task must not silently take ownership of unfinished core compaction or atomic update algorithms. Any necessary narrow core adapter additions must be coordinated and reviewed, with their ownership and tests recorded.

## 15. Automated and native validation

### 15.1 Deterministic automated coverage

Use the existing desktop test support and core fixtures. Use barriers/controllable task completion to test races; do not rely on arbitrary sleeps.

| Area | Required checks |
| --- | --- |
| Opening | Plain-folder defaults; configured cache reuse; explicit schema/cache mapping; empty-path rejection; malformed configuration; cancelled/late picker response |
| Fast cached startup | Search succeeds while reconciliation is paused; dialog closes after usable cache, not scan completion |
| Active operations | Save acts on current handle; rebuild/compact publish into it; successful load installs the replacement; failed load preserves it |
| Progress | Query and file check coexist; save status does not erase validation/query error; unknown totals remain indeterminate |
| Scheduling | Defaults off; initial scan still runs; enable/disable settings; scoped preference restoration; no duplicate scan queues |
| Result ownership | Late old page/projection/count rejected; no mixed-revision batch appends; no-op scan/save does not refresh |
| Refresh policy | Startup/explicit completion refreshes once; periodic changes show pending refresh; user edits/invalid draft preserved; no duplicate history |
| Position | Canonical identity preserved; removed anchor handled; no full-prefix body loading; outdated-row action cannot open a different record |
| Tabs | Edited/deleted/moved records; same-record different-query tabs; independent contexts; stale navigation; safe Refresh; late response after close |
| Relations | Old backlink/target result rejected; missing target explicit; no full relation traversal for progress |
| Source output | Stale range not read; clipped preview not copied/exported as complete content; stale Source action handled safely |
| Counts | Exact/unknown/pending/outdated/incomplete distinctions; exhausted incomplete results not labelled exact |
| Persistence | Correct selected path; source/configuration collision; failed write; old saved revision leaves new revision dirty |
| Replacement | Index-instance change invalidates old locators; old schedulers stop; old events cannot modify new session |
| Cancellation | Query Stop does not cancel index work; task Cancel does not stop unrelated exports/tabs; closing dialog does not cancel |
| Exports/batches | Captured query/revision and destination remain owned; source incompleteness and partial output honest |
| Shutdown | Manual mode no implicit save; automatic final save; slow/failing close; released subscriptions/workers |
| Regression | Existing bookmarks/history, filter builder, sort text, completion, columns, themes, read-only outputs and keyboard flow |

### 15.2 Native GUI evidence

Use the established native headless Sway workflow and GPUI Kit application, not a web recreation. Keep the configured virtual output at 1920×1080, scale 1, 60 Hz, and exercise smaller application windows inside it as the existing GUI instructions require.

Capture and inspect the actual application after interactions. Required evidence:

- Open dialog with plain-folder defaults and configured-repository cache settings.
- Searchable cached workspace with file checking still running.
- Index management surface with explicit load/save destinations and both recurring settings.
- Query execution concurrent with rebuilding or compacting.
- Outdated results and their successful refresh.
- Outdated/removed record tab with safe Refresh behavior.
- Partial file-check failure and failed save with retained workspace.
- Light and dark themes and a smaller window without clipped controls.

Record dataset, source/index state, branch/commit, environment, and the action producing each screenshot. Native screenshots validate layout; deterministic tests validate races. Neither substitutes for the other.

Check mouse, keyboard, focus restoration, Escape, tab order, tooltips, native picker cancellation, and screen-reader labels exposed by the toolkit. The existing hamburger trigger remains icon-only, and checkbox settings use actual controls.

No lifecycle status update may move the filter row, hide the invalid-filter message, steal editor focus, or turn the results table into a progress screen.

### 15.3 Repository checks

Run current documented formatting, workspace tests, lint/spacing checks, and relevant desktop/core acceptance checks. Confirm commands against the actual repository; baseline commands include:

```bash
cargo fmt --all --check
cargo test --workspace --all-targets
```

Keep CLI/TUI tests in scope where shared adapters or arguments changed. Record unavailable native dependencies or required core APIs as specific missing validation, not a passing result.

## 16. Performance and resource acceptance

Compare the same dataset, release build/profile, viewport, theme, worker settings, and cache state. Measure native interaction separately from SSH capture latency.

| Measurement | Required purpose |
| --- | --- |
| Open to first searchable cached view | Show that filesystem reconciliation is no longer on the critical opening path |
| First query while scan is paused/running | Demonstrate actual service availability, not only visible controls |
| Typing/scrolling/tab activation during maintenance | Detect UI-thread blocking and progress redraw overhead |
| Time from committed change to explicit-completion refresh | Verify bounded event/refresh delivery |
| Number of searches per reconciliation | No query per changed record or per progress update |
| Progress event/repaint rate | Coalesced updates with prompt terminal states |
| Page/detail allocation and copying | No full payload clones on every status change |
| Memory with many tabs and repeated index revisions | Obsolete repositories and page projections are released |
| Large result refresh/deep anchor | No eager re-materialization of all preceding record bodies |
| Periodic idle work and cache writes | No content scan on unchanged files; no flush per record; no duplicate desktop scheduler |
| Cancellation and repository replacement | Workers/subscriptions stop or detach under the documented ownership policy |

Reuse the existing aggregate page/detail budgets and bounded tab-history behavior. A stale tab may retain its bounded display projection; it may not pin a full old repository indefinitely.

Do not add expensive exact counts, field discovery, sort-key materialization, or complete diagnostic formatting to render callbacks. Metadata counters should come from core events/status.

Target the existing responsive UI behavior. Do not invent a performance improvement from a screenshot. Report repeatable regressions and their cause, especially repository copying, duplicate scans, redundant query execution, and synchronization on the GPUI thread.

## 17. Acceptance and completion report

Full completion requires:

- All index actions reach the intended core operation on the active session or install a validated replacement.
- A usable cached index is searchable while its initial scan runs.
- Maintenance and persistence progress coexist with ordinary query interaction.
- Opening, schema/cache paths, defaults, and recurring settings obey the specified policy.
- Main result batches/counts and tab operations reject obsolete revisions and handles.
- Refresh behavior preserves query text and canonical identity without mixing snapshots.
- Missing source files, incomplete scans, stale records, and save failures have truthful local states.
- Compaction/rebuild preserve the workspace rather than resetting every tab through a generic reopen.
- Existing desktop appearance and navigation requirements remain intact.
- Required automated checks, native interaction evidence, and performance measurements are complete.
- Source files, schemas, and repository configuration remain untouched.
- One integration branch contains all UI work and identifies its core dependency clearly.

Append an Implementation report to the completed task document with:

1. Actual UI base and core dependency commits, integration branch/worktree, and completion date.
2. Implemented control-to-API mapping and any public adapter changes.
3. Cache-ready startup behavior and selected defaults.
4. Index revision, result refresh, tab freshness, and cancellation ownership.
5. Persistence, automatic policies, session replacement, and shutdown behavior.
6. Test commands/results and native screenshot evidence paths.
7. Performance comparison, memory findings, and unexplained/unavailable coverage.
8. Approved deviations, explicit limitations, contributing branches, and cleanup state.
9. Pushed implementation commits and the final completed-document path.

Store requested screenshots/reports according to the repository GUI and generated-document workflow. Keep the integration branch/worktree for the user and never merge into master automatically.

## 18. References

- [Core index lifecycle guide](https://chatgpt.com/library?file_id=libfile_d288d43e6ac4819199eef4b8b0161ff2&account_id=89ce9547-be86-4c0f-a24f-189ad5a0a1d2)
- [Desktop query and navigation specification](https://chatgpt.com/library?file_id=libfile_2c27543efa908191bd7e32409d96d232&account_id=89ce9547-be86-4c0f-a24f-189ad5a0a1d2)
- [Desktop interface redesign](https://chatgpt.com/library?file_id=libfile_0d46c4015d088191a59c47328438c848&account_id=89ce9547-be86-4c0f-a24f-189ad5a0a1d2)
- [Inspected desktop opening backend](https://github.com/Simon-Martens/objekte/blob/6dc2ef194bc81bb8e4897b7652635bc7c7cb1cbb/crates/desktop/src/backend/open.rs)
- [Inspected desktop controller](https://github.com/Simon-Martens/objekte/blob/6dc2ef194bc81bb8e4897b7652635bc7c7cb1cbb/crates/desktop/src/controller/runtime.rs)
- [Inspected desktop state](https://github.com/Simon-Martens/objekte/blob/6dc2ef194bc81bb8e4897b7652635bc7c7cb1cbb/crates/desktop/src/ui/state.rs)
- [Inspected desktop models/actions](https://github.com/Simon-Martens/objekte/blob/6dc2ef194bc81bb8e4897b7652635bc7c7cb1cbb/crates/desktop/src/ui/model.rs)
- [Inspected index panel](https://github.com/Simon-Martens/objekte/blob/6dc2ef194bc81bb8e4897b7652635bc7c7cb1cbb/crates/desktop/src/ui/panels.rs)
- [Repository workflow](https://github.com/Simon-Martens/objekte/blob/6dc2ef194bc81bb8e4897b7652635bc7c7cb1cbb/docs/ai/README.md)




## Dependency order confirmed (8 November 2026)

The user explicitly placed the core lifecycle guide in `docs/ai/tasks/` and directed implementation of that core API first, followed by this desktop integration. The earlier prerequisite blocker is resolved. Base this task on the pushed core integration commit as specified in Section 3.

## Implementation report

**Task:** `desktop-index-lifecycle`

**Completed:** 8 November 2026

**Desktop UI base:** `9a94bc90d0`

**Core API dependency:** `integration/index-lifecycle` at `a39434ca00`

**Integration branch:** `integration/desktop-index-lifecycle`
**Integration worktree:** `/home/simon/source/objekte-integration-desktop-index-lifecycle`

The desktop now holds the active `IndexedRepository` and calls its lifecycle API for file checks, compaction, rebuild, explicit save, cache replacement, and automatic scheduling. Its backend resolves configuration and concrete cache paths once per session. A plain folder opens in memory with no hidden save destination. A configured repository detects schemas and its default cache; compatible cache data becomes searchable before the initial background file check completes. Load or rebuild and save has an explicit recovery policy for incompatible caches. Save as makes an independent copy; it does not silently retarget automatic saving.

The Index panel shows current source, cache load/save destinations, file and record counts, progress, failures, and separate recurring check/save settings. Operations use controller workers instead of GPUI render callbacks. Check, maintenance, load, save, query, export, and batch work have separate operation identities and cancellation guards. A publication invalidates old result contexts; refresh preserves the editor draft, uses a pinned new view, and replaces the visible batch coherently. A deep result position falls back to the first page because the core has no efficient ordered anchor lookup. Record tabs keep canonical identity and their originating query; stale navigation resolves against the latest view, and a deleted record stays open with an unavailable state. Exports and batches retain their captured query, destination, cancellation, and index view.

Changing to a different repository resets old tabs only after the replacement opens successfully. Replacing an index for the same repository preserves tab and query definitions while invalidating old data contexts. Automatic file checking is opt-in after the mandatory first cache check. Automatic saving requires a configured repository and selected destination; settings persist per repository. Manual-mode close has no implicit cache write. Automatic-mode close waits off the UI thread for a final save and offers Retry, Cancel close, and Close without saving index on error. Source files are unaffected by an index save failure.

Contributing task branches and pushed implementation commits:

| Contribution | Branch | Commits merged into integration |
| --- | --- | --- |
| Core prerequisite | `integration/index-lifecycle` | `a39434ca00` (base) |
| Desktop lifecycle adapter | `task/desktop-index-lifecycle/adapter` | `d44964b7e2`, `8f2cca01b2`; merge commits `a0e59b1dad`, `2f23810dd9` |
| Index controls and preferences | `task/desktop-index-lifecycle/controls` | `fd42490dd3`; merge commit `a69b8cd961` |
| Revision-safe query, tab, and output adapters | `task/desktop-index-lifecycle/navigation` | `17b02f372a`; merge commit `1a779b9eef` |
| Supervisor integration, final save, and status redraw fix | `integration/desktop-index-lifecycle` | `bc64e2772d`, `b940d7c8f5`, `94cbba9b25`, `c74268725c` |

`cargo test --locked -p objekte-desktop --lib --quiet` passed 109 tests. `cargo test --locked --workspace --all-targets` passed across core, CLI, TUI, and desktop targets after the final code change. `cargo check --locked -p objekte-desktop --bin objekte-desktop --quiet` and `git diff --check` passed. Repository-wide format checks still report pre-existing, unrelated formatting differences; touched Rust files were formatted locally without rewriting the repository.

The [validation report](../generated/desktop-index-lifecycle/validation.md) records the Fedora 44 native Sway run at 1920 × 1080, 100% scaling and 60 Hz, a 1100 × 700 floating window, dark/light screenshots, cached startup, refresh/removal, partial file check, save failure and successful retry, benchmark commands, raw data, and environment. The final binary was reopened on the 3,000-record fixture, then a generated 100,000-file fixture; its query completed while a rebuild was still running. A separate in-memory native build of 148,486 Musenalm records was also queried. That large debug run exposed redundant status redraws, fixed in `c74268725c` with a focused event test. The existing release `bench_lifecycle_mutations small` example was compared at the core base and pre-redraw-fix desktop integration revision with three runs each. Its median flush was 1.197/1.204 ms, restore-to-searchable 0.732/0.614 ms, and no-change reconciliation 0.033/0.039 ms (base/current). The tiny fixture is a smoke comparison; the core prerequisite's separate large-fixture report covers full Musenalm behavior. The old desktop UI has no comparable maintenance interaction, so a numeric pre-integration “query during maintenance” result is unavailable. The native screenshot's query timing excludes SSH capture latency.

No additional dependency or repository source-data write was introduced. The native test fixtures live in ignored `target/`; their temporary permission changes were restored. The toolkit's screen-reader announcements were not observable through headless Sway, and forced process termination cannot guarantee a final save. The completed task document and validation artifacts are committed on this integration branch; the branch and integration worktree remain for the user's merge. Completed subordinate worktrees and temporary task memory were removed after integration validation and push.
