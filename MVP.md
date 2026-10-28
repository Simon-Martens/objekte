# Objekte MVP: local repository library and CLI

Objekte is a schema-driven editor for structured, file-based repositories. This rebuild uses a Rust library and a command line interface first. A native Rust desktop UI calls the same library after the repository, query, and search behavior is correct and measured.

The files in a repository are authoritative. **All reads can fall back to those files, and all edits are made to those files.** The default Rust query path reads needed collections directly and reuses derived data in a bounded in-process cache. A disposable SQLite cache may accelerate hot views; it never becomes a second source of truth. The previous implementation and its MVP remain under `deprecated/` as references.

## Product boundaries

| Concern | Responsibility |
| --- | --- |
| Repository and collection configuration | Collection paths, source formats, identity, permitted operations, schema assignments, and search fields |
| JSON Schema | Stored structure, types, validation constraints, defaults explicitly declared by the schema, and semantic content annotations such as Markdown |
| View definition | Source collection, parameters, filter, selected fields, relation expansion, ordering, pagination, and optional grouping |
| UI options | Labels, visible columns and fields, controls, layout, formatting, and navigation presentation |

The schema must not encode a particular list layout. UI options must not change stored values or validation rules. A view selects data; UI options decide how to display its result. Views that return original records retain collection, ID, and source path so an editor can open the authoritative file. Aggregated or computed rows are read-only.

The library owns repository discovery, identity, validation, reference rules, safe writes, structural query execution, search, and structured results and errors. It must work for any configured JSON file collection, without collection-specific Rust structs or hard-coded field names. The CLI exercises the library's public API; it does not contain a separate implementation of repository behavior.

## PocketBase as a product reference

