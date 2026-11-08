# Objekte initial desktop application implementation

Date: 8 November 2026  
Task identifier: desktop-application-initial

Implement the native GPUI frontend after the GUI repository migration has been merged into the primary branch. Build on the existing GPUI/GPUI Kit package, working headless Sway environment, control scripts, and documented operating knowledge.

**Scope: the same functionality as the existing TUI crate, plus the full functional capabilities of the CLI. Source documents remain read-only.** The merged CLI and TUI define the feature list. Deliver those capabilities through conventional desktop controls; do not expand this into an editor or a broader product redesign.

## 1. Start from the completed migration

Start only after the previous integration/gui-repository-migration work has been merged. Use the actual primary branch, whether main or the existing master; do not rename branches or perform the user's merge.

Read AGENTS.md, docs/ai/README.md, the code guidelines, the merged migration report, and docs/development/gui_environment.md. Verify that the desktop package, environment scripts, validation example, and operating knowledge are present. Record the base commit. A squash/rebased merge is acceptable when the equivalent integrated files and completion record are verified.

Update task-owned service paths through the existing command if they still point at the migration worktree. Verify the established GUI loop once. Reuse packages, dependency versions, graphics configuration, and build caches. Do not repeat setup experiments or revive the separate gpui-development-environment repository.

If migration is not merged, report the prerequisite and leave implementation unstarted. This is a new application task, not further work on the migration branch.

## 2. DBDelve is the presentation reference

Use **DBDelve** as inspiration for layout, information density, and normal desktop interaction:

- Project and demonstrations: https://github.com/ShayanAbbas1/dbdelve
- Website: https://dbdelve.github.io/
- Query view: https://github.com/ShayanAbbas1/dbdelve/blob/main/assets/screenshots/query-dark.png
- Record view: https://github.com/ShayanAbbas1/dbdelve/blob/main/assets/screenshots/table-dark.png

Inspect the current screenshots and demonstrations before implementing the layout. Adapt the navigation, query controls, result grid, and record panel to Objekte. Use existing GPUI Kit components. DBDelve guides presentation; its additional features are not requirements, and copying its code/assets is not required.

| Reference concept | Objekte presentation |
| --- | --- |
| Object navigation | Repository and collection selection |
| Query area | Existing TUI search/filter controls and CLI operation options |
| Result grid | Virtualized results with existing column and ordering behavior |
| Record panel | Existing preview, detail, source, diagnostics, and relation workflows |
| Execution status | Loading, query activity, cancellation, counts, and diagnostics |

A single main workspace is sufficient. Multiple query tabs, project management, dashboards, extra connection systems, and custom navigation patterns are not required. Ordinary window resizing, mouse input, keyboard navigation, and readable controls are part of a usable GUI.

## 3. Functional parity is the acceptance contract

Inventory the merged CLI argument definitions, query/input/output handling, TUI actions, key handlers, settings, record views, exports, and tests. Record the baseline commit. Maintain a parity table with the existing capability, source/test reference, desktop control or equivalent workflow, and verification result.

The tables below are the inspected starting point. Complete the inventory against the merged code. Do not omit existing capabilities because they are absent from these tables, and do not expose unrelated core APIs merely because they exist.

### TUI parity

| Existing capability | Required graphical equivalent |
| --- | --- |
| Collection scope | Select one collection or all collections; preserve single-folder and configured-repository behavior |
| Search and filter | Separate full-text search and structured-filter inputs that can be combined, as in the TUI |
| Search options | Literal/regex and case-sensitive/insensitive controls with the same defaults and semantics |
| Execution | Debounced live searching, explicit execution/confirmation, cancellation, and stale-result rejection |
| Ordering/navigation | Sort selection, inline SORT BY and its conflict rules, bounded result navigation, and keyboard selection |
| Columns/preview | Show/hide columns, horizontal navigation, preview visibility, and existing match indications |
| History/saved queries | Recall history, save named queries, and restore text, filter, collection, case, and regex state with existing semantics |
| Record detail | Structured/raw views, object/array expansion, metadata/diagnostics, and section navigation |
| Detail navigation/search | Previous/next result, in-record find with next/previous match, relation traversal, and Back |
| Clipboard | Copy the selected field/relation value and the complete record |
| Export | Selected-record and all-match exports using the existing TUI JSON representation and query-range semantics |
| Interface actions | Action menu, keyboard help, mouse/keyboard navigation, light/dark theme, and existing persisted preferences |

