# Objekte desktop interface redesign

Date: 8 November 2026

Task identifier: desktop-interface-redesign

Reviewed baseline: master at 4e15721d3853a6c93e9b7b4bb73e18233144effa
Companion: objekte_desktop_visual_reference.pdf

## Objective

Make Objekte a compact, modern, intuitive native data browser, with DBDelve as the visual and interaction reference. The ordinary workflow is **open data → choose a collection → search → inspect a record**. Give that workflow priority; retain the existing CLI/TUI capabilities through contextual controls and advanced dialogs.

Source records and schemas remain read-only. Saved queries, preferences, exports, and explicitly requested index writes remain supported. This is a presentation and interaction task in the existing Objekte repository and GPUI package. Keep the working core, backend, headless Sway environment, and existing feature parity.

The companion PDF specifies four target screens: the populated workspace with an editable filter line, the opening dialog, the synchronized filter builder, and an invalid filter state. Its records, counts, and timings are illustrative. It is a design reference, not evidence of implemented changes.

## 1. Findings at the reviewed baseline

The code and committed acceptance screenshots establish the following issues. The captures in docs/ai/generated/desktop_application_evidence predate the latest usability commit: current master already closes the opening form after successful loading, uses fixed-height ellipsized grid rows, improves scrolling, and starts with a dark welcome screen. Preserve those improvements. Do not use the older captures to claim those problems still exist.

The native application was not executed during this review. Validate the resulting implementation on the existing tested GUI host; repository screenshots alone cannot establish current interaction or visual correctness.

| Location | Observed implementation | Required correction |
| --- | --- | --- |
| ui/panels.rs: render_open_options | A full-width layout panel offers three modes, a multiline path field, schema, recursion, inference, alphabet, diagnostics, four snapshot buttons, and two execution actions | A compact modal with folder selection first and advanced settings collapsed |
| controller/runtime.rs: choose_folder and WorkerEvent::Picked | A native picker already exists, but its result only fills the separate form | Reuse the picker in a coherent Open dialog; avoid another picker implementation |
| ui/render.rs: render_query | Stacked Full Text and Structured fields followed by checkbox, execution, sort, offset, limit, column, and preview rows | One persistent search row; an optional second free-text filter line with a builder button on its right; sort/columns near the table; ranges in query options |
| ui/render.rs: render_sidebar | Each collection is a full-width button | A normal navigation list with small icons, left-aligned labels, and one selection highlight |
| ui/render.rs: render_grid | Every data column is 160 px; the header is inside the vertical scroll content; selected match evidence can add another line | A stable grid with a stationary header, useful widths, consistent row height, and match details in the inspector |
| ui/panels.rs: render_detail | Preview and field/source presentations compete with multiple control rows in a fixed-width panel | One readable inspector with Fields/Source tabs and contextual record actions |
| ui/panels.rs: render_actions; ui/state.rs | History, saved queries, columns, exports, and diagnostics share an expanding bottom panel; independent booleans allow several large panels together | Dedicated menus/popovers/dialogs that do not progressively squeeze the table |
| ui/render.rs and ui/panels.rs | Repeated hard-coded colors and similar button emphasis across unrelated actions | Shared theme tokens, typography, spacing, borders, and action hierarchy |
| ui/render.rs: render_grid; ui/panels.rs: render_detail | Full bounded page/detail values are cloned while rendering | Avoid copying retained record payloads merely to redraw controls; measure before and after |

Feature parity is not a requirement to display all controls simultaneously. The previous parity matrix remains the functional checklist; this task changes where and how those capabilities are presented.

## 2. Visual direction and layout

