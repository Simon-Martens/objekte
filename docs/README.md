# Objekte implementation

Objekte reads JSON collections from a repository. The source files are authoritative. An in-memory bigram index serves default JSON filters; optional typed postings accelerate selected paths. Indexes are built lazily per collection and reused while a `Repository` handle remains open.

Repositories can discover collections without a manifest or schema. See [Collection discovery and opening APIs](collections.md) for folder ownership, nested IDs, optional schemas, and code examples.

## Start here

- [Query examples](queries.md) show the CLI syntax on Musenalm.
- [Filter semantics](filters.md) describe exact matching, arrays, and backend selection.
- [Validation](validation.md) explains on-demand JSON Schema checks.
- [Index implementation](implementation.md) describes refresh, rebuild, and revision behavior.
- [Code guidelines](code-guidelines.md) document source layout, comments, tests, and diagnostics.
- [Performance results](Performance.md) summarize warm queries and historical experiments.
- [MVP and roadmap](MVP.md) separate library features from later product work.

```sh
cargo test --workspace --all-targets
cargo run --release -p objekte-cli -- examples/musenalm query contents 'preferred_title ~ :ignorecase("almanach") && year >= 1800' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm view contents --sort year --limit 10 --output json
```

`--cache memory` is the default. `--cache disabled` makes views scan source files directly. JSON queries also provide explicit `--backend direct` and `--backend grep` paths. Every JSON query returns records in collection and byte-sorted ID order, and verifies matches against source JSON.

The picker can be warmed or rebuilt per collection or across all collections using the `Repository` API. These operations write no application cache files. A new CLI invocation starts a new process and builds its own picker.

Where a schema exists, validation runs for JSON query matches, in-memory view pages, and explicit validation. A parseable `get_record` response has `pending` schema status; records without a schema have `no_schema` status. Reference occurrences are extracted from declared relations on request; complete backlink lookup scans referring collections.
