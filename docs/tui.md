# Terminal UI

Run `cargo run --release -p objekte-cli --bin obj -- --repo examples/musenalm --tui` from the workspace
root. The terminal window opens before repository startup and shows the elapsed
index-building time. On a fresh build, the file catalogue opens after discovery
and search can begin while records are still indexed. A valid stored generation
opens without rebuilding. The first run may take
longer while the index is built; the release build is recommended for the
Musenalm corpus. After each search,
the result status line shows its execution time.
The terminal UI is an optional feature of `crates/cli`, enabled by default for the `obj` binary. It uses the schema files under
`REPOSITORY/.objekte/collections/` to construct the core's in-memory
`CollectionDefinition` values. Schema loading belongs to the CLI adapter;
applications can also construct definitions directly and call
`Index::open_with_storage`.

Type literal text in **Search**, or press Tab for a structured **Filter**.
The Filter offers fields from the current index publication, built-in metadata,
modifiers, Boolean literals, and bounded indexed-value suggestions. During
construction, field suggestions refresh as inferred fields are published;
indexed-value suggestions become available after construction completes.
Use Alt+Up/Down to choose a suggestion, Enter to insert it, or Esc to dismiss it.
Tab again selects the results. Enter runs the query when no suggestion is
showing; in results, it opens the selected record. Ctrl+O selects a collection,
Alt+S sorts within it using fields
declared in `x-sort-indexes`, Ctrl+P toggles the preview pane, F1 shows
keyboard help, and Ctrl+Q quits. Ctrl+R opens recent queries, Ctrl+B opens
saved queries, and Ctrl+S saves the current one. Alt+L shows or hides result
columns. Alt+E exports the selected record; Alt+A exports all matches to a
new JSON file. The UI fetches result pages of 100 records as you approach
the end. Settings live in `.objekte/.cache/tui.json`.

Ctrl+K opens **Index storage**. Choose Memory, Temporary, Repository, or
Custom; Custom also asks for an existing base directory. Disk choices offer
**Keep entire index in memory**. Applying a changed setting reopens the
repository and shows the opening state until the replacement is ready.
Disk indexes synchronize
changes during use and flush on orderly close.

Ctrl+I switches between search results and the physical file catalogue. The
catalogue shows repository-relative paths, sizes, and file timestamps; a
listed file is not a claim that it contains a searchable record. During a
fresh build, matching records appear as batches become available and the
result count is marked provisional. Some queries wait for more indexing work.
The final count, result paging, and export of all matches become available
after construction completes. Editing a query cancels its previous search
without stopping indexing. A build failure is shown in the status area.

Displayed and exported source-file paths are relative to the opened repository
root. The `@file.path` filter uses that same value; `@repository.root` exposes
the root separately.

The adapted TUI supports core search, filters, collection scope, paging,
sorting, record inspection, saved queries, display settings, and export.
The earlier TUI's inference, backlinks, editing, regex and case modes,
and text representation switch await corresponding core
APIs.
