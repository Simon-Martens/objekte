# Local record search core

This workspace builds the new `crates/core/` crate. The archived project remains in
`deprecated/`, and the source fixture remains in `examples/musenalm/`. The new crate
implements record-oriented search over local byte ranges; the Musenalm schema
adapter and workload tools live under `crates/core/examples/`.

## Build and run

Run these from the repository root with Rust 1.99 or newer:

```bash
cargo fmt --all --check
cargo test --workspace --all-targets
cargo run --release --example musenalm_cli -- --case Q031 --explain-analyze
cargo run --release --example musenalm_cli -- --collection contents --fts Frühling --alphabet ascii --quiet-hits
cargo run --release --example accept_musenalm -- --alphabet ascii
cargo run --release --example accept_musenalm -- --alphabet unicode
```

`musenalm_cli` accepts `--root` (default `examples/musenalm/`), `--case`, or
separate `--collection`, `--fts`, and `--filter` inputs. `--explain` shows the
chosen plan without execution; `--explain-analyze` adds actual work. `--reference`
runs the independent scanner. `--alphabet` chooses `ascii` or `unicode`;
`--return` chooses `locator`, `bytes`, or `logical`; `--limit` and `--offset`
override the page. `--quiet-hits` prints counts and work without every locator.
The fixed cases and their options are in
[`musenalm_queries.json`](../crates/core/benches/musenalm_queries.json).

The benchmark tool provides `--cases`, `--profiles`, `--repetitions`,
`--result-dir`, `--root`, and `--verified-cache-bytes`. Its four standard
profiles are `ascii-off`, `ascii-warm`, `unicode-off`, and `unicode-warm`;
the warm profiles default to a 64 MiB verified-result cache. A custom cache
budget is recorded with the run and its actual hits or misses. Use a fresh
result directory for each run:

```bash
cargo run --release --example bench_musenalm -- \
  --profiles ascii-off,ascii-warm,unicode-off,unicode-warm \
  --repetitions 5 --result-dir /tmp/musenalm-search-run
```

The [final benchmark](../bench/results/musenalm/final-8dd0c2aa6d/REPORT.md)
contains the exact measured revision, 3,000 raw request samples, resolved
queries, plans, cache outcomes, and build/restore measurements. The
[C26 comparison](../bench/results/musenalm/ascii-mixed-comparison/REPORT.md)
records the measured ASCII mixed-literal correction. Correctness was checked
separately against the reference evaluator; see the
[final acceptance logs](../bench/results/musenalm/acceptance-final-8dd0c2aa6d/REPORT.md).

## Source and registration model

`FileInventory` assigns repository-wide file IDs and keeps native relative
paths, size, modified time, optional created time, and collection membership.
Each immutable `CollectionGeneration` owns independent record slots, record
IDs, original byte ranges, a file-to-record offset table, and reverse
record-to-file/range entries. Multiple records can share one file, and one
file can participate in more than one collection. A JSON reader frames one
standalone object per file; a JSON Lines reader frames independently parseable
object lines. Source extraction parses each framed record once and feeds the
postings, text filters, sorting columns, and inventory from that value. A
shared file is parsed once across its participating collections during build.
Built-in and caller-supplied readers are resolved through `SourceRegistry`
when a collection opens. A registration has a stable ID, semantic version, file
extension, and reader. The prepared reader handles framing, query verification,
and payload decoding. Snapshot configuration includes the resolved registration
identity and version, so a changed registration rebuilds its state.

`Repository` publishes its inventory, collection generations, definitions, and
indexes through shared-reference inspection methods. Callers cannot replace or
mutate those structures after opening. The repository owns prepared and
verified caches for that immutable set of generations. The verified cache
budget is the only runtime cache control; `verified_cache_budget()` reports
its effective value, while `options()` reports the opening configuration.
Future complete-file replacement must construct a new generation with both
file-to-record and record-to-file mappings before publishing it.

The Musenalm adapter reads the actual seven `.objekte/collections/` schemas.
Each declares its collection path, JSON format, and filename identity. It
traverses nested fields and arrays, translates `x-index-paths` selectors,
registers explicit `index` paths, and rejects unsupported format, identity,
type, or unresolved reference declarations. Its `@record.id` is each filename
stem; embedded arrays remain part of their parent record. The current fixture
has 148,486 records: 4,010 `agents`, 133,948 `contents`, 4,928 `entries`,
4,180 `items`, 8 `pages`, 210 `places`, and 1,202 `series`.

The fixture adapter registers sort columns for `agents.name`,
`contents.numbering`/`preferred_title`, `entries.year`/`preferred_title`,
`places.name`/`@file.size`, and `series.title`. It also registers the test-only
`agents.name:folded` binding (version 1): query and record strings use the same
Unicode case fold, a separate folded bigram filter narrows candidates, and
the scanner verifies them. The generic core has no Musenalm names or
`x-objekte` vocabulary. Only JSON is required by this fixture; the core also
has a JSON Lines reader.

