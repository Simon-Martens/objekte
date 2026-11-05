# Objekte Rust library API and extension contracts

This is the external consumer reference for the current `objekte-core` checkout (crate version `0.1.0`). It describes the public Rust surface that another statically linked crate can call or implement. Here **ABI** means the Rust library contract: types, methods, traits, ownership and semantic identities. Objekte does not expose a C ABI, a stable binary layout, a dynamic plugin loader, or a promise that a compiled extension can be loaded into a different executable. Recompile downstream crates with the compatible `objekte-core` version. The source of truth for exact compiler signatures is `crates/core/src/lib.rs` and its public `bigram`, `extensions`, and `file_picker` modules.

All examples use `objekte_core` as the crate name. Unless shown otherwise, `Result<T>` means `std::result::Result<T, objekte_core::Error>`. `&str` inputs are borrowed for the call; `Path` inputs are filesystem paths; `serde_json::Value` is the JSON-compatible logical value type. Methods taking `&mut Repository` may warm indexes, reconcile sources, update metrics, or populate caches. They do not write source records. Methods returning `Option<T>` distinguish an absent/unavailable value from an error.

## Contents

1. [Vocabulary and scope](#vocabulary-and-scope)
2. [Architecture and data structures](#architecture-and-data-structures)
3. [How a search runs](#how-a-search-runs)
4. [Extension surface and built-in behavior](#extension-surface-and-built-in-behavior)
5. [Common repository workflows](#common-repository-workflows)
6. [Opening and registering](#opening-and-registering)
7. [Ordinary queries and views](#ordinary-queries-and-views)
8. [Indexes, snapshots, refresh and inspection](#indexes-snapshots-refresh-and-inspection)
9. [Extension interfaces](#extension-interfaces)
10. [Public helper types and methods](#public-helper-types-and-methods)
11. [Low-level bigram and JSON picker APIs](#low-level-bigram-and-json-picker-apis)
12. [Complete consumer workflows](#complete-consumer-workflows)
13. [Errors, lifetime rules and compatibility](#errors-lifetime-rules-and-compatibility)

## Vocabulary and scope

Objekte is a source-backed search library. The source files or provider resources remain authoritative; indexes and cached results are derived and disposable. The following terms are used throughout this reference:

| Term | Meaning |
| --- | --- |
| **Repository** | A live `Repository` handle over a root directory, resolved configuration, collections, registered definitions, indexes and process-local caches. It never edits source records through query APIs. |
| **Collection** | A named search scope. Default discovery groups JSON files by folder, while an optional schema can make a folder and its descendants one collection. Registered providers can contribute records to the same scope. |
| **Source** | The bytes selected and read by a `SourceProvider`: normally a `.json` file, but a registered provider can choose another file or resource. The current decoder contract produces one logical record per source. |
| **Source descriptor** | A `SourceDescriptor` containing collection, provider identity, source identity, record ID, locator, format identity, revision and declared metadata. Its locator says *where to read*; its record ID says *how to identify the result*. Neither is inferred from the other for a custom source. |
| **Logical record** | The object/array/scalar fields exposed by a `FormatDecoder`. A decoder may lend values from its input buffer; only result APIs that need an owned `Record` materialize a JSON-compatible tree. A JSON Schema, when present, describes these logical fields, not the source file's encoding. |
| **Field path** | An address into a logical record, such as `title`, `authors[0].name` or `authors[*].name`. Extension decoders visit JSON-pointer paths such as `/authors/0/name`; query binding resolves the requested path. |
| **Source format** | The identity used to choose a `FormatDecoder`, for example `objekte:json` or a registered Markdown-file decoder. This differs from a *field text format* such as `x-full-text: markdown`, which controls how one string is rendered for search. |
| **Text representation** | One defined view of a logical string: stored decoded text or visible normalized text, for example. The representation used to build a text posting must agree with the one used for exact verification. |
| **Typed value** | A null, Boolean, exact decimal number or string used for scalar filtering and ordering. A `ValueExtractor` can derive zero or more typed values from a record. Missing, explicit null and a present path yielding no values are different states. |
| **Slot** | A compact integer position for one source or record within an index generation. Posting lists, bitsets and cached orderings contain slots, not source bytes. A slot has meaning only with its matching source table and generation. |
| **Posting** | An index entry mapping a necessary text feature or a typed key to the slots that contain it. A posting reduces source reads; it is not automatically proof of an exact text match. |
| **Candidate** | A source that an index has not ruled out. A conservative candidate set may include false positives. An *unsupported* constraint means scan or use another safe condition; an empty supported set means no candidates. |
| **Exact verification** | Reading the source and evaluating the complete predicate under the bound field, text and error semantics. It is required for approximate text candidates and for any predicate the index cannot prove exactly. |
| **Generation** | The maintained identity of a source inventory and bound semantics. Cached slots and selected postings must belong to the same generation. Refresh or relevant configuration changes invalidate incompatible state. |
| **Snapshot** | A versioned on-disk copy of derived index state under `.objekte/.cache`, or at an explicit path. It is validated before restoration and is rebuilt on an incompatible cache miss. |
| **Query cache** | A bounded, process-local cache of verified membership and/or ordered slot lists. It does not store document contents and is separate from index snapshots and the operating system's file cache. |

The two **ordinary query families** are `JsonQuery` and `TextQuery`. `JsonQuery` evaluates a structured filter over logical fields, metadata and declared relations; it can include a field-level `:fts` comparison. `TextQuery` searches included scalar leaves for a text pattern and may add a structured filter. `query_derived` is a narrower typed-extractor API, and `query_frontend` runs an application-defined compiled plan. These use the same repository ownership of source selection, ordering, pagination and cache publication. They do not require four separate source stores.

## Architecture and data structures

The repository separates **discovery**, **projection**, **candidate selection**, **exact execution** and **result delivery**. Extensions can define inputs and semantics at those boundaries; the engine keeps the indexes, source generations, scheduling, caches and public result order.

```text
repository root + schemas + registrations
        │
        ├─ collection/source selection ──► source descriptors + generation
        │                                   (provider, format, ID, locator, revision)
        ├─ index build: read → decode/project → bigram and typed postings
        │                                   + presence and sort structures
        └─ query: compile/bind → safe candidates → exact verification
                                          → count/order → requested page
                                          → reread/materialize page records
```

There are two principal index families:

1. A **bigram index** answers “which slots could contain this text?” It stores byte-pair features in compressed posting columns and is deliberately approximate. The native picker keeps a general stored-text filter and, when needed, separate normal/visible filters for declared full-text fields. Selected custom fields can use engine-owned representation postings with the same bigram builder.
2. A **typed ordered index** answers safe scalar comparisons and ranges by typed key. Its existing B-tree-backed `TypedIndex` maps each key to source slots. Native schema fields, runtime JSON-path postings and registered derived values all use this implementation. Exact decimal comparison avoids converting numbers to floating point.

Presence bits, sort keys and modification order are additional structures; they are not a third general text or value index. They answer “does this path exist?”, “what is this slot's sort key?” and “which live slot comes next by default?” respectively.

| Internal structure | Contents and purpose | Lifetime or invariant |
| --- | --- | --- |
| `RepositoryModel` and `Collection` | Resolved root, collection boundaries, schema fields, views, mappings and relations. | Configuration is loaded at open/reload, not parsed for every character or source value. |
| Native `FilePicker` | A byte arena of IDs, `Vec<Option<FileEntry>>` stable slots, a sorted live-ID view, stamps/revisions, tombstones, `ModifiedOrder` and index members. | The concrete path for JSON-only scopes. A deleted slot is not silently reassigned while old postings still refer to it. |
| Selected `CustomSourceSnapshot` | Shared `Arc<Vec<SourceDescriptor>>`, selection fingerprint, source-generation fingerprint, cacheability and applicable definition descriptors. | Used when the scope contains custom records. A provider rule matters even if it currently returns no records. The source table and cached slots are pinned to one generation. |
| `BigramFilter` and `TextIndex` | Compressed dense or sparse bitset columns for observed adjacent and skip-one byte pairs; optional per-field stored and visible text filters. | A bit says “possible match”, never “verified text match”. Encoding and representation are part of index identity. |
| `BigramOverlay` and changed-slot state | New text features for edited/inserted native JSON slots plus deletion tombstones beside an immutable base filter. | Changed records remain visible before compaction/rebuild; stale base bits cannot make a deletion a result. |
| `TypedIndex` and `PresenceIndex` | `BTreeMap<TypedKey, Vec<slot>>` posting maps for null, missing, Boolean, exact number and string keys, with path-presence and retained per-slot old keys. | Safe scalar predicates can be exact. Refresh removes old keys from retained state before inserting new keys, without rereading overwritten old source bytes. |
| `SortIndex`, `ModifiedOrder`, `SortBuffer` | Slot-major collation/sort keys and precomputed orders for declared native indexes; newest-first default order; a bounded best-prefix buffer for unsupported sorts. | Stable collection and ID tie breaks remain part of result ordering. A fallback sort need not retain whole record contents. |
| `VerifiedPredicateCache` | Generation map and `evaluated`/`matched` slot bitsets for reusable predicate work. | A known false is distinct from an unevaluated slot. Complete membership is published only after successful verification. |
| `OrderedResultCache` | Ordered `Vec<u32>` slot lists with `Complete`, `Prefix` or `CappedPrefix` coverage, keyed by predicate and sort semantics. | A complete hit slices only the requested page; offset and limit do not create a different ordering. Records are reread for the returned page. |
| Index snapshots | Native base bigram snapshot and secondary typed/presence/sort sidecar; versioned selected inventory and selected posting metadata. | A source, encoding, schema, format or extension-semantic mismatch is a cache miss or error, never permission to reuse incompatible slots. |

The native index build reads each required JSON source once and uses its Sonic parse for fused text, presence, typed and sort projection. It does not allocate a canonical `serde_json::Value` tree merely to call extension traits. A selected build reads and decodes each source once per necessary pass and feeds the requested compatible representation and extractor outputs from that access. The engine allocates reusable scratch per worker and does not dispatch through a trait object for each byte or bigram of the native scanner.

### What the bigram index proves

For a logical string such as `archive`, adjacent byte pairs include `ar`, `rc` and `ch`; a skip-one pair links bytes with one byte between them. An index column is a bitset of slots containing a pair. Intersecting required columns can reject many sources quickly, but a source with the same pairs in another order is a false positive. Every retained text candidate is checked with the exact scanner. Features are generated within one logical value: unrelated fields or array members are not concatenated into an exact-match stream.

`BigramEncoding::Ascii` uses printable ASCII pairs; `Utf8Bytes` uses UTF-8 **byte** pairs, not Unicode scalar pairs. ASCII case normalization and separate Unicode-folded projections support safe candidate planning for case-insensitive modes. A query must use the posting for its actual representation: raw stored bytes do not safely narrow a markup-aware visible-text match unless a specific compatibility proof permits it. Short patterns and broad regexes may have no safe pair and therefore scan. A compressed column's dense/sparse layout changes memory and speed, not match semantics.

### What the typed index proves

`TypedKey` separates null, missing, Booleans, exact decimal numbers and strings. A B-tree key range can supply exact membership when the bound comparison and path cardinality make that safe; otherwise it supplies a necessary condition or execution falls back to source evaluation. `exists(path)` includes explicit null and present empty containers; missing means the path is absent. Ordinary scalar comparisons can report an error when a path reaches multiple values, while existential `?` comparisons can accept any reached value. A flattened posting must not hide a cardinality error or assume two values came from the same array element. Registered extractors have the same distinction between `Missing`, `Present` with zero values, explicit `Null`, multiple values and `Err`.

Sorting is related but separate. Declared native sort indexes retain ICU collation keys and ordered slots. A supported indexed order can be traversed directly. Other sort requests use bounded `SortBuffer` selection; sorted results still use the same stable tie rules. The default unsorted order is by collection, then descending source modification time with record ID breaking ties. Requested `offset` and `limit` select a page from that order. `Options.total_limit` caps the accessible match sequence and sets `truncated` when more matches are proven.

## How a search runs

The path below describes the ordinary APIs. A registered frontend supplies a `LogicalPlan` at step 1 and then hands execution back to the engine. A JSON-only scope keeps specialized native execution; a scope containing custom records uses the selected descriptor table and logical record access.

| Stage | Structured `JsonQuery` | Full-text `TextQuery` | Shared rule |
| --- | --- | --- | --- |
| **1. Parse and bind** | Compile filter syntax, parameters, metadata/relations, `:fts` mode and optional sort. | Compile word/raw/regex pattern, optional field selection and optional JSON filter. | Validate scope and page bounds. Bind schema annotations and only the participating extension definitions once. |
| **2. Pin sources and freshness** | Select requested collections and their current native picker or selected inventory. | The same source routing. | `MetadataOnOpen` uses maintained state until explicit refresh; request-time policies perform their required revision/content validation. Query execution must not mix source slots from generations. |
| **3. Check reusable results** | Reuse a compatible verified predicate or completed ordered list when permitted. | Reuse a compatible text membership/order when permitted. | Keys include compiled terms, scope, bound semantics and source generation. A page change can reuse one complete order. This cache contains no source bytes. |
| **4. Plan candidates** | Combine safe typed, presence, text and metadata conditions; some indexed predicates prove exact membership. | Use the posting for the chosen raw/visible representation and any compatible filter conditions. | Approximate candidates include changed/unindexed slots. An unsupported hint means scan. `AND` may use a necessary condition; `OR` needs coverage of every branch; `NOT` cannot complement an approximate set. |
| **5. Verify and count** | Read candidates as needed, evaluate the full filter, preserve missing/null/cardinality and relation behavior. | Read candidates, traverse included scalar leaves, render each logical value and apply the exact matcher; collect matched field/path data. | Errors propagate. Native safe exact postings may count without reading every matching file; approximate candidates must be verified. |
| **6. Order and select a page** | Use default modification order, a compatible native sort order or bounded fallback. | Apply the same ordering and requested sort terms to text hits. | Count and truncation are computed under `total_limit`. Offset/limit are checked and select only the requested range; stable ties remain stable. |
| **7. Materialize and publish** | Return `Record` values with status, metadata and optional original bytes for a malformed format. | Return `TextHit` values containing records plus canonical fields and concrete matched paths. | Reread/check returned-page sources. Publish a complete cache entry only after successful evaluation; failures or cancellation cannot turn partial work into a complete result. |

A `JsonQuery` is a field/metadata query, even if one field uses `:fts`. Its ordinary `~`/LIKE behavior is distinct from `:fts` text comparison. A `TextQuery` searches across included leaves by default: `Word` matches visible normalized text, `WordRaw` matches stored decoded text, and `Regex` runs a Rust regex on stored text with flags in the pattern. Markdown/HTML/XML rendering is chosen per declared field, not by treating an entire source file as markup. The optional `TextQuery.filter` narrows records before text matching. Results from both families can be sorted, counted, paged and streamed.

A custom operator's candidate hint says only that excluded values cannot **match**. Evaluation of an excluded value may still fail, so `CompiledOperator::can_skip_non_candidates()` defaults to `false`. A custom operator must explicitly promise safe skipping before either registered text search or frontend execution prunes its evaluation. Boolean short-circuit order and errors remain observable; an error is not converted into an empty result. Native operators retain their concrete optimized path.

### Three distinct caches and freshness policies

1. **In-memory indexes** hold bigram, typed, presence and sort postings. They are built lazily or by explicit warm/rebuild calls and may have a changed-record overlay.
2. **Disk snapshots** save compatible derived index state under `.objekte/.cache`. They speed later opening but are never authoritative. Native JSON snapshots retain their own format; selected postings carry versioned inventory and semantic metadata. `save_bigram_cache` saves the ordinary repository-managed cache, including active selected postings when present.
3. **The query-result cache** retains process-local predicate bitsets and ordered slot sequences within `Options.query_cache_bytes`. A complete ordered hit does page-range work plus page materialization, not an inventory or full-match copy under `MetadataOnOpen`. A verified hit may still read returned records. Streaming does not reuse this cache unless `stream_query_cache` is enabled.

The operating system's page cache is a fourth, external effect on timing; Objekte does not call a warm-index query a cold physical-storage read. The freshness setting determines when cached *source knowledge* is checked:

| `Freshness` | Selection and query behavior | Application responsibility |
| --- | --- | --- |
| `MetadataOnOpen` (default) | Inventory/stamps are established at open or first index use. Later compatible query-cache hits do no new collection inventory solely for lookup; returned pages still receive the existing source checks. | Call `refresh(collection)` after source edits, additions, deletions or renames. Use `reload()` or `refresh_configuration()` after configuration/schema changes. |
| `MetadataOnRequest` | Re-enumerates/reconciles required source metadata before requests and cache reuse. This can cost collection work even for a tiny cached page. | Keep provider revisions and metadata accurate. Explicit refresh remains available. |
| `HashOnRequest` | Rechecks content on requests and bypasses verified-query-cache reuse; current index work may be repeated. | Choose this when request-time content checking is required, accepting its cost. |

A default filesystem JSON read stays within its configured root, uses descriptor-relative secure opening/no-follow behavior and the documented before/after revision checks. A custom provider implements its own bounded read and consistency policy; the engine checks the selected descriptor and propagates an unstable read or decoder failure. The repository does not watch files or rewrite source records. Refresh and reload invalidate affected membership, ordered results and selected postings by maintained generations; they do not scan every document merely to construct a cache lookup key.

## Extension surface and built-in behavior

An extension is statically linked Rust code registered on `RepositoryBuilder` **before** opening. Repository files do not authorize execution of extension code. The application author provides semantics at one or more boundaries; the engine retains source inventory ownership, index storage, candidate combination, exact query scheduling, result ordering, pagination, cache budgets and publication.

| Programmer-supplied contract | What the implementation decides | What the engine still does |
| --- | --- | --- |
| `SourceProvider` | Which sources belong to a collection, their stable identities, locator, format, revision and metadata; how to read a chosen descriptor. | Merge providers with built-in JSON discovery, reject collisions, retain an inventory/generation and select records for ordinary queries and refresh. |
| `FormatDecoder` | Which source-format identity it accepts and how bytes expose one borrowed logical record with fields and path presence. | Validate format selection, call it during record access and materialize owned results only where required. |
| `TextRepresentation` | Which bound field it applies to and how one logical string becomes stored or visible text, borrowing input or using reusable output. | Build compatible bigram postings, choose safe candidate constraints and perform exact field traversal. |
| `ValueExtractor` | Which record/schema binding it applies to and which zero, one or many typed values it emits, with missing/null/error semantics. | Use the existing typed B-tree for optional derived postings, filter, count, sort, refresh and snapshot compatibility. |
| `QueryOperator`/`CompiledOperator` | Validate parameters once; evaluate an exact predicate over a prepared value; optionally declare a safe necessary candidate and whether noncandidates may be skipped without losing errors. | Supply the index, check representation compatibility, combine candidates conservatively and propagate failures. |
| `QueryFrontend` | Translate an application request into a `LogicalPlan` over registered handles, Boolean terms, scope, projection, sort and page. | Read/decode sources and execute the plan, including candidates, short-circuit behavior, ordering and caching. |

Definitions have a namespaced identity, semantic version, canonical configuration fingerprint, input/output kinds and declared dependencies. Binding can inspect retained resolved JSON Schema annotations and resolves identity strings to handles once. Stable descriptors, parameters, applicable provider/decoder selection and source generation determine reuse; registration order and pointer values do not. An unrelated volatile operator or extractor does not disable another query's cache. A volatile definition actually used by a query, index or snapshot prevents the reuse its contract forbids. Borrowed decoder values, text output and worker scratch cannot escape their callback; compiled definitions are immutable and shareable.

### What comes with Objekte

| Built-in capability | Scope and behavior | Registration detail |
| --- | --- | --- |
| Filesystem JSON source provider (`objekte:filesystem`) | Discovers regular `.json` files under default collection boundaries, supplies IDs without the suffix and secure locator reads. | Used by ordinary repository opening; not replaced by registering another provider. |
| JSON decoder (`objekte:json`) | Sonic-based logical JSON access. Native JSON indexing uses the fused one-read/one-parse projection instead of an owned generic tree. Malformed JSON has its specific metadata and standalone raw-text fallback. | Available as the built-in decoder/fallback for JSON descriptors. |
| Stored text (`objekte:stored`) | The decoded string exactly as stored; raw word and regex modes see this text. | A public `StoredRepresentation` adapter is available and is registered automatically when source providers require representation binding. |
| Visible plain/Markdown/HTML/XML (`objekte:visible-*`) | Per-field rendering for schema `x-full-text` declarations. Plain text normalizes spacing/punctuation; markup modes remove markup, decode entities and preserve inline joins/block boundaries. `exclude_fts` removes declared fields from standalone text search. | Public `RenderedRepresentation::new(TextFormat)` adapters are available; applicable source-provider openings install them. Ordinary native text search uses the same concrete rendering semantics without trait dispatch per character. |
| Native text scanner | `Word` is a case-aware literal over visible normalized text; `WordRaw` is a literal over stored text; `Regex` is a Rust regex over stored text and owns its flags. | Ordinary `TextQuery` uses concrete scanners. `NativeTextOperator::new(mode, case_sensitive)` is an optional public adapter for registered operator calls, not an automatically registered custom frontend. |
| Native JSON field and typed projection | JSON paths, metadata, relations, schema `index`/sort declarations and optional runtime JSON-path postings use existing exact evaluator and typed B-tree. | No `ValueExtractor` registration is required for stored JSON fields. A custom derived value does require its own extractor. |
| Query entry points | `JsonQuery`, `TextQuery`, named views and their streaming forms. | Already available through `Repository`; no `QueryFrontend` registration is needed for ordinary queries. |

**A Markdown, HTML or XML *field* renderer is not a Markdown, HTML or XML *file* decoder.** The built-in source file format is JSON. To index a `.md` file as a record, register a provider that selects it and a decoder that exposes its logical fields; `crates/extension-tests` contains the separate consumer example. There is no built-in YAML/TOML decoder, general archive reader, dynamic plugin loader or built-in application-specific query frontend. Registered extensions are trusted application code, not programs supplied by repository files.

## Common repository workflows

### Open a repository and choose options

`Repository::open(root)` is the shortest default JSON path. Use the builder to configure options or register extensions. `Options::default()` chooses memory indexes, `MetadataOnOpen`, ASCII bigrams, a 10,000-match cap, a 64 MiB process-local query-cache budget and no streaming cache reuse. A schema can override the effective bigram encoding for its collection.

```rust,no_run
use objekte_core::{Freshness, Options, Repository};

fn main() -> Result<(), objekte_core::Error> {
    let mut options = Options::default();
    options.freshness = Freshness::MetadataOnOpen;
    options.query_cache_bytes = 64 * 1024 * 1024;
    let mut repo = Repository::builder("my-repository")
        .options(options)
        .open()?;
    repo.warm_bigram_index("books")?; // optional: otherwise build/restore on first use
    Ok(())
}
```

For another source format, chain `register_source`, `register_format` and any field-specific `register_representation`/`register_extractor` calls before `.open()`. A source-only registration is incomplete if its descriptors name a format with no matching decoder. A JSON Schema can describe the decoder's logical fields; `x-full-text` on a `body` field selects visible Markdown/HTML/XML semantics independently of the file extension. The [complete custom-format workflow](#ordinary-custom-format-repository) below uses the separate consumer crate.

### Collect structured and text results

Use `JsonQuery` for field, metadata or relation predicates. Use `TextQuery` for a pattern across included scalar leaves; `fields` narrows the searched logical fields. Both default to all collections, offset zero, a page of 50, no total count and no explicit sort. Set `count: true` when a total is needed. A matching index is an optimization: unsupported candidates still execute by exact scanning.

```rust,no_run
use objekte_core::{JsonQuery, Repository, TextMode, TextQuery};

fn main() -> Result<(), objekte_core::Error> {
    let mut repo = Repository::open("my-repository")?;
    let books = vec!["books".to_owned()];
    let field_page = repo.query_json(&JsonQuery {
        filter: "year >= 1848 && exists(title)".into(),
        collections: Some(books.clone()),
        count: true,
        limit: 20,
        ..Default::default()
    })?;
    let text_page = repo.search_text(&TextQuery {
        pattern: "Goethe".into(),
        mode: TextMode::Word,
        collections: Some(books),
        fields: Some(vec!["annotation".into()]),
        count: true,
        limit: 20,
        ..Default::default()
    })?;
    println!("fields: {:?}; text hits: {:?}", field_page.total, text_page.total);
    Ok(())
}
```

`JsonQuery.filter` can contain `:fts` for a field-specific text comparison, while `TextQuery.filter` adds a structured predicate before leaf search. `Sort { field, direction }` requests explicit ordering; `offset` and `limit` choose a page. The `Record` in either result owns the returned logical data. `TextHit` additionally reports canonical matched fields and concrete array paths. A custom `query_frontend` is needed only for an application-defined request shape, not for a registered file format.

### Stream results and handle provisional output

`query_json_stream` and `search_text_stream` invoke a callback for numbered page records or hits. Ordinals start at **one**; they are emitted positions, not stable record IDs. The callback borrows its value for that call. Some routes verify in parallel batches or materialize a sorted/selected page before callbacks, so streaming is an output contract rather than a promise of immediate first-byte delivery. The final summary is returned only on success. If a later read, decoder, scalar-cardinality check, operator or callback fails, treat earlier output as provisional; the engine does not publish a completed cache entry. The `*_stream_updates` variants deliver replaceable page slices for interactive displays; rows may move until completion.

```rust,no_run
use objekte_core::{JsonQuery, Repository, TextQuery};

fn main() -> Result<(), objekte_core::Error> {
    let mut repo = Repository::open("my-repository")?;
    let request = JsonQuery {
        filter: "year >= 1848".into(),
        ..Default::default()
    };
    let summary = repo.query_json_stream(&request, |ordinal, record| {
        println!("{ordinal}: {} / {}", record.collection, record.id);
        Ok(())
    })?;
    println!("{} records returned", summary.returned);
    let text = TextQuery { pattern: "archive".into(), ..Default::default() };
    repo.search_text_stream(&text, |ordinal, hit| {
        println!("{ordinal}: {}", hit.record.id);
        Ok(())
    })?;
    Ok(())
}
```

By default, streams verify candidates under the ordinary freshness rules without reusing the verified query cache. Set `Options.stream_query_cache = true` for an interactive application that deliberately accepts the same cache reuse and explicit reconciliation responsibility as collected queries. `search_text_stream_with` is the registered-operator streaming variant. `query_frontend` and `query_derived` currently return collected pages; they do not have separate streaming entry points.

### Reconcile source changes and persist derived state

The repository does not watch the filesystem. Perform the source write in the application, then report it before relying on an on-open index or cached result. These operations change derived state; they do **not** create, edit, delete or move source files themselves.

| Situation | Call | Effect |
| --- | --- | --- |
| One known collection changed, including custom-provider files | `repo.refresh("books")?` | Re-enumerates/reconciles selected sources and native JSON files; invalidates affected query entries. Active custom selected postings currently rebuild as a collection. |
| Known native `.json` create/edit/delete/move paths | `repo.apply_source_changes(&[SourceChange::...])?` | Applies targeted picker updates without a full directory inventory. This specialized API rejects selected custom-provider scopes. |
| Need a native JSON change report | `repo.diff("books")?` or `diff_all()` | Compares native picker inventory and updates its state; moves appear as delete plus create. Use `refresh` for selected providers. |
| Schema, collection rules or other loaded configuration changed | `repo.refresh_configuration()?` or `repo.reload()?` | Rebinds affected configuration and selection; a new or removed collection may require `reload`. |
| Build indexes ahead of the first query | `warm_bigram_index`, `warm_text_index`, or their `*_all_*` forms | Restores compatible snapshots or builds missing native and selected outputs. |
| Force an index rebuild | `rebuild_bigram_index`, `rebuild_text_index`, or `reindex_bigram_cache` | Reconstructs applicable derived postings; `reindex_bigram_cache` also saves repository-managed snapshots. |
| Persist current derived indexes | `save_bigram_cache("books")?` or `save_all_bigram_caches()?` | Writes the native cache and active selected postings under `.objekte/.cache` for later compatible reopen. |

```rust,no_run
use objekte_core::Repository;

fn main() -> Result<(), objekte_core::Error> {
    let mut repo = Repository::open("my-repository")?;
    // After the application has changed sources in this collection:
    repo.refresh("books")?;
    repo.save_bigram_cache("books")?;
    Ok(())
}
```

For a native JSON-only scope, `apply_source_changes` is an **alternative** to full refresh when the application knows the changed paths. Report only events that actually happened, after the filesystem operation:

```rust,no_run
use objekte_core::{Repository, SourceChange};
use std::path::PathBuf;

fn main() -> Result<(), objekte_core::Error> {
    let mut repo = Repository::open("my-repository")?;
    let changes = [
        SourceChange::Created(PathBuf::from("data/books/new.json")),
        SourceChange::Changed(PathBuf::from("data/books/edited.json")),
        SourceChange::Deleted(PathBuf::from("data/books/removed.json")),
        SourceChange::Moved {
            from: PathBuf::from("data/books/old.json"),
            to: PathBuf::from("data/books/renamed.json"),
        },
    ];
    repo.apply_source_changes(&changes)?;
    repo.save_bigram_cache("books")?;
    Ok(())
}
```

For a custom-bearing collection use `refresh` rather than `apply_source_changes`. When a move crosses collections, reconcile both affected collections; when a provider creates or removes an entire custom-only collection, `reload()` updates collection membership. On `MetadataOnRequest` the engine performs its promised request-time metadata validation; on `HashOnRequest` it performs the stronger request-time source work and bypasses verified result reuse. `MetadataOnOpen` needs explicit refresh for external changes. A failed selected build or refresh never publishes partially valid postings under a new generation; a failed collection may remain unavailable until a successful refresh/reload. Save after successful reconciliation if later processes should restore the new index state.

## Opening and registering

### `RepositoryBuilder`

Call `Repository::builder(root)` when using extensions or nondefault options. Each registration method consumes and returns the builder, so calls can be chained. The registered implementation must be `'static`; the trait itself requires `Send + Sync`. Descriptor validation, duplicate identities, and ambiguity errors are returned before or during `open`.

| Function | Arguments and effect |
| --- | --- |
| `Repository::builder(root: impl AsRef<Path>) -> RepositoryBuilder` | `root` is the repository directory containing collection/configuration files and source data. Creates an unopened builder. |
| `RepositoryBuilder::register_source(source: impl SourceProvider + 'static) -> Result<Self>` | Adds a source selector/reader; its descriptor must have `Source → Source` kinds and a unique namespaced identity. |
| `register_format(format: impl FormatDecoder + 'static) -> Result<Self>` | Adds a decoder for selected descriptors; descriptor kinds are `Source → Record`. |
| `register_representation(representation: impl TextRepresentation + 'static) -> Result<Self>` | Adds a text rendering definition; descriptor kinds are `Text → Text`. |
| `register_extractor(extractor: impl ValueExtractor + 'static) -> Result<Self>` | Adds a typed extractor; descriptor kinds are `Record → TypedValues`. |
| `register_operator(operator: impl QueryOperator + 'static) -> Result<Self>` | Adds a compiled exact predicate; input is `Text` or `TypedValues`, output `Predicate`. Text-search entry points require a text operator. |
| `register_frontend(frontend: impl QueryFrontend + 'static) -> Result<Self>` | Adds a request compiler; descriptor kinds are `Query → Query`. |
| `options(options: Options) -> Self` | Replaces the repository cache/freshness/encoding/limit options. |
| `data_root(path: impl AsRef<Path>) -> Self` | Overrides the data root relative to the repository layout. |
| `without_default_schemas() -> Self` | Disables automatic discovery of default schemas. |
| `schema(base: impl AsRef<Path>, path: impl AsRef<Path>) -> Self` | Associates the schema at `path` with the collection base directory `base`. Paths are resolved during open. |
| `open() -> Result<Repository>` | Resolves configuration and registrations, validates selected source/decoder rules, and returns the live handle. |

Convenience constructors are `Repository::open(root) -> Result<Self>`, `Repository::open_with_schema(root, base, schema) -> Result<Self>`, and `Repository::load_repository(root, options) -> Result<Self>`. They accept paths with the same meanings above; `load_repository` supplies explicit `Options`. Use the builder for extensions.

### `Options`, status and configuration

`Options::new()` and `Options::default()` select `CacheMode::Memory`, `Freshness::MetadataOnOpen`, ASCII bigrams, `total_limit = 10_000`, 64 MiB verified-query cache, and `stream_query_cache = false`. Its public fields are:

| Field | Meaning |
| --- | --- |
| `cache: CacheMode` | `Memory` permits in-memory indexes/caches; `Disabled` disables that mode. |
| `freshness: Freshness` | `MetadataOnOpen` relies on explicit refresh after opening; `MetadataOnRequest` checks inventory/revisions on requests; `HashOnRequest` checks content and does not reuse verified query results. |
| `bigram_encoding: BigramEncoding` | Default `Ascii` or `Utf8Bytes`; a collection can override it in configuration. |
| `total_limit: usize` | Maximum accessible matches across all pages; exceeding it sets `truncated`. |
| `query_cache_bytes: usize` | Budget for verified predicate/ordered results; zero disables the query cache. |
| `stream_query_cache: bool` | Opts streamed requests into reuse of verified query results under the selected freshness policy. |

`Repository::model() -> &RepositoryModel` exposes resolved `root`, `collections`, `views`, and relationship `graph`. `RepositoryModel::collection(name) -> Result<&Collection>` resolves one collection or returns an unknown-name request error. `Collection::open(folder)` and `Collection::open_with_schema(folder, schema)` construct standalone resolved collections; `folder` is a JSON data folder and `schema` is its JSON Schema file. `Collection` exposes its source `path`, schema/UI paths, `recursive` selection, declared `fields`, `indexes`, sort/index paths, encoding override and semantic hashes. Treat these as resolved metadata; changing a returned struct does not mutate a repository.

`Repository::extensions() -> Option<&Extensions>` returns the immutable registry if present. `Extensions::is_empty()`, `descriptors() -> Vec<&Descriptor>`, `fingerprint() -> String`, and `bindings() -> ExtensionBindings` inspect registrations. A standalone `Extensions::default()` also offers `register_source`, `register_format`, `register_representation`, `register_extractor`, `register_operator`, and `register_frontend`; each takes `&mut self` and an implementation of the named trait, returns `Result<()>`, and applies the same descriptor validation. The builder is the repository registration route; there is no public method that attaches a separately built registry to an already opened handle. `fingerprint` covers the whole registry for inspection; query cache identity uses only bound dependencies.

## Ordinary queries and views

### JSON field queries

`Repository::query_json(&mut self, request: &JsonQuery) -> Result<JsonQueryResult>` evaluates a compiled field expression against the requested scope. `JsonQuery` is `Default` and has these arguments:

| Field | Meaning |
| --- | --- |
| `filter: String` | JSON filter expression, including comparisons, Boolean expressions, existence, metadata and relation syntax. `JsonFilter::compile` can check it separately. |
| `parameters: BTreeMap<String, Value>` | Typed named parameter values referenced by the filter. |
| `collections: Option<Vec<String>>` | `None` selects all collections; `Some([])` is an empty scope; names select specific collections. |
| `sort: Vec<Sort>` | Ordered terms; `Sort { field, direction }` names a field or supported metadata key. Empty uses the query's default order. |
| `offset`, `limit: usize` | Zero-based page start and maximum returned records. `limit` must be positive and within supported bounds. Changing the page does not split a compatible completed ordered cache entry. |
| `count: bool` | Request a total. When false, result `total` is `None`. `truncated` reports the repository count cap. |
| `backend: JsonQueryBackend` | `Auto` (default), `Direct`, `Grep`, or `Bigram` for native execution selection. Mixed custom scopes use the selected logical-record route for correctness. |
| `fts: FtsOptions` | Legacy `mode: TextMode` and `case_sensitive`, plus `search_options: TextSearchOptions` for explicit syntax/case overrides on registered `:fts` definitions. Ordinary non-FTS LIKE retains its separate meaning. |

`JsonQueryResult` contains `records: Vec<Record>`, optional `total`, `truncated`, `elapsed_ms`, `dependencies: JsonQueryDependencies`, `filter_hash`, per-query `metrics`, and `query_plan`. `JsonQueryDependencies` lists scoped collections, referenced `JsonPath`s and metadata fields. `Record` contains `collection`, `id`, logical `path`, byte size, exact decimal-string modification nanoseconds, content `hash`, logical `data: Value`, `state: RecordState`, and optional `raw` original bytes for malformed formats. A malformed record's `data: Null` is a compatibility placeholder, not a decoded logical null.

Streaming variants preserve query/error behavior and return `JsonQueryStreamSummary` (`returned`, optional `total`, `truncated`, `elapsed_ms`, dependencies, filter hash, metrics and plan):

| Function | Callback argument meaning |
| --- | --- |
| `query_json_stream(request, on_record: impl FnMut(usize, &Record) -> Result<()>)` | `request` is a `JsonQuery`; `on_record` receives the one-based emitted position and a borrowed record. Returning `Err` stops the stream. |
| `query_json_stream_updates(request, on_page: impl FnMut(&[Record]) -> Result<()>)` | `on_page` receives a borrowed batch/page of records; returning `Err` stops it. |

Streaming may have emitted earlier records when a later read, verification or callback fails. A failure does not publish a completed cache entry.

### Full-text queries and registered text operators

`Repository::search_text(&mut self, request: &TextQuery) -> Result<TextQueryResult>` searches logical scalar leaves. `search_text_with(request, representation, operator)` uses the registered identities named by `representation: &str` and `operator: &str`; it passes `request.pattern` to `QueryOperator::compile` as a JSON string. The returned `TextQueryResult` contains `hits: Vec<TextHit>`, optional `total`, `truncated`, `elapsed_ms`, metrics and plan. Each `TextHit` has its `record`, canonical matching `fields`, and concrete array-member `matched_paths`.

| `TextQuery` field | Meaning |
| --- | --- |
| `pattern: String` | Literal or regex expression according to `mode`; empty has mode-specific validation. |
| `filter: Option<String>` | Optional JSON field predicate applied alongside text matching; it may include supported `SORT BY` syntax. |
| `parameters: BTreeMap<String, Value>` | Named typed values for `filter`; parameters without a filter are invalid. |
| `mode: TextMode` | `Word` searches normalized visible text; `WordRaw` searches stored decoded text; `Regex` runs a Rust regex on stored values. Regex flags belong in the pattern. |
| `case_sensitive: bool` | Applies to the two word modes. Regex controls its own case behavior. |
| `search_options: TextSearchOptions` | Optional `syntax: Option<TextSyntax>` and `case_sensitive: Option<bool>` request overrides for registered definitions. `Some(false)` is distinct from absence. Target options take precedence. |
| `collections: Option<Vec<String>>` | Same scope convention as `JsonQuery`. |
| `fields: Option<Vec<TextFieldSelector>>` | Optional leaf paths such as `body` or `contributors[].note`. A bare selector uses the field default; `TextFieldSelector::with_search(path, definition)` selects an alias or canonical registered definition. `.with_options(TextSearchOptions)` adds explicit target syntax/case settings. Bare strings still deserialize and convert from Rust `&str`/`String`; an explicit selector serializes as `{"path":"body","search":"objekte:markdown"}` and may add `"syntax"` and `"case_sensitive"`. Empty selection is invalid; `None` searches included leaves. |
| `sort: Vec<Sort>` | Optional final ordering terms; empty uses the existing text-search order. |
| `offset`, `limit`, `count` | Page start, positive page size, and optional total, with the repository's `total_limit` cap. |

The corresponding callbacks are `search_text_stream(request, on_hit: FnMut(usize, &TextHit) -> Result<()>)`, `search_text_stream_with(request, representation, operator, on_hit)`, and `search_text_stream_updates(request, on_page: FnMut(&[TextHit]) -> Result<()>)`. The borrowed hit/page cannot be retained without cloning. Each returns `TextQueryStreamSummary` with `returned`, optional `total`, `truncated`, `elapsed_ms`, metrics and plan. A registered operator's `Err` is propagated even when its bigram hint excludes the failing value, unless that operator explicitly permits skipping it.

Markdown, HTML and XML fields declared by the schema use their visible representation for `Word`: markup is removed, logical value boundaries remain separate, punctuation and whitespace are normalized, and Unicode case folding applies unless case sensitive. Legacy `WordRaw` and `Regex` see stored decoded values; an explicitly selected definition's regex sees that definition's prepared text. Malformed JSON has its existing `$raw` standalone-search behavior; a custom decoder declares its own failure policy. `TextSearchOptions` resolves explicit target settings before explicit request settings, legacy mode/case, and operator defaults. Full Rust struct literals need `search_options: Default::default()` after this additive request field.

### Derived values and custom query frontends

`query_derived(&mut self, query: &DerivedQuery) -> Result<DerivedQueryResult>` evaluates one registered extractor in one collection. Its arguments are `collection` (name), `extractor` (registered identity), `predicate` (`AnyValue`, `Eq`, `Ne`, `Lt`, `Le`, `Gt`, or `Ge` with a `TypedValue`), optional `sort: DerivedSort { direction, reduce }`, `offset`, positive `limit`, `count`, and `backend: DerivedBackend::{Direct, Indexed}`. `DerivedSort::reduce` uses `SortReduce::{Min, Max, First, Last}`; the indexed derived API currently rejects `First` and `Last`. `DerivedQueryResult` returns records, optional total and truncation. Missing, present with no output, null, multiple values and errors remain distinct; `AnyValue` includes an emitted null but not missing/no output.

`query_frontend(&mut self, frontend: &str, request: &Value) -> Result<ExtensionQueryResult>` calls a registered frontend by namespaced identity. `request` is the application's JSON-compatible request object; the frontend validates and compiles it into a `LogicalPlan`. Each `ExtensionQueryRow` carries the selected `SourceDescriptor` and projected columns as `BTreeMap<String, Vec<TypedValue>>` (an absent/empty column is an empty vector; null is an explicit typed value). `ExtensionQueryResult` contains rows, optional total, truncation, stable semantic `identity`, elapsed time, metrics and plan. The frontend cannot open files or own pagination execution.

### Views, records and relations

| Function | Arguments and result |
| --- | --- |
| `open_view(collection: &str, request: &ViewRequest) -> Result<ViewResult>` | Runs an ad hoc scalar view over `collection`. `ViewRequest` has `filters: Vec<Filter>`, optional primary `sort`, optional `sub_sorts`, optional projected `columns`, optional `limit`, `offset`, and `count`. `Filter { field, operator, value }` uses `Operator::{Eq,Lt,Le,Gt,Ge}` and a JSON value. `ViewResult` gives columns, record states, rows, total, metrics, plan and definition hashes. |
| `run_view(name: &str, arguments: &BTreeMap<String, Value>) -> Result<NamedViewResult>` | Runs a configured named view. `name` selects the view; `arguments` binds its declared shared typed parameters. |
| `run_view_with_query_arguments(name, arguments, query_arguments: &BTreeMap<String, BTreeMap<String, Value>>) -> Result<NamedViewResult>` | Also overrides native request fields per query name before shared argument binding. Unknown view/query names or wrong argument types fail. |
| `get_record(collection: &str, id: &str) -> Result<Option<Record>>` | Resolves one selected record ID in a collection; `None` means absent. |
| `references_from(collection, id, check_targets: bool) -> Result<References>` | Finds outgoing declared relation edges from one record; `check_targets` resolves target presence. |
| `references_in_record(record: &Record) -> Result<Vec<RecordRelation>>` | Inspects declared edges in an already obtained logical record; no ID lookup argument. |
| `references_to(collection, id, complete: bool, check_targets: bool) -> Result<References>` | Finds incoming edges to target `collection`/`id`; `complete` requests complete coverage and `check_targets` resolves targets. |

`ViewRow` contains collection, ID, path, display label and projected values. `NamedViewResult` contains name, `ViewType::{List,Single}`, and query-name keyed `ViewQueryResult::{Json,Text}`. A `ViewDefinition` contains name, type, argument declarations (`ViewArgument { kind, default }`), `ViewQuery` templates and its path. `ViewArgumentType` accepts string, integer, number, boolean, array or object. `RecordRelation` has source collection/ID/location, target collection/ID and `TargetState::{Unknown,Present,Missing}`. `References` adds collection `Coverage` to its edges.

## Indexes, snapshots, refresh and inspection

### Ordinary lifecycle

These methods are on `Repository`. `collection: &str` names one configured collection; an `all` variant repeats the operation for every collection and returns a name-keyed map. `Metrics` results are operation deltas; `Metrics::since(&before)` subtracts cumulative counters with saturation. All source files remain read-only.

| Function | Arguments and effect |
| --- | --- |
| `warm_bigram_index(collection) -> Result<Metrics>` / `warm_all_bigram_indexes() -> Result<BTreeMap<String, Metrics>>` | Restore or build the main index and required selected-source projections if needed. |
| `warm_text_index(collection) -> Result<Metrics>` / `warm_all_text_indexes()` | Restore or build full-text/declared visible text postings. |
| `rebuild_bigram_index(collection) -> Result<Metrics>` / `rebuild_all_bigram_indexes()` | Force a fresh main index build. |
| `rebuild_text_index(collection) -> Result<Metrics>` / `rebuild_all_text_indexes()` | Force a fresh full-text build. |
| `refresh(collection) -> Result<Metrics>` | Reconcile inserts, edits, deletes and renames with active index/cache state. Selected postings are rebuilt for a changed custom-bearing collection before its new generation is exposed. |
| `reload() -> Result<()>` | Reload configuration and selection definitions from the same registered implementations; invalidates affected derived state. |
| `configuration_changes() -> Option<&ConfigurationChanges>` | Returns the last detected configuration-file change report, if any. |
| `refresh_configuration() -> Result<ConfigurationChanges>` | Detects and applies configuration changes, returning changed inputs/affected collections/views. |
| `save_bigram_cache(collection) -> Result<PathBuf>` / `save_all_bigram_caches() -> Result<BTreeMap<String, PathBuf>>` | Persist the current ordinary cache and active selected postings to repository cache paths. |
| `save_bigram_snapshot(collection, path: &Path) -> Result<()>` | Write a main snapshot at caller-chosen `path`; selected index snapshots have their own APIs below. |
| `reindex_bigram_cache(collection) -> Result<PathBuf>` / `reindex_all_bigram_caches()` | Rebuild and save repository-managed cache files. |
| `bigram_cache_path(collection) -> Result<PathBuf>` | Return the repository-managed main cache file path for a collection. |
| `restore_cached_index_unchecked(collection) -> Result<bool>` | Attempt early restoration of a cached native index; `false` means no compatible cache. “Unchecked” does not replace the repository's later freshness reconciliation. |

### Selected representation and derived postings

These are advanced controls. Ordinary warm/build/save/reopen now handles configured custom sources; explicit methods are useful when an application requests a custom field or wants a particular snapshot path. `representation`, `extractor` and `field`/`path` are registered identity or logical field-path strings as indicated. A `usize` build result is the number of selected source slots processed or represented by that build; inspect the current implementation if using it as a diagnostic, not as a correctness signature.

| Function | Arguments and result |
| --- | --- |
| `build_representation_index(collection, representation, path) -> Result<usize>` | Build a bigram posting for registered `representation: &str` on logical text field `path: &str`. |
| `build_text_search_index(collection, definition, path) -> Result<usize>` | Resolve query-facing `definition: &str` (canonical identity or alias) on logical string `path: &str`, then build its representation posting through the same engine-owned builder. |
| `build_selected_derived_posting(collection, extractor) -> Result<usize>` | Build a selected-source B-tree typed posting for registered `extractor: &str`. |
| `build_selected_indexes(collection, requests: &[SelectedIndexRequest]) -> Result<usize>` | Build several `Text { representation, path }` and/or `Derived { extractor }` requests in one selected read/decode pass. Empty lists, duplicates and more than 4096 outputs are rejected; already built keys are reused. |
| `build_derived_posting(collection, extractor) -> Result<()>` | Ordinary named derived-posting preparation. |
| `derived_value_counts(collection, extractor) -> Result<Vec<(TypedValue, usize)>>` | Distinct-record counts of values emitted by the extractor. |
| `save_selected_representation_snapshot(collection, representation, field, path: &Path) -> Result<()>` | Write one selected text posting to `path`, with its semantic companion manifest. `field` is the bound logical text path. |
| `restore_selected_representation_snapshot(collection, representation, field, path) -> Result<bool>` | Restore only a compatible posting; `false` is a missing/incompatible/corrupt cache miss. |
| `save_selected_derived_snapshot(collection, extractor, path: &Path) -> Result<()>` | Persist one selected typed posting. |
| `restore_selected_derived_snapshot(collection, extractor, path) -> Result<bool>` | Restore one compatible selected typed posting; `false` is a cache miss. |

### Inventory, source changes, validation and diagnostics

| Function | Arguments and result |
| --- | --- |
| `source_descriptors(collection) -> Result<Vec<SourceDescriptor>>` | Enumerate the built-in JSON and applicable registered providers, with collision/decoder validation. Ordering is deterministic. |
| `with_source_record<T>(descriptor: &SourceDescriptor, visit: impl FnOnce(&dyn DecodedRecord) -> Result<T>) -> Result<T>` | Validate a selected descriptor, read it, decode one borrowed record, run `visit`, and return its value. Borrowed fields cannot escape the callback. |
| `read_source_leaves(descriptor) -> Result<Vec<DecodedLeaf>>` | Owned convenience list of JSON-pointer paths and `TypedValue`s; exposed only after complete successful decoding. |
| `collect_collection_inventory(collection) -> Result<CollectionInventory>` | Collect a native source inventory independently of the live picker. The opaque value is passed to `apply_collection_inventory`. |
| `apply_collection_inventory(inventory: CollectionInventory) -> Result<usize>` | Reconcile the collected native inventory; returns a change count. |
| `diff(collection) -> Result<CollectionChanges>` / `diff_all()` | Report native JSON source changes since the handle's known picker inventory. |
| `apply_source_changes(changes: &[SourceChange]) -> Result<BTreeMap<String, CollectionChanges>>` | Apply caller-reported native `.json` file changes (`Created`, `Changed`, `Deleted`, `Moved { from, to }` paths). Use ordinary `refresh` for provider sources. |
| `record_revision(collection, id) -> Result<Option<RecordRevision>>` | Native JSON picker revision, including tombstones; custom descriptors instead expose their revision string. |
| `validate_collection(name) -> Result<ValidationMetrics>` / `validate_all()` | Validate logical records against their configured schema. |
| `infer_and_auto_index(collection: Option<&str>, options: InferOptions) -> Result<InferenceReport>` | Scan one named collection or all (`None`), infer native JSON field shapes, optionally build indexes or write schema as directed by options. |
| `write_inferred_schema(report: &mut InferenceReport) -> Result<usize>` | Write inferred schema files from a prior report; returns written count. Native picker operation. Reload the handle before using the new schema there. |
| `build_posting(collection, path) -> Result<()>` / `build_posting_all(path)` | Prepare the native JSON scalar B-tree posting at filter-syntax `path`; it does not include custom provider records. |
| `value_counts(collection, path, limit) -> Result<Option<Vec<ValueCount>>>` | Return at most `limit` most common non-null/nonmissing native indexed values, with distinct record counts; `None` means no current complete posting (including mixed custom collections). |
| `index_bytes(collection) -> Result<Option<usize>>` / `text_index_bytes(collection)` | In-memory native index byte estimates when present. |
| `cache_status() -> Result<CacheStatus>` | Cache mode/freshness, effective encodings, collection coverage and configuration changes. |
| `metrics() -> &Metrics`, `reset_metrics()`, `query_cache_bytes() -> usize` | Inspect/reset cumulative work counters and inspect current query-cache size. |
| `fork_unindexed() -> Repository` | New handle sharing immutable model/registrations but without warmed index state. |

`SourceChanges { created, changed, deleted }` has `len()` and `is_empty()`. `CollectionChanges` also reports `indexed` and optional `saved_cache`. `RecordRevision` exposes collection, ID, numeric revision, changed-at nanoseconds and presence. `Coverage` reports collection, state, verification, configuration currency, expected schema hash and record count. `CacheStatus` includes collection coverage and effective encoding. `Metrics` exposes cumulative counts/timings such as enumerations, reads, bytes, parses, validations, candidate scans, cache hits/evictions, index builds and feature-gated stage counters; use `since` for a delta around a call. Counters describe performed work, not guaranteed hardware costs.

## Extension interfaces

### Descriptors and stable identity

Every implementation returns `descriptor(&self) -> &Descriptor`. Build it with:

```rust
Descriptor::new(identity, version, &configuration, input, output, dependencies)
```

`identity: impl Into<String>` is a unique `namespace:name` (for example `example:markdown`); `version: SemanticVersion { major, minor, patch }` changes when meaning changes; `configuration: &impl Serialize` is canonical, deterministic configuration (use ordered maps); `input` and `output` are `DataKind::{Source,Record,Text,TypedValues,Predicate,Query}` according to the registration table; `dependencies: Vec<Dependency>` declares inputs affecting results. `Descriptor::new` sorts/deduplicates dependencies and hashes configuration to a 64-character SHA-256 string. `Descriptor::cacheable()` is false when dependencies contain `Volatile`.

`Dependency` variants are `SourceBytes`, `FileName`, `Metadata(String)` (named source metadata), `Schema(String)` (a custom `x-` schema annotation), `Relation(String)` (declared relation), `ExternalVersion(String)` (application-supplied versioned input), and `Volatile` (request-local result only). A hidden clock, network or environment dependency must be versioned explicitly or marked volatile. Duplicate identities across kinds fail registration. `objekte:filesystem` and `objekte:json` are reserved. Registration order and pointer addresses are never persistent semantic identities. A used volatile definition prevents prohibited cache/snapshot reuse; an unrelated one does not disable a stable query.

### Source selection: `SourceProvider`

| Method | Arguments, return and contract |
| --- | --- |
| `descriptor(&self) -> &Descriptor` | Stable `Source → Source` identity and selection/read dependencies. |
| `may_select(&self, collection: &str) -> bool` | Optional fast invariant rule for a collection name. Default `true`. Returning `false` promises `enumerate(collection)` would be empty; do not use it for mutable content-dependent choices. |
| `enumerate(&self, collection: &str) -> Result<Vec<SourceDescriptor>>` | Return every selected source for that collection with stable IDs, locator, format, revision and declared metadata. Do not return duplicates. |
| `read(&self, source: &SourceDescriptor, output: &mut Vec<u8>) -> Result<()>` | Read exactly the descriptor's source into reusable `output`; clear/replace it, bound resource use, and reject stale revision or unsafe locator according to the provider's policy. |

`SourceDescriptor` has `collection` (owner), `provider` (the provider descriptor identity), `source_id` (stable identity within that provider), `record_id` (logical ID unique within a collection across providers), `locator: PathBuf` (actual read location), `format` (decoder identity), `revision: String` (source generation token), and `metadata: BTreeMap<String,String>`. Metadata commonly includes `size_bytes`, `modified_unix_nanos`, optional `created_unix_nanos`, and a logical `path`. The source ID and locator may differ; renames follow the provider's identity rule. Collision and ambiguous decoder errors stop selection. A decoder returning `true` from `accepts` must have the identity named by `source.format`. A provider may be part of cache dependency identity even while its inventory is empty.

The built-in filesystem JSON provider retains root containment, descriptor-relative no-follow reads and before/after checks. A custom provider owns its own read safety and revision promises. Source files do not authorize executing extension code: the application statically links and registers trusted Rust definitions.

### Format decoding: `FormatDecoder`, `DecodedRecord`, `RecordAccess`

| Method | Arguments, return and contract |
| --- | --- |
| `FormatDecoder::descriptor(&self) -> &Descriptor` | Stable `Source → Record` format/implementation identity. |
| `accepts(&self, source: &SourceDescriptor) -> bool` | Declares whether this decoder handles the selected descriptor, including its format/locator/metadata rules. More than one accepting decoder is an error. |
| `decode(&self, bytes: &[u8], visit: &mut dyn FnMut(&dyn DecodedRecord) -> Result<()>) -> Result<()>` | Legacy entry point. Decode the borrowed bytes and call `visit` for one record; propagate callback errors. An opt-in decoder translates a malformed outcome to `Error::MalformedSource` here, never success with zero records. |
| `decode_outcome(&self, bytes, visit) -> Result<DecodeOutcome>` | Additive outcome-aware entry point. Default calls `decode` and returns `Decoded`; legacy `Err` remains an error. An override returns `Decoded` after exactly one callback, or `Malformed(Vec<FormatDiagnostic>)` after none. Parse complete syntax before calling back. |
| `malformed_text<'a>(&self, bytes: &'a [u8]) -> Result<Option<Cow<'a, str>>>` | Optional original-source text fallback, called only after confirmed malformed content. Default `None`; UTF-8 can borrow, other encodings may return owned text. Errors propagate. |
| `FormatDecoder::visit_leaves(&self, bytes, visit) -> Result<()>` | Optional convenience/default: calls `decode`, then the record's leaf visitor. |
| `DecodedRecord::visit_leaves(&self, visit: &mut dyn FnMut(&str, ScalarValue<'_>) -> Result<()>) -> Result<()>` | Visit scalar leaves at JSON-pointer paths. This is required. A callback error stops visitation. |
| `DecodedRecord::visit_nodes(&self, visit: &mut dyn FnMut(&str, LogicalNode<'_>) -> Result<()>) -> Result<()>` | Optional richer traversal for ordinary owned records: emit parent `Object`/`Array` nodes before children, and `Scalar` leaves. Default adapts `visit_leaves`. Needed to preserve empty containers or numeric object keys. |
| `RecordAccess::path_present(&self, path: &JsonPath) -> Result<bool>` | Does the path reach a value, including an empty array/object? Required for correct missing/no-output distinction. |
| `RecordAccess::visit_path(&self, path, visit: &mut dyn FnMut(ScalarValue<'_>) -> Result<()>) -> Result<usize>` | Visit reached scalar values in source order; return number visited, propagate callback errors. Empty containers visit zero. |

`ScalarValue` is borrowed `Null`, `Bool(bool)`, `Number(&str)` or `Text(&str)`. Number text must be a valid JSON number and preserve exact decimal meaning. `LogicalNode` is `Object`, `Array` or `Scalar(ScalarValue)`. JSON pointers use `/body`, `/items/0/title`; escape `~` as `~0` and `/` as `~1`. Emit arrays in index order without gaps. Borrowed strings and record resources must remain alive through the callback and must not escape it. `RecordAccess` is already implemented for `serde_json::Value` and Sonic values, so a convenience decoder may delegate to an owned tree, but native JSON queries are not forced through such a tree.

`FormatDiagnostic { code, message, span }` explains unavailable logical data; an empty list receives `format_syntax`. `SourceSpan { start, end }` is a zero-based half-open byte range checked against the input. The first callback failure wins even if a decoder catches it and reports success or malformed content. A callback followed by malformed content, zero/multiple callbacks for success, or an invalid diagnostic is a contract error. Native JSON keeps its concrete parser. Built-in JSON used through a provider maps syntax failures to `IncorrectJson` and keeps the existing JSON raw-plus-best-effort-unescaped fallback; other formats do not inherit JSON unescaping. `with_source_record` and `read_source_leaves` require a logical record and return `Error::MalformedSource` for recoverable malformed input. [The downstream article fixture](../crates/extension-tests/src/malformed_formats.test.rs) implements the new contract and borrowed UTF-8 fallback.

### Text representation: `TextRepresentation`

| Method | Arguments, return and contract |
| --- | --- |
| `descriptor(&self) -> &Descriptor` | Stable `Text → Text` semantic identity. |
| `binds(&self, context: &BindingContext) -> bool` | Applicability to resolved `collection`, canonical `path`, format name and optional retained schema JSON (including custom annotations). Binding is compiled once where inputs are invariant. |
| `preparation(&self) -> TextPreparation` | `Stored` (default) or `VisibleNormalized` to tell the operator what it receives. |
| `native_index_projection(&self) -> Option<NativeTextProjection>` | Optional proof of byte-for-byte equivalence to an existing native `Stored` or `VisibleNormalized(TextFormat)` projection. Default `None` prevents reuse of that native projection; an independent representation posting may still be built and used. A false claim can omit matches. |
| `binds_explicit(&self, context: &BindingContext) -> bool` | Applicability when a composed query-facing definition is explicitly selected. Default delegates to `binds`; built-in rendered adapters allow an explicit compatible string-field choice even if legacy `x-full-text` names a different renderer. |
| `render<'a>(&self, value: &'a str, output: &'a mut String) -> Result<Option<&'a str>>` | Render one logical string. Return borrowed input or reusable output, or `None` if no text representation. Do not retain either borrow. Errors propagate. |

`BindingContext` fields are `collection`, `path`, `format`, and `schema: Option<&Value>`. Here `format` is the **field's text rendering format** (`plain`, `markdown`, `html`, `xml`, or `derived` for a typed binding), not the source decoder's format identity. `schema` is the retained resolved JSON Schema tree, including declared `x-` annotations. `TextFormat` is `Plain`, `Markdown`, `Html` or `Xml`. Built-ins are `StoredRepresentation::default()` and `RenderedRepresentation::new(format) -> Result<Self>`; the latter selects a visible normalized renderer for the chosen format. `NativeTextProjection` is a candidate-index compatibility declaration, not merely a display label. Index construction and exact verification must use the same bound representation. Logical strings and array members remain separate matching streams.

### Query-facing text definition: `TextSearchDefinition`

`TextSearchDefinition` composes a registered `TextRepresentation` with exact `QueryOperator` mappings. Its descriptor has `Text → Predicate` kinds. `TextComparison::{Contains,Equal}` is the positive operation; the filter language applies `!~`/`!=` as negation of the exact result and never complements approximate bigram candidates. `TextSyntax::{Literal,Regex}` tells the operator how to compile the pattern. `TextCompileRequest { pattern, comparison, syntax, case_sensitive }` is passed once at binding. `case_sensitive: None` retains the operator's default; an explicit boolean is distinct from absence.

| Public method | Arguments and effect |
| --- | --- |
| `TextSearchDefinition::new(descriptor, representation)` | `descriptor: Descriptor` supplies stable query semantics; `representation` is the registered representation identity, not a field path or a format decoder. |
| `.alias(name)` | Add a short modifier name such as `abbreviations`. It must start with an ASCII letter and then use letters, digits or `_`; structural and built-in reserved names are rejected. |
| `.operator(comparison, identity)` | Map `Contains` or `Equal` literal comparison to a registered operator identity. Missing mappings reject that query shape at binding. |
| `.regex_operator(identity)` | Add a compiled regex `Contains` mapping. Regex source is passed unchanged; regex equality is unsupported. |
| `descriptor()`, `representation()`, `aliases()` | Inspect the stable descriptor, component identity and aliases. |
| `RepositoryBuilder::register_text_definition(definition)` | Register before `open`. Component registration may occur in any order, but the complete graph is validated at open. |

`QueryOperator::compile_text(&self, request: &TextCompileRequest)` is the additive typed compilation hook. Its default adapts a legacy `compile(Value::String(pattern))` implementation for literal contains without an explicit case override. Override it to implement equality, regex or case overrides. `TextRepresentation::render` prepares document content; the operator prepares a literal pattern as text, never by parsing it as Markdown/HTML/XML. A visible representation is already normalized, so `matches_prepared` avoids a second normalization. A scanner's necessary `BigramLiteral` must identify the bound representation, and `can_skip_non_candidates()` must be true before that hint prunes exact invocation. A missing or incompatible posting causes a scan, not a semantic change.

The schema `x-text-search` object maps canonical definition identities to `{ "index": true|false }` and may contain `"default": "identity"`. A lone definition is an implicit default; multiple definitions without one are ambiguous for `:fts` or a bare standalone field. The old `x-full-text` declaration maps to the corresponding built-in default. An explicit selector may bind a compatible string field without a declaration; it scans unless an explicit posting was built. `TextFieldSelector::new(path)` uses the configured default, `with_search(path, definition)` chooses one. `TextFieldSelector::parse("body:markdown")` is the CLI form; `body:text("example:expanded-abbreviations")` selects a canonical identity. `JsonQuery` parses the same modifiers in `body:markdown ~ "word"` and `body:text("example:expanded-abbreviations") ~ "word"`. Existing ordinary `~`/LIKE keeps separate semantics.

The built-in composed identities are `objekte:plain`, `objekte:markdown`, `objekte:html`, `objekte:xml`, and `objekte:raw-text`; short aliases are `plain`, `markdown`, `html`, `xml`, and `raw`. These are distinct from low-level `objekte:visible-*` representation identities and `objekte:word`/`objekte:word-raw` operators. See the runnable [public-only example](../crates/extension-tests/examples/text_search_definition.rs) for registration, a field default plus a second definition, ordinary indexed FTS and JSON filters, save/reopen, and refresh.

### Typed extraction: `ValueExtractor`

`descriptor(&self) -> &Descriptor` supplies `Record → TypedValues`. `binds(&self, context: &BindingContext) -> bool` decides collection/derived-path/schema applicability. `extract(&self, record: &dyn RecordAccess, output: &mut Vec<TypedValue>) -> Result<Extraction>` appends zero or more values to a worker-reused vector and returns `Extraction::{Missing,Present}`. The engine clears/reuses the output; `Missing` with emitted values is invalid. `Present` with zero values is distinct from missing; `TypedValue::Null` is explicit null; `Err` is failure. `TypedValue` is `Null`, `Bool(bool)`, `Number(String)` or `Text(String)`. Numbers use exact decimal comparison, not floating point conversion. Multi-value predicates are existential and indexed sorting needs a supported reduction. Extractors with volatile inputs cannot use persistent postings. The application cannot use the reserved internal derived path to alias a stored JSON field.

### Exact scanning and candidates: `QueryOperator`, `CompiledOperator`

`QueryOperator::descriptor(&self) -> &Descriptor` supplies a text/typed-values to predicate identity. `compile(&self, parameters: &Value) -> Result<Box<dyn CompiledOperator>>` validates parameters **once per binding** and returns immutable, shareable compiled state. For `search_text_with`, `parameters` is `Value::String(request.pattern.clone())`; a frontend may provide another JSON value. The current public `LogicalPredicate::Call` execution binds **text** operators; a `TypedValues` operator can be registered but has no corresponding general `Call` execution route yet. Use `LogicalPredicate::Derived`/`DerivedPredicate` for typed extractor comparisons.

| Compiled method | Arguments, return and contract |
| --- | --- |
| `matches(&self, value: ScalarValue<'_>, scratch: &mut String) -> Result<bool>` | Exact predicate on one borrowed value. `scratch` is worker-local reusable output; do not retain it or `value`. Preserve errors. |
| `matches_prepared(&self, value, scratch) -> Result<bool>` | Optional exact match for a representation already producing visible normalized text. Default delegates to `matches`. |
| `candidate(&self) -> CandidateConstraint` | Optional necessary condition: `Unsupported` (default), `BigramLiteral { literal, representation }`, or `TypedRange { path, lower, upper }`. The engine supplies postings; the extension does not build its own store. |
| `can_skip_non_candidates(&self) -> bool` | Default `false`. Return `true` only if exact evaluation of excluded values cannot fail; an index hint alone proves only nonmatch, not absence of errors. |

`NativeTextOperator::new(mode: TextMode, case_sensitive: bool)` supplies the built-in scanner as a registered operator. A candidate constraint is used only with a compatible representation/preparation/index and the skip permission. Boolean `AND`, `OR` and `NOT` retain short-circuit/error behavior; an approximate `NOT` cannot be complemented. Errors never turn into empty matches, and cancellation never publishes a complete cache entry.

### Custom request compilation: `QueryFrontend`

`descriptor(&self) -> &Descriptor` supplies `Query → Query`. `compile(&self, request: &Value, bindings: &ExtensionBindings) -> Result<LogicalPlan>` validates application syntax and constructs a logical plan. `bindings.operator(identity)`, `.representation(identity)`, and `.extractor(identity)` each return `Option<u32>`: a temporary numeric handle for an identity in this registration set, or `None` if absent. Resolve handles at compile time; do not persist them or assume registration order. Persistent query identity is built from descriptors, parameters and source generation.

`LogicalPredicate` variants are `Call { operator, representation, path, parameters }`, `Derived { extractor, predicate }`, `And(Vec<_>)`, `Or(Vec<_>)`, and `Not(Box<_>)`. `Call.path` is a logical field path; `parameters` is passed to that operator's compiler. `LogicalPlan` fields are `collections: Vec<String>`, `predicate`, `sort: Vec<Sort>`, projected `columns: Vec<String>`, `offset`, positive `limit`, and `count`. The frontend must not read sources, scan records, combine candidates, sort, page, or manage caches itself. A plan may fall back to exact scan when no safe index constraint is available.

## Public helper types and methods

| Item | Externally usable operation and arguments |
| --- | --- |
| `JsonFilter::compile(text: &str, parameters: &BTreeMap<String, Value>) -> Result<Self>` | Parse/bind a filter with default `:fts` options. |
| `JsonFilter::compile_with_fts(text, parameters, fts: FtsOptions) -> Result<Self>` | Bind the same filter with explicit text mode/case for `:fts`. |
| `JsonFilter::matches(&self, value: &Value) -> Result<bool>` | Evaluate against one logical JSON value without repository source metadata/relation context. Use repository query APIs for full semantics. |
| `JsonFilter::source_candidate_pattern() -> Result<Option<String>>` | Obtain a conservative raw-source regex prefilter when available; `None` means unsupported, not empty results. |
| `JsonFilter::parse_path(text: &str) -> Result<JsonPath>` | Parse filter-syntax paths for `RecordAccess` or selected index requests. `JsonPath` is `Vec<JsonPathStep>` (`Key`, numeric `Index`, or `Each`). |
| `JsonFilter::paths()`, `paths_in_query_order()`, `metadata_fields()`, `hash()` | Inspect referenced paths, first-use path order, metadata names and stable compiled filter hash. |
| `split_sort_clause(input: &str) -> Result<(String, Vec<Sort>)>` | Split a filter plus trailing sort clause into filter text and parsed terms. |
| `parse_sort_terms(clause: &str) -> Result<Vec<Sort>>` | Parse sort terms alone. |
| `ValueType::scalar(self) -> bool` | Whether schema type is a scalar rather than object/array/any. |
| `SourceChanges::len()` / `is_empty()` | Number of created+changed+deleted IDs / emptiness. |
| `BigramEncoding::as_str()` / `parse(value: &str)` | Stable name (`ascii` or `utf8_bytes`) / parse one into `Option<BigramEncoding>`. |

Schema types exposed by `lib.rs` include `Field` (kind, nullability, indexing, language, sort reduction, text format, exclusion, enum/suggestions, children and relation target), `ValueType`, `TextFormat`, `PathStep`, and `SortReduce`. `Direction::{Asc,Desc}` and `Sort { field, direction }` describe ordering. Validation uses `RecordState { status, issues }`, `RecordStatus::{NoSchema,Pending,Valid,Invalid,IncorrectJson,IncorrectFormat}`, `ValidationIssue { location, schema_path, code, message, source }`, optional `SourceIssue { format, span }`, and `ValidationMetrics`. `IncorrectJson` remains serialized as `incorrect_json`; `IncorrectFormat` is `incorrect_format`. `Invalid` means a decoded logical record violated schema. Source issues use empty `location`; their byte spans are not JSON Pointers into `Record.data`. Adding an enum variant and issue field requires downstream exhaustive matches and struct literals to be updated. Configuration-change types report changed input path/kind/digests and affected collections/views. `InferOptions { auto_index, write_schema }` controls explicit native field inference; `InferenceReport`, `CollectionInference`, `FieldInference`, `InferredKind`, `InferredLink` and `InferredLinkKind` expose its results. `CollectionGraph`/`CollectionRelation` expose declared relationship edges, connected components and cycles. All are result/configuration data, not extension callback entry points.

## Low-level bigram and JSON picker APIs

These public modules are usable from another Rust crate, but they are specialized storage utilities. They do not replace `Repository` for mixed-provider lifecycle, schema binding, queries or cache semantics. `FilePicker` remains JSON-only.

### `objekte_core::bigram`

| Function or method | Arguments and meaning |
| --- | --- |
| `extract_bigrams(content: &[u8]) -> Vec<u16>` / `extract_bigrams_with_encoding(content, encoding)` | Extract adjacent/skip-one bigram keys from bytes using default ASCII or explicit encoding. |
| `build_bigram_index_from_content(contents: &[Option<&[u8]>]) -> BigramFilter` / `build_bigram_index_from_content_with_encoding(contents, encoding)` | Build a filter from slot-ordered optional byte slices; `None` represents an absent slot. `encoding` chooses ASCII or UTF-8 byte keys. |
| `release_thread_buffers()` | Release thread-local reusable bigram build buffers after work. |
| `BigramIndexBuilder::new(file_count)` / `new_with_encoding(file_count, encoding)` | Allocate a builder for a fixed number of numeric file slots. |
| `BigramIndexBuilder::add_file_content(&self, skip_builder: &Self, file_idx, content)` | Add one slot's bytes to adjacent and supplied skip-one builders; `file_idx < file_count`. |
| `BigramIndexBuilder::is_ready()`, `columns_used()` | Inspect accumulated readiness and column count. |
| `BigramIndexBuilder::compress(self, min_density_pct: Option<u32>) -> BigramFilter` | Consume the builder into compressed storage; optional density threshold governs representation choice. |
| `BigramFilter::query(pattern: &[u8]) -> Option<Vec<u64>>` | Candidate bitset for query bytes; `None` means no usable necessary condition. It is never exact text verification. |
| `BigramFilter::common_column_bitset(key)` / `column_bitset(key)` | Borrow or materialize bitsets for encoded bigram `key: u16`, if stored. |
| `BigramFilter::set_skip_index(skip)` | Attach a compatible skip-one filter. |
| `BigramFilter::is_candidate(candidates: &[u64], file_idx)` / `count_candidates(candidates)` | Test one numeric slot / count set bits. |
| `BigramFilter::new(lookup, dense_data, dense_count, words, file_count, populated)` | Low-level constructor: `lookup: Vec<u16>` maps keys to columns, `dense_data: Vec<u64>` is the packed word array, `dense_count` counts dense columns, `words` is words per column, `file_count` is source slots, and `populated` counts populated keys. These must form a mutually consistent layout. Prefer builders/snapshots for application code. |
| `BigramFilter::is_ready()`, `file_count()`, `encoding()`, `columns_used()` | Basic index state. |
| `sparse_count()`, `sparse_bytes()`, `heap_bytes()`, `dense_count()`, `populated()`, `words()` | Storage/layout diagnostics; bytes are in-memory estimates. |
| `has_key(key)`, `column(key)`, `lookup()`, `dense_data()`, `skip_index()` | Inspect encoded key mapping, column number, borrowed storage slices and optional skip filter. `key` is a `u16` bigram key. |
| `BigramOverlay::new(base_file_count)` / `new_with_encoding(base_file_count, encoding)` | Track changed/deleted numeric slots after an immutable base filter. |
| `BigramOverlay::modify_file(file_idx, content)` / `delete_file(file_idx)` | Replace a slot's projected bytes or mark it deleted. |
| `BigramOverlay::query_modified(pattern_bigrams: &[u16]) -> Vec<usize>` | Candidate changed slots for already extracted query keys. |
| `BigramOverlay::base_file_count()`, `tombstones()`, `modified_indices()` | Inspect base slot count, deleted bitset and changed slot numbers. |

`BIGRAM_KEY_SLOTS` is the ASCII key-space constant; `SKIP_INDEX_MIN_DENSITY_PCT` is the built-in skip-index compression threshold. `BigramEncoding::{Ascii,Utf8Bytes}` is serialized in the index identity. A candidate bitset must be paired with the same source-slot generation and exact representation; it is not portable across unrelated inventories.

`BigramFile` carries slot ID, filename, size, exact modification nanoseconds and filesystem identity. `BigramSnapshot` carries source root, recursion flag, optional Git commit, ordered files, changed slots/bigrams and filter. Its public methods are:

| Method | Arguments and effect |
| --- | --- |
| `BigramSnapshot::new(source_root: &Path, recursive: bool, files: Vec<BigramFile>, filter: BigramFilter) -> Result<Self>` | Construct a snapshot for this source scope and matching slot table. |
| `save(&self, path: &Path) -> Result<()>` | Persist at destination path. |
| `save_filter(path, source_root, recursive, files: &[BigramFile], filter: &BigramFilter) -> Result<()>` | Build/write directly from a filter and source metadata. |
| `save_filter_with_changes(path, source_root, recursive, files, changed_slots: &[usize], changed_bigrams: &BTreeMap<usize, Vec<u16>>, filter) -> Result<()>` | Also persist changed-slot overlay keys. |
| `load(path) -> Result<Self>` | Decode snapshot without separately supplied source-scope check. |
| `load_scoped(path, source_root, recursive)` | Load and check the source root and recursive-selection setting; later reconciliation may still find changed files. |
| `load_checked(path, source_root, recursive)` | Also check every ID, filename, stamp and Git commit against current sources before use. |
| `candidate_files(pattern: &[u8]) -> Vec<&BigramFile>` | Return slot metadata for a byte-pattern candidate query; exact matching is still required. |

### `objekte_core::file_picker`

| Method | Arguments and effect |
| --- | --- |
| `FilePicker::open(root: &Path)` / `open_with_encoding(root, encoding)` | Open a JSON file directory, defaulting to ASCII or using explicit byte encoding. |
| `profile_collection(collection: &Collection, encoding)` | Build one resolved collection without repository cache lookup, for profiling. |
| `root()`, `len()`, `is_empty()` | Source root, live record count and emptiness. |
| `refresh() -> Result<usize>` / `refresh_id(id: &str) -> Result<()>` | Reconcile all JSON files or one record ID. |
| `rebuild_index() -> Result<()>` | Rebuild its native content index. |
| `save_bigram_snapshot(path: &Path) -> Result<()>` | Persist picker base snapshot. |
| `revision(id: &str) -> Option<u64>` | Current numeric revision for a live JSON record. |
| `git_status() -> Result<Option<BTreeMap<String, GitState>>>` | Optional on-demand Git index/worktree status; `None` if Git is unavailable/not applicable. `GitState` has `index` and `worktree` status characters. |
| `index_bytes() -> usize` | In-memory content/text index byte estimate. |
| `candidate_ids(literals: Option<&[String]>) -> Vec<&str>` | Candidate IDs for necessary literal terms; `None` requests unconstrained candidates. |
| `regex_candidate_ids(pattern: &str) -> Vec<&str>` | Conservative candidates for a regex. |
| `search_values_regex(pattern: &str) -> Result<Vec<String>>` | Exact regex search over stored JSON values. |

With the `objekte-core/index-timings` feature, `FilePicker::profile_restore_collection(collection, encoding, snapshot_path)` profiles base plus secondary restoration; `profile_secondary_snapshot(path)` writes/times the secondary sidecar; `profile_index_sizes()`, `build_timings()`, and `restore_timings()` expose `IndexSizes`, `BuildTimings`, and `RestoreTimings`. These structs contain stage wall/CPU nanoseconds and posting-size diagnostics. Counting/timing instrumentation changes runtime cost; compare only like builds.

## Complete consumer workflows

### Ordinary custom-format repository

The external consumer test crate supplies working `MarkdownProvider`, `MarkdownDecoder`, `PublicationYearExtractor`, and a prefix operator using only public core APIs. A normal application sequence is:

```rust,no_run
use objekte_core::{
    DerivedBackend, DerivedPredicate, DerivedQuery, Options, Repository,
    TextQuery, extensions::TypedValue,
};
use objekte_extension_tests::{MarkdownDecoder, MarkdownProvider, PublicationYearExtractor};

let root = std::path::Path::new("my-repository");
let open = || Repository::builder(root)
    .options(Options::new())
    .register_source(MarkdownProvider::new(root.join("data/books")))?
    .register_format(MarkdownDecoder::default())?
    .register_extractor(PublicationYearExtractor::default())?
    .open();

let mut repository = open()?;
repository.warm_text_index("books")?;
repository.build_derived_posting("books", "example:publication_year")?;
let page = repository.search_text(&TextQuery {
    pattern: "history".into(),
    collections: Some(vec!["books".into()]),
    fields: Some(vec!["body".into()]),
    ..TextQuery::default()
})?;
let years = repository.query_derived(&DerivedQuery {
    collection: "books".into(),
    extractor: "example:publication_year".into(),
    predicate: DerivedPredicate::Ge(TypedValue::Number("1848".into())),
    sort: None, offset: 0, limit: 50, count: true,
    backend: DerivedBackend::Indexed,
})?;
repository.save_bigram_cache("books")?;
repository.refresh("books")?; // after the application changes source files
drop(repository);
let mut reopened = open()?;
let restored = reopened.search_text(&TextQuery {
    pattern: "history".into(),
    collections: Some(vec!["books".into()]),
    ..TextQuery::default()
})?;
# let _ = (page, years, restored);
# Ok::<(), objekte_core::Error>(())
```

### Small custom text operator and frontend

This example shows the division of labor. A representation supplies the text; an operator compiles exact logic; a frontend chooses handles and a plan. The `objekte-extension-tests` crate contains complete compiling implementations of all three.

```rust,no_run
use objekte_core::{Repository, TextQuery};
use objekte_extension_tests::{PrefixOperator, TrimmedRepresentation};

let mut repository = Repository::builder("my-repository")
    .register_representation(TrimmedRepresentation::default())?
    .register_operator(PrefixOperator::default())?
    .open()?;
let hits = repository.search_text_with(
    &TextQuery { pattern: "Archive".into(), ..TextQuery::default() },
    "example:trimmed", "example:prefix",
)?;
# let _ = hits;
# Ok::<(), objekte_core::Error>(())
```

To implement the operator, validate the incoming `Value` in `compile`, store the prefix in an immutable compiled struct, and implement `matches` for exact prefix semantics. Return `CandidateConstraint::Unsupported` unless a bigram literal is a necessary condition for the **same** representation; return `can_skip_non_candidates() == true` only if all excluded values are guaranteed not to error. To implement a frontend, use `ExtensionBindings` to resolve identities, then return a `LogicalPlan` with `Call`/`Derived` and Boolean nodes. The engine performs reads, index selection and paging.

Here is a complete small frontend definition. It accepts a JSON object with `collection`, `prefix`, and optional `offset`/`limit`. Registration is the same builder call as above, followed by `query_frontend("example:body-prefix", &request)`.

```rust,no_run
use objekte_core::{
    Direction, Error, Result, Sort,
    extensions::{
        DataKind, Dependency, Descriptor, ExtensionBindings, LogicalPlan,
        LogicalPredicate, QueryFrontend, SemanticVersion,
    },
};
use serde_json::Value;

struct BodyPrefixFrontend { descriptor: Descriptor }

impl BodyPrefixFrontend {
    fn new() -> Self {
        Self { descriptor: Descriptor::new(
            "example:body-prefix",
            SemanticVersion { major: 1, minor: 0, patch: 0 },
            &(), DataKind::Query, DataKind::Query,
            vec![Dependency::SourceBytes],
        ) }
    }
}

impl QueryFrontend for BodyPrefixFrontend {
    fn descriptor(&self) -> &Descriptor { &self.descriptor }

    fn compile(&self, request: &Value, bindings: &ExtensionBindings<'_>) -> Result<LogicalPlan> {
        let required = |key: &str| request.get(key).and_then(Value::as_str)
            .ok_or_else(|| Error::Request(format!("missing string {key}")));
        let collection = required("collection")?;
        let prefix = required("prefix")?;
        let operator = bindings.operator("example:prefix")
            .ok_or_else(|| Error::Request("prefix operator is not registered".into()))?;
        let representation = bindings.representation("objekte:stored")
            .ok_or_else(|| Error::Request("stored representation is not registered".into()))?;
        let number = |key: &str, default: u64| {
            let value = request.get(key).map_or(Some(default), Value::as_u64)
                .ok_or_else(|| Error::Request(format!("{key} must be nonnegative")))?;
            usize::try_from(value)
                .map_err(|_| Error::Request(format!("{key} is too large")))
        };
        Ok(LogicalPlan {
            collections: vec![collection.into()],
            predicate: LogicalPredicate::Call {
                operator, representation, path: "body".into(),
                parameters: Value::String(prefix.into()),
            },
            sort: vec![Sort { field: "@id".into(), direction: Direction::Asc }],
            columns: vec!["body".into()],
            offset: number("offset", 0)?,
            limit: number("limit", 20)?,
            count: true,
        })
    }
}

# let _ = BodyPrefixFrontend::new();
```

For example, after registering `MarkdownProvider`, `MarkdownDecoder`, `StoredRepresentation`, `PrefixOperator`, and `BodyPrefixFrontend::new()` on the builder, call `query_frontend("example:body-prefix", &serde_json::json!({"collection":"books","prefix":"Archive","offset":0,"limit":20}))`. The returned rows contain projected `body` values, and the engine owns sorting, candidates, exact verification and cache publication.

## Errors, lifetime rules and compatibility

`Error` variants are `Io { path, source }`, `Config { path, location, message }`, `Record { path, location, message }`, `Request(String)`, `MalformedSource { format, message }`, `Unstable(PathBuf)`, and `Cache(String)`. `MalformedSource` is the typed low-level error when a caller requires a logical record. Operational I/O, decode, extraction, scanner and callback failures remain errors and are never reclassified by text parsing. `Unstable` means a source changed during a consistency boundary. A failed refresh/build does not publish partial selected postings; a failed query/stream does not publish a completed result cache entry. A stream cannot retract records emitted before its terminal error.

Borrowed `ScalarValue`, `DecodedRecord`, input bytes, renderer output, worker scratch and streamed records/pages are valid only for their callback/call. Return owned copies if application state must retain them. Definition objects are `Send + Sync`; compiled operators are immutable `Send + Sync`; mutable scratch is worker-local. Do not put a global lock in byte/character loops or retain a pointer to a reused input buffer.

Source/provider/decoder identities, applicable selection precedence, schema mapping, representation/extractor/operator versions/configuration, compiled parameters and source generation determine cache/snapshot compatibility. A format's descriptor version/configuration must change when its malformed classification, text encoding or fallback semantics change. Used volatile dependencies disable forbidden reuse. Native JSON snapshot bytes are unchanged; repository-selected manifest version 2, selected text companion manifest version 6 (including malformed slots), and selected derived magic `OBJDRV02` reject older/incompatible bytes as cache misses. Cache-on page changes reuse compatible ordered results without copying all slots; cache-off and `HashOnRequest` still perform exact work. Small selected-source edits remap unchanged typed rows and use a changed-record overlay for immutable text postings; broad deltas or semantic changes rebuild. Malformed slots remain outside logical postings and are verified from original text only for unscoped built-in text search. Many malformed sources can increase that fallback verification cost. Native `build_posting`, `diff`, `apply_source_changes`, `record_revision`, picker `value_counts` and inference are JSON-picker-specific; use ordinary `refresh`, selected descriptors, and named derived APIs for custom sources.

This document is the architecture and external API reference. For copyable extension implementations and a compatibility checklist, see [Library extensions](library-extensions.md) and `crates/extension-tests/src/lib.rs`; for internal index and snapshot details, see [Implementation](development/implementation.md). Private `pub(crate)` functions, inaccessible items inside private modules, and CLI/benchmark-only entry points are not part of the downstream Rust API.
