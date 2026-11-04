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

For `markdown`, `html`, and `xml`, the rendered search view removes markup and decodes entities. Inline formatting joins adjacent text: `**He**lp` and `He<b>lp</b>` become `Help`. Block boundaries become spaces. Then, in every format including plain text, Unicode punctuation (general category `P`: commas, periods, dashes, slashes, brackets, quotes, `_`, and the like) becomes a space, and runs of whitespace collapse to one space, trimmed at both ends. `„Goethe“, Johann – Faust` reads `Goethe Johann Faust`. Symbols such as `$`, `+`, `€`, or `©` stay. HTML script, style, and head content is excluded. Malformed XML that cannot be parsed has no rendered match; its decoded source string remains available to raw regex search. The source JSON is never rewritten.

Four standalone search modes use the included fields:

| Mode | What it matches | CLI |
| --- | --- | --- |
| Word | Literal substrings in rendered text without punctuation, including partial words; case insensitive by default | `cargo run --release -p objekte-cli -- examples/musenalm search contents 'Help' --mode word --limit 3` |
| Raw word | Literal substrings in decoded source values with markup intact | `cargo run --release -p objekte-cli -- examples/musenalm search contents 'Help' --mode raw-word --limit 3` |
| Rendered regex | Regex over rendered text without markup or punctuation | `cargo run --release -p objekte-cli -- examples/musenalm search contents 'Help\s+world' --mode regex --limit 3` |
| Raw regex | Regex over decoded field strings including Markdown, HTML, or XML markup | `cargo run --release -p objekte-cli -- examples/musenalm search contents '<b>lp</b>' --mode raw-regex --limit 3` |

`--case-sensitive` applies to every mode. Word mode treats the input as literal text, so `Lichtenber` matches `Lichtenberg` and `chtenbe` matches within it. The input is normalized like the text: `goethe, johann` and `Goethe Johann` both find `Goethe, Johann`, and `Graphik Verzeichnis` finds `Graphik-Verzeichnis`. A pattern of only punctuation is rejected. Raw word and raw regex keep the source characters, so they are the way to search for punctuation. It uses Unicode case folding by default. For whole-word matching, use regex mode with boundaries such as `\bLichtenberg\b`. Regex uses Rust regex syntax and its Unicode case-insensitive matching. Raw regex sees decoded JSON values for parseable files, so a JSON `\n` escape has already become a newline. For malformed files it searches the raw source plus best-effort unescaped text. By default, all modes return matching records by collection, then file modification time newest first, with byte-sorted record ID breaking ties. Each hit has canonical `fields` for query scope, such as `contents[].incipit_statement`, and concrete `matched_paths` for the leaves that matched, such as `contents[9].incipit_statement`. These identify array elements, not character ranges. Use `--field annotation` or an array path such as `--field 'notes[].body'` to narrow the fields, and a positional collection or `--collection` to narrow the collections. `--limit`, `--offset`, and `--count` work as for filter queries.

Full-text patterns are passed unchanged to the selected word or regex matcher; core never removes an appended sort clause from `TextQuery.pattern`. Supply structured `TextQuery.sort` terms in the library or a separate CLI option, such as `search contents 'Wasser' --sort 'preferred_title ASC && year DESC'`. In the TUI's full-text mode, an appended `SORT BY` is split into structured sort terms by the UI before the core call. Sorted `--output events` uses provisional `page` events whose `hits` arrays can change as verification proceeds; completion fixes the final page. Alphabetic ordering uses schema language metadata and ignores case, independently of search case matching. Repository `total_limit` caps accessible matches across pages.

Add `--filter 'year >= 1800'` to `search` to run a JSON field predicate before the full-text matcher. `--param NAME=JSON` binds typed values in that predicate. For example, `search contents 'Wasser' --filter 'year >= $min SORT BY preferred_title' --param min=1800 --count` returns only records that satisfy both conditions and sorts those final matches. The API exposes the same combination through `TextQuery.filter` and `TextQuery.parameters`. A filter's `SORT BY` clause supplies the final result sort; it cannot be combined with a separate `TextQuery.sort` or CLI `--sort`. The filter runs before text matching, counts, paging, and sorting. When safe, its typed, presence, and bigram candidates narrow the full-text candidate set. A malformed JSON file has no JSON fields, but can satisfy a metadata predicate such as `@status = 'incorrect_json'`.

The JSON filter language also accepts `:fts` on a field operand. It compares the field's full-text version, as full-text search reads it: rendered with its declared format, or as plain text, and in both cases without punctuation and with whitespace collapsed. A compared string is normalized the same way, keeping `%` and `_` wildcards, so `title:fts ~ "Goethe, Johann"` finds `Goethe,Johann`. It applies the ordinary comparison afterwards, so it composes with numbers, booleans, `&&`, `||`, `~`, `!~`, `=`, and `:ignorecase`:

```sh
cargo run --release -p objekte-cli -- examples/musenalm query contents 'year = 1822 && annotation:fts ~ "Goet"' --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm query contents 'annotation:fts ~ :ignorecase("GOET")' --count --limit 3
```

