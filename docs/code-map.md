# Core and TUI code map

This is a reading guide for the query and index code in `crates/core/src/`. Source JSON files are authoritative. An index selects *candidates*; a query still checks source content before returning a record.

## Start with a query

| Task | Entry point | Next file to read |
|---|---|---|
| JSON field query | `repository/json_query.rs` | `json_query/memory.rs` for indexed candidates, `source.rs` for direct and grep reads |
| Full-text query | `repository/text_query.rs` | `text_query/matcher.rs` for pattern and field matching |
| Explicit sort | `json_query/sorted.rs` or `text_query/sorted.rs` | `sorting.rs` for collation keys, `sorting/buffer.rs` for the unindexed fallback |
| Filter syntax | `json_filter/mod.rs` | `json_filter/parser.rs` and `json_filter/prefilter.rs` |
| Named view | `view_definition.rs` | The same JSON or full-text query entry points |

The sorted query modules choose between two routes. A matching schema sort index on a **verified** picker supplies final candidate order directly. Otherwise, the bounded buffer projects current source values and retains the best prefix needed for the requested page. An indexed query stops after the batch that finds a match beyond `total_limit`; that extra match establishes whether the capped result is truncated. Candidate verification runs in parallel batches; callbacks publish rows in order after a batch finishes.

## Follow an index

`file_picker/mod.rs` defines `FilePicker`, stable file slots, and initial opening. The slots point into one ID byte arena. Postings refer to slots, so updates do not renumber the immutable bigram index.

| File | Responsibility |
|---|---|
| `file_picker/refresh.rs` | Reconcile directory metadata or one changed ID; update overlays and secondary keys |
| `file_picker/rebuild.rs` | Read and project source files, compact slots when necessary, then publish new immutable postings |
| `file_picker/candidates.rs` | Intersect conservative bigram, typed, and presence candidates; order them for queries |
| `file_picker/sort_index.rs` | Store locale-aware single and composite sort keys and ordered slot views |
| `file_picker/modified_order.rs` | Maintain default newest-first order over stable slots |
| `file_picker/secondary.rs` | Save and restore typed, presence, sort, and other secondary indexes |
| `file_picker/text.rs` | Maintain the separate rendered/raw full-text postings when needed |

The immutable bigram filter and changed-file overlay live in `bigram/`. A restored cache can be used before filesystem reconciliation finishes, so `FilePicker::verified()` gates the direct sorted path. Refresh patches changed files into the overlay; rebuild creates a new immutable index. Saved sort keys include the schema language in their configuration fingerprint.

## Tests and style

Integration tests are separate from production code under `crates/core/tests/`. Start with `sorting.rs` for collation, indexed/unindexed parity, pagination, and cache restore; `combined_search.rs` for JSON plus full-text filtering; `default_modified_order.rs` for the default order; and `text_search.rs` for full-text semantics. File-picker unit tests are separate `*.test.rs` files loaded by their production modules.

The workspace convention is one blank line between functions and two around `impl` blocks. Run `cargo fmt --all`, then `python3 scripts/format-rust-spacing.py`; verify with the script's `--check` flag. The repository's [code guidelines](code-guidelines.md) cover comments, tests, and diagnostics.

## Terminal UI

`crates/cli/src/tui/mod.rs` owns the event loop and keyboard routing. `state.rs` owns the visible query, page, selection, and pending work. `search.rs` sends query events from the worker to that state; `warm.rs` manages background index work. `render.rs` now keeps the frame layout in `draw`, with its pieces in `render/`:

| File | What it draws |
|---|---|
| `render/query.rs` | Editable query boxes |
| `render/controls.rs` | Search options and detail headers |
| `render/table.rs` | Result columns, cells, and the visible table |
| `render/detail_panel.rs` | A selected record and its relations |
| `render/export_prompt.rs` | Export destination dialog |
| `render/inference_prompt.rs` | Inference dialogs |
| `render/hints.rs` | Context-sensitive footer shortcuts |

`render.test.rs` is kept separate from rendering code and checks the final terminal buffer, including narrow layouts and keyboard hints.