### CLI parity

| Existing CLI capability | Required graphical equivalent |
| --- | --- |
| --path, repeated --path, --repo | Open a single folder, multiple folders as separate collections, or a configured repository; preserve naming, overlap, root, and configuration rules |
| --schema, --no-recurse | Choose an existing schema for the supported single-folder mode and control recursion; preserve incompatible-option validation |
| Direct query and --fts | Full accepted structured syntax, literal full-text input, and quoted JSON-string interpretation for CLI-style input |
| --query-file FILE and --query-file - | Execute a query file or pasted line-based batch with equivalent parsing, line numbers, ordering, and per-query outcomes |
| --limit, --offset, --count | Explicit ranges, unlimited results, zero-limit/count-only operation, and exact counts; viewport size is not a total-result limit |
| --auto-index | Explicit automatic inference/index construction with current behavior |
| --index-alphabet ascii/unicode | Existing alphabet selection, independent of exact query case sensitivity |
| --load-index, --write-index, custom directory | All snapshot policies, default/custom paths, index-only execution, and supported combinations |
| --diagnostic / --diagnostics | Operational diagnostics, inference/index findings, counters, timings, and errors |
| Complete output | Original-source payload and metadata semantics, fallback/unavailable-data handling, and CLI-compatible NDJSON for direct/batch execution |
| Cancellation/failures | Equivalent visible query/run success, cancellation, and input/startup/output failures; no false success for incomplete output |
| Help/version and --tui | Discoverable help/version; the GUI supplies interactive presentation rather than launching a terminal screen |

Terminal transport need not be imitated literally. File/paste input can replace stdin, explicit NDJSON export can replace stdout redirection, and visible status/diagnostics can replace exit codes and stderr. Preserve the underlying semantics and data. Keep the original CLI and TUI working.

### Read-only boundary

Do not implement source-document or cell editing, record creation/deletion/rename, schema editing, media mutation, Git write controls, synchronization, or background source rewrites. Read-only is the intended scope, not a placeholder pending editor implementation.

Preserve existing saved-query/preferences, export, and generated-index capabilities. These write application state or derived output, not edited source records. Label actions precisely: **Save Query**, **Export Results**, and **Write Index**. Do not provide a document Save action.

Keep output separate from scanned source data. Exports must not overwrite source files or collection schemas or add generated records to a scanned collection. Preserve the TUI's non-overwriting export behavior and CLI snapshot-directory validation/exclusion rules. Ordinary opening/searching must not silently write snapshots; explicit index operations may do so. Existing local settings/cache behavior remains supported.

## 4. Repository and agent workflow

All code belongs in the existing Objekte desktop package, normally crates/desktop, using its migrated package name, initialization, assets, and root Cargo workspace/lockfile. Keep the environment-validation example separate from the production entry point.

Keep the original primary checkout and index unchanged. Use:

- Integration branch: integration/desktop-application-initial.
- Integration worktree: /home/simon/source/objekte-integration-desktop-application-initial.
- Subtask branches: task/desktop-application-initial/<subtask>.
- Subtask worktrees: siblings under /home/simon/source, following repository conventions; never under /tmp.

Use an integrator and delegate separable work after shared service/state interfaces are agreed:

| Owner | Responsibility | Boundary |
| --- | --- | --- |
| Integrator | Parity inventory, shell/options, integration, review, and GUI acceptance | Owns shared contracts/manifests, integration worktree, and live test session |
| Data integration agent | CLI loading/options, queries/batches, snapshots, diagnostics/output, cancellation, and record/relation access | Owns agreed services; no independent UI state model |
| TUI interface agent | Results/detail, search controls, history/saved queries, columns, clipboard, and export controls | Uses shared services and stable identities; no duplicate execution engine |

Give each agent a recorded base commit, file ownership, interfaces, and acceptance checks. Only the integrator merges into the integration branch. One owner operates the test display and host services.

Follow the merged docs/ai/README.md for commits/pushes, task documents, notes, validation, reports, and cleanup. Push task/integration commits as required there. Never commit to, merge into, or push updates to the primary branch. Leave one integration branch/worktree for the user's merge.

## 5. Desktop interaction requirements

