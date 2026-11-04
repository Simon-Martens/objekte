# Library extensions

Objekte extensions are statically linked Rust application code. A repository file never authorizes loading or executing an extension program. Register definitions on `Repository::builder` before `open`; the definitions are shared immutably by a repository handle and its unindexed forks.

## Stable definitions and lifetimes

Each definition supplies a namespaced `Descriptor` such as `example:prefix`, a semantic version, a canonical configuration fingerprint, input/output kinds, and declared dependencies. `Descriptor::new` hashes serializable configuration with a versioned domain. Dependencies are sorted and unique. Duplicate identities, incompatible kinds, malformed identities, and empty external-version dependencies are rejected at registration. Registration order and runtime addresses are absent from fingerprints. Change the semantic version or configuration whenever result semantics change. A `Volatile` dependency disables verified-result caching for a bound text query. Definitions are `Send + Sync`; compiled operators are also `Send + Sync` and may be shared by worker threads.

`SourceProvider` enumerates `SourceDescriptor`s with separate collection, provider, source, record, locator, format, revision and metadata identities, then reads into an engine-supplied byte buffer. `FormatDecoder::decode` lends one `DecodedRecord` only during its callback. The record exposes path presence, including present empty containers, and borrowed logical leaves at JSON-pointer paths (`/body`, `/items/0/title`); object keys escape `~` and `/`. These contracts do not require a canonical JSON tree. A decoder may own one for convenience. Source providers must bound reads and define their own secure locator and before/after revision checks; borrowed record data cannot outlive the callback. An ambiguous match between decoders fails rather than silently selecting one.

`TextRepresentation` binds to a collection, canonical field path, format and retained schema tree, then returns text borrowed from the source value or a reusable output buffer. The engine compiles declared paths, including `[]` array rules, once per collection query. A worker retains a bounded lazy cache for paths encountered only in schemaless records. Its render and scanner buffers are borrowed once per document; a nested search uses separate scratch. The current binding inputs are invariant during a query. A future content- or mutable-metadata-dependent binding must declare those inputs and evaluate them at the appropriate revision instead of caching the decision as a field rule.

`TextPreparation` states whether the representation returns stored text or already normalized visible text. An operator can implement `matches_prepared` to consume the latter without normalizing it again; the default delegates to `matches`. Both methods borrow their input only for the call. `ValueExtractor` visits logical paths through `RecordAccess` and appends zero or more typed values to a reusable vector. `Extraction::Missing`, a present value with no outputs, `TypedValue::Null`, and `Err` are distinct. Number outputs preserve decimal spellings so the engine can apply its existing exact comparison. `QueryOperator` compiles parameters once into an immutable exact scanner and may offer a necessary index condition. A condition is only usable with an index built from the same representation semantics; unsupported conditions fall back to source verification. `QueryFrontend` compiles application requests into logical plans; it must leave candidate combination, scheduling, source reads, ordering, pagination, and cache ownership to the engine.

The public built-in adapters `StoredRepresentation`, `RenderedRepresentation`, and `NativeTextOperator` use the same concrete text renderer and scanner as legacy queries. They are useful for explicit parity checks. `StoredRepresentation` lends the decoded text directly. `RenderedRepresentation` supports plain, Markdown, HTML and XML; malformed XML returns no rendered value. `NativeTextOperator` compiles the literal once and consumes prepared visible text without a second normalization pass. Its raw mode still uses stored decoded text; regex syntax is passed unchanged to the regex compiler.

`NativeTextProjection` is an explicit promise that a representation matches an existing engine-owned index projection. The built-in stored adapter declares stored compatibility; the visible adapters declare compatibility with their format's normalized projection. A `BigramLiteral` hint is used only when its requested projection, the bound preparation, and the representation's declaration agree. A visible query uses the existing clean postings when a compatible text index is present. Plain schemaless text can use a necessary word from the raw posting; markup without a compatible index falls back to verification. An independent representation defaults to scan-only, even if its descriptor resembles a built-in name. A false compatibility declaration can omit matches, so application authors must test it and bump the descriptor version/configuration if semantics change. The engine still verifies every candidate and includes changed or unindexed slots through its existing overlay.

