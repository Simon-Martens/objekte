# Objekte documentation

Objekte reads JSON files as the source of truth. Collections can be discovered
from folders or declared in `.objekte/collections/`; named views live separately
in `.objekte/views/`. Search indexes are derived data. A repository handle
builds or restores them on first use, and saved snapshots live under
`.objekte/.cache`.

## Use the library and CLI

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
