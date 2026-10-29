# Objekte: implemented library and next product work

Objekte reads collections of JSON files through a Rust library and CLI. Source files are authoritative. The CLI is read-only; it can inspect, query, sort, validate, and find declared references. A desktop editor and source-file write operations are future work. See [queries.md](queries.md) for runnable commands and [filters.md](filters.md) for query semantics.

## Current capabilities

- Repository configuration and JSON Schema load without enumerating every collection. Collections are opened lazily.
- `get`, configured table `view`, and general `query` over declared JSON fields. Views support configured columns, filters, scalar sorts, secondary sorts, pagination, and optional counts. JSON queries support nested paths, arrays, parameters, Boolean expressions, and exact decimal comparisons. They return records in collection and byte-sorted ID order.
- The default in-memory file picker builds consecutive and skip-one bigram postings over decoded values and typed postings for every top-level declared number, integer, boolean, and enum field. The same picker supplies ordered keys for eligible views. Query candidates are checked against original JSON with Sonic; the index alone never decides a match.
- Optional persistent values-only segment shards with ID-sorted metadata, ordered scalar lists, revisions, changed-file updates, and per-shard compaction. Select `--cache segments` for segment-backed views and add `--backend segments` for segment-backed JSON queries. `--cache disabled` makes views scan source files. JSON queries also offer explicit `direct` and raw-file `grep` backends.
- Explicit collection/all-collection validation, JSON/schema status on returned pages where validation runs, source refresh, configuration-change reporting, cache status, and per-record revision lookup. The default in-memory revisions last only for the repository handle; segment revisions persist across processes. Validation status is not stored in either index.
- Declared outgoing references are extracted on request. Incoming references can be discovered by scanning referring source collections. The CLI's `relations --to ... --complete` requests complete coverage; `--check-targets` checks current target files. There is no persistent edge table or automatic source rewrite on a move/delete.

The cache indexes fields independently of `list.filters`, `list.sortable`, and `list.defaultSort`. Those UI options still control which filters and sorts a view request may use. Enum ordering is bytewise; string ABC sorting is planned. The cache is derived data and can be rebuilt from source. [implementation.md](implementation.md) describes its lifecycle.

## Product boundaries

| Source | Responsibility now |
| --- | --- |
| Repository configuration | Collection paths, schemas, source identity, declared relations, and UI settings |
| JSON Schema | Stored types and on-demand validation rules |
| UI settings | Permitted view columns, filters, and sorts; presentation defaults |
| Library | Source reads, query compilation and execution, validation, reference lookup, freshness, and disposable indexes |
| CLI | Parse commands and display library results; no separate query or cache implementation |

Paths through embedded arrays inspect values in the same source record. JSON filters do not traverse declared relations, group records, or search metadata such as source path. Malformed JSON does not match a JSON query. A parseable schema-invalid record can still match a declared field. See [validation.md](validation.md) for the precise validation behavior.

## Work still planned

- Source-file create, update, move, and delete operations, with revision conflict checks, atomic writes, and reference handling. The current repository API does not rewrite referring JSON files.
- Saved and grouped views, aggregates, relation traversal within a query, and metadata operands.
- User-facing raw grep results with source line/offset and cleaned-content search. The `--backend grep` option is only a candidate prefilter for a structural JSON query; it returns matching records, not grep lines.
- A persistent desktop UI with filesystem watching, background refresh, result patching, cancellation, and optional prefetch. [UI.md](UI.md) tracks this work.
- Further measurements of first-use index construction, repeated queries, broad matches, external edits, and memory across multiple repositories. [Performance.md](Performance.md) summarizes the current measured query path and links historical experiments.

The old application's implementation is archived under `deprecated/`. Historical engine experiments are retained under `crates/bench/`; their earlier architecture recommendations do not describe the current runtime.
