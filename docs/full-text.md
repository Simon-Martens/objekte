# Full-text search

Full-text search includes every JSON leaf value by default, including undeclared fields and schema-free collections. Strings are searched as plain text unless a schema declares Markdown, HTML, or XML rendering. Numbers, booleans, and null are searchable by their JSON value text. Objects and arrays contribute their nested leaves.

Use `x-full-text` on a schema string field to select its rendering format. Use `exclude_fts: true` on any schema field to omit that field and its descendants from standalone full-text search and, when built, the separate full-text indexes. The default is `false`:

```json
{
  "type": "object",
  "properties": {
    "annotation": { "type": "string", "x-full-text": "markdown" },
    "transcription": { "type": "string", "x-full-text": "xml" },
    "private_note": { "type": "string", "exclude_fts": true },
    "notes": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "body": { "type": "string", "x-full-text": "html" }
        }
      }
    }
  }
}
```

For `markdown`, `html`, and `xml`, the rendered search view removes markup and decodes entities. Inline formatting joins adjacent text: `**He**lp` and `He<b>lp</b>` become `Help`. Block boundaries become spaces. Carriage returns, newlines, tabs, and other Unicode whitespace collapse to one space in every rendered format. HTML script, style, and head content is excluded. Malformed XML that cannot be parsed has no rendered match; its decoded source string remains available to raw regex search. The source JSON is never rewritten.

Four standalone search modes use the included fields:

| Mode | What it matches | CLI |
| --- | --- | --- |
| Word | Literal substrings in rendered text, including partial words; case insensitive by default | `cargo run --release -p objekte-cli -- examples/musenalm search contents 'Help' --mode word --limit 3` |
| Raw word | Literal substrings in decoded source values with markup intact | `cargo run --release -p objekte-cli -- examples/musenalm search contents 'Help' --mode raw-word --limit 3` |
| Rendered regex | Regex over rendered text without markup | `cargo run --release -p objekte-cli -- examples/musenalm search contents 'Help\s+world' --mode regex --limit 3` |
| Raw regex | Regex over decoded field strings including Markdown, HTML, or XML markup | `cargo run --release -p objekte-cli -- examples/musenalm search contents '<b>lp</b>' --mode raw-regex --limit 3` |

`--case-sensitive` applies to every mode. Word mode treats the input as literal text, so `Lichtenber` matches `Lichtenberg` and `chtenbe` matches within it. It uses Unicode case folding by default. For whole-word matching, use regex mode with boundaries such as `\bLichtenberg\b`. Regex uses Rust regex syntax and its Unicode case-insensitive matching. Raw regex sees decoded JSON values for parseable files, so a JSON `\n` escape has already become a newline. For malformed files it searches the raw source plus best-effort unescaped text. By default, all modes return matching records in collection and byte-sorted record ID order. Each hit has canonical `fields` for query scope, such as `contents[].incipit_statement`, and concrete `matched_paths` for the leaves that matched, such as `contents[9].incipit_statement`. These identify array elements, not character ranges. Use `--field annotation` or an array path such as `--field 'notes[].body'` to narrow the fields, and a positional collection or `--collection` to narrow the collections. `--limit`, `--offset`, and `--count` work as for filter queries.

Append a `SORT BY` clause to order matches within each collection, for example
`'Wasser SORT BY preferred_title ASC && year DESC'`. To include a literal
`SORT BY` phrase in the search pattern while also sorting, quote the pattern
inside the query text: `'"SORT BY" SORT BY preferred_title'`. The Rust
`TextQuery.sort` field accepts the same terms separately. Sorted `--output
events` uses provisional `page` events whose `hits` arrays can change as
verification proceeds; completion fixes the final page. Alphabetic ordering
uses schema language metadata and ignores case, independently of search case
matching. Repository `total_limit` caps accessible matches across pages.

Add `--filter 'year >= 1800'` to `search` to run a JSON field predicate before the full-text matcher. `--param NAME=JSON` binds typed values in that predicate. For example, `search contents 'Wasser SORT BY preferred_title' --filter 'year >= $min' --param min=1800 --count` returns only records that satisfy both conditions. The API exposes the same combination through `TextQuery.filter` and `TextQuery.parameters`. The filter runs before text matching, counts, paging, and sorting. When safe, its typed, presence, and bigram candidates narrow the full-text candidate set. A malformed JSON file cannot satisfy the filter.

