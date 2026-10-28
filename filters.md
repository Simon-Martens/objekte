# General JSON filters

Implemented by `objekte-core`, independently of collection schema fields, UI filter
controls, and existing typed view indexes. The same expression works on different
collection shapes and on standalone JSON values.

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
| `~`, `!~` | Literal case-insensitive substring match or its negation, on strings only |
| `?=`, `?!=`, `?<`, `?<=`, `?>`, `?>=`, `?~`, `?!~` | Any reached value satisfies the scalar comparison |
| `&&`, `||`, `(...)` | AND, OR, grouping; AND binds more tightly than OR |
| `exists(path)` | At least one value exists at the path, including null/objects/empty arrays |
| `$name` | Typed scalar parameter provided separately from the expression |

Strings may use single or double quotes, with backslash escapes. Numbers use JSON
syntax, including fractions and exponents. Other literals are `true`, `false`, and
`null`. Bare string values, field-to-field comparisons, functions other than
`exists`, and metadata/application identifiers are not supported in this version.

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
more than one value are errors and require `?`. A single reached value may use either
form. Objects themselves are not scalar comparison operands.

`tags ?!= "poet"` matches `["poet", "editor"]`: some element differs. It does not
mean that no element equals `"poet"`. Different predicates may match different
array elements; same-element compound quantifiers are not implemented.

Substring matching uses default, full, locale-independent Unicode 16.0 case folding
through pinned `caseless 0.2.2`; `Straße` matches `STRASSE`. There is no accent removal
or Unicode normalization. `%`, `_`, and regex punctuation are literal characters.
Equality and ordered string comparisons are case-sensitive UTF-8 binary comparisons.
All rules are shared by direct Rust and SQLite execution.

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
The filter hash includes the bound expression and Unicode semantics, independently
of pagination or collection scope; it is not a complete result-cache key.

The compiler uses the separation seen in PocketBase's
[filter compiler](https://github.com/pocketbase/pocketbase/blob/master/tools/search/filter.go)
and [dbx expressions](https://github.com/pocketbase/dbx/blob/master/expression.go):
expression tree first, then SQL fragments and separate bound parameters. There is
no Go dependency or embedded user SQL. Every path and value is bound; only known
operators and compiler-generated SQL are emitted.

SQLite executes the grouped predicates, correlated `EXISTS` subqueries, optional
count, and pagination over `records`/`record_validation`. Registered Rust SQL
functions supply compatible JSON path extraction, exact decimal comparison, and
Unicode substring matching. The path accessor decodes raw cached bytes with Sonic
and shares at most one decoded document between predicates during the query.
`json_each` iterates typed selected values; number lexemes remain text until the exact
comparator consumes them. This preserves last-member-wins duplicate keys and avoids
the numeric conversion of SQLite's native JSON extraction. Original cached bytes
remain unchanged; there is no second persistent JSON representation.

Scalar-path cardinality is checked for every scoped parseable record, regardless of
boolean short-circuiting or the requested page. SQLite materializes matching IDs
before paging; it does not materialize all record payloads. This first implementation
therefore performs a full scoped scan even when the requested page is small. Existing
UI-derived indexes are not selected by this compiler. General index optimization,
relation traversal, sorting overrides, grouping, metadata fields, and query-result
caching remain later work.

Filters are limited to 16 KiB, 256 atoms/groups, 32 nested groups, and 32 path steps.
Syntax, unbound parameters, nonscalar operands, and unknown collection names fail
before source enumeration or cache creation. Runtime cardinality errors fail the
query instead of silently returning a partial page.

## Verification

`crates/core/tests/json_filter.rs` checks direct/SQL equivalence, independent expected
results, generated mixed-type comparisons, exact decimals, array semantics, missing/
null, Unicode, quoted keys, duplicate keys, SQL injection-shaped values, scope,
restart updates, dependencies, and pagination. CLI tests exercise typed parameters,
all-collection/scoped queries, JSON/NDJSON, and millisecond output.

```sh
cargo test -p objekte-core -p objekte-cli
cargo test --release -p objekte-core --test musenalm_json_filter -- --ignored --nocapture
```

The [Musenalm measurements](crates/bench/JSON-FILTERS-2026-10-28.md) compare three
global workloads against source execution. These unindexed scans should not be
confused with indexed UI-view latency.
