# Musenalm query examples

Run these commands from the project root. Each line is independent and uses the
seven registered collections in `examples/musenalm`. Each command starts a new
process and builds or restores an index on first use of a collection;
repeated queries on one library `Repository` handle reuse it. Use `--release`
for the large `contents` collection. Source JSON files are never changed.

`query` reads stored JSON fields and source metadata. It returns collections in
name order and records by modification time, newest first, with ID breaking
ties. The default page has 50 records; `--count` requests a total. String values
in filters are quoted, while numbers and booleans are not. A dotted path through
an embedded array examines its values. A path that continues past a field with a
schema `x-relation` reads the linked record, as if it were embedded there:
`entry.preferred_title` in `contents` compares the title of the linked entry.
See [filters.md](filters.md#relations) for the exact semantics.

`missing(path)` selects records without a value at that path; `null` is present,
so `missing(year)` and `year = null` are different. A long-lived library caller
can use `Repository::build_posting(collection, "path")` to add an ordered posting
for a nested or undeclared path before querying it.

`Repository::value_counts(collection, "path", limit)` returns the most common
values of an indexed path with the number of records holding each, read from the
posting without opening source files. A record that holds a value twice counts
once; `null` and missing values are left out. It returns `None` when the path has
no current posting in that handle, for example before the collection is indexed.
The TUI uses it to suggest values after a comparison.

## First queries and collection scope

```sh
# Small collection: fictional places (7 matches in the bundled data).
cargo run --release -p objekte-cli -- examples/musenalm query places 'fictional = true' --count --limit 3

# A page by its stored key.
cargo run --release -p objekte-cli -- examples/musenalm query pages 'key = "kontakt"' --count

# An agent name and a string field.
cargo run --release -p objekte-cli -- examples/musenalm query agents 'name ~ :ignorecase("Goethe")' --count
cargo run --release -p objekte-cli -- examples/musenalm query agents 'profession = "Text"' --count --limit 3

# Omit COLLECTION to search all seven registered collections.
cargo run --release -p objekte-cli -- examples/musenalm query 'year >= 1800 && year < 1805' --count --limit 3

# Equivalent explicit scope using --collection instead of positional COLLECTION.
cargo run --release -p objekte-cli -- examples/musenalm query 'year = 1800' --collection contents --count --limit 3
```

## Numbers, booleans, and Boolean logic

```sh
# Numeric range and fractional equality.
cargo run --release -p objekte-cli -- examples/musenalm query entries 'year >= 1800 && year < 1810' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query contents 'numbering = 0.1' --count --limit 3

# A stored boolean combined with a year range.
cargo run --release -p objekte-cli -- examples/musenalm query contents 'year >= 1800 && year < 1805 && pseudonym = true' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query agents 'corporate_body = true' --count --limit 3

# Parentheses, OR, and a field-to-field comparison.
cargo run --release -p objekte-cli -- examples/musenalm query contents '(year = 1800 || year = 1801) && pseudonym = true' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query contents 'numbering < year' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query agents 'created < updated' --count --limit 3
```

## Strings and patterns

`~` searches for a case-sensitive literal substring when the pattern has no
unescaped `%`. With `%`, it uses a LIKE-style wildcard pattern. Wrap the pattern
in `:ignorecase(...)` for Unicode case-insensitive matching; `Ä` matches `ä` and
`ß` matches `ss`. Ordinary `=` remains case-sensitive.

```sh
# Literal substring, prefix wildcard, and negated substring.
cargo run --release -p objekte-cli -- examples/musenalm query contents 'preferred_title ~ :ignorecase("almanach")' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query series 'title ~ :ignorecase("Taschen%")' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query contents 'preferred_title !~ :ignorecase("almanach") && year = 1800' --count --limit 3

# ASCII lowercase modifier and a typed string parameter.
cargo run --release -p objekte-cli -- examples/musenalm query contents 'preferred_title:lower ~ "almanach"' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query agents 'profession = $job' --param 'job="Text"' --count --limit 3
```

## Source metadata

Metadata operands begin with `@`. Sizes use decimal units such as `KB` or
binary units such as `KiB`. Unquoted dates compare whole UTC periods, so
`2026-10` means that month. Modification and creation times depend on the
local checkout and filesystem. See [filter semantics](filters.md) for all
metadata fields and date operators.

```sh
cargo run --release -p objekte-cli -- examples/musenalm query contents '@size > 1KB && year = 1800' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query entries '@modified >= 2026-10 && @filename ~ "0%.json"' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query pages '@status = "valid" && @created != null' --count --limit 3
```

## Arrays and nested JSON

`?` means **any reached value**. For example, `language ?!= "ger"` matches an
entry with at least one language other than `ger`; it does not mean that `ger` is
absent. `:each` requires a nonempty selection where every value matches.

```sh
# Array membership, every-item matching, and any-item inequality.
cargo run --release -p objekte-cli -- examples/musenalm query entries 'language ?= "ger"' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query entries 'language:each = "ger"' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query entries 'language ?!= "ger"' --count --limit 3

# Array lengths and a reference from each standalone content to its entry.
cargo run --release -p objekte-cli -- examples/musenalm query entries 'items:length >= 1' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query contents 'entry = "00fx8ln651na0e3"' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query contents 'entry = "00fx8ln651na0e3" && year >= 1800' --count --limit 3

# A relation field alone compares the stored ID.
cargo run --release -p objekte-cli -- examples/musenalm query entries 'places ?= "d7kbujq7399g22j"' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query entries 'agents.type ?= "Verlag"' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query contents 'contributors.agents ?= "5e64t7ca8x2wyim"' --count --limit 3

# Paths past a declared relation read the linked records, including several hops,
# arrays of relations, field comparisons, and sorting.
cargo run --release -p objekte-cli -- examples/musenalm query contents 'entry.year = 1800 SORT BY entry.preferred_title' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query contents 'contributors.agents.name ?~ "%Goethe%"' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query series 'entries.places.name ?= "Leipzig"' --count --limit 3

# Other arrays in the contents and series collections.
cargo run --release -p objekte-cli -- examples/musenalm query contents 'musenalm_type ?= "Gedicht/Lied"' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query contents 'contributors.type ?= "Autor:in"' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query contents 'contributors:length > 0' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query series 'entries:length >= 3' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query series 'entries ?= "1o25nw5kc1t9ko4"' --count
```

## The remaining collections and opaque data

```sh
# Item booleans, ownership, and array membership.
cargo run --release -p objekte-cli -- examples/musenalm query items 'owner = "Theodor Springmann Stiftung" && basis = true' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query items 'media ?= "Reproduktion"' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query items 'basis = false' --count --limit 3

# Page data has an open schema and remains queryable as JSON.
cargo run --release -p objekte-cli -- examples/musenalm query pages 'data.description ~ "Musenalm"' --count --limit 3
```

## Parameters, paging, and output

Append `SORT BY field [ASC|DESC]` to a filter, with further terms separated by `&&`:

```sh
objekte examples/musenalm query contents 'year >= 1800 SORT BY preferred_title ASC' --limit 25 --count
objekte examples/musenalm query contents 'year >= 1800 SORT BY year DESC && preferred_title ASC' --limit 25
```

Sorting stays within each collection; collections remain in name order, and
filename ID breaks ties. `ASC` is the default. Strings and string enums use
case-insensitive, accent-sensitive Unicode collation: English by default or
the schema's `x-language` value. Null, missing, and invalid values follow
valid values in both directions. The Rust `JsonQuery.sort` field also accepts
an ordered list of `Sort` terms without an inline clause. An indexed order
verifies candidates in sort order and can stream stable rows; an unindexed
order scans candidates and sends provisional page replacements before the
final page is known. See [collection schemas](collections.md) for index rules.

Sort terms can also name source system fields: `@id`, `@filename`, `@path`,
`@size`, `@modified`, `@created`, and `@status`. IDs, filenames, paths, and
status names order bytewise, like the ID tie-break; size and times order
numerically. `@created` is null on filesystems without a creation time and
sorts after real values. Only `@status` validates every matching record before
it is ordered; the other system fields use stored file metadata, and
`@created` reads it once per match. `SORT BY @modified DESC` alone is the
default order and runs unsorted. Append `:fts` to a field with a declared
`x-full-text` format to sort by its rendered text, so `**Zeta**` sorts as
`Zeta`; on an undeclared field, `:fts` sorts by its normalized plain text.
Either way punctuation is ignored, so `„Zeta“` sorts as `Zeta`. System and
`:fts` terms never use a schema sort index.

```sh
objekte examples/musenalm query contents 'year = 1800 SORT BY @size DESC && @id' --limit 25
objekte examples/musenalm query contents 'exists(annotation) SORT BY @status && annotation:fts' --limit 25
```

The repository's `Options.total_limit` sets the maximum accessible matches
across all pages and exports (default 10,000). `--total-limit N` sets it for a
CLI invocation. A counted result reports the capped total and `truncated:
true` when more matches exist; the TUI displays a trailing `+`. An unindexed
sort still examines all candidates to find the best page.

Returned records are validated on demand. An explicitly scoped query searches only its selected collection, even for undeclared fields or literal types that disagree with the schema. An unscoped query searches every collection. Actual JSON values decide comparisons; schema-invalid but parseable JSON remains searchable. Malformed JSON has no JSON field paths, but can match source metadata predicates such as `@status = "incorrect_json"`. Standalone word and regex search can also return malformed source files with raw bytes, size, and modification time.

Parameters are JSON values. Keep the filter in single shell quotes so `$from`
and `$to` reach the filter parser; quote a string parameter as JSON.

```sh
# Two typed numeric parameters.
cargo run --release -p objekte-cli -- examples/musenalm query contents 'year >= $from && year <= $to' --param from=1800 --param to=1805 --count --limit 3

# The second page of five matching records.
cargo run --release -p objekte-cli -- examples/musenalm query contents 'year = 1800' --limit 5 --offset 5 --count

# One JSON object per line, suitable for piping to another tool.
cargo run --release -p objekte-cli -- examples/musenalm query items 'media ?= "Reproduktion"' --limit 3 --output ndjson

# Emit numbered records as they are verified, followed by completion metadata.
cargo run --release -p objekte-cli -- examples/musenalm query contents 'year >= 1800' --count --limit 3 --output events

# Direct source execution without an index (small scope recommended).
cargo run --release -p objekte-cli -- examples/musenalm query places 'fictional = true' --backend direct --cache disabled --count --limit 3

# Search source files with ripgrep, then verify candidate JSON with Sonic.
cargo run --release -p objekte-cli -- examples/musenalm query contents 'preferred_title ~ :ignorecase("almanach")' --backend grep --count --limit 3

# Default query: build or restore bigrams and ordered postings, then verify source candidates.
cargo run --release -p objekte-cli -- examples/musenalm query contents 'preferred_title ~ :ignorecase("almanach")' --count --limit 3

# Keep result JSON on stdout and write formatted diagnostics JSON to stderr.
cargo run --release -p objekte-cli -- examples/musenalm query contents 'year = 1800' --count --limit 3 --diagnostics 2> /tmp/objekte-diagnostics.json
```

`--output events` writes one JSON event per line and flushes each line. An unsorted record
event has `type: "record"`, a one-based `ordinal`, `provisional: true`, and a
complete `record`. After the query succeeds, a `type: "complete"` event supplies
`returned`, `total` (`null` without `--count`), dependencies, filter hash, query
plan, and metrics. If a later source or scalar-cardinality error occurs, the
stream ends with `type: "error"`, a message, and `provisional_records`; the CLI
exits nonzero. Consumers should treat earlier records as final only after the
`complete` event. Index construction or restoration completes before records
can arrive; parallel verification publishes finished batches in default
modification-time order. The existing `--output ndjson` writes raw records after
the query finishes.
For a sorted query, provisional `page` events contain the current ordered
`records` array; later pages may replace earlier ones. The `complete` event
confirms the final page and carries `truncated`.

## Named views

`query` orders by collection, then modification time and ID. Named views combine
one or more native queries with typed arguments and a list or single presentation
type. Their definitions are independent of collections; see
[Independent views](views.md).

```sh
cargo run --release -p objekte-cli -- examples/musenalm view contents_list --arg page=2
```
