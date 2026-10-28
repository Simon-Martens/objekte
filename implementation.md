# Implementation path

This is the implementation contract for the clean Rust library rebuild. The old
library and CLI have been replaced; the NDJSON experiment is retained only in the
historical benchmark crate, with no compatibility requirement on the new API.
The broader [MVP](MVP.md) describes later capabilities; this document determines
the first supported subset and the order of work. Stages A–F are implemented in
`crates/core` and exercised by `crates/cli`; see the [test guide](crates/core/tests/README.md). Query dependency tracking,
query-specific result caches, cost models, and adaptive index selection are later
work, not prerequisites for this implementation.

The library owns configuration, validation, collection and record relationships,
file discovery, hashes, cache synchronization, index construction, and view data.
The CLI supplies options, invokes those operations, and formats their results as
tables or machine-readable output. Tests and benchmarks call the same library.

## 1. First data and configuration profile

### Musenalm data

All seven existing Musenalm JSON collections are included. Simplify their schemas
and UI options, not their source records. All existing fields remain available
through record reads, even when a default table shows only a few scalar fields.
HTML and Markdown are ordinary strings: no rendering, cleaning, interpretation,
or conversion is needed for the first version. Dates and URLs also remain strings.

| Collection | Files | What it exercises |
| --- | ---: | --- |
| `places` | 210 | Small first end-to-end fixture; strings, numbers, booleans |
| `agents` | 4,010 | More records, long strings, opaque JSON including null |
| `series` | 1,202 | Arrays of references and embedded membership objects |
| `entries` | 4,928 | Large nested records, embedded arrays, nested references |
| `contents` | 133,948 | Large file count, numeric sorting, fractional numbering |
| `items` | 4,180 | String arrays, booleans, owner filters |
| `pages` | 8 | Small collection with an opaque `data` field |
| **Total** | **148,486** | |

Start the first end-to-end implementation with `places`, then `agents`, then
relations in `series`/`entries` and `contents`. All seven are valid input fixtures
from the start. Small generated fixtures supply cases the example does not have,
including collection cycles, missing targets, and missing versus null values.

`contents.numbering` contains 1,038 fractional values. Numeric support cannot be
restricted to integers or implemented as string sorting. Embedded contents inside
an entry and standalone contents files have different shapes; do not merge them
or infer a relationship from an embedded `source_id`.

### Repository and collection files

Keep `.editor/repository.json` with `version: 2`, the repository UI path, and an
explicit collection-name-to-settings-path map. The Musenalm map registers all
seven collections. Only registered collections are managed.

The initial collection settings contain `type: "data"`, `format: "json"`, a source
directory, `id: "filename"`, and schema/UI paths. Paths resolve relative to the
repository root. The first layout is a flat directory of `<id>.json` files; reject
unsupported layouts explicitly. Identity is `(collection, filename stem)`.
Validate path containment and collection registration before reading records.

Named documents, alternative identity mappings, format adapters, operation
policies, and ID generation are later features. Musenalm's old instance settings
files are retained but are not registered as documents in this first profile.

### JSON Schema

Use the Draft 2020-12 dialect and a restricted feature profile. Use an established
validator for these standard constraints; the library adds profile checks and
relation semantics. A schema contains structural rules, never table layout.

| Keyword/shape | Initial meaning |
| --- | --- |
| `$schema` | Root dialect declaration |
| `type` | `object`, `array`, `string`, `number`, `integer`, `boolean`, `null` |
| Nullable type | One concrete type plus `null` |
| `properties` | Recursively declared object fields |
| `required` | Property presence; not a nonempty-string constraint |
| `additionalProperties` | Boolean; preserve the choice declared by the schema |
| `items` | One schema for all array elements, including embedded objects |
| `{}` | Opaque JSON value, including null; retained without invented structure |
| `x-relation: {"collection": "…"}` | A stored string ID referencing that collection |

