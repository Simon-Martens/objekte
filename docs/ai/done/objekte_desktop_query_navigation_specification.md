# Objekte desktop query and navigation specification

**Task identifier:** `desktop-query-navigation`  
**Date:** 2026-11-08  
**Revision:** Includes the follow-up requirements for menus, bundled fonts, record fields, relations, source actions, and metadata.  
**Repository:** [Simon-Martens/objekte](https://github.com/Simon-Martens/objekte)  
**Reviewed revision:** [40bd5c3b97a7d29c184646f87b84670669ae7424](https://github.com/Simon-Martens/objekte/commit/40bd5c3b97a7d29c184646f87b84670669ae7424), on `master`, “Record completed desktop task cleanup”.  
**Deliverable:** Implementation specification for a subsequent authorized task. No application changes were made while preparing this document.

## 1. Objective and precedence

Extend the implemented desktop redesign with visible bookmarks and history, independent record tabs, a conventional sortable and resizable table, query completion, and continuous result loading. Preserve the current restrained appearance and the second filter input below full-text search. Continue to use [DBDelve](https://github.com/ShayanAbbas1/dbdelve) as visual inspiration for compact navigation and a dominant data table.

This is a follow-up to `objekte_desktop_interface_redesign.md`. Its requirements supersede the following earlier decisions, including conflicting illustrations in the earlier visual reference:

| Earlier behavior or restriction | Required behavior |
| --- | --- |
| Saved-query and History panels; naming dialog when saving | Inline sidebar lists; immediate save with an automatic title |
| Single record inspector; multiple record tabs excluded | Several record tabs inside the right inspector, each retaining its query context |
| Separate sort controls | One authoritative ordering clause in the visible filter text |
| Editable query limit/offset and Previous/Next page controls | Automatic loading in batches of 100; no desktop per-query range controls |
| Grid restricted to single-line cells | Single-line truncation by default; optional wrapping per column |
| Validation message below the filter input | Validation inside the existing input row; no validation-driven vertical movement |
| Text-labelled Menu and query Options controls | Icon-only application menu; remove the query Options control entirely |
| Generic column-selection actions | Left-aligned column names with actual checkboxes in a fixed left gutter |
| Existing interface fonts | Bundle Source Sans 3 and Source Code Pro; load them before the first window |
| Inspector field search and mixed field/relation presentation | No inspector search; two-column Fields view and a separate Links tab |
| Source text tab and persistent record action buttons | Right-aligned Source icon; Copy record and Export record icons only in Source view |
| Metadata below the record body | Collapsible metadata above the record body, with a validity dot |

All other implemented behavior remains in scope for regression protection: opening configured repositories or folders, free-text filters and synchronized filter builder, regex/case options, source inspection, relations, export, count, diagnostics, themes, keyboard access, and index operations.

The application remains **read-only for source records, schemas, and repository configuration**. Bookmark and display preferences may be persisted through the established settings mechanism. Explicit export and index operations retain their existing semantics. Do not introduce record editing, a second GUI repository, a new compositor, a new theme framework, or a new query language.

This specification is based on source inspection at the pinned revision. It is not a report of a native GUI test. Before implementation, compare the current branch with that revision and retain any intervening improvements.

### 1.1 Bundled typography

Use **Source Sans 3** for the interface: menus, sidebar, tabs, field names, ordinary record values, table text, buttons, and status messages. Use **Source Code Pro** for monospace content: query expressions, raw source, code, and identifiers where fixed-width alignment helps. Source Code Pro is the Adobe Source family’s monospace face intended by “Source Mono” in this task.

Import the actual font files into the desktop application’s version-controlled assets and bundle them in the distributed application. Register the embedded fonts with GPUI before constructing the first window; merely specifying a family name or requiring users to install fonts does not satisfy this requirement. No runtime font download or network access is permitted.

Use the official [Source Sans repository](https://github.com/adobe-fonts/source-sans) and [Source Code Pro repository](https://github.com/adobe-fonts/source-code-pro). Pin the selected release or commit, record the origin and checksums, and include each distribution’s license and notices. Package the weights actually used, including genuine bold for matched field labels; prefer supported static font files if variable-font support is uncertain. Reuse the existing asset-loading and font-registration mechanisms. Keep sensible system fallbacks for characters absent from the bundled families.

Apply font families through shared typography definitions rather than individual widgets. Recheck line height, clipped labels, menu density, mixed Unicode text, and column sizing with the actual bundled fonts. Verify the packaged application offline on a machine without these fonts installed. Font loading is a startup operation, not repeated per view or render.

### 1.2 Application menu and column chooser

Replace the visible **Menu** text with a conventional **hamburger icon only**. Provide the accessible name and tooltip “Application menu”. The opened menu should resemble a compact desktop system menu: left-aligned labels, consistent icon/checkmark gutter, keyboard shortcuts aligned right, restrained separators between related groups, and no cards, large buttons, or oversized gaps. Start with approximately 26–28 logical-pixel rows and adjust for the bundled font and platform scaling without clipping or sacrificing usable hit targets.

Use standard menu focus, Up/Down navigation, Enter activation, Escape dismissal, outside-click dismissal, and disabled states. Anchor the menu to its button; opening it must not move the table. Keep actual menu-item text inside the menu—the icon-only requirement applies to its trigger.

The **Columns** menu contains genuine checkbox controls to the **left** of left-aligned field names. Use the existing toolkit’s checkbox/menu-item semantics, including checked state and keyboard operation; do not substitute decorative Unicode boxes, colored dots, centered pills, or right-aligned checkmarks. Clicking the checkbox or its label toggles the same column once. Keep the menu open while several columns are selected, and preserve its scroll position. A checked item means that column is visible; unavailable items explain why they are disabled. Long names remain left-aligned and receive an ellipsis/tooltip. Use the same rootless field-name display rules as section 4.5.

### 1.3 Query toolbar simplification

The filter-row visibility toggle uses a **funnel icon**, with a pressed state and the accessible name/tooltip “Filter”. The button at the right of the filter input uses a distinct suitable builder icon, with the accessible name/tooltip **“Filter Builder”**. Neither button needs permanent text. Preserve the existing distinction: the funnel shows/enables the free-text filter row; the builder button opens its synchronized small builder. Do not replace free-text filtering with a builder-only workflow.

Remove the **Options** button and its query-options popover entirely. There is no replacement generic options drawer. Limit and offset are removed from desktop browsing, and sorting is controlled through table headers and the visible `SORT BY` text. Keep case sensitivity and regex as compact search-field controls. If exact count currently lives under Options, move it to a direct result-footer action; retain its asynchronous behavior. Update keyboard commands and help so no obsolete Options action remains reachable.

## 2. Findings in the current implementation

The following are observed code behaviors, not inferred visual defects:

| Location at the reviewed revision | Finding and consequence |
| --- | --- |
| [ui/render.rs](https://github.com/Simon-Martens/objekte/blob/40bd5c3b97a7d29c184646f87b84670669ae7424/crates/desktop/src/ui/render.rs), [ui/panels.rs](https://github.com/Simon-Martens/objekte/blob/40bd5c3b97a7d29c184646f87b84670669ae7424/crates/desktop/src/ui/panels.rs) | Sidebar Saved queries and History open action panels; application-menu entries duplicate those destinations. Replace these with the sidebar lists. |
| [ui/preferences.rs](https://github.com/Simon-Martens/objekte/blob/40bd5c3b97a7d29c184646f87b84670669ae7424/crates/desktop/src/ui/preferences.rs), `save_query_local` / `restore_saved` | Bookmarks are replaced and restored by name. Two different queries with the same generated title would collide. Empty search/filter queries are also rejected, although a collection-only bookmark is useful. |
| Same file, `to_query` / `from_query` | Persisted queries omit independent sorting and derive filter activation from nonempty filter text. A retained but disabled filter can therefore become active after restoration. Preserve full query state explicitly. |
| [ui/mod.rs](https://github.com/Simon-Martens/objekte/blob/40bd5c3b97a7d29c184646f87b84670669ae7424/crates/desktop/src/ui/mod.rs), `sort_by_column`; [backend/query.rs](https://github.com/Simon-Martens/objekte/blob/40bd5c3b97a7d29c184646f87b84670669ae7424/crates/desktop/src/backend/query.rs), `build_query` | Headers modify a separate sort input. When inline sorting exists, header clicks only focus the filter; supplying both forms can produce a conflict error. Header indicators read the separate state. Remove this competing ordering state. |
| [controller/runtime.rs](https://github.com/Simon-Martens/objekte/blob/40bd5c3b97a7d29c184646f87b84670669ae7424/crates/desktop/src/controller/runtime.rs), `start_query` and detail completion | A query clears the single selected hit, detail view, and relation-back stack. Detail responses are guarded by the main query generation. Merely adding tab labels would not provide independent tabs. |
| [controller/projection.rs](https://github.com/Simon-Martens/objekte/blob/40bd5c3b97a7d29c184646f87b84670669ae7424/crates/desktop/src/controller/projection.rs), `page_view_with_available` | Columns are discovered from top-level fields in the current page. Nested filter paths are not projected as columns; subsequent pages can change the available set. The result projection carries match field names but does not expose record status as a dedicated column. |
| [ui/query_bar.rs](https://github.com/Simon-Martens/objekte/blob/40bd5c3b97a7d29c184646f87b84670669ae7424/crates/desktop/src/ui/query_bar.rs) | Validation includes a separate “Filter valid” / “Checking filter…” message area. Query options still expose independent sort, offset, and limit controls. |
| [ui/model.rs](https://github.com/Simon-Martens/objekte/blob/40bd5c3b97a7d29c184646f87b84670669ae7424/crates/desktop/src/ui/model.rs), [ui/render.rs](https://github.com/Simon-Martens/objekte/blob/40bd5c3b97a7d29c184646f87b84670669ae7424/crates/desktop/src/ui/render.rs) | The renderer builds fixed-height rows for one bounded page. Simply concatenating pages would grow retained UI elements and projected text without bound. Display labels are clipped; they must never become persistent record identity. |

Reuse existing capabilities. In particular, [core/posting_values.rs](https://github.com/Simon-Martens/objekte/blob/40bd5c3b97a7d29c184646f87b84670669ae7424/crates/core/src/posting_values.rs) already provides `Repository::posting_values` with typed values, prefix/range restrictions, limits, continuation cursors, coverage information, and visited-key counters. Do not implement autocomplete by scanning source documents or copying complete posting dictionaries.

## 3. Sidebar: saved queries and history

### 3.1 Layout and activation

Below Collections, show a **Saved queries** heading followed immediately by saved-query rows. Keep saved queries expanded by default. A compact empty state reads “No saved queries”.

Below that, show a **History** disclosure heading. History starts **collapsed on each application launch**. Expanding it reveals newest-first query rows; collapsing it retains the entries. Do not provide a second History menu, popover, or bottom panel. A keyboard command may expand and focus this same section.

Clicking a query row restores its complete search state and executes it. Highlight the active matching query without relying on text labels as identity. Long titles use ellipsis and a tooltip containing the complete generated description; sidebar width changes must not alter stored titles.

The sidebar has ordinary scrolling and keyboard navigation. Long lists must not create one expensive mounted component per retained entry. Keep the existing bounded history retention policy unless correctness requires a documented migration.

### 3.2 Automatic titles

Use one formatter for history descriptions and default bookmark titles:

`<collection> · <FTS term> · <active filter text>`

Rules:

- Use “All collections” for an unscoped query.
- Omit the FTS or filter segment when empty or inactive.
- If both terms are absent, use `<collection> · All records`.
- Keep `SORT BY` visible in the filter segment when present.
- Preserve the actual saved query strings. Whitespace normalization and ellipsis affect display only; never change quoted literals or regex patterns.
- Case and regex settings are restored even if they are shown only in the tooltip or a compact option indicator.

Examples:

| Query | Automatic title |
| --- | --- |
| Collection `letters`; FTS `Heidelberg`; filter `year >= 1900` | `letters · Heidelberg · year >= 1900` |
| All collections; no FTS; filter `year >= 1900 SORT BY year DESC` | `All collections · year >= 1900 SORT BY year DESC` |
| Collection `letters`; no search or filter | `letters · All records` |

### 3.3 Saving, renaming, and starring

Saving creates the bookmark immediately and inserts its row under Saved queries. **No title dialog appears.** Provide Rename through the row context menu and F2; edit the title inline. Enter commits a nonempty name; Escape cancels. A custom title survives subsequent restoration and application restarts. Renaming changes only the bookmark title.

Each history row has an outline star, filled when its query is bookmarked. Clicking the star saves that history entry immediately without executing it, changing the main query, or opening a naming dialog. Stop event propagation. Clicking the filled star removes the matching bookmark; provide the same Remove action in its context menu.

Use a stable bookmark identifier independent of title and position. Identical titles must coexist. Repeatedly saving the same complete query definition reuses its existing bookmark and preserves any custom title.

The query-definition comparison includes collection scope, FTS text, regex/case options, filter text, and its enabled state. It must not strip significant characters to manufacture equivalence. A retained disabled filter is part of the restorable definition even though it does not constrain execution.

Saving the main query captures the current validated definition. Disable saving invalid drafts, with the reason available at the control. Saving a history entry captures that entry, never the current text fields. Saving an empty search within a collection is permitted.

### 3.4 History and persistence

Record successful, distinct queries, including successful zero-result queries. Do not add entries for validation failures, canceled requests, pagination, tab activation, record navigation, column resizing, or wrapping.

Coalesce a continuous live-search editing burst into its latest successful query; Enter commits the final search immediately. Repeating a query moves its history entry to the top rather than filling the list with duplicates. Restoring a bookmark/history entry records that execution under the same rules.

Introduce a versioned desktop preference representation where necessary. Preserve existing saved names, history order, hidden columns, and theme. Migrate old records without destructive rewriting of the TUI’s own preferences. Legacy state lacks an explicit filter-enabled flag; derive that flag once using the old behavior, then persist it explicitly. Do not promise recovery of sort settings that the old format never stored.

Preference I/O stays off the UI thread and preserves write ordering. A failed write produces a visible retryable error; do not claim the bookmark was durably saved. Existing usable settings must survive a failed replacement.

## 4. Record tabs in the right inspector

### 4.1 Opening and tab controls

Place a horizontal record-tab strip above the inspector. Each record contains the navigation toolbar, a collapsible metadata section at the top, and the Fields/Links view selector with a right-aligned Source icon described below. These are **record tabs**; the main query pane remains one pane. Fields, Links, and Source are views within the active record tab, not additional record tabs.

An explicit row click opens or activates a retained record tab when record preview is enabled. Enter opens the selected record regardless of preview preference. Keyboard movement through rows changes selection without creating a tab for every keypress. Do not silently replace an existing retained tab when another record is opened.

Reuse an already open tab only when both the record identity and its originating query context match. The same record opened from different queries may have separate tabs because Previous/Next must have different meanings.

Each tab has a concise record title or ID, tooltip with full identity, active styling, and close button. Closing activates an adjacent tab; closing the final tab returns to the inspector’s empty state or hides it consistently with the current layout. Overflow scrolls horizontally or uses a standard tab-list control; it must not squeeze labels to unreadability.

The inspector toolbar is ordered as follows:

1. **Back**, leftmost; always present and disabled when that tab has no prior navigation.
2. **Previous record** and **Next record**, navigating the originating result set.
3. A compact position indicator when known.
4. **Open query**, restoring the tab’s originating query to the main pane.

Record-specific actions always act on the active tab’s record. Copy record and Export record are icon-only actions visible exclusively while that tab shows Source; remove the dedicated Copy value button. Relations belong in Links, and diagnostics remain available through metadata. Query-level all-results export remains a separate application action.

### 4.2 Independent query context

Opening a record captures an immutable context containing:

- Repository session identity and relevant index generation.
- Collection scope and the complete effective query, including case/regex and ordering.
- Restorable UI definition, including filter visibility and any inactive filter draft.
- Resolved time/parameter values where the query uses them, so navigation does not silently reinterpret the query later.
- The record’s stable locator and position or continuation information in the ordered result set.

Tab state also contains its current record, origin-result anchor, Back stack, Fields/Links/Source selection, metadata and complex-value expansion state, scroll positions, pending operation, and error state. Returning to a tab restores these choices; opening a different record starts complex arrays collapsed.

Share immutable query contexts and the open repository between tabs. Do not clone an index, complete result list, or record payload for every tab. Display-clipped IDs and transient row numbers are not record locators. Use the core’s canonical identity within the captured repository generation.

Changing the main search, activating history, sorting the table, or loading more rows must not close tabs, clear their Back stacks, or change their query contexts.

### 4.3 Navigation semantics

Previous/Next move through the originating query’s **full ordered dataset**, including across batch boundaries. They do not use whatever rows currently happen to be visible in the main pane. Disable Previous at the first result and Next at a confirmed end; show a local loading state while the next boundary is established.

Successful Previous/Next and relation navigation push the previous navigation frame onto that tab’s Back stack. Back restores the record, origin anchor, and view state and pops a frame; it does not create another Back entry. Failed navigation does not discard the current record.

Following a relation opens the target within the active tab. Preserve the original query and anchor: the related record need not belong to the query. While viewing it, the position label identifies the original result anchor, and Previous/Next move relative to that anchor. Back returns to the previous record. Provide an ordinary “Open in new tab” action for a relation when useful; it inherits the originating context and anchor.

A frame stores lightweight identity and navigation state rather than complete copies of record bodies. Retain the existing bounded navigation-history approach or an equivalent documented bound.

### 4.4 Open query, lifecycle, and asynchronous work

Open query restores collection, FTS, filter, enabled state, case/regex, and sorting in the main pane, then reveals the tab’s current origin-result anchor. It must work when that anchor is beyond the first 100 results without loading every preceding record body. If the tab is showing a related record outside the result set, reveal its anchor instead.

The control remains visible. Enable it whenever restoring the query or revealing the anchor would change the main pane. Activating it never creates another record tab or replaces the tab’s context.

Tag record operations with repository session, tab identifier, and tab-operation generation. Main-query cancellation must not cancel unrelated tab work. Closing a tab cancels or ignores its outstanding operations; a late result cannot resurrect it or overwrite another tab. Apply the same ownership rule to relations, copy, and record export.

Maintain one active repository session. After a different repository has opened successfully, close tabs belonging to the old session and release their state; a failed or canceled open preserves the old workspace. Reopening or rebuilding an index must never silently rebind old record locators. Either retain a compatible generation explicitly or mark affected tabs stale and offer a deliberate refresh that recomputes their query and locates the record by stable identity.

Do not retain a complete old repository merely to keep stale tabs alive. Cross-repository tabs and restoration of open tabs after restart are not required.

### 4.5 Fields: two-column record display

Use a compact **two-column field/value layout**: field name on the left, value on the right. Align the columns across rows, with top alignment for multiline values, a restrained separator or spacing, and more available width for values. Long values wrap within their column; field labels may wrap or truncate with a tooltip without pushing values out of view. This replaces stacked label/value cards or an undifferentiated formatted JSON block. Keep the overall inspector resizable.

Remove the **record-local search box** at the top of the inspector and its associated filter/reset controls for this iteration. This does not remove the main FTS input, the structured filter, or their autocomplete. Opening a record starts directly with its metadata and content views.

Displayed field names must have **no synthetic `$.` prefix**. Show `title`, not `$.title`; use the surrounding hierarchy or an unambiguous rootless path for nested fields. Apply this presentation rule to field labels, table headers, column choices, match labels, and human-readable suggestions. Keep canonical field identity internally and insert the grammar-correct path into query text. Do not alter raw source, rewrite existing filters, strip literal characters from actual source keys, or collapse distinct nested/literal-dot names into one identity.

Hide empty fields in the Fields presentation, including missing values, explicit null, empty or whitespace-only strings, empty arrays `[]`, and empty objects `{}`. Apply the same rule to child fields when a container is expanded. **Zero and false are not empty** and remain visible. This is display filtering only: the original record, Source view, clipboard record, and exported record remain complete and unchanged. Diagnostics and logical-unavailable errors must not disappear as if the record simply had no data. If nothing remains visible, show a concise “No non-empty fields” state with Source still accessible.

| Value shape | Fields presentation |
| --- | --- |
| Nonempty scalar | Show the formatted value in the right column |
| Array containing only simple scalar values, allowing null entries | Inline values separated by semicolon and space, for example `Graphik; Gedicht`; preserve order and duplicates; no disclosure control |
| Array containing an object, another array, or a mixture of scalar and container elements | Expandable collection, **collapsed by default**, with an item count; expand elements lazily in original order |
| Empty array or object | Hide its field row |
| Nonempty object | Nested field/value rows using the same two-column alignment and empty-field rules; use a disclosure control when necessary to keep the view bounded |

For a simple array, omit empty/null elements from its display, retain `0` and `false`, and hide the field if no display values remain. Semicolon joining is a readable presentation, not a new serialized format; values containing semicolons remain intact and are unambiguous in Source. Nonempty complex arrays do not require a whole-array traversal merely to determine their collapsed summary. Apply child emptiness filtering as elements are requested.

Collapsing and expanding must be local to the record tab. Do not eagerly flatten every object or element to build hidden rows, and do not fetch related records just to render scalar IDs. Bound text projection and expansion work using the existing detail limits; provide incremental expansion where needed without discarding access to the complete source.

### 4.6 Query and FTS emphasis in the record

Use the **record tab’s captured query context**, not the current query-pane text, to emphasize relevant fields:

- Subtly emphasize field labels referenced by the active structured predicate. This indicates participation in the query; it does not claim that every branch of an OR expression matched.
- Emphasize field labels with verified FTS evidence and highlight actual matching spans in their displayed values where supported. When there is **no structured filter**, FTS-matched fields are still emphasized.
- When both apply, show their combined emphasis without duplicating rows. Use a short tooltip or accessible description to distinguish “Used in filter” from “Full-text match”; do not add an extra permanent badge column.

Use restrained background/weight and accurate match highlighting, with usable contrast in both themes. Reuse section 6’s evidence and Unicode/regex rules. A sort-only field is not a searched predicate field. Do not fabricate field matches from raw-source-only evidence or mark an entire relation target as matched merely because the originating record matched.

Preserve the collapsed default for complex arrays even if a descendant matches. Mark the visible container label when supported by its descendant path evidence, and reveal exact child emphasis on expansion. Hidden empty fields remain hidden even when referenced by an absence predicate; metadata/query information can explain the predicate without reintroducing empty rows. Never perform a full collection search or unbounded relation traversal to compute inspector highlights.

### 4.7 Links and Source

Place the **Links** tab immediately next to **Fields**. It contains both outgoing links and incoming backlinks, separated into clearly labelled groups. Use the existing relation identity, direction, and navigation semantics. Keep relation lists out of the Fields content; ordinary stored relation IDs may still appear there as values without a second set of relation controls.

Load relations when Links is opened and use bounded or paged delivery. Show useful target/source labels and the relation field, with a truthful unresolved-link state and a concise empty state. Large backlink lists must not delay opening Fields or force a complete scan merely to display a count. Opening a link follows the same tab-local Back/anchor rules as section 4.3, for both directions. Returning from a linked record restores the previous view and its scroll position.

**Source is an icon at the far right of the Fields/Links selector row**, using the existing icon set’s code/source symbol and the accessible name/tooltip “Source”. It is a mutually exclusive third view, not a text-labelled tab or an external editor launcher. Clicking Fields or Links returns from Source without opening or closing a record tab. Expose an active state and normal keyboard access for the Source control.

Only while Source is active, show **Copy record** and **Export record** as compact icons at the right of that view’s toolbar. Hide them in Fields and Links, including duplicate record-action buttons or menu entries there. Tooltips and accessible names identify the commands. The navigation controls and Source selector remain available in every view. Query-level export remains in the application menu and keeps the semantics in section 8.3.

Remove the dedicated **Copy value** action button from the inspector and field rows. Ordinary text selection and its standard keyboard/context-menu copy behavior remain available; do not replace the removed button with another persistent per-value icon.

Source presents the original record content through the existing bounded source viewer, with an explicit truncation/loading indication where applicable. Copy record and Export record use the complete backend record under the established output format, never the clipped source preview or the nonempty-field projection. Preserve format selection where already supported; this task changes control placement, not record serialization. Show completion or failure without moving the record layout.

### 4.8 Metadata above the record

Move the **collapsible Metadata section above the record content**, below the record navigation toolbar and before the Fields/Links/Source selector. Its position stays consistent for all three views. Start collapsed for a newly opened record, preserve expansion within that record tab, and keep the compact header visible while collapsed.

Display the record’s validity as a **small green or red dot in that metadata header**: green for a confirmed valid state, red for a confirmed invalid state. Derive it from actual backend status/diagnostics, not the presence of text or an empty error list. An unknown, unavailable, or pending validation state uses a neutral indicator rather than a false green. Supply an accessible state label and tooltip; the expanded section explains the status and diagnostics, so color is not the only available information.

Keep metadata such as identity, collection, location, and available diagnostics in this section using the same compact field/value layout. Omit empty metadata values without hiding diagnostic failures. Do not append a second metadata block or validity badge beneath the record.

This dot requirement applies to the **record inspector**. The results table retains its explicit textual Status column from section 5.1, without a status dot.

## 5. Results table

### 5.1 Column order and identity

The default leading columns are:

**ID → Status → Match (only when FTS is active) → active filter fields → remaining data fields.**

An optional row-number gutter is separate from that sequence. Status is readable text, **not a status dot**. Derive it from the backend’s file status, record class, and diagnostics; use the existing domain vocabulary and a tooltip for detail. Do not guess validity from an empty payload. Synthetic metadata column identities must remain distinct from source fields named `id`, `status`, or `match`.

Extract filter-referenced field paths from the parsed predicate, in first-appearance order, and deduplicate by complete path. Handle nested paths and relation references according to the core grammar. A field already represented by a leading metadata column is not duplicated. Words inside string literals, operators, and the `SORT BY` clause do not become predicate-field columns.

Add newly referenced filter fields automatically and place them immediately after the leading columns. Resolve nested values using the core field-path semantics, not top-level JSON object lookup. Relation-qualified values require correct traversal semantics; if a value cannot be projected cheaply, show an explicit unavailable marker rather than a misleading local-field value.

A column chooser remains available, using the left-aligned checkbox menu specified in section 1.2. Explicit hiding is respected for the current query; newly activated filter fields are revealed by default. Preserve widths and wrapping by stable field identity when fields move. Handle more fields than the display-column budget with an explicit overflow indication and chooser; never silently drop query-required fields behind unrelated discovered fields.

Build the available field set from declared/inferred metadata plus bounded observed fields. Within one query, keep a stable column order while additional batches arrive. Late-discovered fields may be appended, but existing columns must not reorder on every batch. For all-collection queries, preserve access to collection identity.

Distinguish missing, explicit null, empty string, and unavailable logical data in presentation and tooltips. Do not change export values to match display placeholders.

### 5.2 Resizing and wrapping

Each header has a resize handle with a normal horizontal resize cursor. Dragging changes the shared header/body width, with sensible minimum and maximum widths. A resize gesture must not trigger sorting. Preserve one horizontal scroll position for header and rows.

Persist column widths and per-column **Wrap text** settings for the repository and collection scope. The header context menu or column chooser exposes Wrap text. Default is one line with ellipsis. Wrapped cells use variable row height, with all cells in a row aligned to its maximum measured height.

Use virtualized rows and measure only visible rows plus a small overscan region. Cache heights by row identity, column widths, and wrap/font settings. Maintain the scroll anchor when widths or wrapping change. Very large cell previews retain a documented display bound and an accessible path to the full value in the inspector; wrapping does not authorize unbounded text layout.

Double-click auto-fit may be provided, but it may measure only headers and already available rows. It must not scan all results.

### 5.3 Sorting through the filter text

A sortable header cycles **ascending → descending → no sort**. Clicking another field selects that field ascending. Ordinary clicks use one sort key; multi-key ordering remains available through manual `SORT BY` text and the existing grammar.

The header action edits the authoritative filter string:

| Before | Action | After |
| --- | --- | --- |
| `year >= 1900` | Click Year | `year >= 1900 SORT BY year ASC` |
| `year >= 1900 SORT BY year ASC` | Click Year | `year >= 1900 SORT BY year DESC` |
| `year >= 1900 SORT BY year DESC` | Click Year | `year >= 1900` |
| Empty active filter | Click Year | `SORT BY year ASC` |

Use parsed clause boundaries and the core formatter/escaping rules. Never search and replace a literal substring that might occur inside a quoted value, regex, or nested expression. Preserve the predicate and meaningful user text. Manual multi-key sorts show directional indicators and key precedence; a normal header click deliberately changes to the documented single-key sort.

When the filter is disabled and empty, sorting reveals the filter row and inserts the clause. If a disabled filter retains a predicate, keep it inactive: a header click must not silently activate that predicate. Preserve it as the inactive draft while starting a visible sort-only expression, with an explicit restore action for the retained predicate. Never discard it. If this requires distinct active and suspended drafts, keep that state in the editor, not as a second execution sort source.

If the currently active filter is invalid, do not rewrite it speculatively. Focus its error and leave results and sorting unchanged until corrected.

Remove independent desktop sort controls and derive header indicators, execution, bookmarks, history, exports, and tab contexts from the same parsed text. Turning Filter off disables its entire expression, including ordering; header indicators follow the effective query.

Only advertise sortable headers that the core can sort. Match snippets are not implicitly sortable. Status and ID need their actual supported metadata bindings. Array/object fields must not acquire an invented reducer or lexicographic string conversion; offer supported explicit syntax or explain the unsupported action in the header tooltip.

## 6. FTS match evidence

Show Match immediately after Status whenever an effective FTS term exists. Hide it when FTS is empty; an enabled case/regex option alone does not activate it.

Each evidence line is formatted **field name:** excerpt, with the field name bold and the colon visible. For example, **title:** Letter from Heidelberg. Prefer actual matched fields and a short excerpt around the match. Highlight the matching range where the core evidence or a bounded evaluation of the loaded record can identify it accurately.

Support multiple matched fields with a bounded number of preview lines and a “+N more” indication leading to the inspector. Use field paths for nested fields. Do not use the first part of the entire serialized record as if it were a field-specific match.

Respect literal versus regex and case-sensitive versus insensitive semantics. Handle Unicode offsets correctly. Do not mark an arbitrary substring as a match when offsets cannot be mapped reliably. Raw-source matches and logical-unavailable records require truthful labels such as Source; a missing excerpt must not hide the fact that the record matched.

Reuse `SearchHit.matched_fields` and existing evidence. Where snippets need additional extraction, do it only for bounded delivered/visible records, outside render callbacks, with cancellation. Do not rerun FTS over the entire collection to fill the Match column. Keep complete record inspection and export independent of clipped snippets.

## 7. Filter validation and autocomplete

### 7.1 Stable validation layout

Keep the second filter input freely editable, with the icon-only Filter Builder button at its right, as specified in section 1.3. Its height and the table’s top edge do not change between pending, valid, and invalid states.

Remove the “Filter valid” helper line. A valid filter needs no success sentence. Pending state may use a small indicator in reserved input space. An invalid filter uses a border/icon plus a concise accessible error explanation in an anchored popover or the existing status area. Opening error details must not reflow the workspace.

Keep the current last-valid result set while editing an invalid filter, visibly labeled as last-valid in existing table/status space. Do not label it as the invalid query’s results. Disable actions that require a valid current query, including query export and saving that draft; record actions in existing valid tabs remain available.

Retain generation guards for validation. A delayed validation response cannot replace the state of newer text.

### 7.2 Completion sources and syntax

Provide a conventional suggestion popup at the caret in the free-text filter, and reuse the same provider for appropriate filter-builder fields.

Field-name suggestions come from the opened schema and available inferred field metadata. Include nested paths and type information; do not limit them to fields appearing in the first result page. Preserve configured collection scope. When searching all collections, merge names with collection/type information where ambiguity matters.

Value suggestions come from schema enums and existing **sorted distinct posting values** for the selected field. Reuse `Repository::posting_values` with a small page, normally 20 items, and a strict combined cap such as 50. Use typed bounds or a native string prefix. Do not copy posting bitmaps, enumerate all dictionary values, sort the entire dictionary, scan source files, or build new indexes on demand.

The existing API is collection-scoped. For all-collection completion, merge bounded pages from relevant collections with bounded worker concurrency and cancellation. Do not issue unbounded fan-out on each keystroke. Results may arrive progressively with their scope identified.

Honor the API’s binding and coverage semantics. Inferred short-string dictionaries may be incomplete; lack of a suggestion never invalidates typed input. Transformed posting keys must not be presented as original source values unless they are valid literals for the selected query operator/binding. Do not call `select_exact` for every keystroke merely to restrict suggestions to all other predicates. Initial completion uses collection-level values, not dynamic faceting.

No numeric-prefix scan is required: use type-appropriate numeric bounds or a bounded ordered page. Missing postings mean no indexed suggestions, not a fallback document scan. Schema fields still complete when inference or posting construction is unavailable or disabled.

Completion must understand the caret’s token context even while the complete filter is temporarily invalid. Suggest field names where a field is expected, values where a literal is expected, and appropriate fields after `SORT BY`. Insert correctly escaped paths and typed literals. Preserve surrounding text, operators, and the remainder of the query.

### 7.3 Keyboard and request behavior

- Up/Down selects suggestions while the popup is active.
- **Enter accepts the selected suggestion and consumes the key event.** It must not also execute the query, open a record, or insert a newline. A subsequent Enter may execute normally.
- Escape dismisses completion first, without closing another surface or clearing text.
- Pointer selection has the same acceptance behavior.
- IME composition and ordinary caret navigation remain functional.

Debounce requests, cache small results by repository generation, scope, field, binding, and prefix/range, and discard stale responses by editor revision and caret context. Repository access and dictionary traversal never run on the UI thread. FTS-term dictionary completion and collection-wide faceting are not part of this task.

## 8. Automatic loading in batches of 100

### 8.1 User-facing behavior

Remove desktop per-query Limit and Offset controls, the entire query Options button/popover, and the grid’s Previous/Next page buttons. The first request loads **up to 100 records**. Near the bottom of the loaded region, automatically request the next 100; repeat until exhaustion. Users can continue scrolling through the whole result set.

Here 100 is a **delivery batch size**, not a logical query limit. Do not write it into a saved query, a tab’s total result range, or an all-results export. Keep CLI and batch-file range semantics compatible; simplifying desktop browsing does not remove CLI options.

Use a compact loading row or existing footer while another batch is pending. On failure, keep existing rows and offer Retry for that same continuation. Prevent duplicate requests and duplicate appends when repeated scroll events occur.

Until an exact count exists, show “100 loaded”, “200 loaded”, and so on, not an invented total. If an exact count is requested, run it independently and display progress without blocking the first batch. An exhausted final batch establishes the total. A full batch of 100 is not proof that another record exists; tolerate one empty continuation request or use a bounded lookahead.

PageUp/PageDown scroll by a viewport, rather than replacing the dataset with another backend page. Keyboard navigation near the loaded boundary requests the next batch without losing selection. Do not implement End by eagerly loading an unknown million-result tail.

### 8.2 Continuations, stability, and memory

Maintain an immutable executed-query identity and generation for the result list. A new effective query or sort starts at the beginning and cancels its old pending grid requests. A stale page cannot append to a newer list. Existing record tabs remain independent.

Use a deterministic order within a repository generation, including a stable identity tie-break where the core requires it. Verify that equal sort keys cannot cause duplicates or omissions at batch boundaries. Prefer existing ordered-result cache/continuation facilities. If using offsets, measure deep-page work: do not repeatedly reparse and resort the full corpus for each batch.

Keep delivery progress separate from display projection. Byte clipping of a cell or preview must not become end-of-results, skip a record, or change the backend continuation. Preserve lightweight identity for every delivered record even when display text is clipped.

Virtualize the table and retain a bounded cache of page projections and record bodies. Merely virtualizing widget creation while keeping every 4 MiB projected page is insufficient. Evict distant projections and reconstruct them when scrolling back. Retain only necessary lightweight navigation information; share core-owned ordered membership rather than copying it for every tab.

Set and document one aggregate cache budget covering grid and record previews, with smaller inactive-tab budgets. The existing 4 MiB page, 8 KiB cell, and 2 MiB detail limits are useful individual bounds; multiplying them by unlimited pages or tabs is not an aggregate policy.

Opening a tab, navigating across a page boundary, or Open query revealing a distant anchor must work after a page projection has been evicted. It must not require eagerly materializing all intervening record bodies.

### 8.3 Export and failure consistency

“All results” export uses the complete captured effective query, not only loaded rows or the 100-row batch size. Export remains streaming and cancelable. Record export uses the active tab’s record; changing the main search during export does not retarget it.

Preserve an explicit “loaded rows” export only if already supported and clearly named. Do not silently change the meaning of existing export actions.

A filter edit that is invalid leaves the last-valid list intact and labeled. A repository open failure preserves the existing session. A continuation failure preserves its current list. These are separate states and must not reset each other’s navigation.

## 9. State and implementation boundaries

Use explicit concepts equivalent to the following; exact Rust types may follow repository conventions:

| Concept | Responsibility |
| --- | --- |
| Query definition | Restorable search text, active/inactive filter state, scope and options |
| Executed query context | Immutable effective query, repository generation, resolved parameters/time, ordering |
| Result list state | Continuations, loaded extent, page cache, selection, scroll anchor and request generation |
| Record tab state | Stable tab ID, shared query context, record locator, origin anchor, Back stack, Fields/Links/Source view, metadata/array expansion and local scroll state |
| Bookmark | Stable bookmark ID, query definition, generated/custom title |
| Completion request | Editor revision, caret token, scope, field/binding, bounded prefix/range |
| Table preferences | Stable column identities, width, visibility and wrapping per scope |

Keep parsing, field-path interpretation, sorting, matching, and posting semantics in existing core services. The desktop layer owns interaction and projections. Reuse the current controller/worker separation; extend typed actions and responses rather than adding repository access to render callbacks.

An invalid editor draft and the last successful executed context must remain distinct. The active tab must not be represented by the main grid’s selected row index. Grid and tab jobs need independent cancellation ownership.

Store lightweight Back frames and references to shared contexts. Release canceled requests, closed-tab state, evicted projections, and obsolete completion results. A cosmetic tab implementation over the current global detail fields does not satisfy this specification.

## 10. Execution plan and commits

Follow the current `docs/ai/README.md`, repository instructions, and `docs/development/gui_environment.md`. Start implementation only when this task is assigned.

Use:

- Integration branch: `integration/desktop-query-navigation`.
- Integration worktree: `/home/simon/source/objekte-integration-desktop-query-navigation`, assuming the original checkout is `/home/simon/source/objekte`.
- Optional subtask branches: `task/desktop-query-navigation/<subtask>`.
- Optional subtask worktrees: sibling paths under `/home/simon/source`.

Keep the original checkout on `master`. All implementation, integration, verification, and commits occur in the task worktrees. One supervisor owns integration. If delegating, suitable independent areas are sidebar/preferences, table/completion, and tab/navigation; agree on shared context and action contracts before those branches diverge. The supervisor handles shared controller changes and integration conflicts.

| Phase | Implementation and reviewable commit boundary |
| --- | --- |
| 1. Query and preference contracts | Separate restorable definitions from executed contexts; define stable identities; migrate preferences; establish one authoritative sort representation |
| 2. Sidebar, menus and typography | Generated titles, immediate save, inline rename, history disclosure/stars; compact hamburger menu and checkbox column chooser; bundled Source Sans 3 and Source Code Pro; remove obsolete query Options |
| 3. Result delivery | Append 100-row batches, generation-safe continuation/retry, virtualized rendering and bounded projection cache; preserve all-results exports |
| 4. Record tabs | Independent tab state, Back/Previous/Next/Open query, relation navigation, cancellation and repository lifecycle |
| 5. Record presentation | Two-column Fields view, empty-value filtering, scalar/complex array rules, rootless labels, query/FTS emphasis, Links view, Source-only action icons, metadata placement and validity indicator |
| 6. Table presentation | Resizing, wrapping, stable column schema, textual Status, filter-field priority, parser-aware header sorting |
| 7. Completion and evidence | Schema/posting completion, Enter handling, icon-only filter controls, stable validation indicators, bounded field-specific match previews |
| 8. Integration verification | Interaction tests, migration tests, packaged-font/offline checks, native screenshots, relevant performance measurements, documentation and help updates |

These phases produce incremental commits on the same integration branch. Adjust ownership to reduce conflicts in `controller/runtime.rs` and UI model/action definitions. Keep each commit coherent; do not introduce a second temporary sorting path to simplify a phase.

After each commit, push its branch and verify success before the next commit. The supervisor reviews and merges subtask branches into the integration branch, pushing integration commits as well. Never merge into or push `master` for this task.

At completion, follow the repository’s task-document lifecycle: preserve user comments, move the authoritative task into `docs/ai/done/` on the integration branch, append an implementation report, commit and push it. Keep the integration branch/worktree for the user’s merge. Clean up only fully integrated subordinate worktrees and completed temporary notes.

## 11. Acceptance and validation

Use targeted unit tests for query transformations, state transitions, and preference migration; integration tests for asynchronous ownership and continuation; and actual native interaction checks for layout and input behavior. Avoid tests that merely assert the presence of widget labels.

| Area | Required check |
| --- | --- |
| Menus | Application-menu trigger is only a hamburger icon; opened menu is compact with left-aligned items and standard keyboard behavior; Columns uses actual checkboxes on the left and remains open for multiple toggles |
| Bundled fonts | Packaged application uses Source Sans 3 and Source Code Pro offline without system installations; real bold, missing-glyph fallback, licenses, startup registration and scaled layout are verified |
| Query controls | Filter and Filter Builder have distinct icon-only triggers with tooltips, keyboard access and active states; no query Options control remains; case/regex/count capabilities remain reachable |
| Field layout | Names are left, values right; no inspector search or synthetic `$.` label prefixes; literal source keys and query paths remain unchanged |
| Empty fields and arrays | Missing/null/empty string/whitespace/`[]`/`{}` rows are hidden; `0` and `false` remain; `Graphik; Gedicht` renders inline; complex arrays start collapsed and expand lazily in original order |
| Inspector emphasis | Filter-participating fields and verified FTS fields/spans are highlighted using the tab’s context; FTS-only queries still highlight; changing the main query does not change open-tab highlights |
| Links and source views | Links is beside Fields and contains outgoing links and backlinks; Source is a right-aligned icon; view switching preserves local state and both link directions support Back |
| Record actions | Copy record and Export record icons appear only in Source and operate on complete record data; no Copy value button; standard text selection still works; query export remains available |
| Metadata | One collapsible section appears above content; collapsed header shows green/red only for confirmed validity and neutral for unknown state; accessible labels explain it; table Status remains textual |
| Automatic saving | Saving adds a sidebar row without a dialog; collection-only queries can be saved; reopening restores case, regex, filter-enabled state and ordering |
| Identity and migration | Two distinct queries with identical displayed titles survive; renaming does not change identity; old custom names survive migration; a disabled retained filter stays disabled |
| History | Collapsed on startup; newest-first on expansion; star does not execute; no second History menu; pagination and tab navigation do not add history entries |
| Tab isolation | Open records under query A, change to query B, then navigate each tab through A; switching tabs preserves individual Back and view state |
| Back and relations | Initially disabled Back; Previous/Next and relation navigation enable it; Back restores the correct anchor and record; failed navigation preserves the current frame |
| Open query | Restores all query A controls and reveals its anchor, including beyond row 100 and after page eviction; no duplicate tab is opened |
| Async ownership | Delay record load, change the main query, close a tab, and open another: no late response updates the wrong tab or recreates a closed one |
| Sorting | Header edits the actual filter; typed sort updates indicators; predicates containing the literal words “SORT BY” are unchanged; sort-only filters work; resize does not sort |
| Disabled filter | Sorting cannot silently activate a retained predicate or discard it; Filter off clears effective sort indicators |
| Column behavior | Required order is stable across batches; nested filter paths are projected correctly; Status is text; source fields with metadata-like names remain distinct |
| Wrapping and resizing | Width persists; header/body remain aligned while scrolled; wrapping preserves selection and scroll anchor; huge values remain bounded and inspectable |
| Match evidence | Field label is bold with colon; excerpts correspond to actual FTS evidence; multiple fields, regex, Unicode, and raw/logical-unavailable cases are truthful |
| Validation | Pending/valid/invalid states have the same table top coordinate; no “Filter valid” line; invalid draft retains visibly labeled last-valid results |
| Completion | Declared and inferred fields complete; values use bounded posting requests; Enter only accepts; Escape only dismisses; stale responses and IME input are handled |
| Incomplete dictionaries | Missing or partially covered postings do not cause scans, suppress valid typed values, or falsely claim exhaustive suggestions |
| Loading | Exercise result counts 0, 1, 99, 100, 101, 200, 201 and at least 1,000; repeated bottom events do not duplicate requests/records; failures retry the same continuation |
| Stable order | Equal sort keys and deep scrolling do not skip or repeat records; clipped previews do not change continuation or identity |
| Export | All-results export includes results beyond the loaded batches; active-tab export is unaffected by a different main query |
| Read-only behavior | Source files, schemas and repository configuration are unchanged after the interaction suite |

Run the repository’s workspace test suite against the integrated result and existing benchmarks relevant to query paging, sorted-result reuse, and posting-value enumeration. Record actual commands and results; do not invent a new benchmark framework.

Measure on the same dataset and comparable environment at the base and integration revisions:

- First-batch latency and subsequent batches at shallow and deep positions, with and without sorting.
- Completion latency and visited dictionary keys for a high-cardinality field; source-document reads attributable to completion should be zero.
- Main-thread stalls while resizing, wrapping, switching tabs and scrolling.
- Opening a record with large complex arrays and many backlinks: collapsed Fields must not eagerly flatten arrays or enumerate the complete relation set; expansion and Links loading remain bounded.
- Memory after a long scrolling session and after opening/closing multiple tabs; retained record bodies and projections must obey the declared cache budget.
- Time and additional memory needed for Open query to reveal a distant tab anchor.

Use the established headless GUI environment and explicit resolution. Capture the actual application at 1440×900 and 1280×800 in both themes, plus the supported small-window case where practical. Include states showing sidebar history, several record tabs, FTS evidence with filter fields, wrapped/resized columns, invalid input, and completion. Also show the open application and Columns menus, two-column Fields with metadata above it, inline scalar arrays and collapsed complex arrays, Links with backlinks, and Source with its two record-action icons. Capture with the bundled fonts actually loaded. Existing remote/headless rendering limitations must be reported accurately.

If deep offsets repeatedly rescan/sort the corpus, or correct tab isolation requires retaining a large repository/result copy per tab, report the measured limitation before claiming completion. Correctness and bounded memory take priority over decorative additions; performance problems caused by the data model must be visible to the user.

## 12. Completion criteria

The integration branch is ready when all requested interactions work together, existing read-only functionality remains available, meaningful tests pass, and the report contains actual GUI and performance evidence.

Update help text and shortcuts to match sidebar history, inline saving, tab navigation, header sorting, continuous loading, icon-only menu/filter/source controls, and the Fields/Links presentation. Remove obsolete naming, history-panel, independent-sort, query Options, limit/offset, page-navigation, inspector-search and Copy value instructions. Do not leave both old and new routes visible. Include bundled-font origin, versions and notices in the implementation report.

The report must distinguish implemented behavior, measured results, and remaining limitations. Include branch/worktree, base and implementation commits, preference migration behavior, cache bounds, checks performed, and any blocked acceptance criteria. Deliver one integration branch for the user to merge; no automatic merge into `master`.

## Implementation report

Completed on 2026-11-08 for task `desktop-query-navigation`. The integration branch is `integration/desktop-query-navigation` in `/home/simon/source/objekte-integration-desktop-query-navigation`. The original checkout stayed on `master`. Base revision: `40bd5c3b97a7d29c184646f87b84670669ae7424`. Implementation commits, each pushed and verified before the next, are `e7d4d72de6`, `1aa26e3925`, `1b60624fd7`, `a08bba02bd`, `6d76c549e1`, `4989625dbb`, and `2c58f7f4ff`. This completed specification and report are committed separately on the same branch. Nothing was merged into `master`.

### Implemented behavior

- The desktop now has a persistent collection and saved-query sidebar, generated titles, inline renaming, immediate saving, stars, and collapsed newest-first history. Definitions have stable IDs independent of their displayed titles. Version 1 desktop preferences migrate to version 2; existing custom names, columns, theme, filter-enabled state, case/regex settings and ordering are retained. The TUI preference file is separate.
- A hamburger application menu and checkbox Columns chooser replace the old query Options route. Filter and Filter Builder have distinct icon controls. Table headers edit the authoritative `SORT BY` text, including sort-only filters; sorting does not activate a disabled retained predicate. Width, visibility and wrapping are stored per collection scope. Status is text, and the Match column has a readable minimum width.
- Results arrive in 100-record batches and scroll continuously. The controller keeps an immutable query context and generation for a result list, rejects stale continuations, retries failed ones, retains lightweight locators, and reconstructs evicted projections for distant navigation. A full 100-record batch is followed by a continuation check before declaring exhaustion. All-results export still uses the complete captured query; record export follows the active tab.
- Record tabs hold independent query contexts, anchors, Back stacks, Fields/Links/Source views and view scroll state. Previous, Next, relation navigation and Open query use the tab context after the main query changes. Fields has two columns, hides empty values while retaining zero and false, renders short scalar arrays inline, and expands complex arrays lazily. Links pages backlinks. Source contains the complete-record Copy and Export actions. Metadata sits above the content with confirmed-validity status.
- Filter completion is parser and caret aware, including quoted nested paths and `SORT BY`. It offers declared and inferred fields and bounded indexed values, including builder values, without scanning source records. Enter accepts a suggestion without executing the query; pointer selection works. Requests are debounced, cached by repository generation and context, canceled when superseded, and checked against the editor revision. A partial dictionary is labeled as partial, and typed input remains valid when no posting suggestion exists. FTS excerpts use verified match spans; the inspector emphasizes query fields and verified FTS spans from each tab's captured context.
- Help text and the native GUI guide describe the sidebar, menus, filter controls, continuous scrolling, header sorting, tabs, Fields/Links/Source, and current shortcuts.

### Native checks and source integrity

The [native capture index](../generated/desktop_query_navigation_evidence.md) preserves actual application screenshots at 1440 × 900 and 1280 × 800 in both themes, a 900 × 650 narrow case, the application and Columns menus, tabs, field/FTS emphasis, value completion, invalid filter, wrapped and resized columns, complex fields, Source actions, backlinks, and distant Open query. The application windows were captured on the fixed 1920 × 1080, scale 1, 60 Hz Sway output. The launcher verified those settings; WayVNC's `-R` keeps viewer connections from changing output geometry. Native interaction also exercised checkbox toggling without header sorting, Enter and pointer completion, 1,201 continuously loaded rows, a distant tab anchor, tab isolation after a main-query change, and a 1,200-backlink record. The rebuilt final app opened the synthetic repository and showed its first 100 rows.

For the offline font check, `FONTCONFIG_FILE` pointed to an empty configuration and `fc-list` returned zero installed fonts; a separately launched native window still rendered its typography ([capture](../generated/desktop_query_navigation_evidence/fontconfig-empty.png)). The binary registers its embedded fonts before the first window. Bundled static Source Sans 3 Regular/Semibold/Bold comes from Adobe's `source-sans` release commit `87b37a2daaed80fcb8e8ccb0085c4d72ddade12e`; Source Code Pro Regular/Bold comes from `source-code-pro` release commit `803b7e23ec97ae58b6232ea76519a76d428ba268`. Both carry SIL Open Font License 1.1, with copyright and license notices in their bundled `LICENSE.md` files. File hashes and source links are in [the font manifest](../../../crates/desktop/assets/fonts/README.md). The empty-font check demonstrates offline loading of the bundled faces; it does not establish fallback behavior for every missing Unicode glyph on every host.

The synthetic 121-record repository's 123 source and schema files passed `sha256sum -c /tmp/objekte-query-fixture-before.sha256` after the interaction suite. Desktop preferences under its `.objekte/.cache/` were expected to change. No source, schema or repository configuration edit was observed.

### Tests and measurements

`cargo test --locked --workspace` passed on the final implementation (including 78 desktop library tests, 333 core tests and other workspace suites). Targeted tests cover preference migration, sort/filter transforms, tab and asynchronous ownership, all batch edges 0/1/99/100/101/200/201/1,000, equal sort keys, projection bounds, completion replacement, FTS emphasis and record expansion. `python3 scripts/format-rust-spacing.py --check` and `git diff --check` passed. `cargo clippy --locked -p objekte-desktop --all-targets` exited successfully with warnings; the new single-range test warning was fixed afterward. The final native rebuild succeeded. The read-only fixture checksum check above passed.

The established `bench_musenalm` release benchmark used the same 148,486-record corpus and five repetitions at base and integration revisions. `cargo run --locked --release -p objekte-search-core --example bench_musenalm -- --root /home/simon/source/objekte/examples/musenalm --extended --cases Q134,Q135,Q136 --profiles ascii-off --repetitions 5 --result-dir …` measured counted, sorted 100-record locator pages. Median times (base → integration) were offset 0: **4,196.551 → 4,370.892 µs**; offset 1,000: **6,426.220 → 6,622.546 µs**; offset 10,000: **27,807.421 → 27,865.600 µs**. Differences are +4.2%, +3.1%, and +0.2%. Core query code was unchanged by this task, so these differences are run variation rather than a claimed speed improvement. The deep offset takes about 6.4 times the first page, even with the existing ordered-result reuse; deep browsing is still measurably more expensive.

The existing `bench_corrective_cases` high-cardinality fixture provided an unsorted page-only case and a counted case at both revisions (31 repetitions): page-only median **592.4 → 566.2 µs**; counted median **3,421.1 → 3,423.6 µs**. Posting-value prefix enumeration was **0.5 → 0.6 µs**, range enumeration about **0.5 µs** in both runs. Each visited 11 dictionary keys and made **zero source-document reads**. These are core API timings, not end-to-end completion popup latency. Benchmark artifacts and raw logs remain in ignored `target/gui-validation/desktop/` and `/tmp/objekte-*` on this host; only the results and native screenshots are committed.

The 1,201-record GUI fixture began at **182,668 kB VmRSS** after the first batch, reached **183,948 kB** after 1,100 rows and **183,960 kB** after exhaustion. A distant tab, a changed main query, four tabs and closure of all tabs brought VmRSS to **184,684 kB**; allocator retention means this process figure does not prove every allocation was freed. Opening the distant anchor through Open query took **311.6 ms** for VNC action dispatch and **446.3 ms** including capture, with **16 kB** additional VmRSS. The 1,200-backlink author used **184,808 kB** in collapsed Fields and **185,904 kB** after opening Links and loading its first 100 backlinks. Fields and Links dispatch/capture measurements were respectively **309.2/426.0 ms** and **309.8/425.6 ms**. These numbers include the control bridge and screenshot path; they are not pure GPUI frame times.

The retained UI display text budget is three pages at at most 4 MiB each plus one active detail at at most 2 MiB: **14 MiB**. Each page has at most 100 rows and each detail at most 512 structured and 512 relation rows. At most two page projections are pending concurrently; these are temporary worker allocations outside the retained budget. Inactive tabs retain identity, shared context and bounded navigation/view state rather than record bodies. The core repository and ordered-result index memory are separate from this UI text budget. A Back stack holds at most 32 frames per tab.

### Remaining limits

No functional acceptance criterion is blocked. The benchmark still shows that deep sorted offsets cost more than the first page; this is an existing core query characteristic and should be considered for very long result lists. The native action timings above put an end-to-end bound on observed interactions, but direct main-thread stall and frame-duration instrumentation was not collected, so this report does not claim a specific frame-time ceiling for resize, wrap, tab switching or scrolling. The screenshots show the correct states, and no freeze was observed in the exercised fixture. Missing-glyph fallback on a font-free host was not separately verified; the offline check verified the embedded faces themselves. The completion API intentionally caps all-collection dictionary traversal at eight collections and labels such results partial.