The working repository entry points are `search_text_with` and `search_text_stream_with`. They accept a normal `TextQuery` plus registered representation and operator identities. The query's `pattern` is passed as one JSON string to the operator's `compile` method. Matching retains leaf boundaries, canonical fields and concrete array paths. The engine retains the existing candidate, batch, cache, sorted page, result and provisional streaming paths. The registered path never changes `TextQuery`, `JsonQuery`, views or CLI behavior when unused. A compiled bound text identity contains both descriptors and the parameter value; scope, fields and the bound JSON filter also enter cache identity. A cache hit still reads and checks returned-page sources. Errors and callback cancellation do not publish a completed verified result. Built-in index reuse adds no index columns or snapshot payload. The existing secondary snapshot checks its schema/field configuration and encoding before reuse; changed slots remain in the overlay.

The typed-extractor contract is consumed by `build_derived_posting`, `derived_value_counts`, and `query_derived`. Registered source providers and format decoders are consumed by `source_descriptors`, callback-scoped `with_source_record`, and `read_source_leaves`. Reads check descriptor currency before and after provider I/O; the convenience leaf list is exposed only on complete success. Default JSON descriptors use the existing secure, descriptor-relative filesystem reader; `JsonFormatDecoder` visits Sonic values. Provider selection can discover a collection containing only custom source files, with or without a JSON Schema describing its logical fields. `reload` updates collection membership after provider additions or removals. Builder opening and reload validate collisions and ambiguous decoder selection; a collision introduced later is reported on the next descriptor inventory. The indexed `Source`/`FilePicker` and legacy query paths still discover `.json` files only. A legacy query or `refresh` over a collection with selected provider records returns an explicit unsupported error rather than silently omitting them. Custom sources are not yet queryable or included in index snapshots. Query-frontend registrations are still type checked but are not yet executed.

## Derived typed values

A registered `ValueExtractor` receives borrowed `RecordAccess` and appends values to a worker-reused vector. `path_present` distinguishes a missing path from a present empty array; `visit_path` visits scalar leaves in source order and stops on callback errors. The native Sonic projection and direct JSON evaluation expose the same canonical number spellings. An extractor reports `Missing`, `Present` with zero or more values, explicit `Null`, or an error. Emitting values while reporting `Missing` is an error. Invalid decimal output is an error. An extractor with `Volatile` dependencies can be used by direct evaluation but cannot create a persistent posting.

`Repository::build_derived_posting` builds one named posting over the current collection using the existing B-tree and one source read/parse per record. It does not create a second index store. `Repository::derived_value_counts` counts distinct records per non-null value. `Repository::query_derived` supports direct and indexed execution, exact equality/inequality/ranges, existential matching over multiple outputs, min/max sort reduction, stable ID ties, count and paging. `AnyValue` matches a null output but not a missing path or a present path that emits no values. `First` and `Last` sort reductions are rejected for this indexed API until source-order information is retained in the posting. Direct and indexed modes return identical ordered IDs and errors in the external tests. The indexed path walks posting keys without per-record vectors; returned page records are read again and checked against their recorded size and modification stamp before return.

The internal posting path begins with a reserved marker, so it cannot alias a stored JSON path. It contains the extractor descriptor, collection and schema binding fingerprint; these stable bytes enter the existing secondary snapshot. A changed descriptor or schema binds a different path and does not reuse the old posting. Refresh patches active derived postings from retained old keys before inserting new values; inactive saved definitions are marked stale. Cached complete ordered lists include the bound path, predicate and sort terms. `MetadataOnOpen`, `MetadataOnRequest`, `HashOnRequest` and the normal query-cache budget still govern reuse; cached pages read only the returned source records.

## Existing behavior that extensions must preserve

Standalone text search visits every included JSON scalar leaf. Strings are stored text unless a schema declares a visible format; numbers, booleans and null have their JSON value text. Objects and arrays contribute separate nested leaves, never one concatenated exact-match stream. `exclude_fts` excludes declared descendants from standalone search. Returned hits identify canonical field paths and concrete array member paths. Malformed JSON has a special `$raw` fallback in standalone text search, but no JSON paths for field predicates.

`TextMode::Word` is a literal substring of visible normalized text. Markdown, HTML and XML render with the existing parsers; inline joins and block boundaries are preserved. Unicode punctuation becomes spaces, symbols remain, whitespace collapses, and Unicode case folding applies unless case sensitive. The literal query is normalized as plain text; regex syntax is not. `WordRaw` is a literal substring of stored decoded values. `Regex` is a Rust regex over those stored values and controls its own case flags. JSON `:fts` has its existing comparison/LIKE rules; `:ignorecase(...)` in `:fts` is rejected. Ordinary non-FTS LIKE remains its existing operation. A custom operator must declare its own exact semantics rather than reinterpret these modes.

