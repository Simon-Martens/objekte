*obj* allows to query, filter, select and sort json file datasets just like a DB. It works with or without using json schema for type hints; including querying validation state. It and maintains efficient B-Tree indices for structual access to selected paths and provides very fast full-text search and grep matching in field values, supporting specially declared string types (eg. markdown or XML). With all that done, obj gets really close to raw JSON parsing speeds, with little over 1 GBps.

**Important:** obj uses several small in-memory indecies to speed up queries. It is not a replacement for **jq** or other one-shot query engines. Also, it provides text search features like full regex support, JSON unescaping, markdown and XML and HTML Tag skips, white-space normalization etc. If very basic access to the raw bytes of the files is enough for the task at hand use **ripgrep**. If you're running repeated queries or need advanced full-text options *obj* is probably the fastest way to go. 

The goal of obj is to provide conveniant access to potentially millions of data files via full-text or structured queries. To archieve low millisecond results, the library builds different kinds of indeces: for bools, enums & numbers it builds B-Trees, for full text indexing it builds multiple layers of bloom-like tracked bigram filters. All indices can be marshalled, read and updated on request. The API provides a way to patch the index on fs updates.

obj is heavily SIMD-accelerated and requires AVX2/NEON instructions support by the CPU. It consists of three parts:

- **Core library:** used to privide all the neccessary index, search and query functionality. Can be used as standalone library.
- **cli/TUI:** used mainly for testing, allows quick queries against a data repository
- **GUI:** unfinished

## Some documentation 

- [Collections and schemas](collections.md): discovery, nested IDs, and index declarations.
- [Query examples](queries.md): runnable Musenalm commands, sorting, and output.
- [Filter language](filters.md): JSON paths, metadata fields, operators, and backends.
- [Full-text search](full-text.md): word and regex modes, rendering, and exclusions.
- [Independent views](views.md): repository-level definitions, arguments, and queries.
- [Terminal UI](cli-tui.md): search controls, result navigation, and export.
- [Validation](validation.md): JSON and schema status on source records.
- [Field inference](inference.md): detected types, runtime postings, and schema updates.

## Understand and extend the implementation

- [Index and cache lifecycle](implementation.md): construction, snapshots, refresh, and source verification.
- [Core and TUI code map](code-map.md): entry points and module responsibilities.
- [Code guidelines](code-guidelines.md): layout, comments, tests, and diagnostics.
- [Performance](Performance.md): current benchmark entry points and measurement limits.
- [Current capabilities and roadmap](MVP.md), with a separate [desktop UI plan](UI.md).

```sh
cargo test --workspace --all-targets
cargo run --release -p objekte-cli -- examples/musenalm query contents 'preferred_title ~ :ignorecase("almanach") && year >= 1800' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm view contents_list --arg page=2
cargo run --release -p objekte-cli -- examples/musenalm
```

The default JSON query backend uses the in-memory picker and verifies source
records. Queries return collections in name order, with records newest first
by file modification time and ID breaking ties. `--backend direct` scans source
files; `--backend grep` uses ripgrep as a conservative source prefilter. The
CLI's `cache save` and `cache reindex` commands manage index snapshots.
