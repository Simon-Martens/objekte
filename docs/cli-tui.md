# Interactive terminal search

Run `cargo run --release -p objekte-cli -- examples/musenalm` from the project root to open the terminal UI. The installed `objekte` binary also accepts a repository path, defaults to the current directory, and accepts an optional `tui` subcommand. The UI opens in full-text mode, searching all collections with literal, case-insensitive substrings. Tab switches to field-filter queries. After configuration and schemas load, the interface opens while a worker builds the in-memory indexes. The footer reports build progress. You can type immediately; a search submitted during warmup runs automatically when the indexes are ready. The UI requires a terminal on stdin and stdout. There is no editing mode.

One-shot commands remain available for scripts and pipes:

```sh
cargo run --release -p objekte-cli -- examples/musenalm query places 'fictional = true' --count --limit 3 --output json
cargo run --release -p objekte-cli -- examples/musenalm search contents 'Goethe' --mode word --count --limit 3 --output ndjson
```

## Search controls

| Key | Action |
| --- | --- |
| Tab / Shift+Tab | Switch search modes; in detail, cycle Fields, Links, and Backlinks |
| Ctrl+Tab / Ctrl+Shift+Tab | Cycle collections forward or backward |
| Ctrl+R | Toggle regex in full-text mode |
| Ctrl+F | Toggle normalized or full text in full-text mode |
| Ctrl+E | Toggle case sensitivity in full-text word mode |
| Alt+I or Ctrl+I | Open field inference, including in schema-backed collections; C changes scope, A toggles advanced indexing, Enter scans. Afterward W updates inferred field types and index rules in existing schemas. `i` also works from the results table. |
| `[` / `]` in results | Flip collections directly from the result table |
| Enter | Run the query, open a result, or open the selected link or backlink |
| Up / Down | Select a result or relation row; scroll detail fields |
| Left / Right | Scroll result columns horizontally; flip through hits in detail, including across result pages |
| n / p | Next or previous 100-result page in results; next or previous hit in detail |
| a / e | Export all matches or the selected hit |
| Esc | From an item, return to results; from results, return to the query input; cancel export or inference prompt |
| Backspace in detail | Return to the previously visited linked item, or to results if there is none |
| q | Quit from any view, including the query input and export prompt |
| Ctrl+C | Leave the UI |

Filter mode shows only **Mode** and **Collection**. Full-text mode also shows **Regex** and **Text** (normalized or full). **Case** appears only when regex is off. Normalized full-text search renders declared Markdown, HTML, and XML, removes formatting, and collapses whitespace. Full text searches decoded source values with markup intact. Without regex, both match literal substrings, including partial words, with the Case option. With regex, Rust regex syntax applies and the pattern controls its own case flags, such as `(?i)`. The corresponding one-shot modes are `word`, `raw-word`, `regex`, and `raw-regex`.

Some terminals cannot report Ctrl+Tab separately from Tab. The UI requests enhanced keyboard reporting where available. In other terminals, use `[` and `]` from the result table to cycle collections.

Changing collections or full-text options reruns an active search from its first page. Searches run on a background worker; verified rows stream into the table in collection/ID order. The total appears when the search completes. Rows shown during a search are provisional and are cleared if a later error occurs. Paging and export become available after completion. Result paging stops at the first and last pages and reports the boundary in the status line.

The result table has one row per record. It shows collection, ID, status, and top-level JSON fields; full-text results also show the first matched leaf as `field: value` directly after status. This match preview is at most 65 terminal cells wide. In filter mode, queried fields appear after the metadata in first-use order, followed by the other fields. A nested query path promotes its top-level field. Other columns are at most 25 cells wide, with an ellipsis for longer text; object and array cells show their sizes. The table heading shows the current result range and total. Left and Right reveal further columns. Enter opens a separate detail view. The Objekte mark and Fields, Links, and Backlinks tabs share its first line, with `[ESC] BACK` at the right. One blank line separates them from a boxed detail panel. Its heading contains the collection, `Search:` term, and record position; the fields or relation table follow inside the box. The search box and its options disappear. Left and Right flip through hits, including across query pages; Up and Down scroll the current hit. The view hides empty fields, joins string arrays with commas, and indents complex array items under their original `[index]` numbers. Values wrap at whitespace within an 80-cell column, even on wider terminals. Full-text hits highlight matched leaf labels using exact array indices. A collapsed string-array row is highlighted if any of its strings matched. There is one blank line between metadata and entry data. File size is shown in decimal KB.

Schemas with `x-relation` declarations define which fields link to which collections. In an item view, Tab cycles through Fields, Links, and Backlinks. Links show the declared field location and target collection/ID; Backlinks show referring collection/ID and field location. Up and Down select a relation row, and Enter opens its record by collection and ID. Backspace returns through visited items; Esc returns directly to the results list. Backlinks are found with indexed filter queries on a background worker; the item opens immediately and backlink rows appear as the lookup proceeds. Without a declared relation, the corresponding table is empty. A missing target reports an error when opened.

Export asks for a path to a **new** JSON file and never overwrites an existing file. `a` streams every match through the query API, independent of the current page. The export contains the query, total, and hits. Each hit contains its record and metadata: collection, ID, path, byte size, modification time, hash, and validation state. Full-text hits also contain canonical `fields` and concrete `matched_paths`, such as `contents[9].incipit_statement`. `e` writes the selected hit with the same metadata. For a record reached through a link, `e` writes that record with `kind: "record"` and no originating query. An export error removes a partial new file.
