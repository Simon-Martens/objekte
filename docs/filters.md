# General JSON filters

`Repository::query_json` compiles a filter for stored JSON paths, including fields absent from a collection schema or collections with no schema. The CLI exposes it through `objekte REPOSITORY query [COLLECTION] FILTER` or `--collection NAME`. With no collection, it searches every discovered collection. Results are ordered by collection and byte-sorted relative record ID; `--limit`, `--offset`, and `--count` control pagination and a total count. See [Musenalm examples](queries.md).

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
| `missing(path)` | No value exists at the path; explicit null is present |
| `$name` | Typed scalar parameter supplied with `--param name=JSON` |
| `start < end` | Field-to-field comparison |
| `tags:length` | Immediate array length |
| `tags:each` | Every reached value must match; requires a nonempty selection |
| `title:lower` | ASCII lowercase applied to strings |

Strings use single or double quotes and backslash escapes. Numbers use JSON syntax, including fractions and exponents. Other literals are `true`, `false`, and `null`. An unquoted name on the right is a field reference. Each field operand accepts one modifier. Apart from `exists`, `missing`, and the `:ignorecase` pattern wrapper, other functions, relation traversal, grouping, and metadata operands are outside this version.

Paths can be `address.city`, `["a.b"]["quote\"key"]`, `authors[0].name`, or `authors[*].name`. A field step through an array visits its members, so `authors.name` also reaches names in an array of objects. Comparisons flatten reached arrays recursively. A scalar comparison that reaches multiple values is an error, including outside the returned page or in a false Boolean branch; use `?` or `:each` for that case. Empty arrays provide no comparison values. `tags ?!= "poet"` means at least one element differs from `poet`; it does not mean `poet` is absent.

Actual JSON types are distinct: `"1800" = 1800` is false, and `true` differs from `1`. JSON numbers such as `1800`, `1800.0`, and `1.8e3` compare equal by exact decimal value, including values outside i64/f64 range. Equality and ordered string comparisons are case sensitive and bytewise. Missing fields match no comparison; explicit null matches `= null`. Booleans and null have equality semantics but no ordering.

`~` wraps a pattern without an unescaped `%` for literal substring matching. With `%`, it is a LIKE-style pattern: `%` matches any sequence and `_` one character, with no implicit surrounding wildcards. Backslash escapes pattern characters. Plain `~` and `!~` are case sensitive. Wrap a string, parameter, or field pattern as `:ignorecase(...)` to use full Unicode case folding: `Ä` matches `ä`, and `ß` matches `ss`. This does not apply canonical Unicode normalization, so composed and decomposed accents remain distinct. In a folded wildcard pattern, `_` consumes one character of the folded text. A pattern from another field is surrounded by `%`.

## Execution

`--backend auto` (the default) and explicit `--backend bigram` use the in-memory bigram and typed indexes. `--backend direct` evaluates original JSON files without an index. `--backend grep` searches original JSON files with ripgrep's parallel searcher. Every path uses the same compiled evaluator for final matching.

The in-memory backend builds consecutive and skip-one bigram postings over decoded scalar values and presence bits for declared top-level fields on first use. Only scalar fields marked `"index": true` in the schema get ordered typed postings by default; each has separate keys for explicit null and a missing field. Missing is not JSON null and matches no comparison. Values with a distinct Unicode-folded form contribute an additional projection to the text index. It intersects safe text, typed, and field-presence candidates, and unions typed candidates across `OR` branches when every branch can be narrowed safely. Equality, inequality, number and string ranges, booleans, null, `exists`, `missing`, and anchored case-sensitive string prefixes such as `title ~ "A%"` use an ordered posting when one safely covers the path. `title = "A%"` remains literal equality. Only returned page records need full source reads and validation for an exact indexed match set. Other candidates are opened and verified with Sonic. Changed files enter a conservative overlay until the index is rebuilt. It does not persist query data.

To opt in, add `"index": true` to a top-level `string`, `number`, `integer`, or `boolean` schema property (a nullable type also works). The default is false; `"index": false` is explicit opt-out. Nested paths, arrays, and objects cannot use this ordered index. For example, `"year": {"type": "number", "index": true}` and `"title": {"type": "string", "index": true}` get B-tree postings and ordered view keys. This setting changes speed and memory use, never which queries are valid or which records match. Unindexed fields remain searchable through bigram candidates and source verification. UI `list.filters` and `list.sortable` are separate permissions for views.

The library can build a runtime posting for a JSON path with `Repository::build_posting(collection, path)`, or for every collection with `build_posting_all(path)`. A call replaces an existing posting for that path. Paths use filter syntax, for example `detail.rank` or `contributors[0].type`. The builder records values, null, missing, and presence from valid JSON. If any record yields multiple scalar values, the posting still accelerates `exists` and `missing`, while scalar comparisons continue to scan to preserve cardinality errors. Runtime postings live in the repository handle, survive an explicit full index rebuild by being rebuilt, and become stale after a source refresh until built again. Unindexed undeclared paths still use source verification. Index construction does not infer field types or record shape bitsets for undeclared paths.

With `--count`, covered scalar and presence predicates count the current in-memory index snapshot directly. They do not check every matching source file; only returned page records are read and validated. Call `refresh` or rebuild after external edits until a file watcher maintains the index in the UI. Other expressions scan every selected candidate to compute the total. Without `--count`, the in-memory backend stops after filling the requested page when it is safe to do so. Source-scanned queries use adaptive parallel batches, and possible scalar-cardinality errors require checking all source records. The first request still builds the collection's in-memory index.

The raw-file grep backend reuses checked source inventory and descriptors. It opens files in parallel, searches a necessary literal if one exists, parses candidates with Sonic, and verifies the complete filter. When no safe literal exists it parses every file. Scalar-cardinality errors in files excluded by the raw-byte prefilter may not be observed by this optional backend; the default bigram backend preserves global checking.

Parseable records remain searchable even when they fail their schema or the requested path is undeclared. Malformed JSON never matches. Returned records are validated on demand when a schema exists; otherwise they have `no_schema` status. The cache stores neither validation status nor full JSON payloads. Each result includes dependency collection names and paths, a filter hash, a query plan, and metrics. The in-memory picker keeps per-record revisions only in its current process.

`Repository::query_json_stream` calls a synchronous callback with each validated page record and a one-based ordinal. It returns totals and query metadata only on success. The caller must hold emitted records provisionally until this return, because later candidates may reveal a source or scalar-cardinality error. The CLI exposes this as `--output events`; JSON and raw-record NDJSON retain their existing behavior.

Filters are limited to 16 KiB, 256 atoms/groups, 32 nested groups, and 32 path steps. Syntax, unbound parameters, nonscalar literal operands, and unknown collections fail before source enumeration. Runtime cardinality errors fail collected queries and invalidate provisional stream records.

## Verification

`crates/core/tests/json_filter.rs` checks expected outputs and parity across bigram, direct, and raw-file grep backends, including mixed types, exact decimals, nested arrays, Unicode, escaped keys, duplicate keys, parameters, scope, edits, and pagination. Streaming tests check page parity and late errors on all three backends. CLI tests exercise JSON, NDJSON, event output, and diagnostics. The opt-in Musenalm test compares complete results over the example repository.
