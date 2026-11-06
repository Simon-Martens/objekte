# Core and TUI code map

This is a reading guide for `crates/core/src/` and `crates/cli/src/tui/`. Source files are authoritative. The native JSON picker retains its concrete query and index paths. A query scope containing custom provider records uses the selected-record inventory, even when that scope also contains JSON files. Indexes supply candidates; exact evaluation and returned-page materialization use current source records.

## Start with a query

| Task | Entry point | Next file to read |
|---|---|---|
| JSON field query | `repository/json_query.rs` | `repository/json_query/memory.rs` for the native picker, `repository/json_query/selected.rs` for selected records, `repository/json_query/source.rs` for native direct and grep reads |
| Full-text query | `repository/text_query.rs` | `repository/text_query/matcher.rs` for native matching, `repository/text_query/selected.rs` for selected records and bound representations |
| Derived-value query | `repository/derived_query.rs` | `repository/derived_query/selected.rs` for selected records, `file_picker/typed/derived.rs` for native postings |
| Explicit sort | `repository/json_query/sorted.rs` or `repository/text_query/sorted.rs` | `sorting.rs` for collation keys, `sorting/buffer.rs` for bounded fallback sorting |
| Filter syntax | `json_filter/mod.rs` | `json_filter/parser.rs`, `json_filter/prefilter.rs`, and `json_filter/fts.rs` |
| Named view | `view_definition.rs` | The ordinary JSON or text query entry points |
| Registered frontend | `repository/extension_query.rs` | `extensions/query.rs` for logical plans, `repository/extension_sources.rs` for selected reads |

`repository/json_query.rs` and `repository/text_query.rs` choose the native or selected route from the configured collections. Their collecting and streaming methods share that routing. A JSON-only scope keeps the native picker path, even if an applicable provider currently contributes no custom records. A scope containing custom records reads through source descriptors and decoders, then evaluates logical fields; a registered frontend compiles a plan for that same selected inventory.

Native sorted execution can use a verified picker order or a bounded fallback. Selected JSON and text queries use `repository/ordered_result_cache.rs` for complete ordered slot lists, with `repository/predicate_cache.rs` managing cache ownership and budget. A completed cache hit selects the requested range; returned records are still read through their descriptors. `repository/coverage.rs` records complete and partial result coverage. Query identities include bound semantic dependencies and source generation.

## Follow a source and an index

`repository.rs` owns the repository facade, source snapshots, index maps, and cache state. `repository/extension_sources.rs` selects provider descriptors, rejects record-ID collisions, chooses the decoder, and reads a selected record with revision checks. `repository/source_records.rs` owns native JSON inventory and secure source reads. `source/filesystem_json.rs` is the built-in provider adapter. `repository/selected_lifecycle.rs` warms, restores, rebuilds, and saves selected indexes through the ordinary lifecycle. `repository/reconciliation.rs` coordinates refresh and native snapshot operations.

`file_picker/mod.rs` defines `FilePicker`, stable JSON file slots, and initial opening. Its postings refer to slots; refresh patches changed records into overlays and rebuild publishes a new immutable index. The native JSON projection remains fused rather than routing through an owned logical record.

| File | Responsibility |
|---|---|
| `file_picker/refresh.rs`, `file_picker/rebuild.rs` | Reconcile native source changes and rebuild immutable picker state |
| `file_picker/candidates.rs`, `file_picker/text/query.rs` | Combine typed, presence and text candidate constraints for native queries |
| `file_picker/projection.rs`, `file_picker/text/projection.rs` | Project native JSON and text fields during index construction |
| `file_picker/sort_index.rs`, `file_picker/modified_order.rs` | Store schema sort keys and default newest-first order |
| `file_picker/secondary.rs`, `file_picker/typed/derived.rs` | Persist native secondary indexes and maintain named derived values |
| `repository/extension_index.rs`, `repository/extension_derived_index.rs` | Engine-owned selected text bigrams and derived typed postings |
| `repository/extension_projection.rs` | Build requested selected text and typed outputs from shared record access |
| `repository/extension_index_snapshot.rs`, `repository/selected_lifecycle.rs` | Validate and restore selected postings and manage ordinary selected-index persistence |
| `repository/extension_query.rs` | Bind logical plans, combine safe candidates, evaluate exact predicates, and materialize frontend pages |
| `extensions.rs`, `extensions/identity.rs` | Register definitions and bind stable semantic identities |
| `extensions/source.rs`, `extensions/format.rs` | Source descriptors, providers, borrowed decoder contracts, and logical records |
| `extensions/projection.rs`, `extensions/operator.rs`, `extensions/query.rs` | Text and typed projections, exact operators, candidate guarantees, and frontend plans |
| `extensions/builtins.rs` | Default JSON and text adapters using the public contracts |

The immutable bigram filter and changed-file overlay live in `bigram/`. The native picker stores typed, presence and sort postings under `file_picker/`; selected representations and derived values use separate engine-owned postings keyed by their semantics and source selection. Selected snapshots must match the current inventory and bound definitions before postings become visible. [Library extensions](../library-extensions.md) describes the public extension contracts.

## Tests and style

Integration tests live in `crates/core/tests/`; `crates/extension-tests/src/lib.rs` exercises public extension registration, mixed formats, and lifecycle behavior. Start with `crates/core/tests/sorting.rs` for indexed and fallback order, `crates/core/tests/combined_search.rs` for JSON plus text filtering, `crates/core/tests/default_modified_order.rs` for default order, and `crates/core/tests/text_search.rs` for text semantics. File-picker unit tests use adjacent `*.test.rs` files.

The workspace convention is one blank line between functions and two around `impl` blocks. Run `cargo fmt --all`, then `python3 scripts/format-rust-spacing.py`; verify with its `--check` flag. The [code guidelines](code-guidelines.md) cover comments, tests, and diagnostics.

## Terminal UI

`crates/cli/src/tui/mod.rs` owns the event loop and warmup/reconciliation handoff. `app.rs` owns visible query state, page, selection, and pending work. `search.rs` runs query work; `warm.rs` prepares indexes in the background. Input and keyboard routing live in `input.rs` and `keys.rs`. `view/mod.rs` lays out the frame, with `view/results.rs`, `view/record.rs`, and `view/overlay.rs` drawing the main results, record detail, and overlays. Adjacent `*.test.rs` files exercise TUI behavior and rendered output.
