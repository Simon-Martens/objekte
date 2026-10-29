# Objekte implementation

Objekte reads JSON collections from a repository. The JSON files are authoritative. The default in-memory picker serves JSON filter queries and views. The optional segment cache has values-only text shards, per-record metadata, and ordered scalar lists.

## Start here

- [Query examples](queries.md) show the CLI syntax on Musenalm.
- [Filter semantics](filters.md) describe exact matching, arrays, and backend selection.
- [Validation](validation.md) explains on-demand JSON Schema checks.
- [Cache implementation](implementation.md) describes snapshots, updates, revisions, and compaction.
- [Performance results](Performance.md) retain historical engine comparisons; current behavior is described here.

```sh
cargo test --workspace --all-targets
cargo run --release -p objekte-cli -- examples/musenalm query contents 'preferred_title ~ "almanach" && year >= 1800' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm view contents --sort year --limit 10 --output json
```

JSON filter queries use an in-memory bigram and typed index by default. Views use the same picker, with ordered in-memory keys for numeric, boolean, and enum fields. `--cache segments` enables the persistent segment cache for views; `--backend segments` selects it for JSON filters and requires `--cache segments`. `--cache disabled` makes views read source files directly. Each JSON query returns records in collection and byte-sorted ID order.

The in-memory picker is built lazily per collection and reused for repeated queries on the same repository handle. Refresh updates changed records, ordered keys, and deletion tombstones. It writes no application cache files. The optional segment cache persists across processes; its shards, compaction, revisions, and atomic snapshot publication are described in [implementation.md](implementation.md).

All top-level fields declared as numbers, integers, booleans, or enums receive ordered lists, regardless of view configuration. String ordering is planned. View sorts and filters keep their existing permissions from collection UI settings; the cache's index coverage is broader than those permissions.

Validation happens when records are returned or explicitly requested. JSON/schema status is not persisted in the cache. Reference occurrences are extracted from source records on request. Backlink lookup scans referring collections so edits, moves, and deletions become visible without a persisted edge table.
