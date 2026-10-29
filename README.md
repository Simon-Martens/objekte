# Objekte

Objekte is a Rust library and command-line application for structured, file-based
repositories. JSON files are authoritative. The first implementation reads a small
JSON Schema/UI profile, models collection and record relationships, and serves
scalar table views through direct reads or a disposable SQLite cache.

All seven [Musenalm collections](examples/musenalm/README.md) use the minimal
profile. HTML and Markdown remain strings. The library handles validation, lazy
loading, typed indexes, source hashes, and synchronization; the CLI supplies
options and displays results.

[implementation.md](implementation.md) defines the supported subset and stages.
[MVP.md](MVP.md) describes broader future capabilities.
[UI.md](UI.md) tracks deferred UI work, including optimistic cached results,
watching, background jobs, and hover/idle prefetch.

## Run

Put the repository first, then the command and its arguments:
`objekte REPOSITORY COMMAND [ARGUMENTS]`.

Use an optimized build when judging latency on large collections:

```sh
cargo run --release -p objekte-cli -- examples/musenalm view contents --diagnostics
```

Plain `cargo run` uses unoptimized debug code, including JSON validation and manifest
serialization. Compilation is separate from the logged execution timers. Release
and debug builds share compatible caches; switching builds does not require a rebuild.

Workspace builds enable `-C target-cpu=native` through
[`.cargo/config.toml`](.cargo/config.toml), for both debug and release builds.
This permits instructions supported by the build machine's CPU. For distribution
or cross-compilation, override it with an appropriate target CPU via `RUSTFLAGS`.
Keep `--release` for performance measurements; CPU targeting does not enable
release optimizations in debug builds.

```sh
cargo run -p objekte-cli -- examples/musenalm view places --data-dir /tmp/objekte-a
cargo run -p objekte-cli -- examples/musenalm view contents --data-dir /tmp/objekte-a --sort numbering --direction desc --limit 20 --diagnostics
cargo run -p objekte-cli -- examples/musenalm view contents --data-dir /tmp/objekte-a --filter 'year>=1800' --count --output json
cargo run -p objekte-cli -- examples/musenalm inspect --collections --indexes
cargo run -p objekte-cli -- examples/musenalm relations --collections
cargo run -p objekte-cli -- examples/musenalm cache refresh --collection contents --data-dir /tmp/objekte-a
cargo run -p objekte-cli -- examples/musenalm cache status --data-dir /tmp/objekte-a
cargo run -p objekte-cli -- examples/musenalm validate --data-dir /tmp/objekte-a
cargo run -p objekte-cli -- examples/musenalm validate contents --data-dir /tmp/objekte-a
cargo run -p objekte-cli -- examples/musenalm query 'year >= 1800 && preferred_title ~ "liebe"' --count
cargo run -p objekte-cli -- examples/musenalm query contents 'year >= 1800' --count
cargo run -p objekte-cli -- examples/musenalm query 'contributors.agents ?= $agent' --param 'agent="1500"'
```

Use `--cache disabled` for direct execution, `--fresh-cache` for a new isolated cache
namespace, and `--output ndjson` for one returned row per line. Table output escapes
control characters and abbreviates long cells; JSON output retains full values.
`--help` lists record/reference operations and all options. The CLI chooses an OS
application-data directory when `--data-dir` is omitted; the library always receives
an explicit directory.

`query FILTER` searches actual JSON across all registered collections, independently
of schema declarations and UI filter controls. Use `query COLLECTION FILTER` or add
`--collection NAME` to restrict the scope. It supports typed comparisons (`"1800"`
differs from `1800`, `true` from `1`), `&&`/`||`, nested paths, arrays with `?`
operators, field-to-field comparisons, `:length`/`:each`/`:lower`, `exists(path)`,
and bound `--param NAME=JSON` values. SQLite executes generated parameterized SQL;
`--cache disabled` uses the same semantics over source files. Configured scalar
comparisons use SQLite expression indexes directly on cached JSON;
other predicates scan their JSON values. There are no extra stored field columns. See
[filters.md](filters.md) for syntax, dependencies, limits, and measured performance.