### Opening and operation options

Provide a conventional Open flow with a supported folder chooser and path-entry fallback for headless use. Expose single-folder, multiple-folder, and configured-repository modes. Place schema, recursion, inference, alphabet, and snapshot options in a compact dialog or settings area. Reuse existing validation and collection configuration; do not invent another file format or silently ignore incompatible options.

Preserve snapshot semantics:

| Selection | Behavior |
| --- | --- |
| No snapshot operation | Build in memory; leave snapshots untouched |
| Write only | Build fresh and save |
| Load only | Require a valid snapshot; do not rebuild/save silently |
| Load and write | Restore a compatible snapshot, or rebuild/save for the existing recoverable cases |

Preserve custom-directory resolution, index-only operation, and fatal I/O/permission errors. Use the merged loader's behavior as authoritative.

Show the window before loading/index construction finishes. Display actual progress where available, otherwise an indeterminate status. Distinguish empty data, loading, results, cancellation, and errors. Index work and exports must not block interaction.

### Search and batch execution

**Do not make Full Text and Structured mutually exclusive modes.** Preserve the TUI's separate search and filter inputs and their combined execution. Include independent regex and case controls.

Match debounced live searching and explicit confirmation. The inspected TUI uses a 180 ms debounce; start from the merged implementation and retain bounded background execution. Cancellation and request identities prevent an older operation from replacing newer results.

Pass the full accepted query language to the existing parser without implementing a reduced subset. Preserve ordering expressions and CLI-style quoted literal interpretation. Show errors beside the affected input while retaining the entered text. Do not reinterpret failed structured input as full-text search.

A query file or pasted batch follows the CLI's UTF-8/BOM, line-ending, blank-line, physical-line-number, and per-line parsing rules. Continue after individual query errors where the CLI does, with the same run outcome semantics. Use a bounded list of per-query results/status; do not build a new scripting language or notebook.

### Results, ordering, counts, and exports

Virtualize result rows and retain bounded pages/previews. Keep requested offset/total limit separate from display-page size. Unlimited output must remain available without retaining every matching document body.

Provide sort controls as well as inline SORT BY, preserving their existing conflict rules. Apply ordering to the full query result through the core, never only the visible page. Support existing column selection, horizontal navigation, row selection, preview visibility, and match evidence.

Exact totals remain available as in the frontends. If rows appear while a requested count is pending, label it as pending and provide the exact result when complete. Do not silently omit explicit counting or display candidate estimates as totals.

Support both TUI JSON export forms and CLI NDJSON output. Preserve complete record payloads, metadata, fallback/raw representations, applicable query range, and required query/run events. An export must include the requested result set, not only the visible columns or loaded rows. Reuse encoders and bounded execution; report output errors and cancellation accurately.

### Inspection, history, and interface actions

Preserve structured/raw detail views, expansion, metadata, diagnostics, previous/next record, section navigation, in-record find, copy-value/copy-record, and relation traversal/Back. Malformed/fallback records must remain inspectable; distinguish JSON null from unavailable logical data. Clearly identify structured representations versus original source.

Keep detail trees, source previews, and relation pages bounded. Use existing relation APIs; do not rebuild backlinks by scanning every file.

History and named saved queries are required, including the existing restoration and persistence/update semantics. Retain theme and column preferences. Use a dedicated desktop settings file or a deliberate compatible shared representation; do not accidentally overwrite TUI-specific state.

Expose every existing action through ordinary controls/menus and keyboard help. Adapt shortcuts where native desktop text editing requires it, while preserving actions. Maintain normal focus traversal, selection, clipboard behavior, and scrolling. Additional query tabs, sessions, or elaborate preference systems are outside the required scope.

## 6. Integration and performance

Use a thin desktop adapter around the existing core and reusable frontend logic. Do not spawn the CLI for each query, duplicate the search engine/index, or introduce an HTTP/server layer.

Inspect these sources after the prerequisite merge:

| Area | Source |
| --- | --- |
| CLI options and validation | crates/cli/src/arguments.rs |
| CLI parsing, loading, execution, and output | crates/cli/src/query_input.rs, execution.rs, output.rs, and loader helpers |
| TUI actions/state | crates/tui/src/tui/app.rs and mod.rs |
| TUI settings, inspection, relations, exports | settings.rs, record.rs, relations.rs, export.rs in that directory |
| Core opening and retrieval | crates/core/src/repository.rs |
| Core queries, cancellation/streams, relations | query.rs, streaming.rs, relation_inspection.rs |

