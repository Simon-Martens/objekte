# Interactive terminal search

Run `cargo run --release -p objekte-cli -- examples/musenalm` from the project root to open the terminal UI. The installed `objekte` binary also accepts a repository path. It defaults to the current directory and accepts an optional `tui` subcommand. The interface opens once configuration and schemas have loaded. A worker builds the in-memory indexes meanwhile, and the header reports its progress. You can type immediately: a search started during warmup runs as soon as the indexes are ready. The UI needs a terminal on stdin and stdout. It is read-only.

One-shot commands remain available for scripts and pipes:

```sh
cargo run --release -p objekte-cli -- examples/musenalm query places 'fictional = true' --count --limit 3 --output json
cargo run --release -p objekte-cli -- examples/musenalm search contents 'Goethe' --mode word --count --limit 3 --output ndjson
cargo run --release -p objekte-cli -- examples/musenalm search contents 'Goethe' --filter 'year >= 1800' --count
```

## Layout

The screen layout never changes; only its content does. From top to bottom:

- **Header:** the collection being searched (Ctrl+O changes it) and the index status.
- **Search line:** full-text search. Option chips at its right show case (Alt+C), regex (Alt+R) and normalized or full text (Alt+T).
- **Filter line:** a JSON filter, as in [queries](queries.md).
- **Summary rule:** the match count, search time and sort, plus the visible row range. A query error also appears here.
- **Results:** shown on the left. A preview of the selected record appears on the right when the terminal is at least 110 columns wide; Ctrl+P hides or shows it.
- **Footer:** key hints for the current state, or a short notice.

Typing searches as you go: a search starts 180 ms after the last key, and Enter searches at once. While you type, a filter that does not parse yet is only hinted at in the summary rule, and the previous results stay. Enter on such a filter shows it as an error. When the query is unchanged, Enter opens the selected record. Esc clears the active line, and a second Esc clears both.

## Keys

| Key | Action |
| --- | --- |
| typing | Edit the active line; search after a short pause |
| Tab | From Search, move to Filter; in Filter, complete from the schema |
| Shift+Tab | Switch between Search and Filter |
| ↑ ↓, PgUp PgDn, mouse wheel | Move the selection; more rows load as you approach the end |
| Ctrl+U / Ctrl+D | Page up or down, in the results and in a record |
| Ctrl+Home / Ctrl+End | First or last result |
| Alt+← / Alt+→ | Scroll result columns; the identity columns stay |
| Enter | Search, or open the selected record |
| Esc | Close suggestions, then clear the line |
| Ctrl+O | Choose the collection, or all collections |
| Ctrl+K | Action palette: export, inference, saving and rebuilding indexes, all toggles |
| Alt+C / Alt+R / Alt+T | Case-sensitive, regex, normalized or full text |
| Ctrl+P | Show or hide the preview |
| F1 | All keys |
| Ctrl+Q / Ctrl+C | Quit |

Line editing supports Home/End, Ctrl+A/Ctrl+E, Ctrl+←/→ (move by word), and Ctrl+W or Alt+Backspace (delete a word).

## Filter completion

Tab in the Filter line completes from the collection schemas, scoped to the chosen collection. It offers:

- **Fields:** with their types and collections. Nested object fields and array items are included. Relation fields continue into the linked collection's fields, matching implicit relation traversal. Keys that are not identifiers are inserted in bracket form, such as `["first edition"]`.
- **System fields, functions and modifiers:** `@id`, `@modified` and the other system fields, `exists(…)` and `missing(…)`, and `:fts`, `:lower`, `:length` and `:each` after a colon.
- **Operators:** chosen for the field type. Numbers get comparisons, text gets `~` and `!~`, booleans only `=` and `!=`, and arrays add the `?` any-value forms.
- **Values:** enum members (also inside an opened quote), `true`, `false`, `null`, `@status` values, and `@now`-style date macros for date-like fields.
- **Connectives:** `&&`, `||`, `SORT BY`, and then sort fields and `ASC`/`DESC`.

Tab inserts the only candidate that starts with the typed text, or extends the text to the longest shared prefix. Otherwise it opens a list. ↑ and ↓ choose in the list, Tab or Enter inserts, and Esc closes it. Tab on a complete object or relation field name steps into its fields. Collections without a schema offer only system fields and functions.

## Searching

Either line may be empty. If both have content, the filter selects records first, and the full-text matcher then searches their included values. Normalized full text renders declared Markdown, HTML and XML, removes formatting and collapses whitespace. Full text searches decoded source values with markup intact. Without regex, both match literal substrings, including partial words, with the case option. With regex, Rust regex syntax applies and the pattern controls its own case flags, such as `(?i)`. The corresponding one-shot modes are `word`, `raw-word`, `regex` and `raw-regex`.

Append `SORT BY title ASC && year DESC` to either line to sort within each collection. System fields such as `@size`, and rendered `title:fts` terms, work as in [queries](queries.md). If both lines specify a sort, the Search line wins, even with no search term. To search for the literal phrase `SORT BY` while also appending a sort, quote the pattern.

Searches run on a background worker, and a newer query cancels an older one. Rows in default order stream by collection and modification time, newest first, with ID breaking ties. Rows with an indexed sort stream in sort order. Without an index, provisional sorted pages replace earlier ones as better matches arrive. The selected record keeps its place on screen while that happens: the selection follows the record, not the row number. After a refresh, the selected record also stays at the same screen row. The total and the elapsed time appear when the search completes. A trailing `+` means the repository's result limit was reached. Rows load 100 at a time as you scroll; there are no pages. Rows shown during a search are cleared if the search later fails.

## Results and records

The table has one row per record. It shows the collection (when searching all collections), the ID and a status dot. Full-text results then show the first matched leaf as `field: value`. Queried fields, sort fields and top-level JSON fields follow. Column widths are fixed when a column first appears, so rows never shift as more results arrive. Object and array cells show their size.

Enter opens the record. Its title row, with the record's position in the results, and its tabs replace the query lines; everything else stays in place. In the record view:

- 1, 2 and 3, or Tab and Shift+Tab, switch between **Fields**, **Links** and **Backlinks**.
- ← and → step to the previous or next result, loading more rows when needed.
- ↑ and ↓ scroll the fields, or choose a link.
- Esc returns to the results.

The fields view hides empty values, joins string arrays with commas, and indents array items under their `[index]`. Field names that the full-text search matched are highlighted. Metadata (path, size, modification time, hash, matched paths) comes first.

Schemas with `x-relation` declarations define which fields link to which collections. Links show each field and its target; Backlinks show the records that refer to this one. Backlinks are found on a background worker, and rows appear while the lookup proceeds. Enter opens the selected linked record. Backspace walks back through the linked records you visited, and Esc returns straight to the results. Opening a missing target reports an error.

## Actions

Ctrl+K lists every action with its shortcut; typing filters the list. It includes:

- **Export all matches / Export selected record.** Each asks for a path to a **new** JSON file and never overwrites an existing one. Exporting all streams every match through the query API, up to the repository's total limit; the file holds the query, the total, the truncation flag and the hits. Each hit carries its record and metadata: collection, ID, path, size, modification time, hash and validation state. Full-text hits also carry `fields` and concrete `matched_paths`, such as `contents[9].incipit_statement`. A record reached through a link is exported with `kind: "record"` and no query. An export error removes the partial file.
- **Infer fields and index rules.** C switches between the current collection and all collections, A toggles advanced indexing, and Enter scans. Afterwards, W writes the inferred field types and index rules to the existing schema files.
- **Save indexes to disk** under `.objekte/.cache`, and **Rebuild all indexes and save**.