Internal named field operations can be registered through `Schema` with a
stable name and version. They define accepted logical types and operators,
FILTER and FTS literal preparation, record-value and text mapping, and whether
posting or bigram indexes cover their values. An unmodified field may select a
configured default operation. Unscoped FTS may select a configured raw-text
operation; each extracted text segment remains separate. A missing or partial
index falls back to scanner evaluation. The binding version and default/raw
selection participate in snapshot compatibility; prepared and verified caches
belong to the opened repository generation.

Custom source readers own their extracted text segments, including omissions
and segment boundaries. A temporal schema declaration does not replace those
segments with strings gathered from the logical value. The built-in JSON and
JSONL readers explicitly derive logical string segments after temporal
conversion, so a coerced date is not indexed as ordinary text. Whole-record
raw bigrams can restrict an ordinary field predicate only when the reader
declares intact coverage of every logical string and the indexed representation
matches the predicate.

Collection discovery has explicit recursion, include/exclude globs, hidden-file,
and symlink settings. Its defaults select regular files directly in the
declared directory with the registered reader's extension. Following links is
confined to the collection root, with cycles and duplicate physical files
skipped. Fresh construction and startup reconciliation use the same discovery
rules. Identity can come from the filename, a configured intrinsic field
extracted by the reader, or a revision-bound generated ID. Duplicate IDs and
explicit IDs in the generated namespace fail construction. An empty JSON file
contributes zero records; other invalid selected records fail construction
with their path/range.

## Query and result semantics

FTS is a separate literal substring input, with optional selected fields and
bindings. It does not tokenize words or infer a query language from the
literal. Standard matching is case-sensitive. Built-in `lower` and the
registered `folded` binding apply their specified transforms to both sides.
FILTER uses the [documented grammar](filter-grammar.md): `&&`, `||`, `!`,
parentheses; typed comparison and contains operators; `?` existential forms;
and `lower`, `length`, `each`, `null`, `exists`, or registered modifiers.
Unquoted leading `@` names select metadata, while bracket-quoted names remain
data fields. `~` supports `%` as its only wildcard. Missing, null, empty
array, and empty string are distinct. Bindings validate all branches before
execution, including branches a Boolean expression may short-circuit. For a
multi-valued path, the default comparison is universal across its elements;
`?` operators are existential. `:each` explicitly requires a multi-valued
field and uses universal comparison. An empty present array satisfies a
universal comparison but not an existential one; a missing path satisfies
neither. `:exists` tests path presence, `:null` tests an explicit null, and
`:length` compares an array's length. Negative operators apply per selected
element under the same quantifier, rather than negating the entire record
predicate.

Declared field types, required status, field nullability, and per-array-level
element nullability are checked during extraction before typed indexes receive
a record. Readable malformed or invalid records become diagnosed raw fallback
records; valid JSONL siblings remain logical records. Reader contract violations
and duplicate explicit identities still fail construction. Query projections validate only decoded
fields. Unhinted arrays infer a common non-null element type; empty arrays
retain an unresolved element type until a declaration supplies one.
Well-formed unhinted data paths use a dynamically typed scanner, including
schema-free collections. They retain each record's actual scalar type, do not
infer dates from strings, and use the existing missing and array quantifier
semantics. Declared fields still validate literals strictly; unknown named
modifiers remain errors.

Values include strings, booleans, objects, typed arrays, null, exact `i128`
integers, finite fractional numbers, dates, datetimes, and Unix-second values.
Declared temporal fields are coerced explicitly; timestamp-looking strings
in Musenalm remain strings. `@file.created` and `@file.modified` are typed
metadata. Numeric comparisons preserve large JSON integers and fractional
values. Array terms use the documented existential/universal behavior; nested
elements are evaluated within their parent record.

Reserved metadata paths are `@record.id`, `@record.byte_start`,
`@record.byte_end`, `@record.byte_length`, `@file.id`, `@file.path`,
`@file.size`, `@file.created`, `@file.modified`, `@file.status`,
`@record.class`, `@collection.name`, and
`@repository.root`. File paths use a reversible percent-escaped query
representation for native non-UTF-8 names. Dates and datetimes require
explicitly typed fields and quoted typed literals; Unix-second fields take
integer literals.