Empty schemas are used only for genuinely opaque fields, such as imported `data`.
They do not make all known objects untyped. Preserve the declared properties,
required lists, and closed-object rules in the simplified Musenalm schemas.

Relations may occur in nested objects or array items. They target filename IDs.
A missing property, null where allowed, or an empty reference string produces no
record edge. A nonempty string naming a missing record produces an unresolved
edge and diagnostic. Shape validation and target existence are separate checks.

Retain source numeric values. The first numeric execution contract uses signed
64-bit integers and finite binary64 values, including fractions; extraction must
detect integers outside the supported exact range instead of silently rounding
them. The same typed values feed direct comparison and SQLite. Opaque JSON is
preserved even when its contents are not eligible as typed index fields.

Defer enums, patterns, format checks, defaults, content annotations, read-only
annotations, schema composition, and `$ref`/`$defs`. The simplified fixture uses
none of them. Reject unsupported schema keywords at a schema location with a clear
diagnostic; never silently claim their constraints were enforced. Removing these
constraints is deliberate for the first profile, not full validation of the old
editor contract. Editing/provenance policies must be specified before writes ship.

### UI options

Repository UI initially supplies only `defaults.pageSize`. Collection UI supplies
`label`, `display`, an optional `pageSize` override, and this list configuration:

```json
{
  "label": "Contents",
  "display": "preferred_title",
  "list": {
    "columns": ["preferred_title", "year", "numbering", "extent"],
    "filters": ["edit_state", "year"],
    "defaultSort": {"field": "preferred_title", "direction": "asc"},
    "sortable": ["preferred_title", "year", "numbering", "musenalm_id"]
  }
}
```

First list paths name top-level scalar properties directly. Nested fields and
arrays remain valid stored data; nested table projections and relation traversal
are later view features. `@id` is a reserved view identity selector, not a stored
property. Do not use the old `_label` indirection in the minimal fixture.

- `display` identifies the label field; absent/null labels display the ID.
- `columns` selects table fields, without causing them to be indexed.
- `filters` declares available controls and indexed fields. An inactive control
  does not filter the records. Strings/booleans use equality; numbers additionally
  support bounds. Substring and full-text search are later features.
- `defaultSort` specifies a stored scalar field or `@id` and `asc`/`desc`.
  Omission means ascending ID order. Sort the stored field, not its displayed ID fallback.
- Optional `subSorts` is an ordered array of `{field, direction}` keys after
  `defaultSort`. Fields must be unique; an explicit `@id` must be last. Append
  ascending ID if not supplied. Null/missing ordering applies at every level.
- `sortable` declares permitted user-selected sort fields. Omission permits the
  default primary and secondary fields. When supplied, it must include all
  non-ID default primary and secondary sort fields.
  ID sorting is always available. No implicit discovery of sortable fields.
- Default page size is 50; collection options and then explicit request options
  override it. Limit must be positive; offset must be nonnegative.

Compile these options into an effective view and an `IndexSpec` using schema
types. The index field set is exactly identity plus filter fields, all default sort keys,
and sortable fields. Validate every named field and type. There is no index
admission heuristic. `sortable` is specified now and implemented with view options.

Start with case-sensitive binary string ordering and typed numeric/boolean
ordering, without string/number coercion. Missing and null are distinct states;
they match no ordered comparison. Explicit null matches equality to null; a
missing value does not. Sort ordinary values in the requested direction, followed
by null, then missing, with ID breaking ties within each group. Encode these states
in SQLite sort keys so its defaults cannot change the result. Locale collation is
later work.

No translation system, recursive UI inheritance, widgets, child tables, rendered
markup, or automatic filters are needed yet.

## 2. Implementation sequence and acceptance gates

Each stage leaves a usable library operation and a thin CLI entry point. Complete
its correctness gate before moving to the next stage. The command names below are
implemented CLI entry points; use `objekte --help` for their options.

### Stage A — Parse configuration and derive fields

