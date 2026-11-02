# General JSON filters

`Repository::query_json` compiles a filter for stored JSON paths, including fields absent from a collection schema or collections with no schema. The CLI exposes it through `objekte REPOSITORY query [COLLECTION] FILTER` or `--collection NAME`. With no collection, it searches every discovered collection. Results are ordered by collection, then file modification time newest first, with ID breaking ties; `--limit`, `--offset`, and `--count` control pagination and a total count. See [Musenalm examples](queries.md).

Use `annotation:fts ~ "Goet"` to compare rendered text when `annotation` has a schema `x-full-text` format. It can be combined with structural predicates, for example `year = 1822 && annotation:fts ~ "Goet"`. Without a declared text format, `:fts` evaluates the stored value just like the unmodified path. The separate FTS index contains all leaf values by default; schema `exclude_fts: true` omits a field and its descendants from that index. See [full-text.md](full-text.md) for formats and standalone word and regex search.

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
| `@id`, `@filename`, `@path` | Source ID, basename including `.json`, and repository-relative path |
| `@size` | Source byte length, with optional decimal or binary unit |
| `@modified`, `@created` | File timestamps compared with unquoted UTC date/time periods |
| `@status` | `valid`, `invalid`, `no_schema`, or `incorrect_json` |

Strings use single or double quotes and backslash escapes. Numbers use JSON syntax, including fractions and exponents. Other literals are `true`, `false`, and `null`. An unquoted name on the right is a field reference. Each JSON field operand accepts one modifier. Metadata operands begin with `@`, so a JSON key named `@size` remains accessible as `["@size"]`. Apart from `exists`, `missing`, and the `:ignorecase` pattern wrapper, other functions, relation traversal, and grouping are outside this version.

~~~text
@size > 1KB && @modified >= 2026-05
@path ~ "data/books/%" && @filename ~ "draft%"
@status = "incorrect_json" || title ~ "draft"
@created = null
~~~

`@size` measures bytes. Bare numbers and `B` mean bytes. `KB`, `MB`, `GB`, and `TB` use powers of 1,000; `KiB`, `MiB`, `GiB`, and `TiB` use powers of 1,024. Decimal amounts such as `1.5MB` are accepted when they resolve to a whole number of bytes; amounts requiring rounding are rejected. Units apply only to `@size`. `@id` excludes `.json`; `@filename` includes it. `@path` is relative to the repository root and uses `/` separators.

Metadata filters require a source record and are evaluated by repository queries. `JsonFilter::matches` on a standalone JSON value returns an error for filters containing metadata operands.

An unquoted time literal denotes its entire UTC period: `2026-05` is May, `2026-12-04` is one day, and `2026-12-04T14:30Z` is one minute. A time of day requires `Z` or an explicit offset such as `+01:00`; seconds and up to nine fractional digits are supported. `=` selects timestamps within the period, `!=` those outside it, `<` timestamps before its start, `<=` timestamps before its end, `>` timestamps at or after its end, and `>=` timestamps at or after its start. Date/time literals currently require `@modified` or `@created`; schema-declared time fields can use the same syntax in a later extension. Creation time is `null` when the filesystem cannot provide it. Unix ctime is not treated as creation time.

Paths can be `address.city`, `["a.b"]["quote\"key"]`, `authors[0].name`, or `authors[*].name`. A field step through an array visits its members, so `authors.name` also reaches names in an array of objects. Comparisons flatten reached arrays recursively. A scalar comparison that reaches multiple values is an error, including outside the returned page or in a false Boolean branch; use `?` or `:each` for that case. Empty arrays provide no comparison values. `tags ?!= "poet"` means at least one element differs from `poet`; it does not mean `poet` is absent.

Actual JSON types are distinct: `"1800" = 1800` is false, and `true` differs from `1`. JSON numbers such as `1800`, `1800.0`, and `1.8e3` compare equal by exact decimal value, including values outside i64/f64 range. Equality and ordered string comparisons are case sensitive and bytewise. Missing fields match no comparison; explicit null matches `= null`. Booleans and null have equality semantics but no ordering.

`~` wraps a pattern without an unescaped `%` for literal substring matching. With `%`, it is a LIKE-style pattern: `%` matches any sequence and `_` one character, with no implicit surrounding wildcards. Backslash escapes pattern characters. Plain `~` and `!~` are case sensitive. Wrap a string, parameter, or field pattern as `:ignorecase(...)` to use full Unicode case folding: `Ä` matches `ä`, and `ß` matches `ss`. This does not apply canonical Unicode normalization, so composed and decomposed accents remain distinct. In a folded wildcard pattern, `_` consumes one character of the folded text. A pattern from another field is surrounded by `%`.

## Execution

`--backend auto` (the default) and explicit `--backend bigram` use the in-memory bigram and typed indexes. `--backend direct` evaluates original JSON files without an index. `--backend grep` searches original JSON files with ripgrep's parallel searcher. Every path uses the same compiled evaluator for final matching.