Reuse current tests and semantics. If necessary, extract the smallest non-terminal helper for configuration, request construction, or encoding. Do not duplicate large frontend sections or redesign the core API to build the GUI.

Keep discovery, indexing, queries, parsing, file reads, export, and snapshot operations off the GPUI event/render thread. Respect actual ownership and Send/Sync constraints; use worker ownership/message passing where needed. Do not force thread safety with unsafe code or hold locks across rendering/long operations.

Retain the repository and its caches across searches and collection changes. Track generation/request identities and batch/query identities where relevant. Cancel superseded work and reject late results. Opening another repository or changing scope must not permit stale data to replace current state.

Bound UI caches by both record count and bytes. Avoid repeated decoding/copying of the same document on every frame. Large records and deep structures need bounded previews/expansion.

Use existing bounded streaming for full output where available. Functional parity does not require reproducing inefficient implementation details such as repeated export-page rescans. Preserve result and output semantics while avoiding unbounded accumulation.

Measure any full-scan, global sort, deep-offset, copying, or allocation limitation affecting responsiveness. Explain material data-model limitations. Do not silently omit an existing capability or weaken correctness to meet a timing target. Limit shared-code changes to those necessary for this task.

## 7. Phases and reviewable commits

| Phase | Deliverable | Suggested commit |
| --- | --- | --- |
| 0 | Confirm merge, inspect DBDelve, inventory CLI/TUI capabilities, agree interfaces | Define desktop CLI and TUI parity requirements |
| 1 | Shell, all data-opening modes, background load, collection selection, real results | Add desktop repository and collection loading |
| 2 | Combined queries, regex/case, live execution, sorting, ranges/counts, cancellation | Implement desktop query and result parity |
| 3 | Full TUI inspection/navigation, history/saved queries, columns, clipboard, theme/help | Implement desktop TUI interaction parity |
| 4 | Batch queries, index operations, diagnostics, TUI exports, and CLI NDJSON | Complete desktop CLI operation and output parity |
| 5 | Close parity inventory, combined checks, GUI verification, and documentation | Validate and document desktop frontend parity |

Split phases into coherent commits and follow repository check/push rules. Integrate delegated changes early. Phase 1 is a working intermediate milestone; it is not task completion. Keep cosmetic work and unrelated features out of the critical path.

## 8. Verification and acceptance

Use the existing headless Sway loop at 1920 × 1080, scale 1, nominal 60 Hz. Launch the production application through the migrated scripts, retaining the separate validation example.

Compare the GUI against the CLI/TUI at the recorded merged baseline. For every parity row, record the desktop action, fixture/input, expected behavior, and verification result. Equivalent presentation is acceptable; unexplained missing behavior is not. Compare deterministic output separately from timestamps, timings, and other run-specific fields.

Use disposable fixtures covering single/multiple folders, configured repositories, schema/discovery options, snapshots, scalar/array/null roots, malformed JSON, large records, ordering, and resolved/unresolved relations. Use Musenalm for scale checks when available. Keep input documents/schemas unchanged.

Verify through graphical input and inspected screenshots:

1. Every opening mode, invalid option combinations, loading, and single/all collection scope.
2. Combined search/filter, regex/case controls, live/explicit execution, sort conflicts, offsets, zero/finite/unlimited limits, and exact counts.
3. Result navigation, columns, preview/detail, structured/raw views, expansion, find/copy, adjacent records, relations, and Back.
4. Named-query saving/restoration, history, theme/preferences, action menu, and help.
5. File/pasted batches, line-specific failures, correct continuation, cancellation, and stale-result rejection.
6. Inference/alphabet options, index-only work, all snapshot modes, custom paths, and diagnostics.
7. Both TUI JSON exports and complete CLI NDJSON, including records outside the viewport, fallback data, counts, event order, and output failures.
8. Fresh-SSH operation and the read-only boundary. Test explicit index/export writes separately in designated output locations.

Use focused automated tests for adapters, restored query state, cancellation, ordering/ranges/counts, batch parsing, output fidelity, and source-data preservation. Run required workspace tests, Clippy, formatting/spacing checks, and relevant benchmarks. Reuse existing fixtures/frameworks; record unavailable checks accurately.