Implement typed repository/collection/UI options and a normalized schema model.
Keep file paths and schema locations on diagnostics. Normalize defaults once and
derive the effective list fields and `IndexSpec` without opening source records.
No persistent cache is required for this stage.

Algorithm: parse configuration, register collection identities, walk each schema's
`properties`/`items`, then resolve UI field names against the resulting field map.
The walk is proportional to schema size. Treat schema property names as data, not
as schema keywords. Keep arrays and opaque values explicit in the model.

Gate: all seven simplified Musenalm configurations load; unknown fields, targets,
types, keywords, and invalid options produce precise errors. Tests assert that
metadata loading performs no source-directory enumeration or record reads.
Expose the normalized fields and intended indexes through CLI inspection.

### Stage B — Collection relationships

Build a directed multigraph from declared `x-relation` nodes. An edge stores source
collection, field location including array steps, and target collection. Register
all collection names before resolving edges so declaration order is irrelevant.

Use adjacency lists and Tarjan's strongly connected components algorithm:
`O(collections + relation declarations)`. Preserve cycles and self-relations.
Condensing components gives an acyclic graph for diagnostics or later scheduling;
it is not an instruction to eagerly read every collection in a component.

The Musenalm graph includes:

```text
series.entries[*]                         -> entries
series.related_entries[*].entry           -> entries
entries.places[*]                         -> places
entries.agents[*].agent                   -> agents
entries.contents[*].agents[*].agent        -> agents
contents.contributors[*].agents           -> agents
```

Embedded `entries.contents` and `entries.items` are values inside an entry file,
not references to standalone collections. `source_id` and `editor` stay ordinary
fields unless a schema explicitly declares a relation.

Gate: exact relation paths are preserved for Musenalm. Synthetic tests cover an
unknown target, a self-reference, A→B→A, multiple edges, and unrelated collections.
CLI output can show the collection graph and its cycle groups without reading data.
Do not build record relationships or query dependency graphs in this stage.

### Stage C — Source reads, persistent manifest, and lazy collection cache

Give opening two explicit meanings:

| Operation | Work it is allowed to trigger |
| --- | --- |
| `load_repository` | Read configuration and schemas; derive metadata/collection graph |
| `refresh_configuration` | Compare/persist configuration digests and report affected collections; no source-record reads |
| `get_record(collection, id)` | Read/parse that file; cache its current bytes and schedule schema validation |
| `open_view(collection)` | On demand, enumerate/read/reconcile/cache the source collection and return its default table |

Calling `open_view` immediately after loading is a normal application opening flow.
A cold global sort generally needs all source records, even when only 50 rows are
returned. Load only requested collections. Do not follow every schema relation
while opening a source-only scalar view.

Create the cache lazily in the caller-supplied data directory. One SQLite database
per repository initially stores full record JSON, a manifest, and collection state.
Key records by `(collection, id)`. A partial cache populated by `get_record` must
not be mistaken for a complete collection inventory.

For each file persist relative path, ID, size, high-resolution modified timestamp,
SHA-256 of the exact bytes, and cache revision. On Unix also persist device, inode,
and ctime; platforms without those fields fall back to size/mtime. Only regular
JSON files are admitted. Store the hash algorithm and cache format version.

The default `metadata-on-open` policy performs one sorted inventory on first
collection use per handle. Hash the manifest using a versioned domain and
unambiguous structured encoding. If the persisted complete/current collection has
that fingerprint and compatible schema/mapping/index definitions, reuse it without
loading the database's per-record manifest or reading source files. A view-only
fingerprint change updates its metadata without rebuilding source rows or indexes.

If the fingerprint differs, load individual saved stamps and compare entries.
Read/hash new or changed candidates only; delete removed records. If bytes are
unchanged, update their stamps without reparsing or rewriting index values. Schema
changes revalidate cached JSON for metadata-unchanged records; index changes derive
new typed values from those cached bytes. Commit the new manifest fingerprint,
record count, records, relationships, and index state together. A failed operation
keeps the old manifest and marks the collection dirty. Partial, interrupted, or
dirty collections cannot take the shortcut; recovery reads/hashes all records.

