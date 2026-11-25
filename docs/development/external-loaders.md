# External source loaders

A new `SourceRegistry` is empty. Install the formats the application accepts before starting discovery or registering a file source. The stable format IDs and representation versions are unchanged.

| Dependency | Registration | Format IDs |
| --- | --- | --- |
| `objekte-loader-json` | `objekte_loader_json::register(&mut sources)` | `json`, `json_decoded_scalars` |
| `objekte-loader-jsonl` | `objekte_loader_jsonl::register(&mut sources)` | `jsonl` |
| `objekte-search-core` | `Index::builder()` for direct records | No file registration needed |

Replace `SourceRegistry::builtin()` with explicit calls to the loaders you select. Pass the registry to `Index::start_build`, `SourceScan::open`, or `Index::builder().with_source_registry(...)`. `objekte_cli::loader::installed_sources()` exposes the shared CLI, TUI and desktop defaults. Linking JSON Lines pulls in the JSON record decoder and registers only `jsonl`.

Replace core JSON Schema helpers with `objekte_loader_json::configuration::load_optional_schema` or `load_repository_configuration`. For one collection, pass the compiled `Schema` and sort fields to the existing collection definition. When the explicit schema is inside the data tree, include its relative path in `discovery.exclude_files`. Repository configuration supplies discovery exclusions and the schema files the host must protect. The compiler supplies an exact-byte digest through `Schema::source_digest()`. Reuse the compiled schema across files and queries. Direct inputs and manually declared schemas need no file.

## Examples

```sh
cargo run -p objekte-search-core --example direct_records
cargo run -p objekte-loader-json --example json_collection -- /path/to/json /path/to/schema.json
cargo run -p objekte-loader-jsonl --example parallel_jsonl -- /path/to/jsonl 4
```

Omit the schema argument for undeclared JSON. A JSON Lines application can apply the same compiled `Schema` to each record.

A custom reader implements `SourceReader` and registers a stable ID, representation version, and extension. Core opens and checks the file, then supplies the opened input to the reader. A reader can use the default document and range adapters, or implement a streaming or pull framer for large files. A range reader must return unchanged source bytes. Core chooses and coalesces ranges, checks the expected revision, bounds work and publishes complete file revisions. The same reader instance may receive concurrent callbacks and must keep per-file state scoped to the current attempt.
