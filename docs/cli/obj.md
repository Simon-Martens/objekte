# `obj`: search a folder of JSON documents

By default, `obj` treats a directory as one collection named `files`. It accepts any valid JSON root as one record per `.json` file, including objects, arrays, strings, numbers, booleans and `null`. It needs no schema, field list or repository configuration. Run it from the directory to search, or use `--path`. Repeat `--path` to open several directories as separate collections. Use `--repo` for a configured multi-collection repository.

```sh
cargo build --release -p objekte-cli
./target/release/obj --fts Goethe
./target/release/obj -p ./documents 'title ~ "Goethe"'
./target/release/obj -p ./documents '@root[0] = "first"'
./target/release/obj -p ./documents 'year >= 1800 && year < 1900 SORT BY year DESC'
./target/release/obj --repo examples/musenalm --tui
./target/release/obj -p examples/musenalm/data/contents -p examples/musenalm/data/entries --tui
```

The positional query is a structured expression unless it is exactly one double-quoted JSON string literal. Thus `obj '"Goethe"'` and `obj --fts Goethe` are equivalent literal full-text searches. A leading `-` in a positional query requires `--` before the query. `obj --help` lists the options without opening a repository.

Source-file paths in NDJSON hits, printed results, TUI rows and exports are
relative to the opened repository root. `@file.path` compares this same value,
so `@file.path = 'contents/123.json'` matches that file in a repository rooted
at `/data/archive`; the absolute string `/data/archive/contents/123.json` does
not. `@repository.root` remains the separate absolute root value. When using
`--path`, resolve a returned source path against the root selected by that
invocation, independently of the process working directory.

## Inputs and result scope

| Option | Behavior |
| --- | --- |
| `-p, --path PATH` | Search `PATH`; repeat to open separate collections. The current directory is the default. |
| `--repo PATH` | Open all collections declared in `PATH/.objekte/collections/*.schema.json`. |
| `--schema FILE` | Apply one schema to the single `files` collection. Unavailable relations are ignored. |
| `--tui` | Open the interactive screen; multiple collections offer collection selection. |
| `--auto-index` | Infer bounded ordered postings in each opened collection. |
| `--no-recurse` | Search immediate `.json` children only. |
| `--fts TEXT` | Search literal scalar text without JSON-string quoting. |
| `--query-file FILE` | Run one query per physical line; `-` reads stdin. |
| `--limit N` | Return a finite page of at most `N` hits. Without it, return all hits. `0` is valid. |
| `--offset N` | Skip `N` hits in final result order; default `0`. |
| `--count` | Compute an exact total for a finite page. All-results mode always computes it. |
| `--index-alphabet ascii\|unicode` | Select bigram alphabet; default `ascii`. Exact case behavior is a query property. |
| `--load-index temporary\|repository\|custom` | Try loading one saved generation from the selected target. A missing generation falls back to a memory build. |
| `--load-index-directory DIR` | Base directory for `--load-index custom`. |
| `--flush-index temporary\|repository\|custom` | Save the completed index publication once to the selected target. |
| `--flush-index-directory DIR` | Base directory for `--flush-index custom`. Relative paths resolve from the invocation's working directory. |
| `--diagnostic`, `--diagnostics` | Include inferred index paths in the final NDJSON event and emit a pretty JSON operational diagnostic on stderr. |

`QUERY`, `--fts`, and `--query-file` are mutually exclusive. Bare `obj` reports a missing query and does not build an index. An explicit load or flush permits an index-only invocation with no query. Each `custom` operation requires its corresponding directory option; loading and flushing may use different targets. The removed combined storage and residency options report migration guidance.

`--repo` is exclusive with `--path`, `--schema`, and `--no-recurse`. It reads collection name, source path, format and ID policy from each schema's `x-objekte` object. Relative source paths must stay inside the repository root. Available `x-relation` targets remain active, so the TUI can show outgoing links and backlinks; declarations pointing to collections absent from the repository are ignored. Repository CLI queries search all collections, while the TUI can select one. The default builds in memory. Use `--load-index` to try one saved target and `--flush-index` to save once.

With repeated `--path`, each directory becomes a collection named after its final directory component. Queries search all selected collections, and the TUI can select one. Names must be distinct; directories must not overlap and must share a useful parent directory. That common parent is the repository root. Only the selected directories are searched. Repeated `--path` does not require `.objekte` or collection schemas, and it cannot be combined with `--schema`. A single `--path` retains the `files` collection name and existing `--schema` behavior.

