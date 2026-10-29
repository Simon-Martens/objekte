# In-memory and segment caches

The source JSON files define the repository. `Repository::load_repository` loads configuration without scanning collections. `Options::new` selects `CacheMode::Memory`. Views and default JSON filters share a lazily built in-memory picker. `CacheMode::Segments` opts into persistent segment-backed views; `CacheMode::Disabled` executes views directly against source files.

The code follows those boundaries: `file_picker/` owns inventory, bigram planning, typed postings, and ordered view keys; `json_filter/` owns syntax and evaluation; `repository/json_query.rs` and `repository/views.rs` execute requests. `repository/segment_cache.rs` owns snapshot reads and storage helpers, while `repository/segment_cache/sync.rs` reconciles and publishes updates. Unit tests live in separate `.test.rs` files; end-to-end tests live under `crates/core/tests/` and `crates/cli/tests/`.

## In-memory picker

The picker enumerates JSON filenames, stores IDs in a byte arena with stable file slots, and builds consecutive and skip-one bigram postings over decoded scalar values. For every top-level declared number, integer, boolean, or enum field, it also stores ordered view keys and typed filter postings, independent of UI filter and sort declarations. Indexed view filters and simple sorts select a page from these keys, then read and validate only page files. Other view shapes scan candidates and sort in memory. JSON filter queries still parse source JSON candidates to verify the complete expression.

Changed files update their ordered view keys and revision timestamps immediately; bigram and JSON-filter typed postings use a conservative changed-file overlay until rebuild. Removed IDs leave revision tombstones. The picker lives only in the repository process. `refresh` applies external changes; request freshness policies can refresh it automatically.

## Optional segment files

## Files

Each collection has a directory under `<data-dir>/repositories/<root-digest>/<namespace>/segments/<collection-digest>/`:

| File | Purpose |
| --- | --- |
| `CURRENT.json` | Cache version, schema and mapping hashes, collection revision, source manifest hash, generation, and current metadata filenames |
| `meta-NNNN-G.bin` | A shard's ID-sorted entries, line map, array markers, and ordered scalar lists |
| `text-NNNN-G.txt` | One values-only line for each appended record version; stale lines remain until compaction |
| `LOCK` | Shared reader and exclusive writer lock |

`segments/CONFIG.json` stores configuration input digests and ownership across restarts. Its own `LOCK` serializes changes. Cache format version 5 is disposable: incompatible or damaged snapshots rebuild from original files.

A text line contains decoded scalar values from the JSON document, without braces, field names, or quotes. Scalars are separated by ASCII unit separator (`0x1f`); newlines in strings are replaced by that separator so one record stays on one line. Text is a necessary-match prefilter, never the final query result. The parser reopens candidate source files and evaluates the full filter. Line numbers map back to IDs; stale lines have no current ID and cannot produce a result.

Metadata for each current ID stores its source size, modification nanoseconds and file identity (including ctime on Linux), content digest, record revision, change timestamp, text line/byte offset/length, typed keys, and array flags. Deletion tombstones retain the last revision and change time so recreating an ID advances its revision. IDs are byte-sorted in the metadata map. A stable hash assigns each ID to a shard. New caches use twice the Rayon worker count, capped by the number of records. Existing caches retain their shard count.

## Updates and publication

Reconciliation inventories the collection and compares its manifest. Unchanged stamps reuse metadata. Changed files are read and hashed; equal content updates only the stamp. A changed value appends a new line in its existing shard, clears the old line's ID, increments its record revision, and updates ordered lists. Deletions clear the current ID and its typed keys. Source JSON is never rewritten by cache maintenance.

When stale text exceeds 25% of live text in a touched shard, only that shard is rewritten with current lines. The new metadata and text file are written and synced before `CURRENT.json` is replaced through a same-filesystem rename. Readers hold a shared lock for the whole snapshot search, so old shard files can be removed after publication without invalidating an active reader. The write lock covers reconciliation and publication. A future process can rebuild everything from source if a write is interrupted.

## Reads

A general JSON filter can use a safe values-text literal and ordered scalar lists to narrow candidates. Ripgrep's `grep-searcher` scans shards in parallel; Sonic parses original JSON candidates and checks the complete expression, including exact numeric comparisons and global scalar cardinality. A view uses ordered lists for simple numeric, boolean, or enum ordering and reads the selected page from source. Other view shapes use candidate scanning and in-memory sort. The direct backend remains the reference for result parity.

The default JSON filter query backend builds FFF-compatible consecutive and skip-one bigram postings over decoded scalar values, plus in-memory ordered postings for all declared number, boolean, and enum fields. FFF's regex syntax planner narrows text candidates; simple conjunctive scalar comparisons narrow them further. Edited files enter a conservative overlay until rebuild. Source JSON is still parsed and checked for every candidate. This index is built on first use and is not persisted. `--backend segments` and `--backend direct` remain available explicitly. Segment-backed views require `--cache segments`.

Ordered lists are built for every top-level declared number, integer, boolean, and enum field, independent of UI filters and sorts. Null, missing, and invalid-type keys retain their view ordering. Current enum ordering is the scalar's bytewise value order. String ABC sorting is planned.

JSON Schema validation is performed on demand for returned records and explicit `validate` commands. The cache stores neither validation states nor reference edges. Outgoing references are extracted from a requested record; incoming references are discovered by scanning referring source collections. `cache status` reports source/cache coverage and revisions, with an empty persisted-validation map.

## Freshness and limitations

The default `metadata-on-open` policy verifies a collection on first use of a repository handle. `metadata-on-request` repeats the check. `hash-on-open` and `hash-on-request` rehash source contents according to the same timing policy. A same-handle edit under an on-open policy needs `refresh` to become visible; reading a changed page file before refresh can return `Unstable` instead of a stale result. Query results themselves are not cached. Page records are read from source after index selection, so a warm page still incurs those source reads. The cache is local to the machine and can be deleted at any time.