The JSON filter language also accepts `:fts` on a field operand. It renders a field with a declared format before applying the ordinary comparison, so it composes with numbers, booleans, `&&`, `||`, `~`, `!~`, `=`, and `:ignorecase`:

```sh
cargo run --release -p objekte-cli -- examples/musenalm query contents 'year = 1822 && annotation:fts ~ "Goet"' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query contents 'annotation:fts ~ :ignorecase("GOET")' --count --limit 3
```

Here `~` retains the filter language's LIKE-style matching; it is not a regex operator. Use `search --mode regex` for regex over rendered text. For a path with no declared `x-full-text` format, including in a schema-free collection or under `exclude_fts`, `:fts` has exactly the same matching behavior as omitting it. An explicit JSON filter can still search an excluded field. When the schema supplies an included format and a positive pattern has a usable literal, the filter planner intersects full-text bigram candidates with other safe JSON index candidates. Negated or broad patterns still verify source records.

```sh
cargo run --release -p objekte-cli -- examples/musenalm search contents 'Help' --mode word --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm search contents 'Help\s+world' --mode regex --field annotation --limit 3
cargo run --release -p objekte-cli -- examples/musenalm search contents '<b>lp</b>' --mode raw-regex --case-sensitive --limit 3
cargo run --release -p objekte-cli -- examples/musenalm search contents 'Help' --output events --limit 3
```

The library API uses `TextQuery` with `Repository::search_text(&query)` or `Repository::search_text_stream(&query, callback)`. Collected results and final stream summaries include `elapsed_ms`, the wall-clock duration of the core search call. The streaming callback receives numbered page hits as they become available. Treat them as provisional until the final summary: a later source error invalidates an earlier hit. The CLI `--output events` emits `hit`, `complete`, and `error` JSON lines with the same contract.

The regular JSON bigram index serves filter queries and standalone word or regex searches when there is no separate full-text index. With the default memory cache, a collection whose schema declares Markdown, HTML, XML, or `exclude_fts` builds the normal and separate full-text indexes in one file read and JSON parse when the collection's index is first warmed. The separate pair covers rendered text and decoded source values; no search or `:fts` modifier creates it otherwise. Every parseable leaf contributes unless its schema field or an ancestor has `exclude_fts: true`. Malformed files contribute raw and best-effort JSON-unescaped text to the normal index and, when present, the separate pair. Standalone word and regex hits on malformed files have `incorrect_json` status and the special matched path `$raw`. A `--field` restriction excludes malformed files because their JSON paths cannot be verified. Exclusion rules also cannot be applied to malformed JSON, so its whole-file text remains searchable. Field-based filters, including `:fts`, never match malformed JSON. Word and regex searches select candidate files from the applicable index and then verify source text. Regex patterns without a safe bigram prefilter scan the included source values. With `--cache disabled`, full-text search scans source text directly. The indexes are derived, process-local data.

For a long-lived UI, `warm_text_index(collection)` and `warm_all_text_indexes()` ensure eligible full-text indexes exist after startup. `rebuild_text_index(collection)` and `rebuild_all_text_indexes()` refresh and replace eligible indexes. These methods do not build a separate pair for schema-free or plain-only collections. The CLI exposes `cache warm-text` and `cache rebuild-text`, optionally with `--collection NAME`; each CLI invocation has its own process and cache. `refresh(collection)` admits added and changed files through a conservative overlay until rebuild, and removes deleted files from candidates. A full bigram rebuild also rebuilds an active full-text index in the same file pass.

`x-full-text` and `exclude_fts` are independent of the schema's scalar `index` setting, which creates ordered postings for JSON comparisons and views. Use `query` with `:fts` for field-specific rendered comparisons, or `search --filter` to combine any JSON predicate with standalone word or regex modes.

To measure the current Musenalm data, run `cargo run --release -p objekte-bench --bin fts-query-mix -- examples/musenalm 3`. The published [benchmark results](fts-benchmark-results.md) predate the addition of `contents.entry`, so new numbers will differ. The benchmark creates a temporary schema overlay for `contents` that marks `annotation` as Markdown, `preferred_title` as plain text, `title_statement` as HTML, and `contributors[].annotation` as XML. Every other leaf is included with plain rendering. It changes no example files. Each case counts every match and returns one record; the reported query time is the median of three warm runs. Ready-state RSS is sampled after each index build, while compressed posting bytes are reported separately.

The complete 30-case measurements are in [fts-benchmark-results.md](fts-benchmark-results.md).