Validate/open the collection directory once per scan. On Unix, open candidate
files relative to its pinned directory descriptor without following symlinks.
Check metadata on the opened descriptor before/after reading, compare against the
inventory entry, and reject unstable reads. Check the directory for membership or
identity changes before publication. Metadata mode enumerates once; it does not
claim an atomic filesystem snapshot or detect every edit invisible to its stamps.

`hash-on-open` remains strict: read/hash every file, including equal-metadata
entries, and retain the final whole-inventory stability check. `metadata-on-request`
and `hash-on-request` repeat their respective policies on every operation.
`refresh` bypasses session reuse but follows the selected policy. Cache format 2
rebuilds older disposable caches once.

Gate: unchanged reopen has one enumeration and zero source reads/DB manifest
loads; a single changed record causes one hash and targeted updates. Test metadata
changes with identical bytes, additions/deletions, schema/index-only changes,
failed transactions, dirty/partial recovery, directory replacement, and symlink
replacement after inventory. Strict mode must still find equal-size edits with
restored mtime. Unix metadata mode separately tests ctime detection of that case.

### Stage D — Individual record relationships

Compile relation field paths from Stage B into extraction steps. For each record
read during discovery/reconciliation, collect its references and store edges:

```text
(source collection, source ID, concrete field location, target collection, target ID)
```

Index edges in both directions. Preserve array occurrence locations so repeated
references can be explained. Store missing targets as unresolved references;
unknown/unscanned target collections are not evidence that a target is missing.
Filename identity permits checking a specific target path without scanning all
target records when a caller requests reference validation.

On an update, compare the old and new edge sets for that record, remove obsolete
edges, and add new ones in the same transaction as the record/manifest update.
Deletion removes outgoing edges; incoming references remain available as unresolved
references. Adding a formerly missing target resolves those references. References
inside embedded contents belong to the containing entry record.

Track whether outgoing references have been extracted for a whole collection or
only some records. Complete backlink requests may need to scan the potential
referring collections identified by Stage B. Cycles never cause recursive loading.
No denormalized relation labels or query result caches exist at this stage, so an
edited target does not require rewriting the unchanged source JSON of its referrers.

Gate: targeted edge updates, duplicate occurrences, unresolved targets later added,
deletion, nested arrays, record cycles, and explicit partial-coverage reporting.
Expose forward/backward relationships and their coverage through CLI inspection.

### Stage E — Indexed default views and table output

Create SQLite expression indexes from `IndexSpec` over the raw JSON in `records`;
there are no additional stored field columns or per-collection value tables.
Fetch record payloads only after choosing the requested page. Sonic-rs
decodes the requested columns and label from each page record while skipping
unselected payloads; separate persistent query projections remain later work.
Full source/schema parsing also uses Sonic, adapting raw numeric values into the
existing validation/API representation without a floating-point conversion. Preserve
last-member-wins duplicate keys and the existing fingerprint serialization.
Create ID, filter, default-sort, and sortable-field indexes when
the collection's view is first requested. Store an index-spec fingerprint. Update
the raw JSON with changed rows; SQLite maintains the corresponding index keys.

Use deterministic index construction. Start with `(field state, typed value, ID)`
keys for configured scalar fields and the default order. Equality filter plus
default sort may use a deterministically derived compound index. Avoid generating
the power set of filter/sort combinations. Source/reverse reference indexes are
also required for Stage D; they are bookkeeping, not a workload heuristic.

For this milestone, assume configured indexes provide adequate speed for every
supported list request. Do not add a cost model, optimizer research, query graph,
or query-specific caches to compensate for expensive combinations. This is a scope
assumption, not a benchmark claim: execute real queries, report actual timings,
and do not fabricate index usage or latency. Later profiling can identify queries
that warrant their own caches.