Fresh collection caches use `CPU threads / 2` ingestion workers by default (minimum
one). Workers read stable files, hash and parse Sonic JSON in bounded batches. One
shared SQLite writer stores records and reference occurrences; SQLite maintains
JSON expression indexes. Schema validation uses a separate pool of `CPU threads / 4`
workers (minimum one); results return to that same writer. Both counts are configurable with `--ingest-workers`
and `--background-workers`. Existing cache updates retain their incremental path.
Bulk index construction also uses a temporary cache of extracted keys with a
128 MiB retained-key budget, released when the build finishes. Compound views read
keys from a covering JSON expression index. Additional scalar query indexes are
built lazily when first needed. SQLite uses WAL, `synchronous=FULL`,
and a 64 MiB page-cache budget by default.
`--sqlite-cache-mib N|default` overrides it; this is not a total process-memory limit.

Records have `pending`, `valid`, `invalid`, or `incorrect_json` status. Content errors
never roll back ingestion: the latest source replaces the old cached row. Malformed
JSON remains visible by ID/path and its exact bytes are retained. Parsed records
stay queryable while validation runs; incompatible typed fields sort after missing
values and do not match typed filters. Diagnostics use JSON Pointers into the original
record. Views return per-row `states`; tables include STATUS and NDJSON includes state.

`--validation background` is the default: return the view without waiting. A short CLI
process may exit with validation still pending. Use `--validation wait` to wait and
print completed statuses, or `--validation manual` to measure ingestion separately.
`validate` reconciles and validates all registered collections; `validate NAME`
does the same for one collection. Both wait for completion, including pending work
left by an earlier process, even with `--validation manual`. Unchanged, already
validated records reuse their results. `cache validate --collection NAME` remains
available. Cache status reports counts by validation state. Without SQLite,
validation runs inline and creates no application data.

The library exposes `validate_all()`, `validate_collection(name)`,
`wait_for_validation()`, `validation_metrics()` and
`poll_validation_updates()` (coalesced collection notifications after commit).
Validation results only apply when ID, revision, content digest and schema hash still
match. Pending work is durable; source changes reset it, schema changes requeue it,
and UI-only changes retain diagnostics. See [the cache schema and validation flow](validation.md).
The [validation-flow measurements](crates/bench/VALIDATION-FLOW-2026-10-28.md) separate
first-view latency from completing validation. Removing redundant digest lookups
remains a separate ingestion optimization; separate typed-table writes are now gone.

```sh
cargo run --release -p objekte-cli -- examples/musenalm view contents --data-dir /tmp/objekte-fast --ingest-workers 4 --sqlite-cache-mib 64 --diagnostics
```

`load_repository` only loads metadata. `open_view` enumerates/reconciles the requested
collection. The default `metadata-on-open` policy fingerprints a sorted manifest
once on first collection use per handle. Unchanged manifests reuse SQLite without
reading record files; changed metadata selects the files to read/hash. Unix stamps
include device, inode, and ctime alongside size/mtime. This verifies metadata, not
all content: use `--reconcile hash-on-open` for strict full-content checking,
including edits invisible to metadata. `metadata-on-request` and `hash-on-request`
repeat their respective checks on every request. Explicit `refresh` uses the
selected policy. Watchers and background/UI work remain deferred in [UI.md](UI.md).

Configuration changes survive restarts too. Before using SQLite, the library reads
the saved `inputs` digests, compares registered configuration/schema/UI files, maps
added/modified/removed inputs to their consuming collections, and atomically updates
the configuration manifest. Shared files retain all consumers. Collection caches
keep their **applied** schema/mapping/index/view hashes until their own reconciliation
succeeds; observing a new file digest does not mark an old cache current.

