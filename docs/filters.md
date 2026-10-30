# General JSON filters

`Repository::query_json` compiles a filter for fields declared by the collection schema. The CLI exposes it through `objekte REPOSITORY query [COLLECTION] FILTER`. With no collection, it searches registered collections whose schemas support the expression. Results are ordered by collection and byte-sorted filename ID; `--limit`, `--offset`, and `--count` control pagination and a total count. See [Musenalm examples](queries.md).

## Syntax

| Construct | Meaning |
| --- | --- |
| `field = value`, `==`, `!=` | Equality or inequality using actual JSON scalar types |
| `<`, `<=`, `>`, `>=` | Number-to-number or string-to-string ordering |
| `~`, `!~` | Case-sensitive LIKE-style string matching and its negation |
| `field ~ :ignorecase("text")` | Unicode case-insensitive matching; also works with `!~`, `?~`, and `?!~` |
| `?=`, `?!=`, `?<`, `?<=`, `?>`, `?>=`, `?~`, `?!~` | Any reached value satisfies the comparison |
| `&&`, `||`, `(...)` | Boolean combination; AND binds more tightly |
| `exists(path)` | At least one value exists, including null or an empty array |
| `$name` | Typed scalar parameter supplied with `--param name=JSON` |
| `start < end` | Field-to-field comparison |
| `tags:length` | Immediate array length |
| `tags:each` | Every reached value must match; requires a nonempty selection |
| `title:lower` | ASCII lowercase applied to strings |

Strings use single or double quotes and backslash escapes. Numbers use JSON syntax, including fractions and exponents. Other literals are `true`, `false`, and `null`. An unquoted name on the right is a field reference. Each field operand accepts one modifier. Apart from `exists` and the `:ignorecase` pattern wrapper, other functions, relation traversal, grouping, and metadata operands are outside this version.

Paths can be `address.city`, `["a.b"]["quote\"key"]`, `authors[0].name`, or `authors[*].name`. A field step through an array visits its members, so `authors.name` also reaches names in an array of objects. Comparisons flatten reached arrays recursively. A scalar comparison that reaches multiple values is an error, including outside the returned page or in a false Boolean branch; use `?` or `:each` for that case. Empty arrays provide no comparison values. `tags ?!= "poet"` means at least one element differs from `poet`; it does not mean `poet` is absent.

Actual JSON types are distinct: `"1800" = 1800` is false, and `true` differs from `1`. JSON numbers such as `1800`, `1800.0`, and `1.8e3` compare equal by exact decimal value, including values outside i64/f64 range. Equality and ordered string comparisons are case sensitive and bytewise. Missing fields match no comparison; explicit null matches `= null`. Booleans and null have equality semantics but no ordering.

`~` wraps a pattern without an unescaped `%` for literal substring matching. With `%`, it is a LIKE-style pattern: `%` matches any sequence and `_` one character, with no implicit surrounding wildcards. Backslash escapes pattern characters. Plain `~` and `!~` are case sensitive. Wrap a string, parameter, or field pattern as `:ignorecase(...)` to use full Unicode case folding: `Ä` matches `ä`, and `ß` matches `ss`. This does not apply canonical Unicode normalization, so composed and decomposed accents remain distinct. In a folded wildcard pattern, `_` consumes one character of the folded text. A pattern from another field is surrounded by `%`.

## Execution

`--backend auto` (the default) and explicit `--backend bigram` use the in-memory bigram and typed indexes. `--backend direct` evaluates original JSON files without an index. `--backend grep` searches original JSON files with ripgrep's parallel searcher. Every path uses the same compiled evaluator for final matching.

The in-memory backend builds consecutive and skip-one bigram postings over decoded scalar values, presence bits for declared top-level fields, and ordered postings for declared number, boolean, and enum fields on first use. Values with a distinct Unicode-folded form contribute an additional projection to the text index. It intersects safe text, typed, and field-presence candidates, and unions typed candidates across `OR` branches when every branch can be narrowed safely. Simple top-level scalar equality/range predicates and `exists` can be resolved by the indexes; only their returned page records need full source reads and validation. Other candidates are opened and verified with Sonic. Changed files enter a conservative overlay until the index is rebuilt. It does not persist query data.

With `--count`, covered scalar and presence predicates count index matches and check their source metadata; other expressions scan every selected candidate to compute the total. Without `--count`, the in-memory backend stops after filling the requested page when it is safe to do so. Source-scanned queries use adaptive parallel batches, and possible scalar-cardinality errors require checking all source records. The first request still builds the collection's in-memory index.

The raw-file grep backend reuses checked source inventory and descriptors. It opens files in parallel, searches a necessary literal if one exists, parses candidates with Sonic, and verifies the complete filter. When no safe literal exists it parses every file. Scalar-cardinality errors in files excluded by the raw-byte prefilter may not be observed by this optional backend; the default bigram backend preserves global checking.

Parseable records remain searchable even when they fail their schema, provided the requested path is declared. Malformed JSON never matches. Returned records are validated on demand. The cache stores neither validation status nor full JSON payloads. Each result includes dependency collection names and paths, a filter hash, a query plan, and metrics. The in-memory picker keeps per-record revisions only in its current process.

`Repository::query_json_stream` calls a synchronous callback with each validated page record and a one-based ordinal. It returns totals and query metadata only on success. The caller must hold emitted records provisionally until this return, because later candidates may reveal a source or scalar-cardinality error. The CLI exposes this as `--output events`; JSON and raw-record NDJSON retain their existing behavior.

Filters are limited to 16 KiB, 256 atoms/groups, 32 nested groups, and 32 path steps. Syntax, unbound parameters, nonscalar literal operands, unknown collections, and incompatible explicitly scoped filters fail before source enumeration. Runtime cardinality errors fail collected queries and invalidate provisional stream records.

## Verification

`crates/core/tests/json_filter.rs` checks expected outputs and parity across bigram, direct, and raw-file grep backends, including mixed types, exact decimals, nested arrays, Unicode, escaped keys, duplicate keys, parameters, scope, edits, and pagination. Streaming tests check page parity and late errors on all three backends. CLI tests exercise JSON, NDJSON, event output, and diagnostics. The opt-in Musenalm test compares complete results over the example repository.
