# External source loaders

A new `SourceRegistry` is empty. Install the formats the application accepts before starting discovery or registering a file source. The stable format IDs and representation versions are unchanged.

| Dependency | Registration | Format IDs |
| --- | --- | --- |
| `objekte-loader-json` | `objekte_loader_json::register(&mut sources)` | `json`, `json_decoded_scalars` |
| `objekte-loader-jsonl` | `objekte_loader_jsonl::register(&mut sources)` | `jsonl` |
| `objekte-search-core` | `Index::builder()` for direct records | No file registration needed |

Replace `SourceRegistry::builtin()` with explicit calls to the loaders you select. Pass the registry to `Index::start_build`, `SourceScan::open`, or `Index::builder().with_source_registry(...)`. `objekte_cli::loader::installed_sources()` exposes the shared CLI, TUI and desktop defaults. Linking JSON Lines pulls in the JSON record decoder and registers only `jsonl`.

## Rust API migration

| Previous API | Replacement |
| --- | --- |
| `SourceFormat::Json` | `objekte_loader_json::JSON_DOCUMENTS` or `objekte_loader_json::folder(name, path)` |
| `SourceFormat::JsonDecodedScalars` | `objekte_loader_json::JSON_DECODED_SCALARS` or `objekte_loader_json::decoded_scalar_folder(name, path)` |
| `SourceFormat::JsonLines` | `objekte_loader_jsonl::JSON_LINES` |
| `SourceFormat::Registered(id)` | `SourceFormat::registered(id)` |
| `Value::from_json(value)` | `objekte_loader_json::value_from_json(value)` |
| `objekte_search_core::query::parse_parameter_json(text)` | `objekte_loader_json::parameters::parse_parameter_json(text)` |
| `Query::from_workload(case)` | `objekte_cli::workload::query_from_workload(case)` for benchmark requests |

The JSON folder helpers select relative-path IDs. The core folder constructor defaults to generated IDs. Use `CollectionDefinition::folder_with_identity` to supply an explicit policy. Use the loader selectors when preserving existing configuration fingerprints. A generic registration can select the same reader with its own configuration fingerprint.

Replace core JSON Schema helpers with `objekte_loader_json::configuration::load_optional_schema` or `load_repository_configuration`. For one collection, pass the compiled `Schema` and sort fields to the existing collection definition. When the explicit schema is inside the data tree, include its relative path in `discovery.exclude_files`. Repository configuration supplies discovery exclusions and the schema files the host must protect. The compiler supplies an exact-byte digest through `Schema::source_digest()`. Reuse the compiled schema across files and queries. Direct inputs and manually declared schemas need no file.

## Examples

```sh
cargo run -p objekte-search-core --example direct_records
cargo run -p objekte-loader-json --example json_collection -- /path/to/json /path/to/schema.json
cargo run -p objekte-loader-jsonl --example parallel_jsonl -- /path/to/jsonl 4
```

Omit the schema argument for undeclared JSON. A JSON Lines application can apply the same compiled `Schema` to each record.

A custom reader implements `SourceReader` and registers a stable ID, representation version, and extension. Core opens and checks the file, then supplies the opened input to the reader. A reader can use the default document and range adapters, or implement a streaming or pull framer for large files. A range reader must return unchanged source bytes. Core chooses and coalesces ranges, checks the expected revision, bounds work and publishes complete file revisions. The same reader instance may receive concurrent callbacks and must keep per-file state scoped to the current attempt.
