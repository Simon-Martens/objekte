# Interactive terminal search

Run `objekte REPOSITORY` to open the terminal UI. `objekte` uses the current directory; `objekte REPOSITORY tui` is equivalent. The UI requires a terminal on stdin and stdout and warms every collection's in-memory bigram index once at startup. It reuses the same repository handle for later searches. There is no editing mode.

One-shot commands remain available for scripts and pipes:

```sh
objekte REPOSITORY query contents 'year >= 1800' --count --output json
objekte REPOSITORY search contents 'Goethe' --mode word --count --output ndjson
```

## Search controls

| Key | Action |
| --- | --- |
| Tab / Shift+Tab | Switch between filter and full-text modes |
| Ctrl+Tab / Ctrl+Shift+Tab | Cycle collections forward or backward |
| Ctrl+R | Toggle regex in full-text mode |
| Ctrl+F | Toggle normalized or full text in full-text mode |
| Ctrl+E | Toggle case sensitivity in full-text word mode |
| `[` / `]` in results | Flip collections directly from the result table |
| Enter | Run the query in the input field; open the selected result in the table |
| Up / Down | Select a result; scroll the detail view |
| Left / Right | Scroll result columns horizontally; flip through hits in detail, including across result pages |
| n / p | Next or previous 100-result page in results; next or previous hit in detail |
| a / e | Export all matches or the selected hit |
| Esc | Return from detail, cancel export, or leave the UI |
| Ctrl+C | Leave the UI |

Filter mode shows only **Mode** and **Collection**. Full-text mode also shows **Regex** and **Text** (normalized or full). **Case** appears only when regex is off. Normalized full-text search renders declared Markdown, HTML, and XML, removes formatting, and collapses whitespace. Full text searches decoded source values with markup intact. Without regex, both match literal substrings, including partial words, with the Case option. With regex, Rust regex syntax applies and the pattern controls its own case flags, such as `(?i)`. The corresponding one-shot modes are `word`, `raw-word`, `regex`, and `raw-regex`.

Some terminals cannot report Ctrl+Tab separately from Tab. The UI requests enhanced keyboard reporting where available. In other terminals, use `[` and `]` from the result table to cycle collections.

The result table has one row per record. It shows collection, ID, status, and top-level JSON fields; object and array cells show their sizes. Each column is at most 25 terminal cells wide, with an ellipsis for longer text. Left and Right reveal further columns. Enter opens a plain two-column detail view. Left and Right flip through hits, including across query pages; Up and Down scroll the current hit. The view hides empty fields, joins string arrays with commas, and indents complex array items under their original `[index]` numbers. Values wrap at whitespace within an 80-cell column, even on wider terminals. Full-text hits highlight matched leaf labels using exact array indices. A collapsed string-array row is highlighted if any of its strings matched. There is one blank line between metadata and entry data. File size is shown in decimal KB.

Export asks for a path to a **new** JSON file and never overwrites an existing file. `a` streams every match through the query API, independent of the current page. The export contains the query, total, and hits. Each hit contains its record and metadata: collection, ID, path, byte size, modification time, hash, and validation state. Full-text hits also contain canonical `fields` and concrete `matched_paths`, such as `contents[9].incipit_statement`. `e` writes the selected hit with the same metadata. An export error removes a partial new file.