Here `~` retains the filter language's LIKE-style matching; it is not a regex operator. Use `search --mode regex` for regex over rendered text. For a path with no declared `x-full-text` format, including in a schema-free collection, on an indexed field, or under `exclude_fts`, `:fts` reads the value as normalized plain text and verifies each candidate record; no full-text postings narrow it. For a declared field, including one with `"index": true`, `:fts` renders the value and uses that field's rendered postings when a safe literal is available. Negated or broad patterns still verify source records.

```sh
cargo run --release -p objekte-cli -- examples/musenalm search contents 'Help' --mode word --count --limit 3
cargo run --release -p objekte-cli -- examples/musenalm search contents 'Help\s+world' --mode regex --field annotation --limit 3
cargo run --release -p objekte-cli -- examples/musenalm search contents '<b>lp</b>' --mode raw-regex --case-sensitive --limit 3
cargo run --release -p objekte-cli -- examples/musenalm search contents 'Help' --output events --limit 3
```

The library API uses `TextQuery` with `Repository::search_text(&query)` or `Repository::search_text_stream(&query, callback)`. Collected results and final stream summaries include `elapsed_ms`, the wall-clock duration of the core search call. The streaming callback receives numbered page hits as they become available. Treat them as provisional until the final summary: a later source error invalidates an earlier hit. The CLI `--output events` emits `hit`, `complete`, and `error` JSON lines with the same contract.

The general normal bigram filter contains decoded values from undeclared fields and fields without `x-full-text`. Each explicitly declared `x-full-text` leaf has **two independent filters**: one over its decoded source value and one over its rendered text. Its value is omitted from the general normal filter. Array elements declared at the same item path share that field's two filters. A combined rendered filter also includes registered fields so unrestricted standalone full-text search remains selective. There is no shared raw full-text filter: raw search unions the general normal filter with the per-field normal filters. All of these filters retain their own skip-one postings. Per-field compression measures posting density against all collection files, so a field present in only a few records keeps its useful pairs. This increases index memory with the number of declared FTS fields; `x-full-text: plain` also creates two field filters even when their input text is equivalent.

The filters are built from one source read and JSON parse when the collection index is warmed. A schema with any `x-full-text` declaration, markup rendering, or `exclude_fts` builds the rendered filters eagerly. A simple top-level `--field` restriction uses that field's filter; a nested or ambiguous display path uses conservative combined candidates. Canonical field paths in JSON `:fts` predicates use the corresponding field filter, including array item paths. If that field has no usable posting for a rendered query, the combined rendered filter provides a conservative fallback. Standalone full-text search also scans distinct B-tree values for indexed scalar fields and unions those matching slots with bigram candidates before source verification. Malformed files contribute raw and best-effort JSON-unescaped text to the general normal filter and, when present, the combined rendered filter. Standalone word and regex hits on malformed files have `incorrect_json` status and the special matched path `$raw`. A `--field` restriction excludes malformed files because their JSON paths cannot be verified. Exclusion rules also cannot be applied to malformed JSON, so its whole-file text remains searchable. JSON field predicates, including `:fts`, do not match malformed JSON; metadata predicates such as `@status = "incorrect_json"` can select it. Word and regex searches select candidate files from the applicable index and then verify source text. Regex patterns without a safe bigram prefilter scan the included source values. With `--cache disabled`, full-text search scans source text directly. Active indexes belong to the repository handle; compatible serialized snapshots can restore them in a later process.

For a long-lived UI, `warm_text_index(collection)` and `warm_all_text_indexes()` ensure eligible full-text indexes exist after startup. `rebuild_text_index(collection)` and `rebuild_all_text_indexes()` refresh and replace eligible indexes. Schema-free collections remain lazy; a declared plain-text FTS field builds its two field filters. The CLI exposes `cache warm-text` and `cache rebuild-text`, optionally with `--collection NAME`; each CLI invocation has its own process and cache. `refresh(collection)` admits added and changed files through a conservative overlay until rebuild, and removes deleted files from candidates. A full bigram rebuild also rebuilds an active full-text index in the same file pass.

`x-full-text` and `exclude_fts` still control standalone full-text verification. An `index` setting also retains typed B-tree postings for exact field comparisons. Use `query` with `:fts` for field-specific rendered comparisons, or `search --filter` to combine any JSON predicate with standalone word or regex modes.

To measure the current Musenalm data, run `cargo run --release -p objekte-bench --bin fts-query-mix -- examples/musenalm 3`. The published [benchmark results](../crates/bench/FTS-MUSENALM-2026-10-31.md) predate the addition of `contents.entry`, so new numbers will differ. The benchmark creates a temporary schema overlay for `contents` that marks `annotation` as Markdown, `preferred_title` as plain text, `title_statement` as HTML, and `contributors[].annotation` as XML. Every other leaf is included with plain rendering. It changes no example files. Each case counts every match and returns one record; the reported query time is the median of three warm runs. Ready-state RSS is sampled after each index build, while compressed posting bytes are reported separately.
