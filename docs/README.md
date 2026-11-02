*Objekte* allows to query, filter, select and sort json file datasets just like a real DB. It works with or without using json schema for type hints; including validation. It builds and maintain efficient B-Tree indices for structual access to selected paths and provides very fast full-text search and grep matching in field values. It supports specially declared string types (eg. markdown or XML). 

**Important:** we use a small in-memory index to speed up queries. Objekte is therefore not a replacement for *jq* or other one-shot-query engines. Also, we provide text search features just like full Regex, unescaping, markdown and XML Tag skips etc. For very basic access to the raw bytes use *ripgrep*.

The goal of objekte is to provide conveniant access to potentially millions of data files via full-text or structured queries. To archieve low millisecond results, the library builds different kinds of indeces: for bools, enums & numbers it builds B-Trees, for full text indexing it builds multiple levels of tracked bigram bloom filters. All indices can be marshalled, read and updated.

Ojekte is heavily SIMD-accelerated and requires AVX2 (Ineel & AMD) or AdvSIMD (ARM & Mac) support. It consists of three parts:

- The library: used to privide all the neccessary index, search and query functionality.
- A cmd/TUI: used mainly for testing, allows quick queries against a data repository
- A GUI: unfinished.

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
