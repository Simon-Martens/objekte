# General JSON filters

Implemented by `objekte-core`, independently of collection schema fields and UI
filter controls. Configured fields have SQLite expression indexes on raw JSON.
The same expression works on different collection shapes and standalone JSON values.

```sh
# All currently registered collections
cargo run -p objekte-cli -- examples/musenalm query 'year >= 1800 && year < 1805' --count

# String matching and nested arrays
cargo run -p objekte-cli -- examples/musenalm query 'preferred_title ~ "liebe"'
cargo run -p objekte-cli -- examples/musenalm query 'contributors.agents ?= "1500"'

# Search one collection
cargo run -p objekte-cli -- examples/musenalm query contents 'year >= 1800'

# The --collection form is also supported; parameters below have different types
cargo run -p objekte-cli -- examples/musenalm query 'year = $year' --param year=1800 --collection contents
cargo run -p objekte-cli -- examples/musenalm query 'year = $year' --param 'year="1800"'
```

Use `query COLLECTION FILTER` to search one collection, or `query FILTER` to search
all registered collections. `query FILTER --collection NAME` is an equivalent
scoped form; supply the collection once, either positionally or through the flag.

Use `--limit N`, `--offset N`, and optional `--count`. Ordering is deterministic by
`(collection, filename ID)`; default limit is 50. Results include each complete
record, its collection, ID, path, content digest and validation status. JSON output
also includes generated SQL, bindings, SQLite's query plan, filter hash, dependency
metadata, and millisecond metrics. `--output ndjson` emits one matching record per
line. `--diagnostics` logs foreground/background metrics to stderr.

## Syntax and semantics

| Construct | Meaning |
| --- | --- |
| `field = value`, `==` | Equality using the actual JSON type; both spellings are accepted |
| `!=` | Scalar inequality; different scalar types are unequal |
| `<`, `<=`, `>`, `>=` | Number-to-number or string-to-string ordering only |
| `~`, `!~` | PocketBase-style LIKE patterns or their negation, on strings only |
| `?=`, `?!=`, `?<`, `?<=`, `?>`, `?>=`, `?~`, `?!~` | Any reached value satisfies the scalar comparison |
| `&&`, `||`, `(...)` | AND, OR, grouping; AND binds more tightly than OR |
| `exists(path)` | At least one value exists at the path, including null/objects/empty arrays |
| `$name` | Typed scalar parameter provided separately from the expression |
| `start < end` | Field-to-field comparison with strict JSON types on both sides |
| `tags:length` | Number of immediate items in an array; other types yield no value |
| `tags:each` | Require every reached value to match; `?` changes this to any |
| `title:lower` | ASCII lowercase on string values; other types yield no value |

Strings may use single or double quotes, with backslash escapes. Numbers use JSON
syntax, including fractions and exponents. Other literals are `true`, `false`, and
`null`. Both operands can be fields, literals, or parameters. An unquoted name on
the right is a field reference, not a string literal. Each field operand accepts one
modifier. Functions other than `exists`, comments, relation traversal, and
metadata/application identifiers are not supported in this version.

`"1800" = 1800` is false and `"1800" != 1800` is true. Likewise `true` differs from
`1`. JSON has one number type: `1800`, `1800.0`, and `1.8e3` compare equal. Numeric
comparison uses exact decimal lexemes, including numbers beyond i64/f64, without
rounding or allocating exponent-sized zero strings. Booleans and null have equality
semantics but no ordering. Missing fields match no comparison, including `!=` and
`!~`; explicit null matches `= null` and differs from other scalar values.

Unquoted key components use letters/underscore followed by letters/digits/underscore.
Paths can use `address.city`, `["a.b"]["quote\"key"]`, `authors[0].name`, and
`authors[*].name`. A field step traversing an array visits its members, so
`authors.name` also reaches names in an array of objects. Comparisons flatten reached
arrays recursively. Empty arrays yield no comparison values; scalar paths yielding
more than one value are errors and require `?` or `:each`. A single reached value may use either
form. Objects themselves are not scalar comparison operands.

`tags ?!= "poet"` matches `["poet", "editor"]`: some element differs. It does not
mean that no element equals `"poet"`. Different predicates may match different
array elements; same-element compound quantifiers are not implemented.

`:each` requires a nonempty selection and every comparison to succeed. When both
operands yield multiple values, this checks every pair; the `?` form checks whether
any pair succeeds. Unmodified scalar operands still reject multiple values, even
inside an otherwise false branch. An array's `:length` is computed before flattening;
`[]:length` is zero (express it through a field, since array literals are unsupported).

`~`/`!~` follow PocketBase's parameter-wrapping behavior and SQLite LIKE's ASCII case
handling. For a literal or parameter containing an unescaped `%`, `%` matches any
sequence, `_` matches one character, and backslash escapes a pattern character. No
implicit surrounding wildcards are added: `"Liebe%"` is a prefix pattern. Without
an unescaped `%`, the pattern becomes a literal substring search, including a literal
underscore; `"a_b"` searches for that exact substring. Escaped `%`, `_`, and backslash
remain literal. Filter string escapes are decoded first, so write `"100\\%"` to
search for a literal percent. A pattern taken from another field is always surrounded
by `%`, matching PocketBase's field-to-field behavior.

Pattern matching and `:lower` fold ASCII letters only: `ALPHA` matches `alpha`, but
`Ä` differs from `ä`, and `Straße` differs from `STRASSE`. Equality and ordered string
comparisons remain case-sensitive UTF-8 binary comparisons. These rules are identical
in the direct evaluator and SQL, and LIKE behavior is checked against SQLite itself.

