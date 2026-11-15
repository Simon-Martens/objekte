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
reader layout must agree. Fresh builds and live indexes retain digests for
base files, so eligible base and delta contributions can discard old postings
physically before publication. Base maintenance compacts a private staged
root, bounded to 262,144 scanned records per collection; ordinary mutations
remain sparse. Invalid or unsupported hints use the normal
tombstone strategy and report a structured fallback reason. This is an index
optimization; it does not change source data. Old pinned views keep their old
immutable contributions.

The verified base path is a storage optimization with collection-sized work.
On the 148,486-file Musenalm fixture it took about 1.28 seconds and temporarily
raised process memory above 1 GiB, while a normal appended update took about
0.33 milliseconds. Use the ordinary update path when write latency matters,
and compact later. A future per-file base partition could make verified base
removal incremental without the current collection-sized rewrite.

For a compatible isolated one-record delta shard, a verified replacement can
retain its segment ordinal and local slot in a new immutable shard. The old
and new views have distinct index revisions; an old pinned view still owns its
original shard. Multi-record or shared-shard changes use a new segment.

A mainly retired main shard is vacuumed in place instead of being dissolved:
dead slots are removed from every posting, gram, order and relation family and
then reused by later appends. One-record build partitions that are mostly
retired move their surviving records into the main shard rather than becoming
new partitions. Small index shards are also folded into one main shard per collection. The
main shard marks replaced records retired and appends the new incarnation;
nothing is rewritten. When at least half of a main shard is retired and it has
at least 4,096 slots, it is dissolved back into ordinary partitions. Verified
maintenance treats a main shard as too expensive to rewrite and falls back to
normal appended replacement for its files.

`Index::compact` constructs dense active-only indexes from the published base
and segments without reading source files. It advances the revision only when
retired slots exist. `Index::rebuild` rereads the current explicit source
configuration while searches continue on the previous view. Both operations
publish only against the revision they captured; a concurrent mutation causes
a bounded retry. Compaction and rebuilding keep all state in memory.
`start_compaction` and `start_rebuild` return observable, cancellable worker
handles. Cancellation before publication leaves the current view installed;
`shutdown` cancels and joins active maintenance workers.

`Index::build` and `Index::start_incremental_build` use explicit definitions
and sources in memory. A progressively built catalogue becomes searchable
before background metadata reconciliation finishes. `wait_for_reconciliation`
lets callers wait for that check. Reconciliation compares size and modification
time; it does not hash unchanged files and cannot detect an edit that restores
both metadata values. There is no index store or flush operation.

The active record set and index revision live in memory and are released when
the owner drops the index. Verified query results stay in memory and are scoped
to the published revision. Source access still checks the recorded revision, so an
old pinned view does not make changed or missing source bytes available.
Availability-oriented searches and streams report incomplete results and
bounded source diagnostics when needed bytes cannot be read. Exact count and
strict search reject such results; direct lookup has a distinct unavailable
outcome. A detected source failure requests coalesced reconciliation.
