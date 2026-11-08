# Index lifecycle implementation contracts

This is the phase 0 design record for task `index-lifecycle`. The implementation
base is `9a94bc90d0d80ff7d14f44e457acf8787212d5ab` on 8 November 2026.
It records interfaces and ownership before production modules are changed.

## Baseline and compatibility

`Repository` is currently an immutable, owned, densely slotted search index.
Its `open_with_sources_and_snapshot_policy` path synchronously compares all
source metadata after restoring a cache. Existing CLI, TUI and desktop callers
use `Repository` directly. The new lifecycle API will coexist with this static
API while callers are migrated; a default `Repository::open` must not acquire
implicit cache writes.

`cargo test --workspace --all-targets` on the base passed 333 core unit tests
(two ignored) and the core integration tests, then failed in the existing
`accept_musenalm` example test
`extended_schema_declares_only_annotated_id_relations`: the actual schema hash
was `479448c83ff23c748e2c971e7020e8b76a40c1e80ed194fdb5276562dd81a66e`
versus expected
`38f2dc5112146aa6ac0514291c76eaeb741421fa8a4624e50fa83c4c0a449bf9`.
The baseline mismatch comes from workspace feature unification: desktop's
`gpui-pre` enables `serde_json/preserve_order`, while the example hashes
serialization whose object-key order then changes. The tracked schema values
match the archived manifest. This is a pre-existing test/example issue, not a
lifecycle regression. The standard `cargo test --workspace` baseline passed
with warning-level diagnostics; the all-targets example needs canonical
serialization to be a reliable workspace gate.

## Public boundary

- A cloneable `Index` handle owns one published index instance. `IndexRevision`
  is a monotonic publication number within that instance; it is not a source
  modification time or a persistent file identifier.
- `Index::view()` pins an immutable `IndexView`. Every search, count, stream,
  record lookup and relation operation against that view uses one revision.
  New requests through the handle acquire the current view. Page clients must
  carry the instance and revision to avoid mixing pages.
- A file handle contains its repository-local `FileId` and `FileVersion`.
  Update, move and delete compare that version at commit. Old handles fail
  with a typed conflict or stale-handle error, including delete/add at one
  path. An explicit current lookup by logical ID is separate from locator
  reads and cannot silently reuse a retired slot.
- Mutation results identify the actual strategy, revision, affected
  collections, old/new record counts and any verified-maintenance fallback.
  An invalid old-byte hint is a successful normal replacement when the target
  version still matches; it never changes a newer incarnation.
- Long operations return observable status/task handles with cancellation and
  wait. Status distinguishes index revision, reconciliation state, cache
  durability, operation progress and query completeness. UI code formats
  messages; it does not parse them to infer state.

## Publication representation

The intended representation is an immutable base plus bounded per-collection
append segments and active-slot domains, with structurally shared unaffected
files/collections. A published root holds the file path/version map and all
affected collection views together. The mutation coordinator prepares a file
with the existing reader/build contribution path, validates versions and IDs,
then swaps one `Arc` root under a short lock. Source I/O, extraction, sorting,
serialization and retired-view destruction occur outside that lock.

Query routes will read a merged collection view: base and delta typed/text
postings, active-domain intersection before NOT, counting and offset/limit,
merged sort order, and current relation target resolution. Cache keys include
instance and publication revision. Reconciliation calls the same file mutation
operations, never a separate indexing engine. Segment count and bytes must be
bounded by measured consolidation policy. A small update must not clone all
records, posting lists, sorted arrays or the file ownership offset table.

Before locking in this representation, a single-file prototype must measure
copy/allocation cost and search parity on a representative collection. If it
cannot meet touched-file cost or relation semantics, revise this record and
the design before exposing mutation as complete.

## Persistence and callers

Explicit build, schema load, cache load and flush receive concrete paths.
Cache load validates internal structure before exposing the restored view and
then schedules the initial metadata scan; callers can wait when exact current
state is required. Only `open_repository` discovers repository-local schemas
and cache destinations. Its `sync_cache` option defaults to off, is runtime
configurable for desktop preferences, and coalesces dirty-revision writes.
Flush pins and reports the saved revision, serializes writers per destination,
and never marks a newer published revision clean. Cache writes cannot target
source, schema or repository configuration files.

The core task owns implementation and minimal caller migration. The separately
authorized desktop task starts from the pushed core integration commit. It
owns the native management surface, preferences, and revision-safe navigation.