Default views support scalar equality/bounds, ordered primary/secondary sorts plus ID tie-break,
selected columns, and limit/offset. A request can override only declared sortable
and filter fields; invalid requests are diagnostics, not automatic schema/index
changes. Return collection, ID, source path, and row values. Source files remain
available for full record reads. Count is optional.

Persist a separate compound view-definition fingerprint with component hashes for
filter controls, configured sort order/permissions, and projection/label/page
settings. Hash schema and typed index definitions separately. Formatting-only
schema changes do not change semantic fingerprints. Domain tags version the
fingerprint contracts.

Each resolved request also returns `view_hashes`: actual filter fields, operators,
and typed values; ordered sort keys/directions including ID; selected columns,
display field, limit, offset, and count. The combined hash includes collection,
source mapping, and schema dependencies. AND-filter permutation/duplicates are
normalized; sort priority and column order remain significant. An explicit primary
sort replaces configured secondary defaults unless the request supplies
`sub_sorts`; `Some([])` explicitly clears secondary keys. These are definition
identities, not result caches or data revisions. Metadata source fingerprints and
collection revisions remain separate. No automatic SQL compound-index generation
is introduced by compound hashing.

Gate: direct source evaluation and SQLite give identical complete ordered outputs,
including fractions, ties, null, missing, empty strings, and booleans. Verify that
the intended indexes were created and expose SQLite plans for diagnostics; do not
require a particular plan for tiny fixtures. CLI prints actual view tables or JSON.

### Stage F — Targeted synchronization and failure recovery

Use one serialized update coordinator first. Reconciliation classifies path-set
differences as added/removed/possibly changed; hashes determine content changes.
Apply each stable source batch to records, manifest, typed values, and outgoing edges
in one SQLite transaction. Do not rebuild unchanged rows. A filename rename is a
deletion plus an addition because filename identity changed.

Persist states such as absent, partial, building, current, and dirty. Publish a
complete source generation after the selected freshness checks; validation is a separate per-record state. Interrupted
work remains untrusted; reopening reconciles it. A final check detects ordinary
concurrent changes but does not turn independent filesystem files into an atomic
snapshot. Bound retries and report continuing outside changes.

A schema change revalidates affected collections and rebuilds their extracted
values/edges. A UI index-field change rebuilds affected indexes. A label/column/page
size change alone need not rewrite the full record cache. A storage mapping or
cache format change invalidates the affected namespace/generation. Invalid JSON
replaces the old cached row with an explicit incorrect-JSON row. Content errors do not roll back a collection; operational failures still do.

Implemented configuration reconciliation uses two distinct checkpoints:

1. **Observed inputs.** `inputs(path,digest)` holds SHA-256 hashes of registered
   configuration/schema/UI files. `input_collections(path,collection)` records
   direct consumers, including every consumer of a shared file. Repository settings
   and root UI defaults conservatively depend on all registered collections.
2. **Applied cache configuration.** Collection schema/mapping/index/view hashes
   identify the versions applied by successful collection reconciliation. Each
   record's validation status also carries its requested schema hash. Observing
   inputs never advances these applied versions or declares validation complete.

At SQLite access, read the previous input manifest before writing anything. Compare
the union of old/new paths, classify added/modified/removed files, and union their
old/new consumer sets. Compare semantic collection hashes to determine necessary
work: a raw formatting change is observable without triggering revalidation.
Commit input digest and consumer additions/removals together. Before committing,
recheck the loaded files to reject a configuration that changed during loading or
while waiting for the writer. Invalid/missing required configuration and transaction
failures preserve the previous manifest. Removal from the registry permits removal
of its old files; files still referenced by another collection remain tracked.

