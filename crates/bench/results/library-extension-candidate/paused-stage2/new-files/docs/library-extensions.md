# Library extensions

Objekte extensions are statically linked Rust application code. A repository file never authorizes loading or executing an extension program. Register definitions on `Repository::builder` before `open`; the definitions are shared immutably by a repository handle and its unindexed forks.

## Stable definitions and lifetimes

Each definition supplies a namespaced `Descriptor` such as `example:prefix`, a semantic version, a canonical configuration fingerprint, input/output kinds, and declared dependencies. `Descriptor::new` hashes serializable configuration with a versioned domain. Dependencies are sorted and unique. Duplicate identities, incompatible kinds, malformed identities, and empty external-version dependencies are rejected at registration. Registration order and runtime addresses are absent from fingerprints. Change the semantic version or configuration whenever result semantics change. A `Volatile` dependency disables verified-result caching for a bound text query. Definitions are `Send + Sync`; compiled operators are also `Send + Sync` and may be shared by worker threads.

`SourceProvider` enumerates `SourceDescriptor`s with separate provider, source, record, locator, format, revision and metadata identities, then reads into an engine-supplied byte buffer. `FormatDecoder` selects a descriptor and visits borrowed logical leaves. These contracts do not require a canonical JSON tree. A decoder may own one for convenience. Source providers must bound reads and define their own secure locator and before/after revision checks; borrowing from a read buffer ends with its callback. An ambiguous match between decoders must fail rather than silently select one.

`TextRepresentation` binds to a collection, field path, format and retained schema tree, then returns text borrowed from the source value or a reusable output buffer. `ValueExtractor` visits logical paths through `RecordAccess` and appends zero or more typed values to a reusable vector. `Extraction::Missing`, a present value with no outputs, `TypedValue::Null`, and `Err` are distinct. Number outputs preserve decimal spellings so the engine can apply its existing exact comparison. `QueryOperator` compiles parameters once into an immutable exact scanner and may offer a necessary index condition. A condition is only usable with an index built from the same representation semantics; unsupported conditions fall back to source verification. `QueryFrontend` compiles application requests into logical plans; it must leave candidate combination, scheduling, source reads, ordering, pagination, and cache ownership to the engine.

The public built-in adapters `StoredRepresentation`, `RenderedRepresentation`, and `NativeTextOperator` use the same concrete text renderer and scanner as legacy queries. They are useful for explicit parity checks. `StoredRepresentation` lends the decoded text directly. `RenderedRepresentation` supports plain, Markdown, HTML and XML; malformed XML returns no rendered value. A registered operator's `BigramLiteral` hint is currently accelerated only when it names `objekte:stored` and that exact representation is bound. Other hints are treated as unsupported and verified by scanning. This rule prevents a custom normalized representation from being narrowed by raw postings.

The working repository entry points are `search_text_with` and `search_text_stream_with`. They accept a normal `TextQuery` plus registered representation and operator identities. The query's `pattern` is passed as one JSON string to the operator's `compile` method. Matching retains leaf boundaries, canonical fields and concrete array paths. The engine retains the existing candidate, batch, cache, sorted page, result and provisional streaming paths. The registered path never changes `TextQuery`, `JsonQuery`, views or CLI behavior when unused. A compiled bound text identity contains both descriptors and the parameter value; scope, fields and the bound JSON filter also enter cache identity. A cache hit still reads and checks returned-page sources. Errors and callback cancellation do not publish a completed verified result.

The source-provider, format-decoder, typed-extractor and query-frontend contracts are registered and type checked, but the current repository execution path does not yet consume their registrations. They are included here to make the intended ownership and lifetime boundary reviewable; do not rely on those registrations to alter discovery, indexing or queries until their engine adapters are implemented. The current `Source` and `FilePicker` paths still discover `.json` files and reconstruct their locators from IDs.

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

The ordered value store, source-provider/decoder adapters, and frontend plan execution are still under implementation. Their examples will be added with their working engine paths.
