# Terminal UI

Run `cargo run --release --bin objekte -- examples/musenalm` from the workspace
root. The terminal window opens before repository startup and shows the elapsed
index-building time. The first run may take longer while the index is built;
the release build is recommended for the Musenalm corpus. After each search,
the result status line shows its execution time.
The binary is in `crates/tui` and uses the schema files under
`REPOSITORY/.objekte/collections/` to construct the core's in-memory
`CollectionDefinition` values. Schema loading belongs to the TUI adapter;
applications can also construct definitions directly and call
`Repository::open`.

Type literal text in **Search**, or press Tab for a structured **Filter**.
Tab again selects the results. Enter runs the query or opens a selected
record. Ctrl+O selects a collection, Alt+S sorts within it using fields
declared in `x-sort-indexes`, Ctrl+P toggles the preview pane, F1 shows
keyboard help, and Ctrl+Q quits. Ctrl+R opens recent queries, Ctrl+B opens
saved queries, and Ctrl+S saves the current one. Alt+L shows or hides result
columns. Alt+E exports the selected record; Alt+A exports all matches to a
new JSON file. The UI fetches result pages of 100 records as you approach
the end. Settings live in `.objekte/.cache/tui.json`.

The adapted TUI supports core search, filters, collection scope, paging,
sorting, record inspection, saved queries, display settings, and export.
The earlier TUI's inference, backlinks, editing, regex and case modes,
text representation switch, and cache controls await corresponding core
APIs.