Inspect [DBDelve's table view](https://github.com/ShayanAbbas1/dbdelve/blob/main/assets/screenshots/table-dark.png) and [query view](https://github.com/ShayanAbbas1/dbdelve/blob/main/assets/screenshots/query-dark.png), together with the supplied visual reference. Borrow their compact navigation, dominant data grid, subtle separators, contextual actions, and restrained inspector. Keep Objekte terminology and native GPUI controls. Do not add SQL connections, editing, multiple document tabs, transparency effects, or a new theme engine.

Use these initial dimensions in logical pixels and tune against actual screenshots:

| Element | Target |
| --- | --- |
| Application header | 40–44 px; repository basename/breadcrumb, small Read-only indicator, Open, application menu |
| Collection sidebar | About 210–230 px; resizable or collapsible; 30–32 px navigation rows |
| Search toolbar | 44–52 px; search field grows to use available space |
| Filter line | A 32–36 px free-text input directly below search when enabled, with a right-hand builder button and a compact validation message below |
| Table header and rows | Header 32 px; rows 28–32 px; no record-body wrapping in the grid |
| Record inspector | Initially 330–380 px, resizable; absent until a record is selected or explicitly opened |
| Status/pagination | Compact 28–36 px footer; no permanent diagnostic log |
| Open dialog | About 560–640 px wide; vertical scrolling only when advanced options need it |
| Typography | Interface 13–14 px, secondary labels 11–12 px; monospace for code, IDs, and raw source |
| Spacing and shape | 4/8/12/16 px spacing; 4–6 px control radius; 1 px separators |

At 1920×1080 and 1440×900, ordinary searching should leave at least 70% of the central workspace height available to the grid, including its header and footer. Opening a menu must not reduce this height. At 1280×800, move secondary toolbar actions into overflow and allow collapsing the sidebar/inspector. Support 1024×768 without unreachable controls or whole-window horizontal scrolling; horizontal scrolling belongs inside the table.

Use the PDF's restrained charcoal/blue direction as the initial dark palette: canvas #17191d, panels #1c1f24, raised controls #24282e, borders #30353c, text #e5e7eb, secondary text #a0a7b1, selected rows #293d55, accent #8bb8f4. Provide corresponding coherent light tokens through the existing theme mechanism. Preserve the stored theme preference. Verify readable text, keyboard focus, disabled states, and selection in both themes.

Use mostly flat controls and list rows. Reserve a filled primary button for the current dialog's confirmation. Avoid oversized rounded cards, decorative gradients, large empty headings, and treating every command as a permanently visible button. Icons need accessible labels and tooltips; color alone must not distinguish enabled search options or errors.

## 3. Opening data

### Default flow

The welcome screen has a primary **Open data…** action and a short explanation. Ctrl+O and the header Open action open the same modal.

The default dialog contains a single-line Folder input, Browse…, an optional compact detection summary, Add another folder, collapsed Advanced options, Cancel, and Open. Browse reuses the current GPUI native directory picker. Typing or pasting a path remains a complete alternative when desktop portals are unavailable over SSH. An empty path must not silently open the process working directory.

After choosing a folder, detect whether it contains Objekte collection configuration using the existing repository configuration logic. Run detection asynchronously with a generation guard and without building an index or walking all source files. If present, show “Objekte repository detected” and use configured-repository mode. Otherwise use plain-folder mode. Do not silently fall back to plain-folder mode when a detected configuration is malformed or unreadable; show the error and provide the explicit mode override under Advanced.

Add another folder changes the dialog into a list of selected folder rows with Add and Remove actions. The list is a multiple-folder opening operation, not a merge of configured repositories. Explain that distinction when moving from a detected configured repository; retain the existing naming, overlap, and common-root validation. Do not use a multiline text box as the normal multi-folder picker.

Cancel, Escape, and a canceled native picker leave the current workspace unchanged. Path detection never opens a dataset by itself. Open starts loading once; successful loading closes the dialog. Failure preserves entered paths/options and displays an actionable error. A closed dialog must not apply a late picker/detection result.

### Advanced options

Keep the existing backend defaults unless the user explicitly changes them. In particular, default snapshot behavior remains in-memory build without snapshot writes. Simplifying the dialog must not silently enable inference, Unicode indexing, snapshot reuse, or disk writes.

| Advanced group | Controls and behavior |
| --- | --- |
| Source interpretation | Automatic / Folder / Configured repository; explicit override remains available |
| Folder discovery | Include subfolders; optional existing schema file with Browse; show only where valid |
| Index construction | Infer field indexes, index alphabet, diagnostic collection; explain purpose in brief help |
| Stored index | Build in memory; Load existing index; Build and save index; Load or rebuild and save |
| Stored index location | Default or custom directory, enabled only for a compatible stored-index operation |

Preserve None/Load/Write/LoadAndWrite semantics, fatal I/O errors, validation, and index-only capability. A collapsed group with nondefault settings shows a short summary. Inapplicable controls must not send stale hidden values: derive the execution request from the active mode, retaining inactive drafts only locally.

Move **Write Index…** to the repository/application menu with a focused options dialog. It is not a second primary action beside Open. Keep index-only execution available there without requiring a search.

## 4. Search and filters

### One persistent search row

Use one field labeled by a search icon and a scope-aware placeholder such as “Search entries…”. This is always the existing full-text search input. Do not guess whether entered text is a structured expression.

Keep case and regex toggles as compact trailing controls, conventionally Aa and .*, with clear pressed state, accessible names, and tooltips. Preserve the current case-sensitive default and the distinction between full-text options and structured predicate semantics.

Retain 180 ms debounced search and Enter for immediate execution/history confirmation. A compact Run query action remains available, especially for restoring or explicitly rerunning a query. Show Stop only while an applicable operation runs. Loading status must not move the toolbar or shift keyboard focus.

Search and filter continue to combine with AND. Collection changes reuse the open repository. Existing cancellation and generation checks must prevent stale results from becoming current.

### Free-text filter line

**The free-text box is the default filter interface.** Activating **Filter** reveals a second line directly below the search field and focuses it. This line contains a freely editable, full-width filter string and a **Build filter…** button on its right. Do not replace it with chips, a summary, or a Conditions/Expression tab switch. It stays visible and editable while filtering is enabled, including while the builder is open.

The input accepts the complete existing structured language: Boolean expressions, modifiers, arrays, related paths, and SORT BY. Use the existing input component with ordinary text selection, copy/paste, undo/redo, and horizontal navigation for long expressions. Keep the label Filter and a useful placeholder such as “year >= 1900 && type = "book"”. No new editor dependency or reduced query language is required.

Use the same 180 ms debounce and explicit Enter/Run behavior as search. Search text and an enabled filter combine with AND; the query executes only when the current combined request is valid. The free-text value is the single authoritative filter string. Keep its exact text and caret position while the user types; parsing must not reformat the field.

An empty filter is valid and adds no predicate. Clearing its text leaves the line open. Turning Filter off explicitly disables filtering, hides the line, and reruns without that predicate; retain its text as an inactive draft for re-enabling. A nonempty filter restored from history or saved queries enables and reveals the line automatically. Any persisted enabled/disabled state must agree with the executed query. Closing the builder never disables or clears filtering.

### Invalid filter states

Validate the current text with the existing parser and the repository's binding/type rules; also surface query-option conflicts such as duplicate sort definitions. Do not rely on syntax validation alone or scan source records merely to validate input. Identify pending validation, valid/empty input, and invalid input distinctly.

An invalid filter has a visible error border, an error icon/text label, and a concise message immediately below the field. Identify the offending range or position when the parser provides one. Use an accessible error announcement; color alone is insufficient. Incomplete expressions such as “year >=” remain editable and show “Expected a value after >=”. They are neither empty filters nor zero-result queries.

Do not execute an invalid combined request, ignore its filter, or silently substitute the last valid filter. Disable Run and query-dependent export while the current request is invalid or not yet validated. The text search remains editable. Retain the previous result page, if any, with **Showing last valid results** visibly attached to it; do not present its count or timing as a result of the invalid text. With no previous page, show the validation state instead of “No matching records”.

Every edit advances the request/validation generation immediately, even before the debounce completes. Cancel superseded query work and reject late validation/results that belong to older text. Once the combined request becomes valid, clear the error and execute the latest search/filter through the normal debounce or explicit run. Validation and execution remain off the rendering path where their cost requires it.

### Synchronized Filter records builder

The right-hand **Build filter…** button opens a small nonmodal popover titled **Filter records**, anchored below the button without covering the filter line. The free-text field remains the primary editor. The popover is an optional Field / Operator / Value helper for that same string, with Add condition and Close; it does not contain a competing Expression field or an Apply/Cancel transaction.

Start with an AND list of scalar equality/inequality and numeric comparisons supported by the core. Use a Boolean selector for known Boolean values. Offer fields from existing schema/index metadata and bounded available columns; allow manual field paths. Unknown or mixed types require an explicit value type rather than guessing whether digit characters are a string or number.

Synchronization is bidirectional:

1. On opening, parse the current text and populate the builder from the supported expression tree, including expressions typed manually or restored from a saved query. Opening the builder does not rewrite the text.
2. Editing the free-text field updates the builder whenever the expression is valid and exactly representable. Do not keep displaying stale condition rows as if they describe newly edited text.
3. Completing a valid builder change immediately updates the same text box, validates it, and schedules the normal search. Serialize field paths and typed literals correctly, including escaping. For example, year / is at least / 1900 produces year >= 1900. Do not stringify numeric values or introduce SQL syntax.
4. Incomplete builder rows remain local drafts and are visibly marked incomplete; they must not erase the last complete filter string. Text edits supersede builder drafts, and synchronization must avoid event loops, duplicate runs, caret jumps, and undo-history pollution. Each complete builder change is one meaningful edit in the text field's undo history; undo/redo updates the builder in turn.
5. Clicking the linked free-text field keeps the builder open so both remain usable together. Closing the builder, clicking outside both editors, or pressing Escape dismisses only the popover. Already synchronized changes stay in the text field. Return focus to that field; use ordinary text Undo to undo a builder change.

The builder is not a second query engine and need not visually represent every legal expression. A valid expression beyond its supported forms remains fully editable and executable in the text box. In that case, the popover says “This expression cannot be represented by the builder. Edit the filter text above.” Preserve the complete expression and show no misleading partial condition rows. Invalid text likewise shows the parser error and directs focus back to the field. Neither case authorizes dropping OR clauses, modifiers, array quantifiers, or inline sorting to manufacture a simpler filter. The normal valid/invalid field indicator depends on the query language, not on builder representability.

Keep the CLI-style quoted JSON-string query convention through the free-text filter line and batch input, with existing validation when combined with full-text input. Explain it in contextual help rather than the normal toolbar. Saved queries and history restore the complete text and options, with validation and builder state derived from that text.

## 5. Results and record inspection

The results table fills the workspace. Keep its column header visible during vertical scrolling and synchronize header/body horizontal movement. Preserve the existing bounded 100-row pages and keyboard navigation; infinite scrolling is not required for this task.

Use useful initial widths, with limited sampling of the already loaded display page. Allow resizing columns and avoid recomputing widths on each query or redraw. Retain column visibility preferences. Prefer useful record fields early; redundant collection metadata in a single-collection scope and full file paths can be hidden by default but remain available through Columns and record metadata. Do not hard-code Musenalm-specific field names into the generic application.

Sort from the column header using ascending/descending/clear actions. Route this through the existing global query sorting, not local page sorting. Preserve arbitrary sort expressions in the free-text filter line or a compact Sort dialog. If inline SORT BY already owns ordering, show that state and let the user edit it; do not append a competing sort silently. Existing invalid/conflicting requests still produce clear errors.

Put ranges and counting in Query options, with pagination in the table footer. Show “1–100” when total is unknown, “1–100 of 328” only when known, and a distinct pending indicator during counting. Keep zero limit, finite/unlimited output, explicit offset, and exact count. A nondefault range needs a visible summary. Do not request an expensive exact count on every keystroke merely to fill the footer.

A single click selects a row and updates the inspector when enabled. Keyboard navigation does the same. The inspector has:

- A compact title with previous/next record and Close controls; relation Back only when usable.
- **Fields** and **Source** tabs. Fields shows readable labels, values, types where already known, nested expansion, and relation links. Source retains the original-source distinction and read-only behavior.
- Find in record, complete-value copying, complete-record copying, and record export.
- Metadata/diagnostics in a secondary collapsible section.

Remove the duplicated raw preview above a full structured view. Preserve the preview capability through one inspector presentation and the existing show/hide action. Keep long values wrapped only in the inspector, with bounded source rendering. Move match evidence here or into a subtle cell indication; selection must not change grid row height.

Use the current relation APIs and complete copy/export paths. Do not use ellipsized display strings as output. Preserve null versus unavailable data and inspectability of malformed records.

## 6. Place remaining capabilities coherently

| Capability | New location |
| --- | --- |
| History and saved queries | Sidebar entries opening a searchable list/popover; query save in query actions with a small naming dialog |
| Columns | Anchored table-toolbar popover with checkboxes; no unrelated export/history panel |
| Export results and CLI NDJSON | Export… dialog: scope, existing format, destination, one Export confirmation |
| Export selected record | Inspector actions and export scope choice |
| Batch file/pasted queries | Application menu → Run query batch…; dedicated bounded dialog/view with per-query outcomes |
| Query ranges and exact count | Query options popover/dialog with visible active-state summary |
| Index-only/load/write operations | Repository menu → Index… |
| Detailed diagnostics | Status-bar action or menu → Diagnostics; closable drawer/dialog |
| Theme and help/version | Application menu, retaining existing shortcuts |

Use native destination prompts where supported and a path-entry fallback. Keep no-overwrite and source-directory protection, complete output schemas, event order, per-query failures, cancellation, and status feedback. Relocation is not permission to omit rarely used capabilities.

Only one primary modal is active at a time. Menus close on selection/outside click/Escape and return focus to the invoking control. Dialogs trap focus. Shortcuts must not steal normal text editing. Ctrl+F focuses search; Ctrl+L enables and focuses the filter line; Ctrl+O opens data; F1 opens help. Escape closes the nearest overlay before affecting workspace state.

## 7. Implementation boundaries

Read the current AGENTS.md, docs/ai/README.md, code guidelines, existing parity record, and GUI environment guide. If master has advanced, inspect the intervening desktop changes before editing.

Use integration/desktop-interface-redesign and /home/simon/source/objekte-integration-desktop-interface-redesign. Keep the original checkout on master unchanged. Follow the repository's commit/push discipline; leave one integration branch for the user's merge.

Use an integrator, delegating only clearly separable work in sibling task worktrees under /home/simon/source:

| Owner | Scope |
| --- | --- |
| Integrator | Shared view/state contracts, layout, visual review, integration, final validation |
| Opening workflow agent | Opening dialog, asynchronous detection, path selection, active-mode validation |
| Query interaction agent | Free-text filter validation, bidirectional builder synchronization, search options, query controls |

Agree file ownership before parallel edits. The integrator owns shared state, renderer entry points, manifests, and live GUI session. Give agents narrower new modules instead of having everyone rewrite panels.rs. Review and merge on the integration branch, pushing each commit as required. Do not modify master.

Reuse GPUI Kit 0.9.0 components and theme facilities already pinned in the repository. Verify actual component APIs before using them. Suggested responsibilities are theme, workspace layout, open dialog, query bar/filter editor, results grid, and inspector; avoid a new general UI framework.

Replace ambiguous independent panel booleans with explicit overlay state where appropriate. Keep the authoritative filter text separate from validated execution/results state and incomplete builder drafts. Use generation identities to keep all three consistent. Avoid cloning PageView/DetailView payloads during each render: borrow stable data or share immutable projections where supported, capture only small identities in callbacks, and keep caches bounded.

Retain existing worker execution, cancellation, source safety, CLI/TUI behavior, and output encoders. Do not rewrite the core to implement a filter form, turn the app into a web application, add a server, or reopen/index the repository when changing cosmetic settings.

## 8. Delivery sequence

| Phase | Deliverable | Suggested commit |
| --- | --- | --- |
| 1 | Current native baseline screenshots; shared theme; compact workspace, sidebar, header, grid, inspector | Establish desktop workspace layout and theme |
| 2 | Modal opening workflow, detection, advanced settings, cancellation/focus | Simplify desktop data opening |
| 3 | Compact search, optional free-text filter line, invalid-state feedback, synchronized builder, sort and range controls | Simplify desktop search and filter interaction |
| 4 | Focused export/history/batch/index/diagnostic surfaces and full action mapping | Organize desktop secondary operations |
| 5 | Visual/interaction review at target sizes, parity regression checks, report | Validate desktop interface redesign |

The highest-value milestone is a useful Open → Search → Inspect flow with the table occupying the window. Complete that before cosmetic refinements or broader filter-builder features. Phases may be split into coherent commits, but do not declare completion with hidden or unreachable existing functions.

## 9. Acceptance and verification

Run the existing native headless Sway workflow at 1920×1080, scale 1, 60 Hz. Inspect actual screenshots after mouse/keyboard interaction; a successful build or parity table is insufficient evidence of visual quality. Also exercise smaller application window sizes without changing the fixed virtual output.

Capture the welcome screen, default/advanced opening dialogs, populated workspace, free-text filter, synchronized builder, invalid filter with retained results, record/source views, light theme, and small-window layout. Record baseline and final commit with every evidence set. Compare against the supplied PDF rather than merely checking that every command exists.

Required interaction checks:

1. A first-time user opens a normal folder without understanding snapshots, alphabets, or schemas. A configured repository is recognized; malformed configuration does not silently change mode.
2. Single/multiple/configured modes, schema/recursion restrictions, all four snapshot policies, custom locations, and index-only operation still work.
3. Typing in search works immediately. Enabling Filter reveals and focuses the second free-text line. Clear, disable/re-enable, and restore have the specified effects. Valid search/filter combinations match the core query; defaults remain unchanged.
4. The field accepts the full grammar. Manually typed supported expressions populate the builder; builder edits update the field; text undo/redo updates both. Incomplete rows, invalid text, unsupported valid expressions, closing the popover, and saved-query restoration never lose or silently simplify content. Invalid states identify the error, prevent execution/export of the invalid request, visibly mark retained results, and reject late updates.
5. Header sorting is global; inline-sort conflicts remain explicit. Exact/unknown/pending counts and zero/finite/unlimited ranges are accurately represented.
6. Table headers remain visible; rows remain aligned and constant-height; horizontal scrolling, mouse wheel, selection, and keyboard navigation work at every test size.
7. Fields/source, find, complete copies, relation traversal/Back, preview visibility, history, saved queries, columns, all exports, batch outcomes, help, and diagnostics remain reachable.
8. Dialog focus, Escape, picker cancellation, delayed picker responses, and returning focus work correctly. Invalid forms keep their input and put errors beside the relevant control.
9. Source documents/schemas remain unchanged. Explicit output actions retain their existing safeguards.

Add focused tests for the new mode detection/request mapping, dialog draft lifecycle, typed filter serialization, bidirectional synchronization, validation generations, undo/redo, unsupported-expression preservation, and restored option visibility. Reuse existing backend/parity fixtures. Run the required workspace tests, Clippy, formatting/spacing checks, and relevant benchmarks following repository instructions.

Measure warm typing, scrolling, selection/inspector updates, and resizing separately from index building and SSH capture. Compare the same dataset, release/debug profile, theme, viewport, and cache state. Do not invent a performance gain from a visual change. Pay particular attention to page/detail cloning, field discovery, automatic width measurement, and counts; none may trigger full-corpus work on redraw.

Completion requires reviewed native screenshots, no clipped/unreachable controls, preserved read-only CLI/TUI functionality, a current action-to-location parity map, and the task/report committed and pushed on its integration branch. Update the GUI guide for changed workflows. Keep the integration worktree for the user's merge.

## Sources

- [Reviewed master commit](https://github.com/Simon-Martens/objekte/commit/4e15721d3853a6c93e9b7b4bb73e18233144effa)
- [Workspace renderer](https://github.com/Simon-Martens/objekte/blob/4e15721d3853a6c93e9b7b4bb73e18233144effa/crates/desktop/src/ui/render.rs)
- [Opening, details, and action panels](https://github.com/Simon-Martens/objekte/blob/4e15721d3853a6c93e9b7b4bb73e18233144effa/crates/desktop/src/ui/panels.rs)
- [UI models and defaults](https://github.com/Simon-Martens/objekte/blob/4e15721d3853a6c93e9b7b4bb73e18233144effa/crates/desktop/src/ui/model.rs)
- [Controller and native picker](https://github.com/Simon-Martens/objekte/blob/4e15721d3853a6c93e9b7b4bb73e18233144effa/crates/desktop/src/controller/runtime.rs)
- [Request mapping](https://github.com/Simon-Martens/objekte/blob/4e15721d3853a6c93e9b7b4bb73e18233144effa/crates/desktop/src/controller/adapt.rs)
- [Existing capability/evidence record](https://github.com/Simon-Martens/objekte/blob/4e15721d3853a6c93e9b7b4bb73e18233144effa/docs/ai/generated/desktop_application_parity.md)
- [Filter grammar](https://github.com/Simon-Martens/objekte/blob/4e15721d3853a6c93e9b7b4bb73e18233144effa/docs/filter-grammar.md)
- [DBDelve](https://github.com/ShayanAbbas1/dbdelve)

## Implementation report

- **Task:** `desktop-interface-redesign`, completed 8 November 2026. Base commit: `4e15721d3853a6c93e9b7b4bb73e18233144effa`. Integration branch: `integration/desktop-interface-redesign`.
- **Result:** The native desktop now opens data through a compact modal, centers the persistent Search and optional free-text Filter lines, shows a synchronized helper for representable conditions, validates the combined request before execution, and keeps last valid results visible on invalid input. The stationary results table, resizable inspector, light/dark palettes, narrow-window behavior, and contextual secondary actions follow the supplied design direction. The CLI/TUI and read-only source model remain intact. [The parity record](../generated/desktop_interface_redesign_parity.md) maps actions to their new locations and contains native baseline/final evidence.
- **Opening subtask:** `feature/desktop-redesign-open`, integrated through `e4a80b4ba6`, `68a47f835f`, and `56f7adcb6f`; source commits include `2a32e68075`, `bd91ec161d`, and `dfff49bd24`. It supplied asynchronous configuration detection, multiple-folder draft handling, the modal, picker cancellation/generation checks, and advanced request mapping.
- **Query subtask:** `feature/desktop-redesign-query`, integrated through `61ce7e35b2`, `4080c5659a`, `59580d3067`, and `3835322ce4`. It supplied no-scan binding validation, the authoritative free-text filter, synchronized typed builder, and invalid-request execution guard.
- **Integrator:** `341f3e2766`, `6f1d0af53a`, `c4dfef6d1a`, `62eaf572bd`, `9274f0f023`, and `d5953e1330` established shared state, the compact workspace and inspector, floating overlays, secondary actions, responsive polish, and export of the current validated query. `afb37079d5` and `6d6fed42ad` added the GUI guide, evidence, parity map, and matched performance measurements. All listed integration commits were pushed after creation.
- **Validation:** `cargo test --locked --workspace` passed on the combined branch (no failures; two ignored core tests). `cargo clippy --locked --workspace --all-targets` passed with existing core/CLI warnings and no desktop warnings. `python3 scripts/format-rust-spacing.py --check` and `git diff --check` passed. The native Sway launcher verified 1920 × 1080, scale 1, 60 Hz. Screenshots and interactions were reviewed at that output and with 1440 × 900, 1280 × 800, and 1024 × 768 app windows. Native index-only and JSON export actions wrote files outside source directories while retaining the open workspace.
- **Benchmark:** Both base and integration ran `bench_musenalm --root examples/musenalm --extended --cases Q035,Q045 --profiles ascii-off --repetitions 5` in release on the same 148,486-file corpus. Q035 median was 35.094 µs base versus 33.963 µs integration; Q045 was 184.267 versus 171.845 µs. Build time was 1.252 versus 1.130 seconds. Eight-sample debug GUI control/capture medians for warm typing, scrolling, selection, and resizing are recorded separately in the parity record. The control bridge dominates those timings, so no GUI frame-time gain is claimed.
- **Deviations and limits:** No dependency or scope deviation was needed. The native review did not manually traverse every rare picker, relation, batch, or export variant; automated request/backend/CLI/TUI tests cover their retained paths. The repository has no isolated GPUI frame-latency benchmark, so the recorded local input and capture times are not application rendering times.
- **Worktree:** `/home/simon/source/objekte-integration-desktop-interface-redesign` is retained for the user's merge. The two clean subordinate worktrees were removed after the report push. The original checkout remains on `master`; its obsolete untracked task source and PDF were removed only after their contents were verified in this committed `done/` copy.
