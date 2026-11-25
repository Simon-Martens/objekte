# Managed files and direct records

A collection may receive records from both a registered file reader and a direct record writer. Register the file catalog and file source before building the index, then call `Index::add_file` for an existing saved file. Submit application-owned records through `RecordWriter`. Both routes use the collection's schema, indexes, queries and collection-scoped record IDs. A record ID can have only one live owner.

The runnable [public example](../../crates/core/examples/record_ingestion_public.rs) shows a rootless direct index, an interleaved file and direct collection, and a versioned direct provider.

## Migration from direct file links

| Previous call or use | Replacement |
| --- | --- |
| `IndexBuilder::enable_file_links` | Register a `FileCatalog` and a file source with a reader for files that own their records. No extra setup is needed for direct records. |
| `Batch::declare_file` and its `FileToken` | Save the file under the catalog root, then call `Index::add_file` or `Index::update_file`. Core opens the file and observes its revision and object identity. |
| `Batch::insert_with_file` or `RecordInput::with_file` | Import the saved file when the record is file-owned. Use `Batch::insert` or `Batch::insert_from_reader` when the application owns the record. |
| `Batch::replace_with_file` | Update the file through `Index::update_file` if it owns the record. Use `Batch::replace` for an application-owned record. Transfer ownership by retiring the old owner before inserting through the other route. |
| `Batch::delete_with_file` | Use `Index::delete_file` for a file-owned record's file, or `Batch::delete` for an application-owned record. |
| A file link used only for navigation | Store the path in an ordinary application field such as `path`. It remains ordinary data and does not populate `@file.*` metadata. |

`update_file_from_bytes` and its options variant remain conditional helpers. They compare the supplied revision and bytes with the source opened by core. A mismatch fails; the supplied bytes never become an indexed file revision. Use `update_file` after saving a changed file when no byte comparison is needed.

The old linked batch form only appeared in process-local overflow spools. Those frames have no restart replay or durability promise; the spool format version changed with the batch shape. The retired direct snapshot formats have no supported reader. Existing active sessions must finish or be cancelled before switching binaries. There is no supported durable hybrid value that needs reinterpretation.

## File revision and pinned reads

Core records file size, modification time and, on Unix, device and inode from the opened file. It validates the path and observed object again before publication. Query-time reads from a pinned view check that observed revision; if a later file at the same path has the same size and timestamp but a different object identity, the older view reports the source as unavailable instead of returning the newer bytes. A new import or update can then publish the new file revision.

On platforms without reliable object identity, lazy query reads return an unverifiable-source error. In-place writers that alter bytes while preserving the same inode, length and modification time cannot be detected by these markers. Applications should replace files atomically or let the normal revision markers change. The index's logical values and metadata remain those of the accepted observation until an explicit update or reconciliation.