Parseable records remain queryable even if they fail their schema, including fields
undeclared by the schema. Malformed JSON is retained in the cache with its existing
diagnostics but does not match a JSON predicate. No metadata filter is added here.

## Library and execution

`JsonFilter::compile(text, &parameters)` builds a reusable, bound expression tree.
`filter.matches(&serde_json::Value)` checks a standalone parsed value.
`Repository::query_json(&JsonQuery)` reconciles the requested scope and executes the
filter. `JsonQuery.collections = None` means all registered collections; `Some([])`
means none. `load_repository` remains lazy. An initial global query necessarily
opens every registered collection; an explicit scope only opens those collections.
Retained cache rows from removed registrations are excluded.

The existing freshness policy applies. First use checks source manifests; explicit
refresh or on-request policies observe subsequent edits. Query results are not cached.
Each result records all scoped collection names and their persistent revisions plus
the referenced JSON paths. Membership depends on nonmatching records and future
additions too. Direct execution reports names/paths without persisted revisions.
The filter hash includes the bound expression and comparison semantics, independently
of pagination or collection scope; it is not a complete result-cache key.

The compiler uses the separation seen in PocketBase's
[filter compiler](https://github.com/pocketbase/pocketbase/blob/master/tools/search/filter.go)
and [dbx expressions](https://github.com/pocketbase/dbx/blob/master/expression.go):
expression tree first, then SQL fragments and separate bound parameters. There is
no Go dependency or embedded user SQL. Comparison values are bound. For expression-index
matching, configured paths are
serialized and safely escaped as SQL literals; other paths remain bindings. Only
known operators and compiler-generated SQL are emitted.

SQLite executes grouped predicates, field-to-field comparisons, correlated `EXISTS`
subqueries, optional count, and pagination over `records`/`record_validation`.
Deterministic Rust SQL functions extract JSON values with Sonic, retaining duplicate
last-member-wins semantics and exact numeric lexemes. There are no persisted field
columns or per-collection value tables. SQLite expression indexes operate directly
on `records.json`, and SQLite maintains them transactionally when records change.

Index fields still come only from the UI/schema configuration. A view or query that
needs them builds the configured set lazily, with statistics collected using ANALYZE.
Unfiltered views with an exact ordering index select it explicitly. Compound views
and filters use a covering expression index containing ID and all configured view
keys; they can sort/filter keys without decoding full records. Count queries use a
matching field index when all predicates reference one field, otherwise the covering
index. These choices are deterministic, not based on query history.
Each configured scalar field has expression indexes for schema-aware view ordering
(ascending/descending, null/missing/invalid states). Its additional actual-JSON scalar
key index is built only when a general query first needs it for a comparison or
scalar-cardinality check. It is then maintained and reused across restarts. The latter indexes strings, booleans and exact decimal
numbers separately, even when a value violates its schema. A custom deterministic
collation compares numeric lexemes without float conversion. These indexes have
different semantics; they all store derived keys only inside SQLite's indexes.
During a bulk build, ingestion seeds a temporary cache of extracted keys, keyed by
content digest. Its retained-key budget is 128 MiB, separate from SQLite's page
cache, with ordinary extraction as a correctness-preserving fallback when full.
It is released after the build, including on failure. Incremental writes reuse one
already-parsed document. This is not a persistent projection or query-result cache.

Plain indexed field versus scalar literal/parameter comparisons (`=`, `!=`, ordered
comparisons, and reversed operands) compile to matching index expressions. Boolean
combinations can mix indexed predicates with general JSON checks. Field-to-field
comparisons, modifiers, wildcard searches, and multi-value `?` comparisons currently
use the general evaluator; SQLite's planner decides how to combine access paths.
No additional index or result cache is created from query history.

Scalar cardinality is checked over the whole scope before pagination: indexed fields
seek a multi-value marker in their index; other scalar paths require a JSON scan.
This preserves errors outside the returned page and across boolean branches. JSON
query results are assembled from matching IDs before payloads are fetched. A small
page can therefore still require scanning unindexed paths or collecting all matches.
Metadata fields, relation traversal, grouping, and query-result caching remain later
work. Cache format 5 rebuilds older disposable caches once, removing the old value
tables. External SQLite connections that run integrity checks or modify the cache
must first call `register_cache_sqlite_functions`; library connections do this
already. Source files remain authoritative.

Filters are limited to 16 KiB, 256 atoms/groups, 32 nested groups, and 32 path steps.
Syntax, unbound parameters, nonscalar literal operands, and unknown collection names fail
before source enumeration or cache creation. Runtime cardinality errors fail the
query instead of silently returning a partial page.

## Verification

`crates/core/tests/json_filter.rs` checks direct/SQL equivalence, independent expected
results, generated mixed-type comparisons, exact decimals, modifiers, array semantics, missing/
null, Unicode/ASCII boundaries, quoted keys, duplicate keys, SQL injection-shaped values, scope,
restart updates, dependencies, and pagination. CLI tests exercise typed parameters,
all-collection/scoped queries, JSON/NDJSON, and millisecond output.

```sh
cargo test -p objekte-core -p objekte-cli
cargo test --release -p objekte-core --test musenalm_json_filter -- --ignored --nocapture
```

`expression_indexes.rs` verifies actual index seeks, absence of materialized field
columns/tables, type and precision parity, updates/deletions, reconfiguration, and
restarts. [The initial scan measurements](crates/bench/JSON-FILTERS-2026-10-28.md)
precede expression-index support; see [the repeated regression audit](crates/bench/INDEX-REGRESSION-AUDIT-2026-10-29.md)
for the current implementation, including cold-build costs.
