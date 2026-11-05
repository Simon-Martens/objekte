# Objekte Rust library API and extension contracts

This is the external consumer reference for `objekte-core` at repository revision `52f849d93b` (crate version `0.1.0`). It describes the public Rust surface that another statically linked crate can call or implement. Here **ABI** means the Rust library contract: types, methods, traits, ownership and semantic identities. Objekte does not expose a C ABI, a stable binary layout, a dynamic plugin loader, or a promise that a compiled extension can be loaded into a different executable. Recompile downstream crates with the compatible `objekte-core` version. The source of truth for exact compiler signatures is `crates/core/src/lib.rs` and its public `bigram`, `extensions`, and `file_picker` modules.

All examples use `objekte_core` as the crate name. Unless shown otherwise, `Result<T>` means `std::result::Result<T, objekte_core::Error>`. `&str` inputs are borrowed for the call; `Path` inputs are filesystem paths; `serde_json::Value` is the JSON-compatible logical value type. Methods taking `&mut Repository` may warm indexes, reconcile sources, update metrics, or populate caches. They do not write source records. Methods returning `Option<T>` distinguish an absent/unavailable value from an error.

## Contents

1. [How the engine fits together](#how-the-engine-fits-together)
2. [Opening and registering](#opening-and-registering)
3. [Ordinary queries and views](#ordinary-queries-and-views)
4. [Indexes, snapshots, refresh and inspection](#indexes-snapshots-refresh-and-inspection)
5. [Extension interfaces](#extension-interfaces)
6. [Public helper types and methods](#public-helper-types-and-methods)
7. [Low-level bigram and JSON picker APIs](#low-level-bigram-and-json-picker-apis)
8. [Complete consumer workflows](#complete-consumer-workflows)
9. [Errors, lifetime rules and compatibility](#errors-lifetime-rules-and-compatibility)

## How the engine fits together

The repository owns collection configuration, the source inventory, index storage, query planning, execution, result caches, pagination and refresh. An extension supplies a named **definition** for one boundary: selecting sources, decoding one source into logical fields, rendering a text value, extracting typed values, matching a value, or translating an application request into a plan. Registration happens before `RepositoryBuilder::open`.

```text
builder registrations + repository configuration
              ↓
collection/source selection → SourceDescriptor inventory and generation
              ↓
provider read → format decoder → borrowed logical record
              ↓
bound text/typed projections → engine-owned bigram/B-tree postings
              ↓
ordinary query or compiled frontend → candidates → exact verification
              ↓
engine-owned order/count/page cache → returned record/page
```

For a JSON-only collection, ordinary queries retain the concrete JSON picker, Sonic projection and native scanners. A custom-only or mixed collection uses the selected descriptor inventory; each record is read from its actual provider and decoded by its selected format. Registering a provider/format is sufficient for ordinary JSON/text queries, views, refresh and save/reopen to see its logical records. An application uses `query_frontend` only when it wants its **own request syntax or composition**, not merely because a source is Markdown or another format.

The engine uses conservative index candidates: a candidate may still fail exact verification; a noncandidate may be skipped only when the operator explicitly promises that doing so cannot suppress an error. An unsupported constraint means scan. Query cache entries are keyed by used definitions and source generation. A completed ordered cache hit slices the requested page and reads only those returned records under `MetadataOnOpen`; request-time freshness policies also validate their inventory.

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
| `fts: FtsOptions` | `mode: TextMode` and `case_sensitive` for `:fts` comparisons in this filter; ordinary non-FTS LIKE retains its separate meaning. |

`JsonQueryResult` contains `records: Vec<Record>`, optional `total`, `truncated`, `elapsed_ms`, `dependencies: JsonQueryDependencies`, `filter_hash`, per-query `metrics`, and `query_plan`. `JsonQueryDependencies` lists scoped collections, referenced `JsonPath`s and metadata fields. `Record` contains `collection`, `id`, logical `path`, byte size, exact decimal-string modification nanoseconds, content `hash`, logical `data: Value`, `state: RecordState`, and optional `raw` bytes for malformed JSON.

Streaming variants preserve query/error behavior and return `JsonQueryStreamSummary` (`returned`, optional `total`, `truncated`, `elapsed_ms`, dependencies, filter hash, metrics and plan):

| Function | Callback argument meaning |
| --- | --- |
| `query_json_stream(request, on_record: impl FnMut(usize, &Record) -> Result<()>)` | `request` is a `JsonQuery`; `on_record` receives the zero-based emitted position and a borrowed record. Returning `Err` stops the stream. |
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
| `collections: Option<Vec<String>>` | Same scope convention as `JsonQuery`. |
| `fields: Option<Vec<String>>` | Optional canonical leaf paths such as `body` or `contributors[].note`; empty selection is invalid. `None` searches included leaves. |
| `sort: Vec<Sort>` | Optional final ordering terms; empty uses the existing text-search order. |
| `offset`, `limit`, `count` | Page start, positive page size, and optional total, with the repository's `total_limit` cap. |

The corresponding callbacks are `search_text_stream(request, on_hit: FnMut(usize, &TextHit) -> Result<()>)`, `search_text_stream_with(request, representation, operator, on_hit)`, and `search_text_stream_updates(request, on_page: FnMut(&[TextHit]) -> Result<()>)`. The borrowed hit/page cannot be retained without cloning. Each returns `TextQueryStreamSummary` with `returned`, optional `total`, `truncated`, `elapsed_ms`, metrics and plan. A registered operator's `Err` is propagated even when its bigram hint excludes the failing value, unless that operator explicitly permits skipping it.

Markdown, HTML and XML fields declared by the schema use their visible representation for `Word`: markup is removed, logical value boundaries remain separate, punctuation and whitespace are normalized, and Unicode case folding applies unless case sensitive. `WordRaw` and `Regex` see stored decoded values. Malformed JSON has its existing `$raw` standalone-search behavior; a custom decoder declares its own failure policy.

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
| `write_inferred_schema(report: &mut InferenceReport) -> Result<usize>` | Write inferred schema files from a prior report; returns written count. Native picker operation. |
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
| `decode(&self, bytes: &[u8], visit: &mut dyn FnMut(&dyn DecodedRecord) -> Result<()>) -> Result<()>` | Decode the borrowed source bytes and call `visit` for the logical record during the call. The current ordinary inventory supports one source producing one record. Callbacks may fail; propagate the error. |
| `FormatDecoder::visit_leaves(&self, bytes, visit) -> Result<()>` | Optional convenience/default: calls `decode`, then the record's leaf visitor. |
| `DecodedRecord::visit_leaves(&self, visit: &mut dyn FnMut(&str, ScalarValue<'_>) -> Result<()>) -> Result<()>` | Visit scalar leaves at JSON-pointer paths. This is required. A callback error stops visitation. |
| `DecodedRecord::visit_nodes(&self, visit: &mut dyn FnMut(&str, LogicalNode<'_>) -> Result<()>) -> Result<()>` | Optional richer traversal for ordinary owned records: emit parent `Object`/`Array` nodes before children, and `Scalar` leaves. Default adapts `visit_leaves`. Needed to preserve empty containers or numeric object keys. |
| `RecordAccess::path_present(&self, path: &JsonPath) -> Result<bool>` | Does the path reach a value, including an empty array/object? Required for correct missing/no-output distinction. |
| `RecordAccess::visit_path(&self, path, visit: &mut dyn FnMut(ScalarValue<'_>) -> Result<()>) -> Result<usize>` | Visit reached scalar values in source order; return number visited, propagate callback errors. Empty containers visit zero. |

`ScalarValue` is borrowed `Null`, `Bool(bool)`, `Number(&str)` or `Text(&str)`. Number text must be a valid JSON number and preserve exact decimal meaning. `LogicalNode` is `Object`, `Array` or `Scalar(ScalarValue)`. JSON pointers use `/body`, `/items/0/title`; escape `~` as `~0` and `/` as `~1`. Emit arrays in index order without gaps. Borrowed strings and record resources must remain alive through the callback and must not escape it. `RecordAccess` is already implemented for `serde_json::Value` and Sonic values, so a convenience decoder may delegate to an owned tree, but native JSON queries are not forced through such a tree.

### Text representation: `TextRepresentation`

| Method | Arguments, return and contract |
| --- | --- |
| `descriptor(&self) -> &Descriptor` | Stable `Text → Text` semantic identity. |
| `binds(&self, context: &BindingContext) -> bool` | Applicability to resolved `collection`, canonical `path`, format name and optional retained schema JSON (including custom annotations). Binding is compiled once where inputs are invariant. |
| `preparation(&self) -> TextPreparation` | `Stored` (default) or `VisibleNormalized` to tell the operator what it receives. |
| `native_index_projection(&self) -> Option<NativeTextProjection>` | Optional proof of byte-for-byte equivalence to engine-owned `Stored` or `VisibleNormalized(TextFormat)` projection. Default `None` means scan fallback. A false claim can omit matches. |
| `render<'a>(&self, value: &'a str, output: &'a mut String) -> Result<Option<&'a str>>` | Render one logical string. Return borrowed input or reusable output, or `None` if no text representation. Do not retain either borrow. Errors propagate. |

`BindingContext` fields are `collection`, `path`, `format`, and `schema: Option<&Value>`. Here `format` is the **field's text rendering format** (`plain`, `markdown`, `html`, `xml`, or `derived` for a typed binding), not the source decoder's format identity. `schema` is the retained resolved JSON Schema tree, including declared `x-` annotations. `TextFormat` is `Plain`, `Markdown`, `Html` or `Xml`. Built-ins are `StoredRepresentation::default()` and `RenderedRepresentation::new(format) -> Result<Self>`; the latter selects a visible normalized renderer for the chosen format. `NativeTextProjection` is a candidate-index compatibility declaration, not merely a display label. Index construction and exact verification must use the same bound representation. Logical strings and array members remain separate matching streams.

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

Schema types exposed by `lib.rs` include `Field` (kind, nullability, indexing, language, sort reduction, text format, exclusion, enum/suggestions, children and relation target), `ValueType`, `TextFormat`, `PathStep`, and `SortReduce`. `Direction::{Asc,Desc}` and `Sort { field, direction }` describe ordering. Validation uses `RecordState { status, issues }`, `RecordStatus::{NoSchema,Pending,Valid,Invalid,IncorrectJson}`, `ValidationIssue { location, schema_path, code, message }`, and `ValidationMetrics`. Configuration-change types report changed input path/kind/digests and affected collections/views. `InferOptions { auto_index, write_schema }` controls explicit native field inference; `InferenceReport`, `CollectionInference`, `FieldInference`, `InferredKind`, `InferredLink` and `InferredLinkKind` expose its results. `CollectionGraph`/`CollectionRelation` expose declared relationship edges, connected components and cycles. All are result/configuration data, not extension callback entry points.

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

`Error` variants are `Io { path, source }`, `Config { path, location, message }`, `Record { path, location, message }`, `Request(String)`, `Unstable(PathBuf)`, and `Cache(String)`. Propagate extension I/O, decode, extraction, scanner and callback failures as errors. `Unstable` means a source changed during a consistency boundary. A failed refresh/build does not publish partial selected postings; a failed query/stream does not publish a completed result cache entry. A stream cannot retract records emitted before its terminal error.

Borrowed `ScalarValue`, `DecodedRecord`, input bytes, renderer output, worker scratch and streamed records/pages are valid only for their callback/call. Return owned copies if application state must retain them. Definition objects are `Send + Sync`; compiled operators are immutable `Send + Sync`; mutable scratch is worker-local. Do not put a global lock in byte/character loops or retain a pointer to a reused input buffer.

Source/provider/decoder identities, applicable selection precedence, schema mapping, representation/extractor/operator versions/configuration, compiled parameters and source generation determine cache/snapshot compatibility. Used volatile dependencies disable forbidden reuse. Native JSON snapshot bytes are unchanged; repository-selected manifest version 1, selected text companion manifest version 2, and selected derived magic `OBJDRV02` reject older/incompatible bytes as cache misses. Cache-on page changes reuse compatible ordered results without copying all slots; cache-off and `HashOnRequest` still perform exact work. For custom collections, active selected postings currently rebuild on refresh rather than patching individual slots. Native `build_posting`, `diff`, `apply_source_changes`, `record_revision`, picker `value_counts` and inference are JSON-picker-specific; use ordinary `refresh`, selected descriptors, and named derived APIs for custom sources.

For behavior-level details and the current extension consumer examples, see [Library extensions](library-extensions.md), [Implementation](implementation.md), and `crates/extension-tests/src/lib.rs`. The reference above intentionally lists external calls and contracts; private `pub(crate)` functions, inaccessible items inside private modules, and CLI/benchmark-only entry points are not part of the downstream Rust API.