Indexes are conservative filters: adjacent and skip-one bigrams, ASCII or UTF-8 byte encoding, folded projections, registered field filters, typed B-tree postings, and changed-file overlays narrow work only when safe. Source verification remains authoritative unless an exact typed posting proves membership. An absent candidate condition means unknown, while an empty exact set means no matches. `AND` may retain available necessary conditions, `OR` requires every branch, and `NOT` cannot complement an approximate set. Missing JSON values never satisfy comparisons, including `!=`; null and missing have separate posting keys. Scalar comparisons can reject array cardinality even when an existential comparison would accept several values. Numeric ordering uses exact decimal comparison. Sorts use current collation, reduction, count caps and stable ties.

`MetadataOnOpen` checks the source at first index use and relies on explicit refresh thereafter. `MetadataOnRequest` repeats metadata reconciliation before reuse. `HashOnRequest` rebuilds on each request and bypasses the verified-query cache. Ordinary streaming verifies candidates even after a provisional hit; `Options::stream_query_cache` opts into the same verified cache reuse and freshness limit as collected queries. Cache lookup itself performs no source inventory. Refresh, rebuild and configuration reload invalidate affected collection entries. The current source reads use directory containment, no-follow descriptor opening and the documented before/after consistency checks; extension adapters must preserve equivalent guarantees without changing the default path.

## Copyable text example

`crates/extension-tests/src/lib.rs` is a separate consumer using only public `objekte-core` APIs. It registers a `TextRepresentation` that trims ASCII edge whitespace and a compiled, case-sensitive prefix `QueryOperator`. The operator offers a raw bigram necessary condition; the engine uses it with `objekte:stored` and scans for the independent trimmed representation. The same crate registers the built-in Markdown, HTML and XML adapters and checks result parity with legacy queries. A minimal call looks like:

```rust,no_run
use objekte_core::{Repository, TextQuery, extensions::StoredRepresentation};
use objekte_extension_tests::PrefixOperator;

let mut repo = Repository::builder("my-repository")
    .register_representation(StoredRepresentation::default())?
    .register_operator(PrefixOperator::default())?
    .open()?;
let result = repo.search_text_with(
    &TextQuery { pattern: "Archive".into(), ..Default::default() },
    "objekte:stored",
    "example:prefix",
)?;
# Ok::<(), objekte_core::Error>(())
```

## Copyable typed extractor example

`PublicationYearExtractor` in the separate `objekte-extension-tests` crate accepts exactly `YYYY` or `circa YYYY` from `publication_date`; ambiguous ranges such as `1848–1850` are errors. It uses only public core traits. A caller can register and query it as follows:

```rust,no_run
use objekte_core::{DerivedBackend, DerivedPredicate, DerivedQuery, Repository, extensions::TypedValue};
use objekte_extension_tests::PublicationYearExtractor;

let mut repo = Repository::builder("my-repository")
    .register_extractor(PublicationYearExtractor::default())?
    .open()?;
let result = repo.query_derived(&DerivedQuery {
    collection: "books".into(),
    extractor: "example:publication_year".into(),
    predicate: DerivedPredicate::Ge(TypedValue::Number("1848".into())),
    sort: None,
    offset: 0,
    limit: 20,
    count: true,
    backend: DerivedBackend::Indexed,
})?;
# Ok::<(), objekte_core::Error>(())
```

## Copyable source and format example

`MarkdownProvider` and `MarkdownDecoder` in `crates/extension-tests/src/lib.rs` select direct `.md` files and expose each as one `/body` leaf. The provider checks a stable revision before and after a bounded read and opens files without following final-component symlinks on Unix. The decoder rejects invalid UTF-8. A minimal call is:

```rust,no_run
use objekte_core::Repository;
use objekte_extension_tests::{MarkdownDecoder, MarkdownProvider};

let root = std::path::Path::new("my-repository");
let mut repo = Repository::builder(root)
    .register_source(MarkdownProvider::new(root.join("data/books")))?
    .register_format(MarkdownDecoder::default())?
    .open()?;
let source = repo.source_descriptors("books")?.into_iter()
    .find(|source| source.record_id == "book-1")
    .expect("book-1.md is selected");
let leaves = repo.read_source_leaves(&source)?;
# Ok::<(), objekte_core::Error>(())
```

The source and format API currently supports inspection. Index and query integration, custom-only collection discovery, and frontend plan execution remain under implementation.
