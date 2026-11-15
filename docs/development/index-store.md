# Index store

`crates/core/src/index_store.rs` keeps a published index in a `redb` file so a
later opening can load it instead of parsing every source file. The store is
derived data: source files remain the truth. A missing, locked, damaged or
configuration-mismatched store is always handled by rebuilding from source.

## API

```rust
let (index, outcome) = Index::open_persistent(
    root,
    definitions,
    options,
    SourceRegistry::builtin(),
    StoreOptions { path: Some(store_path), durable: true },
)?;
index.checkpoint()?;
```

- `StoreOpenOutcome` is `Loaded`, `Rebuilt(StoreRebuildReason)` or
  `WithoutStore(reason)`.
- `Index::checkpoint` writes all changes made since the previous checkpoint.
- `Index::has_store` reports whether a usable store is attached.
- `StoreOptions.path` defaults to `<root>/.objekte/.index/index.redb`.
  `durable: false` skips fsync for tests and benchmarks.
- Store failures are `IndexError::Io("index store: ...")`; no `IndexError`
  variant was added.

`Index::build`, `start_incremental_build` and every CLI, TUI and desktop open
path stay memory-only and never read, create or modify a store file.

## Validity

Store format version 2 adds `free_slots`, `free_file_rows` and `dense_epoch` to
a main shard so vacuumed and reused state survives a checkpoint. A checkpoint
after a vacuum rewrites the shard's dense rows once; later checkpoints write
only new and reused slots.

The `meta` table records the store format version, the 32-byte configuration
identity, the posting chunk size, the revision, the next ordinal and a
completion flag. A store is used only when the format, configuration and chunk
size match and the completion flag says the last checkpoint finished. A
mismatch rebuilds; a failure while reading a valid-looking store is treated as
damage and also rebuilds.

## Tables

All tables use byte keys and byte values. Every shard row starts with a 12-byte
`collection: u32` + `store_id: u64` prefix. Variable key parts (field paths,
column paths, posting keys) carry a `u32` length. Integers are big-endian.
Tables: `meta`, `collections`, `catalog`, `shards`, `active`, `files`,
`records`, `posting_fields`, `postings`, `bitmaps`, `sort_columns`,
`sort_keys`, `composites`, `relations`, `references` and `inference`. Posting
values are stored one row per `(key, chunk)`; fixed bitmaps are stored whole.

`shard_keys` and value codecs live in `index_store_keys.rs`.
`index_store_shard.rs` writes, reads and diffs one shard.

## Checkpoint and load

A checkpoint pins one view, opens one write transaction and marks the store
incomplete. Immutable shards already stored are only updated when their active
mask changes; new immutable shards are written once. A main shard is diffed
against its stored snapshot, so only changed posting and gram chunks and new
rows are written. Removed shards lose their key range. Catalog rows and
collection inference summaries are compared and written only when changed.
The transaction commits atomically and only then replaces the in-memory
snapshot and store counters. A failed checkpoint keeps the index usable and
forces the next checkpoint to rewrite everything.

Loading reads the catalogue and every stored shard, rebuilds the derived
tables (file index, id lookup, sort orders, posting stats and accounting), and
does not run maintenance. It then starts the normal source reconciliation, so
edits made while the application was closed are picked up through the ordinary
mutation path. The known limit is unchanged: an edit that restores both size
and modification time is not noticed.

Queries on a loaded index read RAM; the store is not memory-mapped or demand
paged. Loading costs time proportional to the index size.

The frontends do not use the store yet. Wiring it into CLI, TUI and desktop is
a separate task.
