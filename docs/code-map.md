# Core and TUI code map

This is a reading guide for the query and index code in `crates/core/src/`. Source files are authoritative. An index selects *candidates*; a query still checks source content before returning a record. The current indexed query path handles JSON sources; registered provider/decoder records use a logical executor with exact scan fallback and optional engine-owned representation bigram and derived typed postings.

## Start with a query

| Task | Entry point | Next file to read |
|---|---|---|
| JSON field query | `repository/json_query.rs` | `json_query/memory.rs` for indexed candidates, `source.rs` for direct and grep reads |
| Full-text query | `repository/text_query.rs` | `text_query/matcher.rs` for pattern and field matching |
| Explicit sort | `json_query/sorted.rs` or `text_query/sorted.rs` | `sorting.rs` for collation keys, `sorting/buffer.rs` for the unindexed fallback |
| Filter syntax | `json_filter/mod.rs` | `json_filter/parser.rs` and `json_filter/prefilter.rs` |
| Named view | `view_definition.rs` | The same JSON or full-text query entry points |
| Registered frontend | `repository/extension_query.rs` | `extensions/query.rs` for logical plans and bindings, `repository/extension_sources.rs` for selected reads |

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
| `file_picker/typed/derived.rs` | Bind named extractor values to the existing typed B-tree and patch retained slot keys |
| `repository/derived_query.rs` | Direct and indexed derived-value queries, ordered pages and cache keys |
| `repository/extension_sources.rs` | Merge default JSON and registered source descriptors, reject collisions, perform revision-checked borrowed decode |
| `source/filesystem_json.rs` | Built-in `SourceProvider` adapter for JSON descriptors and secure reads used by registered frontends |
| `repository/extension_query.rs` | Compile registered logical plans, combine safe candidates and exact predicates, retain bounded sorted descriptors, and project returned pages |
| `repository/extension_index.rs` | Explicit engine-owned bigram postings for one registered text representation and selected source set |
| `repository/extension_index_snapshot.rs` | Explicit portable selected-representation filter snapshots with semantic companion metadata |
| `repository/extension_derived_index.rs` | Explicit derived-value postings over the selected source set using the existing typed B-tree, lazy scalar-safe sort order, and versioned selected-posting snapshots |
| `repository/extension_projection.rs` | Fused explicit build of several selected-source text and typed postings from one decode |
| `extensions.rs` | Registration, schema binding and stable definition lookup |
| `extensions/identity.rs`, `source.rs`, `format.rs` | Stable descriptors, source providers and borrowed decoder contracts |
| `extensions/projection.rs`, `operator.rs`, `query.rs` | Text and typed projections, exact operators, candidate constraints and logical frontend plans |
| `extensions/builtins.rs` | Native JSON and text adapters using the public contracts |
| `file_picker/text.rs`, `text/projection.rs`, `text/query.rs` | Build combined and per-field postings, project source values, and choose text candidates |

The immutable bigram filter and changed-file overlay live in `bigram/`. A restored cache can be used before filesystem reconciliation finishes, so `FilePicker::verified()` gates the direct sorted path. Refresh patches changed files into the overlay; rebuild creates a new immutable index. Saved sort keys include the schema language in their configuration fingerprint.

The extension contracts now have separate source, format, projection, operator and query modules. The public `extensions::*` re-exports retain existing import paths. Engine storage stays in `file_picker/` and `bigram/`; specialized execution and verified-result caches stay under `repository/`. Keeping those concrete hot paths in place avoids a module move with no behavior benefit.

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
