# Musenalm query examples

Run these commands from the project root. Each line is independent and uses the
seven registered collections in `examples/musenalm`. The first SQLite query for a
collection builds its disposable cache; later commands reuse it. Use `--release`
for the large `contents` collection. Source JSON files are never changed.

`query` reads stored JSON fields, including fields outside the collection's list
UI. It returns complete records in `(collection, filename ID)` order. The default
page has 50 records; `--count` requests a total. String values in filters are
quoted, while numbers and booleans are not. A dotted path through an embedded
array examines its values; it does not follow a declared relation to another
record. See [filters.md](filters.md) for the exact operator semantics.

## First queries and collection scope

```sh
# Small collection: fictional places (7 matches in the bundled data).
cargo run --release -p objekte-cli -- examples/musenalm query places 'fictional = true' --count --limit 3

# A page by its stored key.
cargo run --release -p objekte-cli -- examples/musenalm query pages 'key = "kontakt"' --count

# An agent name and a string field.
cargo run --release -p objekte-cli -- examples/musenalm query agents 'name ~ "Goethe"' --count
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

`~` searches for a literal substring when the pattern has no unescaped `%`. With
`%`, it uses a LIKE-style wildcard pattern. Matching folds ASCII case only;
ordinary `=` remains case-sensitive.

```sh
# Literal substring, prefix wildcard, and negated substring.
cargo run --release -p objekte-cli -- examples/musenalm query contents 'preferred_title ~ "almanach"' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query series 'title ~ "Taschen%"' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query contents 'preferred_title !~ "almanach" && year = 1800' --count --limit 3

# ASCII lowercase modifier and a typed string parameter.
cargo run --release -p objekte-cli -- examples/musenalm query contents 'preferred_title:lower ~ "almanach"' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query agents 'profession = $job' --param 'job="Text"' --count --limit 3
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

# Array lengths and nested fields inside embedded entry contents.
cargo run --release -p objekte-cli -- examples/musenalm query entries 'contents:length >= 50' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query entries 'contents.year ?>= 1800' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query entries 'exists(contents[0].source_id)' --count --limit 3

# Stored relation IDs are searchable as strings; these do not expand target records.
cargo run --release -p objekte-cli -- examples/musenalm query entries 'places ?= "d7kbujq7399g22j"' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query entries 'agents.type ?= "Verlag"' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query contents 'contributors.agents ?= "5e64t7ca8x2wyim"' --count --limit 3

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

# Page data is opaque to its schema but remains queryable as JSON.
cargo run --release -p objekte-cli -- examples/musenalm query pages 'data.description ~ "Musenalm"' --count --limit 3
```

## Parameters, paging, and output

Parameters are JSON values. Keep the filter in single shell quotes so `$from`
and `$to` reach the filter parser; quote a string parameter as JSON.

```sh
# Two typed numeric parameters.
cargo run --release -p objekte-cli -- examples/musenalm query contents 'year >= $from && year <= $to' --param from=1800 --param to=1805 --count --limit 3

# The second page of five matching records.
cargo run --release -p objekte-cli -- examples/musenalm query contents 'year = 1800' --limit 5 --offset 5 --count

# One JSON object per line, suitable for piping to another tool.
cargo run --release -p objekte-cli -- examples/musenalm query items 'media ?= "Reproduktion"' --limit 3 --output ndjson

# Direct source execution without a SQLite cache (small scope recommended).
cargo run --release -p objekte-cli -- examples/musenalm query places 'fictional = true' --cache disabled --count --limit 3

# Keep result JSON on stdout and write every executed SQLite statement with its
# duration_ms to stderr as JSON lines. The final line summarizes diagnostics.
cargo run --release -p objekte-cli -- examples/musenalm query contents 'year = 1800' --count --limit 3 --validation wait --diagnostics 2> /tmp/objekte-sql.ndjson
```

## Sorted table views

`query` orders by collection and ID. Use `view` for configured scalar columns,
filters, and sorts. Its `--filter` syntax is the simpler `FIELD=VALUE` form and
only configured filter fields are permitted.

```sh
cargo run --release -p objekte-cli -- examples/musenalm view contents --filter 'year>=1800' --sort numbering --direction desc --limit 10 --count
cargo run --release -p objekte-cli -- examples/musenalm view contents --sort year --then-sort numbering:desc --limit 10
cargo run --release -p objekte-cli -- examples/musenalm view entries --columns preferred_title,year,edit_state --limit 5 --output json
```