`Repository::refresh_configuration()` performs only this configuration work. The
CLI equivalent is:

```sh
cargo run -p objekte-cli -- examples/musenalm cache refresh-config --data-dir /tmp/objekte-a --diagnostics
```

It reports previous/current digests and affected collections without enumerating
records. `cache status` exposes `configuration_current`, `applied_schema_hash`, and
`expected_schema_hash`; completed validation remains a separate per-record state.
Schema changes requeue validation on collection use, using cached bytes for unchanged
source files. Formatting-only changes do not invalidate semantic caches. Diagnostics
include the latest manifest comparison and `configuration_ms`. With caching disabled,
the explicit refresh compares in-memory snapshots and writes no application data.

`list.subSorts` adds ordered secondary keys after `defaultSort`; `--then-sort
FIELD:asc|desc` can be repeated on the CLI. An explicit primary sort replaces the
configured default order. Data-field sort keys must be declared sortable; ID is
always available as the final tie-breaker (ascending unless explicitly supplied
last). Views report
compound `view_hashes` for filters, the resolved sort order, projection/pagination,
and their combination. These fingerprints do not store query results or create
additional SQL compound indexes. Schema/mapping, typed indexes, source manifests,
and view settings have separate fingerprints.

Cache format 5 rebuilds older disposable databases once and removes the old per-collection field tables. The initial build still
reads all source records; the new unchanged-manifest shortcut applies on later
opens. `--diagnostics` logs the policy, view fingerprints, integer counters, and
floating-point millisecond timings (`*_ms`) to stderr. All CLI JSON metrics also
use `*_ms`, including views, refreshes, and validation commands; counters remain
integers and source-record values remain unchanged. Library timers retain integer
nanoseconds (`*_nanos`) internally. `cache status`
exposes persisted manifest/view fingerprints.

The implementation is read-only toward source files. Record writes, rendered
markup, richer queries, query-specific result caches, and a desktop UI are later
stages. SQLite holds source JSON text and separate typed index values; it is safe
to discard. A cache error is reported; direct mode remains available independently.

JSON input is parsed with sonic-rs. Source records and schemas retain the public
`serde_json::Value` representation for validation and API compatibility, preserving
numeric precision and last-member-wins duplicate keys. Cached pages decode only
their requested columns and label; unrelated payloads are skipped. Fingerprints
and output serialization keep their existing Serde encoding, so this parser change
does not require rebuilding compatible caches.

## Test

```sh
cargo test -p objekte-core -p objekte-cli
cargo clippy -p objekte-core -p objekte-cli --all-targets -- -D warnings
cargo test -p objekte-core --release --test musenalm -- --ignored --nocapture
```

The regular suite uses isolated temporary copies of
[the minimal test project](tests/fixtures/minimal/README.md). It checks exact query
results, graph cycles, targeted updates, closed-interval edits, equal-metadata
changes, transaction rollback, concurrent handles, and recovery after killing a
cache-building process. The opt-in Musenalm test validates all 148,486 records,
compares complete direct/SQLite outputs and numeric/filter workloads, counts all
173,427 reference occurrences, and verifies reuse after reopening. See
[the test guide](crates/core/tests/README.md).

## Workspace

| Path | Purpose |
| --- | --- |
| `crates/core` | Repository library: configuration, relationships, files, hashes, cache, views |
| `crates/cli` | `objekte` test driver and table/JSON/NDJSON output |
| `tests/fixtures/minimal` | Small independent repository for automated tests |
| `examples/musenalm` | Complete real-data acceptance fixture |
| `crates/bench` | Historical engine comparisons and NDJSON experiment |
| `deprecated` | Archived web editor and backend |

[Performance.md](Performance.md) and [the benchmark documentation](crates/bench/README.md)
retain the earlier engine experiments. The replaced NDJSON implementation now lives
only in `crates/bench/src/legacy_cache.rs`. Other older example repositories still
use richer schema/UI profiles and need migration before the new strict loader can
accept them.
