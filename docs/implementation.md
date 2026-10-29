# In-memory and segment caches

The source JSON files define the repository. `Repository::load_repository` loads configuration without scanning collections. `Options::new` selects `CacheMode::Memory`. Views and default JSON filters share a lazily built in-memory picker. `CacheMode::Segments` opts into persistent segment-backed views; `CacheMode::Disabled` executes views directly against source files.

The code follows those boundaries: `file_picker/` owns inventory, bigram planning, typed postings, and ordered view keys; `json_filter/` owns syntax and evaluation; `repository/json_query.rs` handles the query API, with memory, segment, and source execution in `repository/json_query/`; `repository/views.rs` executes views. `repository/segment_cache.rs` owns snapshot reads and storage helpers, while `repository/segment_cache/sync.rs` reconciles and publishes updates. Unit tests live in separate `.test.rs` files; end-to-end tests live under `crates/core/tests/` and `crates/cli/tests/`.

## In-memory picker

The picker enumerates JSON filenames, stores IDs in a byte arena with stable file slots, and builds consecutive and skip-one bigram postings over decoded scalar values plus distinct Unicode-folded projections. For every top-level declared number, integer, boolean, or enum field, it also stores ordered view keys and typed filter postings, independent of UI filter and sort declarations. Indexed view filters and simple sorts select a page from these keys, then read and validate only page files. Other view shapes scan candidates and sort in memory. JSON filter queries still parse source JSON candidates to verify the complete expression.

Changed files update their ordered view keys and revision timestamps immediately; bigram and JSON-filter typed postings use a conservative changed-file overlay until rebuild. Removed IDs leave revision tombstones. The picker lives only in the repository process. `refresh` applies external changes; request freshness policies can refresh it automatically.

## Optional segment files

### Files

Each collection has a directory under `<data-dir>/repositories/<root-digest>/<namespace>/segments/<collection-digest>/`:

| File | Purpose |
| --- | --- |
| `CURRENT.json` | Cache version, schema and mapping hashes, collection revision, source manifest hash, generation, and current metadata filenames |
| `meta-NNNN-G.bin` | A shard's ID-sorted entries, line map, array markers, and ordered scalar lists |
| `text-NNNN-G.txt` | One values-only line for each appended record version; stale lines remain until compaction |
| `LOCK` | Shared reader and exclusive writer lock |

`segments/CONFIG.json` stores configuration input digests and ownership across restarts. Its own `LOCK` serializes changes. Cache format version 6 is disposable: incompatible or damaged snapshots rebuild from original files.

A text line contains decoded scalar values from the JSON document, without braces, field names, or quotes. Scalars are separated by ASCII unit separator (`0x1f`); newlines in strings are replaced by that separator so one record stays on one line. When Unicode case folding changes bytes beyond ASCII lowercase, a folded projection follows the original values on the same line. Text is a necessary-match prefilter, never the final query result. The parser reopens candidate source files and evaluates the full filter. Line numbers map back to IDs; stale lines have no current ID and cannot produce a result.

Metadata for each current ID stores its source size, modification nanoseconds and file identity (including ctime on Linux), content digest, record revision, change timestamp, text line/byte offset/length, typed keys, and array flags. Deletion tombstones retain the last revision and change time so recreating an ID advances its revision. IDs are byte-sorted in the metadata map. A stable hash assigns each ID to a shard. New caches use twice the Rayon worker count, capped by the number of records. Existing caches retain their shard count.

### Updates and publication

Reconciliation inventories the collection and compares its manifest. Unchanged stamps reuse metadata. Changed files are read and hashed; equal content updates only the stamp. A changed value appends a new line in its existing shard, clears the old line's ID, increments its record revision, and updates ordered lists. Deletions clear the current ID and its typed keys. Source JSON is never rewritten by cache maintenance.

When stale text exceeds 25% of live text in a touched shard, only that shard is rewritten with current lines. The new metadata and text file are written and synced before `CURRENT.json` is replaced through a same-filesystem rename. Readers hold a shared lock for the whole snapshot search, so old shard files can be removed after publication without invalidating an active reader. The write lock covers reconciliation and publication. A future process can rebuild everything from source if a write is interrupted.

## Query reads

A general JSON filter can use a safe values-text literal and ordered scalar lists to narrow candidates. Ripgrep's `grep-searcher` scans shards in parallel; Sonic parses original JSON candidates and checks the complete expression, including exact numeric comparisons and global scalar cardinality. A view uses ordered lists for simple numeric, boolean, or enum ordering and reads the selected page from source. Other view shapes use candidate scanning and in-memory sort. The direct backend remains the reference for result parity.

The default JSON filter query backend builds FFF-compatible consecutive and skip-one bigram postings over decoded scalar values, plus in-memory ordered postings for all declared number, integer, boolean, and enum fields. FFF's regex syntax planner narrows text candidates; safe typed comparisons narrow them further. For `OR`, typed postings are unioned only when every branch has a safe indexed candidate set. Edited files enter a conservative overlay until rebuild. Source JSON is still parsed and checked for every candidate. This index is built on first use and is not persisted. `--backend segments`, `--backend direct`, and `--backend grep` remain available explicitly. Segment-backed views require `--cache segments`.

`query_json_stream` emits validated page records through a callback as batches finish, in collection and ID order. Direct queries emit during their serial source scan. The bigram, segment, and grep backends verify bounded batches in parallel, then publish records in order. Candidate planning and first index construction still precede emission. A final summary carries the total and query metadata; records remain provisional until that summary is returned because a later candidate can fail the query. The CLI `--output events` translates the callbacks and final outcome into flushed JSON lines.

Ordered lists are built for every top-level declared number, integer, boolean, and enum field, independent of UI filters and sorts. Null, missing, and invalid-type keys retain their view ordering. Current enum ordering is the scalar's bytewise value order. String ABC sorting is planned.

JSON Schema validation is performed for returned JSON query matches, memory/segment view pages, explicit `validate` commands, and `get_record` in segment mode. A parseable `get_record` response in memory mode is still `pending`; direct views also return parseable page records as `pending`. The cache stores neither validation states nor reference edges. Outgoing references are extracted from a requested record. Incoming references use previously observed edges in memory mode unless complete coverage is requested; segment mode scans referring source collections. `cache status` reports coverage and revisions, with an empty persisted-validation map.

## Freshness and limitations

The default `metadata-on-open` policy builds/checks a collection on first use of a repository handle. `metadata-on-request` repeats source metadata checks; `refresh` explicitly reconciles a collection. In segment mode, hash policies also compare source contents at the corresponding open/request point. For the in-memory picker, `hash-on-request` rebuilds it on each request; it does not maintain a separate persistent content-hash manifest. A same-handle edit under an on-open policy needs `refresh` to enter the picker; reading a changed page file before refresh can return `Unstable` instead of a stale result. Query results themselves are not cached. Page records are read from source after index selection, so a warm page still incurs those source reads. Segment cache files are local to the machine and can be deleted at any time.