A query file accepts UTF-8, LF or CRLF, and an optional initial BOM. Blank lines are ignored. A nonblank line beginning with `"` must be one complete JSON string literal with no trailing tokens; JSON escapes are decoded once. Every other line is passed unchanged to the core structured-query parser. There is no comment or multiline continuation syntax. An erroneous line gets its own `query_error`, and later lines continue. The process exits nonzero if any line fails. A readable file with no queries is an error unless the invocation also has an explicit load or flush operation.

```text
"Goethe"
title ~ "Goethe"
@root :null
annotation:? = true
items[*]:null = true
@root[0] = "first"
year >= 1800 && year < 1900 SORT BY year DESC
```

`@root` denotes the JSON document itself. `@root.title` is equivalent to `title` for an object root; `@root["a.b"]` addresses a literal key containing a dot. A real property named `@root` can be addressed with a quoted key step. Structured predicates keep their type rules: a numeric JSON value does not equal a string containing the same characters. Default universal array comparisons retain the core's vacuous-empty-array rule; use `?=` for existential array matching.

## Collection and text rules

Discovery recursively selects regular files with the case-sensitive `.json` extension. Hidden JSON files and hidden directories are included. `.git` directories and application-owned index directories are pruned before descent. `.jsonl`, nested symlinks and files excluded by extension are ignored. `.gitignore` does not filter inputs. A top-level symlink passed with `--path` is resolved while canonicalizing the root. Directory discovery errors fail startup; an empty directory is a valid zero-record collection.

The stable public record ID uses the core's `path:` prefix followed by the path relative to the selected root, including the extension. Thus `path:a/001.json` and `path:b/001.json` are different records. Non-UTF-8 path components use the core's reversible encoded representation. Adding an unrelated file does not renumber existing public identities.

Whole-document FTS searches each scalar value as a separate segment:

- Strings are decoded once, so `"Gr\\u00fcnde"` supplies `Gründe`.
- Numbers use their source token spelling. `1`, `1.0`, and `1e0` are distinct FTS text even when numeric comparisons treat them as equal.
- Booleans and null contribute `true`, `false`, and `null`.
- Property names, braces, brackets, commas and colons contribute no text.

An FTS phrase cannot span two scalar values. Duplicate object names use the last occurrence for structured field access; whole-document FTS sees the scalar values of all occurrences. Arrays retain mixed members and order. Explicit schemas validate declared fields. Inferred ordered postings are built only with `--auto-index`.

Malformed JSON follows the core's raw-text fallback behavior. Such a file can match unscoped FTS through recovery text, but its logical fields are unavailable. A syntactically valid document rejected by an explicit declared schema can similarly be a fallback in other core applications; the output distinguishes its source JSON payload from logical availability.

## NDJSON output

Stdout contains one UTF-8 JSON object per line, with `protocol_version: 3`, a monotonic `sequence`, and a `type`. A normal run emits `run_start`, then for each query `query_start`, zero or more ordered `hit` events, exactly one `query_complete`, and finally `run_complete`. Query and run errors or cancellation use the corresponding `query_error`, `query_cancelled`, `run_error`, or `run_cancelled` terminal type. An index-only run has only run events. Stderr carries operational messages and optional diagnostics; it never shares stdout's event stream.

On a fresh build, the CLI may evaluate an accepted query internally as
searchable batches arrive. It waits for construction to complete before
emitting any query events, so hit order, exact counts, exports, and exit
status retain the complete-index behavior. An explicitly loaded generation can open without reparsing unchanged source content. Early internal evaluation covers direct inputs
and regular query files. Stdin and named pipes are read by the normal ordered
query stream after opening so input reads cannot block construction.

`query_start` includes a monotonic `query_id`, original input, `structured` or `fts` kind, optional physical query-file line, requested scope and count policy. A `hit` includes the query ID, zero-based emitted position and requested offset; collection and stable record ID; generation and file ID; relative path, size and available created/modified metadata; record byte range, class and status; diagnostic reference/details when available; and matched-field evidence when the core provides it. Unscoped FTS does not invent an exact field list.

`data` is the complete original source JSON value, not a projection or a serialization of the core's typed model. Its insignificant outer and inter-token whitespace is removed so the event stays on one line; string escapes, number spelling, member order and duplicate names are preserved. `data_available: true` distinguishes a genuine JSON `null` from unavailable source JSON. For malformed raw fallback, `data_available` is false, `data` is null, and the event carries `raw_text` with `raw_encoding: "utf8"` or `raw_base64` with `raw_encoding: "base64"`.