When COUNT is requested, `SearchResult.count` contains the exact total even
for a small page. Otherwise it is `None`, and a broad single-field or identity
order can stop as soon as the globally correct page is established. The
counter marks whether its observed match total covers the whole query. Partial
pages are never inserted into the complete verified-result cache. Sorting uses
ICU4X 2.4.0. Each collection specifies a default locale
and collation options; configured sort fields can override locale, strength,
numeric ordering, and null/missing placement. The Musenalm adapter uses `de`
with the previous defaults. Other scalar values use typed order, and
collection/record identity breaks ties consistently. Both directions retain
precomputed single-field order, and broad page-only queries merge those lists
across collections. Selective and multi-field requests use stored-key top-k
selection. Incompatible sort definitions across selected collections are
rejected. The
executor retains at most `offset + limit` page winners while counting. A hit
always includes its collection, record ID, generation, file ID/path, size,
created/modified times, byte range, and requested matched-field evidence.
It also carries file status (`0` valid, `1` broken, `2` unreadable), record
class (`0` logical, `1` raw fallback), and a file-local diagnostic reference.
`Repository::files().diagnostic(file_id, diagnostic_ref)` retrieves a retained
diagnostic. Each file retains at most 64 diagnostics with 256-byte messages and
counts omitted entries. An unreadable file remains in inventory without a
searchable record.
The three return modes add either no body (`locator`), the original record
bytes (`bytes`), or parsed logical data (`logical`). A fallback returns
`LogicalUnavailable` with its diagnostic reference in logical mode; bytes mode
returns its exact failed range. Payload ranges are read
only for the page and are checked against the recorded source revision.
During multi-record verification, each source uses one handle for byte-bounded
range batches. Each worker retains at most `offset + limit` candidate payloads;
the worker queue is bounded by the configured worker count. Count-only and
locator requests retain no payload. Verified-cache membership is compact and
budgeted separately, and an entry is discarded if it cannot be complete.
Stored-order pages and page materialization use a query-scoped source session
with at most eight open handles and 64 KiB range windows. Interleaved files may
be reopened after eviction. The work counters distinguish open calls from
distinct files; `bytes_read` includes bytes fetched into range windows.

## Indexes, planning, and reuse

Schema-marked lookup fields get typed B-tree posting dictionaries backed by
Roaring record-slot sets. Their attached presence, null, and array-length
entries and coverage keep exact plans safe when a field is missing or a
source has partial registration. Unmarked fields remain searchable through
conservative text candidates or residual scans. Raw text uses record-level
normal and skip bigrams from the extracted string segments; selected custom
text has its own index. ASCII mode retains only safe printable ASCII grams,
including gram subsets inside mixed Unicode literals; Unicode mode indexes
observed character pairs. A candidate filter is never treated as proof of a
literal match: the scanner verifies it. Short or unsupported literals fall
back to scanning.

Raw fallback text has a separate bigram index over the original range decoded
with UTF-8 replacement. An unscoped FTS literal searches that source text with
case-sensitive substring semantics, even when a custom raw binding transforms
valid logical text. Its evidence is `@record.raw_text`. Selected logical-field
FTS does not infer fields from failed source text. A logical predicate on a
fallback is indeterminate: negation remains indeterminate, false AND
indeterminate is false, and true OR indeterminate is true. Only true matches.
Metadata remains evaluable. Sorting a fallback by a logical field places it at
the configured missing-value position. A collection with fallback records
conservatively scans its filter domain; healthy-only collections retain exact
posting plans.

The optimizer enumerates metadata, exact posting, sort-column, dictionary,
raw/custom bigram, and scanner paths where compatible. It estimates selectivity
and work from posting cardinality, coverage, file/record counts, and source
bytes. AND work is reordered using actual surviving cardinality; OR branches
drop exact proven hits from later membership-only work. Remaining predicates
are prepared in cost order and short-circuit where evidence permits. Exact
term proofs skip record-level evaluation; approximate candidates still require
verification. The residual counter records terms actually evaluated against
decoded records. Index-proven matches can avoid source reads entirely. `EXPLAIN`
and `EXPLAIN ANALYZE` expose selected/alternative paths, typed bindings,
estimates, actual input/output, adaptive decisions, candidates, reads, bytes,
parses, and proof counts. The saved final benchmark plans are from separate
uncached diagnostic executions, outside latency samples.

Prepared typed bindings have a repository-local 4 MiB default budget. Verified
result reuse is disabled by default, or configurable through
`RepositoryOptions.verified_cache_bytes` and
`Repository::configure_verified_cache`. It stores complete ordered membership
and evidence under its byte budget, including empty results, but no record
bodies. Requested page bytes/logical values still pass through revision-checked
reads. The standard benchmark disables source-byte and decoded-record caches;
its raw samples report prepared-plan and verified-result hits independently.

## Persistence and freshness

The ASCII and Unicode snapshot files are under the fixture's `.objekte/`
directory and excluded from version control. Each snapshot has a versioned
header and independently checksummed binary sections for the file inventory,
collection generations, postings, text indexes, sort columns, and statistics.
A configuration hash binds model, schema/registrations, alphabet, ICU version,
and platform. Loading validates section bounds/checksums, paths, mappings,
coverage, and sorted state before publication. Startup reconciles every
selected file's path, size, and modified time. A changed file set or revision
causes a fresh private repository build and atomic snapshot replacement. A
full repository rebuild is used today even if one collection changed.

Source extraction and later range reads compare the opened handle and named
path with the recorded size and modified time before and after access. This
rejects ordinary stale reads; a same-size edit with a restored modification
time is not detectable without content hashing. No runtime updater, watcher,
overlay, or partial rebuild is implemented. The
[future complete-file replacement design](future-whole-file-replacement.md)
describes how the file-to-record map would locate all old records, rebuild
affected generations, and publish them atomically after successful extraction.

The archived implementation under `deprecated/crates/core/` was inspected for query,
bigram, and ICU behavior. The new record-slot storage, source ownership, and
index implementations were written for this workspace; no archived crate is
linked into the build.