[PocketBase collections](https://pocketbase.io/docs/collections/) illustrate the organization we want: named collections of identifiable records, field definitions, relations, record operations, and read-only views. Its [record listing API](https://pocketbase.io/docs/api-records/) combines a filter with sorting, pagination, field selection, and relation expansion. Its [API rules and filters](https://pocketbase.io/docs/api-rules-and-filters/) show a concise expression style for comparing fields and combining conditions. Objekte adopts these **product ideas**, not PocketBase's SQLite storage, HTTP API, authentication model, access-control rules, or exact filter semantics. The original JSON files remain authoritative.

For Objekte, a collection is a configured set of JSON files with stable IDs, a JSON Schema, and optional declared references to other collections. The same generic library operations list, filter, sort, page, inspect, validate, create, update, and delete records in any such collection. A saved view is a query specification over a collection; a grouped or computed view is read-only. UI options control presentation separately from the view definition.

## How we chose the architecture

We began with the deprecated server/client application. It established useful schema-guided editing, separate UI options, and dynamic views, but the combined backend and web frontend made the implementation hard to reason about. We chose to prove repository and query behavior through a library and CLI before selecting a new UI. We did not measure which part caused the old application's perceived lag; the library-first design is a product and maintenance choice.

We then tested cache formats and query engines against repositories with very different shapes: Lenz, Musenalm's large nested `entries`, Musenalm's 133,948 standalone `contents`, and 200,000 small synthetic files. [Performance.md](Performance.md) records the workloads, exact results, and limits. The decision-relevant findings are:

| Option tried or considered | What it showed | Decision |
| --- | --- | --- |
| NDJSON snapshots with embedded qj/jaq; jq subprocess | qj beat jaq in the measured embedded queries, but both were slower than projected code for the shared view workload. Building the Musenalm `contents` NDJSON took 1.623 s; an existing snapshot still needs source reconciliation. Fresh-process jq reparses for each query. | Keep as reference or advanced-query tools. The MVP's smaller filter language does not require jq compatibility or a mandatory NDJSON cache. |
| Node arrays loaded from original files | Direct loading and hand-written queries were practical at Musenalm size. A runtime-projected Node run loaded `contents` in 1.467 s and sorted a 50-row page in 53.79 ms, using 192 MiB RSS. | A credible implementation alternative, but it does not require Electron; use it as a performance baseline. |
| Rust retaining full generic JSON trees | The first generic representation used much more memory. A sort measured at 775 ms was corrected to about 83 ms by extracting comparison keys once. | Avoid treating that representation or comparator as Rust's speed limit. |
| Rust runtime-selected field projections from original files | The matching `contents` run loaded in 1.365 s, sorted the page in 13.64 ms, and used 26 MiB RSS. | Preferred direct executor design. Its four measured operations are still hand-written; the general view API must be tested. |
| SQLite JSONB in memory | It imported original `contents` files in 1.139 s, but unindexed warm sort/page was 149.23 ms and import repeats at each start. | Prefer direct Rust execution for repeat views; retain the in-memory database as a measured alternative. |
| Disposable SQLite JSONB on disk with an expression index | An existing database reopened in about 1 ms and returned the indexed page in 0.04 ms. The index took 0.293 s to build; scanning source file stamps took about 0.23 s before trusting the cache. | Selected optional accelerator for measured hot views, with source reconciliation and direct fallback. |
| DuckDB, Turso, SurrealDB | Earlier exploratory runs showed strengths in typed analytics, build speed, or nested indexes, but used different workloads and did not settle generic view semantics or external-file synchronization. | Retain their measurements as alternatives; do not add them to the MVP stack. |
| Separate on-disk KV cache of records or result pages | Considered for default sorts, filters, and frequent queries. It can reuse results, but a new arbitrary query still scans without secondary indexes, and external edits require the same invalidation work. No equivalent on-disk KV benchmark has been run. | Use bounded in-process key-value reuse first. SQLite supplies optional persistent indexes when reuse alone is insufficient. |

This leads to one query contract with two execution paths: source-backed Rust for every supported view, and SQLite only when an equivalent indexed plan is available and its generation is current. All writes go to original JSON files. Default and saved views reuse sorted IDs and projected fields in memory; frequently repeated ad hoc views can be admitted by a bounded heuristic. Cache files are disposable, so schema or source changes can invalidate and rebuild them instead of requiring complex migrations or dual writes.

This is an architecture choice, not a claim that every arbitrary view already meets a latency target. The remaining gate is a generic implementation of nested paths, arrays, relations, grouping, and source-change handling, benchmarked through the same public API across the example and synthetic repositories.

## Three access modes

All three modes are MVP requirements. They have different semantics and explicit CLI/library entry points.

### 1. Structural views and filters

The MVP needs a **small, generic query model**, not the full jq language. Both ad hoc requests and saved views use the same query specification. It can list records from any configured collection, filter on its JSON fields, sort, paginate, select output fields, traverse declared relations, and return simple grouped results. The Rust library compiles this specification into a plan and executes it over original files or reusable in-memory projections. Supported hot views may use SQLite through the same API. qj, jaq, and jq remain benchmark/reference tools, not production requirements or user-facing languages.

The filter expression is deliberately PocketBase-like. Its MVP subset is:

| Construct | Meaning |
| --- | --- |
| `field = value`, `!=`, `<`, `<=`, `>`, `>=` | Typed scalar comparisons without implicit string/number coercion |
| `field ~ value`, `!~` | Case-insensitive literal substring match and its negation on strings; no SQL wildcards or regex in this mode |
| `field ?= value`, `?!=`, `?<`, `?<=`, `?>`, `?>=`, `?~`, `?!~` | At least one value reached through an array path satisfies the corresponding comparison |
| `&&`, `||`, parentheses | Boolean conjunction, disjunction, and grouping |
| `exists(field)` | Distinguish a missing field from a field whose stored value is `null` |
| `$parameter` | A typed value bound by the caller, never inserted into expression text |

Field paths use dot notation for ordinary JSON object keys; bracket-quoted keys address names containing dots or other punctuation. Paths can traverse nested objects and arrays. Unprefixed paths refer to stored record data; `@id` and `@path` refer to source identity. A scalar comparison on a path that yields multiple values is an error: the `?` form must be used. An empty array never satisfies a `?` comparison. Missing fields match no comparison, including `!=`; explicit JSON `null` matches `= null` and `!= null` as a value. Type-incompatible ordered comparisons do not match. The exact case-folding algorithm and sort collation must be pinned and tested so results do not depend on the execution engine. These semantics are **Objekte's subset**, not a claim of PocketBase syntax compatibility; PocketBase applies different default behavior to array-like paths.

The `?` prefix means **any**: `tags ?!= "poet"` is true when at least one tag differs from `"poet"`, which is not the same as saying that no tag equals `"poet"`. The implementation must test this distinction.

Parse and validate a filter once, resolve its field paths and bound parameters, then reuse the compiled form while scanning records. The `~` operator checks a decoded JSON string field; it does not strip Markdown or HTML and does not search property names. Raw grep and cleaned-content search remain separate access modes.

A view specification keeps filtering separate from the other operations. For example:

```json
{
  "source": "people",
  "filter": "birthYear >= $from && tags ?= \"poet\"",
  "parameters": {"from": 1800},
  "sort": ["birthYear", "@id"],
  "fields": ["name", "birthYear"],
  "page": {"limit": 50, "offset": 0}
}
```

The MVP also supports relation expansion and filters through declared references using dotted paths such as `author.name`, with an explicit depth/cycle limit. A path through a multi-record relation uses the same `?` operators as an array. A grouped view specifies field paths to group by and simple `count`, `min`, `max`, or `sum` aggregates; it returns computed, read-only rows. A normal record view retains collection, ID, and source path even when output fields are selected, so the original record can be opened. Paging has a deterministic `@id` tie-breaker, and requesting a total count is optional because full counts can require an extra scan. Saved views and ad hoc views have the same capabilities; neither depends on a collection-specific Rust struct. More operators or arbitrary computed expressions can be considered later based on actual view requirements.

### 2. Raw grep

Use ripgrep's Rust crates against the authoritative source files. This mode matches bytes/text as stored, including JSON keys, markup, frontmatter, and escape sequences. Results identify source path, line, and match. Offer literal and ripgrep-regex modes with explicit case behavior.

### 3. Cleaned-content search

Search text derived from configured record fields, excluding JSON property names and repository metadata. Markdown fields contribute rendered text without markup syntax or link destinations; plain fields contribute their decoded text. HTML and XML cleaning can be added after their expected text semantics are defined. This mode is suitable for ordinary word search and can also accept ripgrep regexes over the cleaned text. It is distinct from raw grep: a pattern against source syntax is not expected to match here.

Start with direct extraction from the configured source fields: parse a record, normalize only the selected field values, and apply ripgrep's matching crates to the resulting text. Return collection, ID, path, field, offset, and excerpt. Flatten line breaks and tabs consistently. Search fields come from collection/view configuration; schema content annotations guide cleaning. Search presentation belongs to UI options. Benchmark this end-to-end path before considering a generated text corpus.

## Rust execution and cache tiers

Opening a repository reads configuration and schemas; it does **not** load every collection. A view reads only its declared source collections. Opening or editing a record uses its original file. When IDs are filenames, a known-ID read can construct the path; other layouts may require an ID-to-path catalog. A first global filter, sort, group, or aggregate over an unindexed collection may have to inspect every record. Loading is lazy by collection and view, not a promise that a global view can avoid a collection scan.

The direct Rust executor compiles filters and field paths once per view. It extracts the fields and relation IDs that the view needs into compact runtime-selected projections, retains source identity, and streams when the operation allows it. Sorting retains compact keys and IDs; grouping retains group state. A new view may request any configured JSON field, so a projection for an earlier view cannot restrict what later views can query.

The default in-process cache has two bounded layers:

1. **Record/projection lookup:** key by repository, collection, ID, source revision, and the projection's field set. Reuse parsed fields while the source version is current. Do not duplicate full JSON trees for every view.
2. **View results:** key by the canonical view definition, bound parameters, query-semantics version, and generations of all source collections and relations it depends on. Retain IDs, sort keys, counts, and small computed rows rather than copies of full records. Cache default and saved view sorts/filters after their first execution. Admit frequently repeated or expensive ad hoc views with a measured cost-and-frequency heuristic and evict by a fixed memory budget. A parameter change makes a different key.

This key-value style reuse accelerates repeated views without a disk cache. It does not make an unseen filter or sort instant: a new global view still needs a scan or an index. Cache admission and eviction affect speed only, never results.

**SQLite is the selected optional disk accelerator for hot views.** Keep one disposable database per repository, or per collection if measurements favor it, in the operating system's user cache directory. A cache row contains collection, ID, source path/revision, and JSONB data. Add expression indexes only for measured hot filter/sort paths and any relation lookup tables justified by real views. The Rust view API decides whether an equivalent SQLite plan is available; otherwise it executes directly over source files. Query semantics for missing fields, arrays, nulls, sorting, and relations must match the direct executor. Do not make SQLite a mandatory startup step or store original edits in it.

The included Musenalm example has **148,486 JSON files and about 358 MB of JSON source bytes**, including 133,948 files in `contents`. Lenz has **3,263 JSON files and about 1.4 MB**. The [corrected benchmarks](Performance.md#28-october-shared-workload-correction) show that runtime-selected Rust projections can be fast, while an indexed SQLite cache can make repeated pages very fast. They do not yet measure the complete general view API. The first cache build, warm queries, memory, source inventory scan, and external edits must all be included in the decision for each collection/view.

### Cache freshness and failure behavior

One library write atomically replaces the authoritative file, then invalidates its in-memory record and every dependent view. Failure to update a derived cache never rolls back a successful source-file edit. A SQLite cache is built as a new generation and published only when complete; the first implementation may rebuild an affected collection after a change rather than maintain complex incremental indexes. Targeted row refresh is a later optimization if rebuild cost requires it.

At startup and after a repository pull or another suspected bulk change, reconcile source paths and file stamps before trusting a persisted SQLite generation. A long-running UI uses file events to invalidate affected collections promptly and also reconciles on return to the app or explicit refresh, since watchers can miss events. If source freshness is uncertain, use the direct executor or rebuild the affected cache before returning an accelerated result. A source change invalidates dependent in-memory view results; a schema, collection configuration, reference definition, or query-semantics change invalidates affected compiled plans and SQLite generations. A changed SQLite layout is discarded and rebuilt, not migrated as authoritative data. Deletions and malformed JSON are reported from source and cannot be silently hidden by an old cache.

The cache is safe to delete at any time. The MVP includes cache status/rebuild controls and verifies that direct and accelerated paths return the same ordered results before treating a view as eligible for acceleration. A separate on-disk KV cache of arbitrary result pages is unnecessary initially: it would share the same invalidation problem while lacking SQLite's secondary indexes.

## Repository correctness

The library must provide collection discovery, stable identity and ID uniqueness, direct record reads, JSON Schema validation, schema-assigned documents, and safe create/update/rename/delete operations. Writes use atomic replacement and conflict detection against the source revision. Schema-declared references are resolved and checked before destructive operations. Embedded values and referenced records retain their different storage semantics. Missing and null values remain distinct, and fields hidden by UI options are preserved.

Validation and writes operate on authoritative files. Query results can open an original record; computed or aggregated rows are not directly editable.

## CLI acceptance path

The initial CLI should support commands equivalent to:

```text
objekte validate
objekte get people goethe
objekte list people --filter 'birthYear >= $from && tags ?= "poet"' --param from=1800 --sort birthYear,@id --limit 50
objekte list people --filter 'author.name ~ "goethe"' --expand author --fields name,birthYear
objekte view run people-by-birthplace
objekte grep 'birthYear'
objekte search 'Weimar'
objekte search --regex '(?i)weimar|jena'
objekte cache status
objekte cache rebuild --collection people
```

The complete acceptance path is: open repository → validate → run an ad hoc record list and a named grouped view over source files → repeat a default view and exercise in-process reuse → raw grep → cleaned search → edit a source record → repeat all three access modes and observe the change. Build an optional SQLite generation for a hot view, compare its complete ordered result with direct execution, then edit a source record and confirm invalidation/fallback or refresh before reuse. Test external edits, Git pulls or bulk replacements, schema/config changes, deletions, malformed files, interrupted cache builds, and deletion of the entire cache directory. A cache miss or cache failure must still leave source-backed operations usable.

## General benchmark and performance decision gate

Musenalm approximates the expected final repository size, but its field names and relationships are **example data, not application logic**. The benchmark runner must call the same generic `list`/`view` library API used by the CLI. It accepts a repository and declarative workload file; it contains no special-case code for Musenalm fields such as `year`, `preferred_title`, or `agents`. Workload files may name different fields for different repositories, but exercise the same operation categories and verify complete result values and ordering.

The reusable workload covers ID lookup, scalar filters, array and nested-path filters, relation expansion/filtering, case-insensitive contains, multi-key sort, pagination with and without total counts, field selection, grouping/aggregates, raw grep, and cleaned search. Use Musenalm including `contents`, Lenz, and generated repositories with varying record counts and sizes. Synthetic fixtures must include nested objects, arrays, references, missing fields, explicit nulls, and strings with punctuation in field names. Do not make a single Musenalm query the deciding benchmark.

Measure cold and warm first-view latency, repeated queries, source enumeration, JSON reading/parsing, query evaluation, result serialization, peak memory, and edits made by another program. Compare direct Rust execution, default in-process reuse, and optional indexed SQLite against the **same query semantics and complete outputs**. Include query-plan compilation, projection/cache build, index build, source reconciliation, invalidation, and rebuild time separately from warm execution. Measure cache hit rates and memory under several default, saved, and parameterized ad hoc views; tune admission and eviction from those workloads. qj, jaq, jq, and Node remain reference baselines, not selected engines. Fixed-shape projected loops are useful lower bounds but cannot substitute for the generic library benchmark.

## Later UI

After the CLI path is correct and fast enough, build a local native Rust desktop UI over the same library. Preserve schema-guided recursive browsing and editing of objects and arrays, reference navigation, and configurable views. Run source scans, cache reconciliation, and expensive queries away from the UI thread. The exact Rust UI toolkit is a later choice. The MVP does not require a web server, browser frontend, accounts, or network service.
