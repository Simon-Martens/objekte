# In-memory index

Source JSON files define the repository. `Repository::load_repository` loads configuration without scanning collections. `Options::new()` selects `CacheMode::Memory`; `CacheMode::Disabled` makes views scan source files directly. The in-memory picker serves views and default JSON filters. It is derived data, lives for one `Repository` handle, and writes no cache files.

`file_picker/` owns source inventory, bigram planning, typed postings, revisions, and ordered view keys. `json_filter/` owns filter syntax and evaluation. `repository/json_query/` contains the in-memory and direct-source query paths; `repository/views.rs` handles views. Tests live in separate files under `crates/core/tests/` and `crates/cli/tests/`.

## Index lifecycle

The picker enumerates JSON filenames, stores IDs in a byte arena with stable slots, and builds consecutive and skip-one bigram postings over decoded scalar values and distinct Unicode-folded projections. It also stores typed postings and ordered view keys for every top-level declared number, integer, boolean, or enum field, regardless of UI filter and sort permissions. Indexed views select a page from these keys, then read the page files. Other views scan candidates and sort in memory.

The picker builds lazily per collection. `refresh(collection)` applies external edits, deletes, and additions. Changed files update ordered view keys and revision timestamps; a conservative overlay keeps bigram and typed filtering complete until rebuild. Removed IDs leave revision tombstones in the handle.

A long-lived UI can call `warm_bigram_index(collection)` or `warm_all_bigram_indexes()` after startup. These build missing indexes without a query. `rebuild_bigram_index(collection)` and `rebuild_all_bigram_indexes()` refresh source inventory and rebuild bigram and scalar indexes. Rebuilding an existing picker retains in-process revisions and deletion tombstones. The all-collection methods run in name order and return per-collection metrics. Use a background thread, or call the collection methods individually to report progress between collections. A separate CLI process cannot reuse a previous process's warm index.

## Query reads

The default `auto` backend and explicit `bigram` backend select candidates through the picker. Safe typed comparisons narrow candidates; typed `OR` branches are unioned only when every branch has a safe indexed set. Source JSON is read, parsed, and evaluated for each candidate, so the index never decides the final match. `direct` evaluates source files without an index. `grep` uses ripgrep over source files as a candidate prefilter. These remain useful for parity checks and query shapes that need a broad scan.

`query_json_stream` emits validated page records through a callback in collection and ID order. Bounded candidate batches may run in parallel, then publish in order. The final summary confirms the total and metadata; a later error invalidates provisional records. The CLI's `--output events` writes the corresponding JSON lines.

View sorting uses ordered keys for eligible numeric, boolean, and enum fields. Null, missing, and invalid-type positions retain their view order. String alphabetical sorting is planned. UI settings still control which view filters and sorts are permitted.

Schema validation runs for returned JSON query matches, in-memory view pages, and explicit validation commands. A parseable `get_record` response remains `pending` until validated. Direct views likewise return parseable page records as `pending`. No validation state or reference edge is stored in the index. Outgoing references are extracted from requested records; incoming references include previously observed edges unless complete coverage is requested, which scans referring source collections.

## Freshness

`metadata-on-open` builds or checks a collection on first use of a handle. `metadata-on-request` repeats source metadata checks. `hash-on-request` rebuilds the picker on each request. `refresh` explicitly reconciles one collection. Under an on-open policy, call `refresh` after an external edit to make it visible to index selection. A changed page file read before refresh can produce `Unstable` rather than a stale result. Query results are not cached; warm page requests still read selected source records.