Measure startup separately from indexing, query work separately from UI preparation/rendering, first-page display separately from exact counts/full export, and local execution separately from SSH transfer. Record dataset size, cache/snapshot conditions, build mode, GPU, and memory. Prioritize warm interaction with a loaded repository and compare equivalent existing frontend/direct-core operations.

All-results execution must not make UI document storage grow with the complete result set. Scrolling/input must remain responsive during background operations. Investigate material regressions and report substantial unresolved limitations before completion.

## 9. Completion and retained knowledge

Deliver one Objekte integration branch with the application and the completed task/report following docs/ai/README.md. Include the base commit, contributing branches/commits, **completed CLI/TUI parity table**, validation evidence, benchmark comparisons, and limitations. A required parity gap blocks completion unless the user explicitly changes the scope.

Update the operating guide with launch commands, capability mappings, read-only behavior, derived-output locations, architecture, and verified troubleshooting knowledge. Preserve the existing environment knowledge. Explain useful departures from the DBDelve presentation reference.

Keep the integration branch/worktree for the user. Remove subordinate worktrees only under repository preservation rules. Document updating service paths when switching to the merged primary checkout.

Completion means full existing TUI functionality and CLI capabilities in a responsive native desktop application, with source documents read-only and no unrelated feature expansion.

## Reference material

- AI workflow: https://github.com/Simon-Martens/objekte/blob/master/docs/ai/README.md
- CLI behavior: https://github.com/Simon-Martens/objekte/blob/master/docs/cli/obj.md
- CLI arguments: https://github.com/Simon-Martens/objekte/blob/master/crates/cli/src/arguments.rs
- TUI actions/state: https://github.com/Simon-Martens/objekte/blob/master/crates/tui/src/tui/app.rs
- TUI action handlers: https://github.com/Simon-Martens/objekte/blob/master/crates/tui/src/tui/mod.rs
- DBDelve: https://github.com/ShayanAbbas1/dbdelve

The merged checkout and pinned dependencies are authoritative. These source references identify the current implementation to inventory, not a frozen API contract.

## Implementation report

Task identifier: `desktop-application-initial`
Completion date: 8 November 2026
Base: `master` at `1da55f0c6d9dce7d2cb73430c2d67d974577fcce`
Integration: `integration/desktop-application-initial` at `/home/simon/source/objekte-integration-desktop-application-initial`

### Delivered

The production `objekte-desktop` binary is a native GPUI Kit window with the requested read-only CLI/TUI capabilities. It opens one folder, several folders, or a configured repository; retains the loaded repository across searches; combines Full Text and Structured inputs; provides case/regex, sort/range/count, bounded grid pages, detail/source/relation navigation, history, saved queries, columns, theme/help, file/pasted batches, snapshots, diagnostics, TUI JSON exports, and CLI NDJSON. The existing CLI loader, core engine/streaming, CLI event encoder, and TUI record/export/relation/settings helpers supply the semantics. Source documents and schemas are never edited. Explicit exports cannot target scanned source directories or existing files; desktop preferences and snapshots use separate derived-output paths.

The application uses background workers and generation checks for opening, query, detail, batch, export, snapshot, and preference work. The GPUI window holds bounded display pages and detail content. Complete export/clipboard payloads are acquired off the render thread. The headless launcher verifies 1920 × 1080 at scale 1 and nominal 60 Hz; WayVNC runs with `-R` so a viewer connection cannot resize the output. Production control succeeded from a fresh SSH login. The [operating guide](../../development/gui_environment.md) gives launch and service-path update commands. The [53-row parity table](../generated/desktop_application_parity.md) records each control/input, expected behavior, source/test, and observed result; [native screenshots](../generated/desktop_application_evidence/) are committed with it.

The DBDelve reference guided the single workspace's collection sidebar, query area, result grid, and right record panel. SQL, connection management, multiple query tabs, and editing were outside the authorized Objekte scope.

### Branches and commits

