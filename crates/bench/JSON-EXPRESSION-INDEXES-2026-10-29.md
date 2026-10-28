# JSON expression indexes and filter extensions, 29 October 2026

Measured on the AMD Ryzen 7 5700G (16 logical CPUs), rustc 1.96.0, release
build with `target-cpu=native`. All seven Musenalm collections contain 148,486
records. Every run uses a temporary application directory and a 64 MiB SQLite
page-cache budget. OS filesystem caches were not flushed. These are individual
acceptance timings, not statistical latency distributions. All durations below
are floating-point milliseconds and exclude CLI rendering/serialization.

The cache has no per-collection value tables or stored/generated field columns.
SQLite expression indexes contain the extracted keys. A configured scalar field
has ascending/descending schema-aware view indexes and a separate actual-JSON
query index; each is restricted to its collection. Query keys preserve JSON
types and exact decimal values, including schema-invalid values. Deterministic
Rust SQL functions use Sonic to extract keys; SQLite owns the B-trees and updates
them transactionally with raw record changes.

SQLite supports [indexes on expressions](https://sqlite.org/expridx.html), including
deterministic application functions. Query expressions must match the indexed
expressions. The compiler now does that for configured scalar comparisons. This
removes the former duplicate field storage and its explicit writes; it does not
remove index construction or maintenance costs.

## Views

```sh
cargo test --release -p objekte-core --test musenalm -- --ignored --nocapture
```

Default CPU-derived ingestion/background worker counts (8/4 on this machine).
Collections build in the order shown, sharing one database. Cold queries request
all rows and a count. Warm queries request the default 50-row page without a count,
after validation has completed. Direct mode validates inline; cold SQLite returns
before background validation completes, so totals are not a pure engine comparison.

| Collection | Records | Direct full ms | Cold SQLite full ms | Index build ms | Warm page ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| agents | 4,010 | 207.922 | 695.621 | 388.403 | 1.832 |
| contents | 133,948 | 5086.422 | 15038.813 | 10140.318 | 1.673 |
| entries | 4,928 | 3069.034 | 7952.048 | 5782.649 | 5.425 |
| items | 4,180 | 122.488 | 1635.913 | 1520.413 | 1.616 |
| pages | 8 | 4.125 | 757.862 | 754.649 | 1.100 |
| places | 210 | 12.153 | 1086.585 | 1075.790 | 1.356 |
| series | 1,202 | 59.491 | 1140.678 | 1075.570 | 1.689 |

Assertions compare complete ordered rows, numeric sorts, filters, counts, compound
sub-sorts, validation results, 173,427 reference occurrences, and metadata/hash-based
reopening. Warm requests read zero source files. Every default view plan uses its
ordering expression index. Unfiltered views select an exact matching ordering index
explicitly: SQLite's estimates over differently sized collections otherwise caused
a full JSON sort for `entries` (342.126 ms in an earlier run).

Cold construction remains substantial. It builds multiple indexes per configured
field and gathers statistics, including shared record/validation access paths.
Those shared operations also contribute to building later, small collections.
These measurements do not establish a cold-ingestion improvement over the former
materialized-column design. Further ingestion optimization remains separate work.

## General JSON filters

```sh
cargo test --release -p objekte-core --test musenalm_json_filter -- --ignored --nocapture
```

Four ingestion workers, manual validation. Direct execution still validates inline.
The first global query builds the raw cache and configured indexes; subsequent
queries reuse that cache. Global queries return every match; the scoped query returns
50 records and a count. Every result's count, ordered identities, paths, and JSON
values are compared with direct execution. Warm repetitions enumerate/read zero
source files. Numeric query plans assert an actual expression-index seek.

| Scope | Filter | Matches / returned | Direct ms | SQL total ms | SQL query phase ms | Warm SQL ms |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| all | `year >= 1800 && year < 1805` | 14,196 / 14,196 | 5955.507 | 20790.397 | 765.953 | 807.857 |
| all | `preferred_title ~ 'liebe'` | 2,797 / 2,797 | 5746.183 | 2463.293 | 2462.042 | 2351.563 |
| all | `contributors.agents ?= '1500' \|\| exists(contents[0])` | 2,394 / 2,394 | 5658.564 | 3534.776 | 3533.333 | 3492.789 |
| contents | `year >= 1800 && year < 1805` | 13,769 / 50 | 3647.879 | 166.877 | 165.558 | 167.171 |

Only the numeric comparisons above use JSON expression indexes. Wildcard, nested
multi-value, field-to-field, and modifier predicates currently use general JSON
extraction. Scalar-cardinality checks seek an index marker for configured fields;
unindexed scalar paths require a scan. Count/ID materialization and returning full
JSON also cost time. An index does not make every predicate or every page constant-time.

## Regression verification

- 151 library/CLI/unit/integration/doc tests passed; both opt-in real-data tests passed.
- Direct/SQL parity and independent expected results cover field operands, `:length`,
  `:each`, `:lower`, PocketBase LIKE parameter wrapping, escaping, empty selections,
  strict types, exact large numbers, duplicate keys, and malformed JSON.
- Index tests inspect query plans, absence of extra columns/tables, updates,
  deletions, reopening, configuration changes, format-4 rebuilding, and integrity.
- SQL-injection-shaped configured keys and filter values remain data.
- `cargo clippy -p objekte-core -p objekte-cli --all-targets -- -D warnings` passes.
  The broader workspace invocation still reports existing `type_complexity` lints
  in the unchanged `musenalm_rust_full` and `musenalm_rust_projected` benchmark bins.

Cache format 5 rebuilds older disposable caches once. Source JSON is unchanged.
External connections checking integrity must register the cache SQL functions through
`register_cache_sqlite_functions`; repository connections do so automatically.
Syntax and current compatibility limits are documented in [filters.md](../../filters.md).