`refresh_configuration()` exposes this operation independently of source scans;
`cache refresh-config` is its CLI driver. Normal cache access also performs it.
`load_repository` and inspection remain metadata-only and do not create a cache.
Existing format-4 caches gain the ownership table without discarding records; an
old changed input without ownership uses a conservative collection candidate set.

Collection work remains lazy. A new observed manifest followed by a crash, a failed
collection update, or opening only another collection cannot hide pending work:
the next use still compares persisted applied hashes with current semantic hashes.
`cache status` separates source coverage, configuration compatibility, applied/
expected schema hashes, and record-validation counts. Reports describe the latest
manifest comparison, not a durable event queue; another process may already have
observed the changes. Per-collection applied versions remain the recovery authority.
Data files use the separate source manifest algorithm above. No watcher or query
cache is needed for either kind of restart detection.

Watchers are a later addition to this mechanism. They mark paths dirty while the
process is running, coalesce repeated events, and trigger reconciliation. They
cannot establish freshness after a closed interval. Always retain startup/first-use
hash reconciliation, explicit refresh, and recovery after event loss/overflow.

Gate: reopen after edits while closed, equal-metadata changes, failed transactions,
interrupted builds, UI/schema changes, bulk replacements, and fresh versus reused
application directories. Check changed-row counts as well as returned values.

Source writes are a subsequent stage: expected-revision validation, atomic file
replacement, a persisted pending-write marker, and reconciliation after a crash
between source replacement and cache commit. Define editing/provenance policies
before enabling writes. Neither cache synchronization nor query execution edits
authoritative files.

## 3. Runtime options and CLI as the test driver

The next JSON-only query increment is implemented in [filters.md](filters.md):
schema-independent filter parsing, typed SQL predicates, global or scoped source
reconciliation, explicit path/collection dependencies, and a direct evaluator with
matching semantics. The `query` command calls this library API. Query-result caches,
adaptive index selection, metadata operands, and relation expansion remain deferred.

The CLI chooses the OS application-data directory and passes it to the library.
The library does not discover the user's home/cache location. Tests pass temporary
directories and may reopen the same one or use a different directory per case.
Namespace cached data by repository identity derived from the canonical repository
path; store that identity for verification. Moving/copying a repository can simply
start a new cache. Include cache format and schema/index fingerprints in validity
checks, not arbitrary fixed Musenalm field names.

Initial options, exposed through the CLI as their stages are implemented:

| Option | Behavior |
| --- | --- |
| Repository path | Explicit source repository |
| Application data directory | Explicit location for disposable library state |
| Cache mode | `disabled` or `sqlite`; default-view behavior is otherwise equivalent |
| SQLite page cache | `sqlite_cache_mib: Option<u32>`; default `Some(64)`, `None` uses SQLite's default; CLI `--sqlite-cache-mib N\|default` |
| Ingestion workers | `ingest_workers`, positive count; default CPU threads / 2 (minimum 1); CLI `--ingest-workers N` |
| Background workers | `background_workers`, positive count; default CPU threads / 4 (minimum 1); CLI `--background-workers N` |
| Validation | Library `Background` or `Manual`, `validate_all()`/`validate_collection(name)`; CLI `--validation background\|manual\|wait`; explicit `validate [COLLECTION]` (all when omitted); legacy `cache validate --collection NAME` |
| Fresh cache | Use a new cache namespace; never delete the whole application directory |
| Reconciliation | Default `metadata-on-open`; `metadata-on-request`, strict `hash-on-open`, or `hash-on-request` |
| Explicit refresh | Reconcile the requested collection now using the selected policy |
| Page/columns/filter/sort | Request overrides checked against the effective view |
| Output | Human-readable table, JSON, or NDJSON |
| Diagnostics/timings | Report reads, hashes, parses, reused/changed rows, indexes, and execution time |