The query terminal includes `emitted_rows`, `observed_matches`, `exact_total` or null, `total_exact`, core work counters, `first_result_ms` (null if there was no hit), and `all_results_ms`. A failed or cancelled query never claims an exact final total. The run terminal reports files, records, fallback records, build parses, `index_timings_ms`, `index_action`, `index_persistence`, `index_flush_report`, and query success/error counts. `index_persistence` separates source reconciliation, current publication revision, loaded origin, last saved revision, destination store generation, and validation diagnostics. `index_flush_report` identifies the saved publication revision, destination, store generation, unchanged outcome, and whether newer changes remain unsaved. File creation and modification timestamps in hit events are Unix-epoch milliseconds with three decimals.

Protocol version 3 replaces version 2's combined `index_storage_status` and `index_storage_action` fields with operation-specific `index_action`, `index_persistence`, and `index_flush_report`. The `last_error_kind` and `validation_stamp_error_kind` values are `access`, `corrupt`, `incompatible`, `conflict`, `resource`, or `other`.

Timing fields use a monotonic clock. `first_result_ms` starts when the query begins and stops when its first hit reaches the output callback; `all_results_ms` stops after the query output flush, before the terminal event is written. Both are reported in milliseconds with three decimals. `index_timings_ms` contains elapsed integer milliseconds from the start of the build or load operation to `metadata`, `search_available`, `full_available`, and `inference_full_available`. The inference field is null when inference is disabled; other fields are null until their milestone is reached. With the Cargo feature `detailed-profiling`, additional preparation, search, sink, storage, and build phase timings are compiled and reported for diagnostics. Ordinary builds omit these detailed fields and their clock reads. The CLI and desktop feature flags forward to the core feature.

With `--auto-index --diagnostic`, the final event includes `automatic_index_paths`, sorted by path. Each entry reports admission status, reason, observed and indexed kind, multivalue state, B-tree value/length key counts and coverage. The same typed report is available to library callers through `Repository::inference_index_diagnostics(collection)`. A built value dictionary does not imply every query can skip verification; multivalued and partially covered short-string paths retain their coverage rules.

In multi-collection mode (`--repo` or repeated `--path`), `--auto-index --diagnostic` reports these paths under `automatic_index_collections`, keyed by collection name.

```sh
obj --path examples/musenalm --load-index repository --auto-index --diagnostic |
  jq -r 'select(.type=="run_complete") | .automatic_index_paths[] |
    select(.index_status=="Built" and .value_keys>0) |
    "\(.path|join("."))\t\(.kind)\tkeys=\(.value_keys)\tmulti=\(.multi)"'
```

Library callers can inspect the same report without creating a CLI process:

```rust
for path in repository.inference_index_diagnostics("files").unwrap_or_default() {
    if path.value_keys.unwrap_or(0) > 0 {
        println!("{:?}: {:?} value keys", path.path, path.value_keys);
    }
}
```

## Index persistence

An ordinary invocation builds in memory and ignores saved indexes. `--load-index` checks only the selected target; on clean absence it reports the absence and builds in memory. A damaged or incompatible saved index is an error. Loading does not write index files. Saved segments are mapped and read on demand.

`--flush-index` saves one complete publication after construction and source reconciliation, before query output. It does not change the active index backing or enable automatic saves. A successful flush returns the saved publication revision and the destination's independent store generation. Later changes remain in memory until another explicit flush. Closing, Ctrl-C, and repository replacement do not save automatically.

| Target | Managed location |
| --- | --- |
| `temporary` | A stable per-user namespace under the operating system temporary directory. |
| `repository` | The repository's `.objekte/.indexes` directory. |
| `custom` | An Objekte-owned namespace under the selected base directory. |

For example, build and save with `obj --path ./data --flush-index repository`; later query it with `obj --path ./data --load-index repository --fts Goethe`. To copy a snapshot, use `--load-index temporary --flush-index custom --flush-index-directory /mnt/index-storage`. Independent custom load and flush directories use their respective directory options. Existing compatible destinations receive an atomically published newer generation; unrelated files and source documents are preserved.

## Memory and exit behavior

All-results execution performs one query traversal and sends bounded ordered batches through synchronous backpressure. It does not repeatedly ask for pages or retain all document bodies. The default stream settings bound each event to 16 MiB and buffered result work to 64 MiB; one record exceeding the event limit can fail explicitly. A computed sort must first evaluate all needed keys before emitting its first row, and key/target state is subject to the core's 64 MiB key and 128 MiB computed-sort budgets. A finite `--limit` keeps bounded top-k selection. The resident index and any individual source record add to process memory; these limits are not a total RSS guarantee.

Exit status is `0` for success and zero matches, `1` for query/input/output errors, `2` for invalid arguments or collection/index startup failures, and `130` for Ctrl-C cancellation. A downstream broken pipe is treated as intentional consumer termination: the command stops without a false completion event and exits successfully.
