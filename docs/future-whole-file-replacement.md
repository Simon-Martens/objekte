# Runtime file index maintenance

The earlier deferral of runtime updates no longer applies. `Index` in
`crates/core/src/lifecycle.rs` owns a versioned published view. A query pins one
immutable view; a file mutation prepares privately, checks the file version and
index revision, then publishes the complete change in one step. Existing pinned
queries may finish on the old view. A new query sees the new view.

The application owns source files. `Index::add_file`, `update_file`, `move_file`,
and `delete_file` change the index only. The caller creates, edits, moves, or
removes the source first, then informs the index. `update_file_from_bytes` can
avoid rereading bytes already held by the caller, but does not write those
bytes to disk. Later payload reads still verify the source revision.

Normal update and delete retire old record slots. Replacement adds a bounded
indexed segment. Active masks suppress retired records in search, lookup,
sorting, posting values, relations, and streams. File IDs and versions are
separate from record slots and the monotonic index revision. A stale file
handle cannot update a newer incarnation. An unchanged update can preserve the
revision when path, metadata, memberships, configuration, and digest agree.

`update_file_with_options` and `delete_file_with_old_content` accept complete
old bytes for verified maintenance. The stored SHA-256 digest and compatible
reader layout must agree. Eligible delta segments can discard old postings
physically before publication. Invalid or unsupported hints use the normal
tombstone strategy and report a structured fallback reason. This is an index
optimization; it does not change source data. Old pinned views keep their old
immutable contributions.

For a compatible isolated one-record delta shard, a verified replacement can
retain its segment ordinal and local slot in a new immutable shard. The old
and new views have distinct index revisions; an old pinned view still owns its
original shard. Multi-record or shared-shard changes use a new segment.

`Index::compact` constructs dense active-only indexes from the published base
and segments without reading source files. It advances the revision only when
retired slots exist. `Index::rebuild` rereads the current explicit source
configuration while searches continue on the previous view. Both operations
publish only against the revision they captured; a concurrent mutation causes
a bounded retry. Compaction and rebuilding do not imply a cache write.

`Index::build` and `Index::load_cache` use explicit definitions and paths.
Loading a compatible cache restores a searchable view first, then starts
metadata reconciliation in the background. `wait_for_reconciliation` is
available when a caller needs the initial check to finish. Reconciliation
compares size and modification time; it does not hash unchanged files and
cannot detect an edit that restores both metadata values. The explicit core
handle never discovers or writes a cache on its own. `flush_cache(path)` saves
one pinned revision to the requested destination. `Index::open_repository` is
the separate convenience entry point for the established `.objekte` layout,
with optional periodic checking and coalesced automatic cache saving.

The active record set and index revision are persisted with the index cache.
Prepared and verified query caches stay in memory and are scoped to the
published revision. Source access still checks the recorded revision, so an
old pinned view does not make changed or missing source bytes available.