The selected `*-on-open` policy verifies the requested collection on first use
per handle. It requires explicit refresh to observe later outside edits; `*-on-request`
checks each request. Metadata verification is weaker than full content verification
when files retain all recorded stamps. CLI diagnostics identify the policy and
report manifest hits, database manifest loads, candidate reads/hashes, cached row
reuse, and timings. `fingerprint_nanos` measures compound manifest hashing inside
`sync_nanos`; do not add it to the parent timer. UI watchers and scheduling remain
in [UI.md](UI.md).

Ingestion and background validation are library responsibilities. Fresh collection
workers perform stable reads, hashing and parsing in batches of at most 256 records
and normally 8 MiB source bytes (an oversized record runs alone). Ordered results go
to one shared writer. Partial/existing collections keep incremental preparation.
Syntactically incorrect JSON and schema-invalid values are retained, never omitted
or allowed to roll back otherwise readable records. Filesystem instability and DB
failures still abort the transaction. Source freshness (`current`) and record validity
are independent. WAL and `synchronous=FULL` remain enabled.

After ingestion commits, background workers validate bounded cached snapshots,
collect all schema/domain diagnostics and return results to the same writer. An
update compares `(collection, id, revision, digest, schema_hash)` and pending status;
stale results are discarded. Status updates touch compact `record_validation` rows
separate from raw payloads. Worker queues coalesce collection requests, while
pending records are durable in SQLite. Shutdown need not drain the collection; the
next use resumes pending work. The first implementation reparses cached bytes for
validation rather than retaining all parsed documents in memory. It never rereads
source files just to validate them. Separate timers report this extra decode work.

See [validation.md](validation.md) for states, index/reference behavior, exact DB
schema, API notifications and recovery semantics. Cache format 5 rebuilds old caches
and replaces materialized fields with JSON expression indexes; there is no deployed-data
migration framework yet. The next ingestion task is to eliminate digest lookups
already answered by the manifest. Separate typed-table writes have been removed; SQLite maintains JSON
expression indexes. Query comparisons on configured scalar fields use these indexes.
The filter grammar also supports field operands, :length/:each/:lower, and LIKE patterns.

Implemented command surface:

```sh
objekte examples/musenalm inspect --collections --indexes --output json
objekte examples/musenalm relations --collections
objekte examples/musenalm view places --data-dir /tmp/objekte-run-a --output table
objekte examples/musenalm view contents --data-dir /tmp/objekte-run-a --sort numbering --limit 20
objekte examples/musenalm view contents --data-dir /tmp/objekte-run-b --cache disabled --output json
objekte examples/musenalm cache refresh --collection contents --data-dir /tmp/objekte-run-a
objekte examples/musenalm cache status --data-dir /tmp/objekte-run-a --output json
```

The library parses/validates filter values and resolves view options. CLI table
formatting is presentation; it must not contain source enumeration, cache policy,
index selection, relation extraction, or a separate evaluator.

## 4. Verification and later work

Use small temporary repositories for exact assertions and all seven Musenalm
collections for real-data acceptance. Each cache configuration runs against the
same data and compares the complete requested result, not merely a count. Tests
must assert laziness and targeted updates with observable I/O/update counters.
Validate all source records against the minimal schemas without modifying them.

First performance exercises: cold default view, warm page, numeric ascending and
descending sorts (including fractional numbering), equality/range filters, reopen
without changes, reopen after a small change, and bulk reconciliation. Report source
inventory, hashing, parsing, cache/index construction, and view execution separately.
Use a second unrelated synthetic schema to ensure implementation remains generic.

Deferred: query dependency graphs and negative-result dependencies, query-specific
caches, incremental result-page repair, heuristic index admission, relation filters
and sorting, array query operators, grouping, computed values, locale collation,
full-text search, rendered HTML/Markdown, richer schema/UI features, and a desktop
UI. Introduce them after collection/record relationships and persistent index
synchronization are correct. The first delivered path is metadata → collection
graph → lazy source/cache reads → record edges → indexed default tables → reliable
reopen and refresh.