- Data integration: `task/desktop-application-initial/data-integration`, pushed head `948d002ac7`, merged into integration at `416ab55bc3`. It supplied CLI loading, batch, NDJSON, snapshot, query, and output adapters.
- TUI interface: `task/desktop-application-initial/tui-interface`, pushed head `e4949951ba`, merged into integration at `2b8e4bfe30`. It supplied bounded GPUI controls, preferences, detail, query, history, and theme.
- Integrator: scaffold `85a8dfaf56`, baseline inventory `4c3858d7ee`, launcher `917b9839fb`, controller `3411e16ee5`, inspection/operation completion `1f2033b75b`, GUI error regression `71280e6d81`, Wayland clipboard completion `2a97e02322`, and parity/evidence `219e647827`. Intermediate phase commits and merges remain in the integration history. The primary branch and original tracked checkout/index were not changed.

### Validation

- `cargo test --locked --workspace`: passed on the combined integration result. The desktop crate has 35 library tests; existing CLI, TUI, and core suites also passed.
- `cargo clippy --locked --workspace --all-targets`: passed. Existing warnings remain in core/CLI/TUI; no desktop warning was reported. A strict warnings-as-errors pass remains unavailable because of those pre-existing warnings.
- `python3 scripts/format-rust-spacing.py --check` and `git diff --check`: passed.
- `bash scripts/gui/run_desktop.sh prepare`, `desktop_control.py status`, native input/screenshots, and `run_desktop.sh cleanup`: verified. The launcher reported 1920 × 1080, scale 1, 60 Hz; all 25 committed screenshots have those dimensions. A fresh `ssh localhost` login independently ran production status and screenshot commands. The prior actual RFB viewer connection left the mode unchanged; WayVNC's `-R` disables viewer-driven resize.
- Disposable GUI fixtures covered folder modes, conflicts, combined/regex/case queries, global sort/ranges/counts, malformed/null/nested records, relation traversal/Back, history/saved queries, JSON/NDJSON exports, BOM/CRLF batch line numbers and errors, snapshot policies, and source-output rejection. Musenalm covered seven collections and 148,486 files. Clipboard inspection with `wl-paste` returned complete Beta source JSON and selected field `8`.
- The release `bench_musenalm` smoke benchmark used `--root examples/musenalm --extended --cases Q035,Q045 --profiles ascii-off --repetitions 1`: Q035 106.819 µs and Q045 357.540 µs; build 1.125 s, snapshot write 0.245 s, peak 680,779,776 bytes, restore median 0.527 s. Results are under ignored `target/gui-validation/desktop/bench-musenalm-integration/`.
- Comparable no-snapshot debug CLI runs on the same corpus and `--fts Musenalm --limit 1 --diagnostic`: base 54.83 s / 571,612 KiB peak; integration 55.57 s / 580,680 KiB peak. These are single measurements (+1.35% elapsed, +1.6% memory), with no core source change. GUI cold debug open took 56.5 s and about 711 MiB RSS; the first 100-row page after opening took 8.1 ms. Host: Fedora 44, Rust 1.99.1, Intel RPL-S Mesa Vulkan, Sway headless. Release core results are not directly comparable to debug GUI opening. SSH screenshot transfer was not timed.

### Limitations and cleanup

Cold debug indexing of Musenalm takes about a minute and substantial memory; it remained off the render thread, and warm queries were responsive. Repository construction supports cancellation before and after the CLI loader call; a canceled cold open may keep using its worker until that call returns, while generation rejection keeps the window responsive and discards its result. Queries, batches, and exports use cooperative cancellation. GUI pages and detail text are intentionally bounded while exports remain complete. On the tested headless Wayland seat, delayed GPUI clipboard writes stayed internal; the app now sends worker-produced clipboard values through the already installed `wl-copy` command on a background thread, and the launcher checks for it. Viewer resize requests from every possible VNC client and the final-SSH-logout persistence case were not separately exercised; the tested viewer connection kept the fixed mode. No required parity gap or unresolved in-scope failure remains, and no separate issue report was needed.

Both subordinate worktrees were clean, pushed, merged, and removed normally after the first report push. The task-owned desktop window and WayVNC pointer server were stopped; Sway remains available for reuse. The integration branch/worktree are retained for the user's merge. Ignored task memory and the obsolete untracked task source in the original checkout were removed after verifying that the pushed report preserved its exact specification. The original tracked files and index remain untouched. After merging into the primary checkout, run `python3 scripts/gui/headless_session.py update` from that checkout to retarget the user service, then verify `status` and launch again.
