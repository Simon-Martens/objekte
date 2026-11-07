# `obj` integration contracts

The fixed prerequisite is `4a93382f0404ae4cc6d1e7c028b7a48cacd006f4`. It includes the accepted optional indexing, general sorting, streaming, and the subsequently committed TUI work. This branch integrates the `obj` command without changing the original checkout.

## Ownership and interfaces

- **General JSON owner:** `model.rs`, query paths and evaluation for `@root`, JSON reader and text derivation, scalar FTS policy, planner and inference guards. Parsed arrays retain all elements in order; the homogeneous summary is optional and mixed remains mixed. Existing `ArrayValue::new` stays strict for declared data. Exceptional numeric values remain numeric, exact, and queryable, with compact comparison. `@root` denotes the data value with zero or more path steps; `@root.title` has the same data selection as `title`.
- **Execution owner:** explicit `All { offset }` versus `Page { offset, limit }` scope in the shared core streaming request; one traversal for all results; independent snapshot load/write policy and explicit directory, with default behavior retained for existing callers. No CLI-specific query engine. `RepositoryOptions` and `repository.rs`/`storage.rs` belong to this owner. They coordinate text-policy fingerprint additions with the general JSON owner.
- **CLI owner:** `crates/cli` package, `obj` binary, argument and line-input parsing, NDJSON output and process tests. The workspace manifest is integrated centrally. Build against the agreed shared core request and snapshot APIs rather than copying core logic. In the CLI, schema and inference are off by default, and no snapshot is read or written without the corresponding flag.
- **Integration owner:** workspace manifest, final version allocation, conflicts, cross-feature tests, benchmarks, documentation, and promotion.

## Stable behavior

The source reader owns one selected JSON text policy. `DecodedScalarValues` visits decoded string values and lexical numeric/boolean/null tokens, one scalar per segment; it never indexes property names or container syntax. The old logical-string policy remains available. Construction and exact FTS verification must use the same policy. The text policy and logical representation version enter snapshot compatibility.

The CLI protocol is version 1, with monotonic event `sequence` and `query_id`: `run_start`, `query_start`, zero or more `hit`, one query terminal, and one run terminal. All-results mode reports an exact total; a finite page can report an unknown total unless `--count` is set. Hit payloads use original JSON tokens compacted into one NDJSON line; malformed fallback has `data_available: false`. Query preparation, core execution, sink callback, and query wall durations are distinct monotonic nanosecond measurements.

The primary ownership boundaries above are fixed. Agents may add private helpers and request narrow interface adjustments through integration review. Existing public APIs remain compatible unless this plan explicitly requires a semantic extension.

## Pinned names at the first interface checkpoint

- `SourceFormat::JsonDecodedScalars` is the built-in schema-free `obj` reader. Its source id is `json_decoded_scalars`, its file extension is `json`, and its reader version starts at 1. `SourceFormat::Json` retains existing logical-string text. `CollectionDefinition::folder("files", PathBuf::new(), SourceFormat::JsonDecodedScalars)` selects the repository root and relative-path identities.
- `ArrayValue::from_unrestricted(values)` admits parsed heterogeneous members. `ArrayValue::new` remains strict for declared arrays. `mixed_elements` is sticky once unlike non-null element kinds are observed. A new exact JSON number variant is used only outside the old native numeric domain.
- `streaming::ResultScope::{Page { offset, limit }, All { offset }}` is passed to `Repository::search_stream_scoped`. `query.options.count` controls a finite page count; `All` always determines the exact total.
- `Repository::open_with_snapshot_policy(root, definitions, options, SnapshotOpenPolicy::new(mode, directory))` selects load-only `SnapshotMode::{Disabled, IfValid, Required}`. Startup reports `SnapshotAction` and an optional restore error. `Repository::open()` defaults to `IfValid`. Opening never saves; the caller invokes `save_snapshot(path)` to persist a built repository. The `obj` flag mapping is documented in [the CLI guide](../cli/obj.md#snapshot-policy).
- `Discovery.exclude_directories` holds exact `PathBuf` directories relative to the collection source root. The existing exclude globs remain available for names such as `.git` at any depth.
