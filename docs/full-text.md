# Full-text search

Full-text search includes every JSON leaf value by default, including undeclared fields and schema-free collections. Strings are searched as plain text unless a schema declares Markdown, HTML, or XML rendering. Numbers, booleans, and null are searchable by their JSON value text. Objects and arrays contribute their nested leaves.

Use `x-full-text` on a schema string field to select its rendering format. Use `exclude_fts: true` on any schema field to omit that field and its descendants from both full-text indexes and standalone full-text search. The default is `false`:

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

Three standalone search modes use the included fields:

| Mode | What it matches | CLI |
| --- | --- | --- |
| Word | Whole words or phrases in rendered text; case insensitive by default | `search 'Help' --mode word` |
| Rendered regex | Regex over rendered text without markup | `search 'Help\s+world' --mode regex` |
| Raw regex | Regex over decoded field strings including Markdown, HTML, or XML markup | `search '<b>lp</b>' --mode raw-regex` |

`--case-sensitive` applies to every mode. Word search uses Unicode word boundaries and Unicode case folding. Regex uses Rust regex syntax and its Unicode case-insensitive matching. Raw regex sees decoded JSON values, so a JSON `\n` escape has already become a newline; it does not search JSON syntax or file bytes. All modes return matching records in collection and byte-sorted record ID order, with the leaf paths that matched. Use `--field annotation` or an array path such as `--field 'notes[].body'` to narrow the fields, and a positional collection or `--collection` to narrow the collections. `--limit`, `--offset`, and `--count` work as for filter queries.

The JSON filter language also accepts `:fts` on a field operand. It renders a field with a declared format before applying the ordinary comparison, so it composes with numbers, booleans, `&&`, `||`, `~`, `!~`, `=`, and `:ignorecase`:

```sh
objekte REPOSITORY query contents 'year = 1822 && annotation:fts ~ "Goet"' --count
objekte REPOSITORY query contents 'annotation:fts ~ :ignorecase("GOET")' --count
```

Here `~` retains the filter language's LIKE-style matching; it is not a regex operator. Use `search --mode regex` for regex over rendered text. For a path with no declared `x-full-text` format, including in a schema-free collection or under `exclude_fts`, `:fts` has exactly the same matching behavior as omitting it. An explicit JSON filter can still search an excluded field. When the schema supplies an included format and a positive pattern has a usable literal, the filter planner intersects full-text bigram candidates with other safe JSON index candidates. Negated or broad patterns still verify source records.

```sh
objekte REPOSITORY search COLLECTION 'Help' --mode word --count
objekte REPOSITORY search COLLECTION 'Help\s+world' --mode regex --field annotation
objekte REPOSITORY search COLLECTION '<b>lp</b>' --mode raw-regex --case-sensitive
objekte REPOSITORY search COLLECTION 'Help' --output events
```

The library API uses `TextQuery` with `Repository::search_text(&query)` or `Repository::search_text_stream(&query, callback)`. The streaming callback receives numbered page hits as they become available. Treat them as provisional until the final summary: a later source error invalidates an earlier hit. The CLI `--output events` emits `hit`, `complete`, and `error` JSON lines with the same contract.

The regular JSON bigram index still serves filter queries. With the default memory cache, a collection whose schema declares an included Markdown, HTML, or XML field builds the normal and full-text indexes in one file read and JSON parse when the collection's index is first warmed. The full-text pair covers rendered text and decoded source values. Collections without an included markup declaration keep the full-text pair lazy: standalone word or regex search, an indexed `:fts` filter on a declared plain field, or explicit text-index warming builds it on demand. An undeclared `:fts` field behaves like an ordinary filter and does not build the pair. Every leaf contributes unless its schema field or an ancestor has `exclude_fts: true`. Word and regex searches select candidate files from the appropriate index and then parse and verify the source JSON. Regex patterns without a safe bigram prefilter scan the included source values. With `--cache disabled`, full-text search scans the included source values directly. The indexes are derived, process-local data.

For a long-lived UI, `warm_text_index(collection)` and `warm_all_text_indexes()` build missing full-text indexes after startup. `rebuild_text_index(collection)` and `rebuild_all_text_indexes()` refresh and replace them. The CLI exposes `cache warm-text` and `cache rebuild-text`, optionally with `--collection NAME`; each CLI invocation has its own process and cache. `refresh(collection)` admits added and changed files through a conservative overlay until rebuild, and removes deleted files from candidates. A full bigram rebuild also rebuilds an active full-text index in the same file pass.

`x-full-text` and `exclude_fts` are independent of the schema's scalar `index` setting, which creates ordered postings for JSON comparisons and views. Use `query` with `:fts` for structural combinations, or `search` for standalone word and regex modes.

To repeat the release measurements on Musenalm, run `cargo run --release -p objekte-bench --bin fts-query-mix -- examples/musenalm 3`. The benchmark creates a temporary schema overlay for `contents` that marks `annotation` as Markdown, `preferred_title` as plain text, `title_statement` as HTML, and `contributors[].annotation` as XML. Every other leaf is included with plain rendering. It changes no example files. Each case counts every match and returns one record; the reported query time is the median of three warm runs. Ready-state RSS is sampled after each index build, while compressed posting bytes are reported separately.

The complete 30-case measurements are in [fts-benchmark-results.md](fts-benchmark-results.md).