The in-memory backend builds consecutive and skip-one bigram postings over decoded scalar values and presence bits for declared top-level fields on first use. The default bigram mode indexes printable ASCII byte pairs. Set `Options.bigram_encoding = BigramEncoding::Utf8Bytes`, pass `--bigram-encoding utf8_bytes`, or set `x-objekte.bigram_encoding` in a collection schema to index UTF-8 byte pairs for multilingual candidates. This pairs bytes, not Unicode code points. Only scalar fields marked `"index": true` in the schema get ordered typed postings by default; each has separate keys for explicit null and a missing field. Missing is not JSON null and matches no comparison. Values with a distinct Unicode-folded form contribute an additional projection to the text index. It intersects safe text, typed, and field-presence candidates, and unions typed candidates across `OR` branches when every branch can be narrowed safely. Equality, inequality, number and string ranges, booleans, null, `exists`, `missing`, and anchored case-sensitive string prefixes such as `title ~ "A%"` use an ordered posting when one safely covers the path. `title = "A%"` remains literal equality. Only returned page records need full source reads and validation for an exact indexed match set. Other candidates are opened and verified with Sonic. Changed files enter a conservative overlay until the index is rebuilt. It does not persist query data.

To opt in, add `"index": true` to a top-level `string`, `number`, `integer`, or `boolean` schema property (a nullable type also works). The default is false; `"index": false` explicitly disables that posting until a later schema edit or an advanced-inference save changes it. This top-level setting creates B-tree postings and ordered view keys. For query-only postings on nested paths and array elements, use the root `"x-index-paths": ["detail.rank", "items[*].score"]` list, or save the paths found by [advanced indexing](inference.md). Objects and arrays themselves do not receive scalar comparison postings. Index settings change speed and memory use, never which queries are valid or which records match. Unindexed fields remain searchable through bigram candidates and source verification. Independent named views use these indexes through their JSON queries; new collection schemas have no view permissions.

The library can build a runtime posting for a JSON path with `Repository::build_posting(collection, path)`, or for every collection with `build_posting_all(path)`. A call replaces an existing posting for that path. Paths use filter syntax, for example `detail.rank` or `contributors[0].type`. The builder records values, null, missing, and presence from valid JSON. For paths with multiple values, existential comparisons such as `items[*].score ?= 20` can use complete value postings; ordinary scalar comparisons still report cardinality errors. Runtime postings live in the repository handle, survive an explicit full index rebuild by being rebuilt, and become stale after a source refresh until built again. Unindexed undeclared paths still use source verification. Initial index construction does not infer field types for undeclared paths; call `infer_and_auto_index` explicitly to do that (see [on-demand inference](inference.md)).

With `--count`, covered scalar and presence predicates count the current in-memory index snapshot directly. They do not check every matching source file; only returned page records are read and validated. Call `refresh` or rebuild after external edits until a file watcher maintains the index in the UI. Other expressions scan every selected candidate to compute the total. Without `--count`, the in-memory backend stops after filling the requested page when it is safe to do so. Source-scanned queries use adaptive parallel batches, and possible scalar-cardinality errors require checking all source records. The first request still builds the collection's in-memory index.

The raw-file grep backend reuses checked source inventory and descriptors. It opens files in parallel, searches a necessary literal if one exists, parses candidates with Sonic, and verifies the complete filter. When no safe literal exists it parses every file. Scalar-cardinality errors in files excluded by the raw-byte prefilter may not be observed by this optional backend; the default bigram backend preserves global checking.

Parseable records remain searchable even when they fail their schema or the requested path is undeclared. Malformed JSON has no JSON field paths, but can match metadata predicates such as `@status = "incorrect_json"` or `@size > 1KB`, including within Boolean combinations. Standalone word and regex search can also match malformed source text; see [full-text.md](full-text.md). Every returned record includes source byte size and signed modification time in decimal Unix nanoseconds; creation time is used for filtering but is not returned. `@status` validates every parseable candidate with a schema before comparison, so broad status queries may cost substantially more than size or modification-time checks. The cache stores neither validation status nor full JSON payloads. Collected results and final stream summaries include `elapsed_ms`, the wall-clock duration of the core search call, along with dependency collections, JSON paths, metadata fields, a filter hash, a query plan, and metrics. The in-memory picker keeps per-record revisions only in its current process.

`Repository::query_json_stream` calls a synchronous callback with each validated page record and a one-based ordinal. It returns totals and query metadata only on success. The caller must hold emitted records provisionally until this return, because later candidates may reveal a source or scalar-cardinality error. The CLI exposes this as `--output events`; JSON and raw-record NDJSON retain their existing behavior.

Filters are limited to 16 KiB, 256 atoms/groups, 32 nested groups, and 32 path steps. Syntax, unbound parameters, nonscalar literal operands, and unknown collections fail before source enumeration. Runtime cardinality errors fail collected queries and invalidate provisional stream records.

## Verification

`crates/core/tests/json_filter.rs` checks expected outputs and parity across bigram, direct, and raw-file grep backends, including mixed types, exact decimals, nested arrays, Unicode, escaped keys, duplicate keys, parameters, scope, edits, and pagination. Streaming tests check page parity and late errors on all three backends. CLI tests exercise JSON, NDJSON, event output, and diagnostics. The opt-in Musenalm test compares complete results over the example repository.
