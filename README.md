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

```sh
cargo run -p objekte-cli -- examples/musenalm view places --data-dir /tmp/objekte-a
cargo run -p objekte-cli -- examples/musenalm view contents --data-dir /tmp/objekte-a --sort numbering --direction desc --limit 20 --diagnostics
cargo run -p objekte-cli -- examples/musenalm view contents --data-dir /tmp/objekte-a --filter 'year>=1800' --count --output json
cargo run -p objekte-cli -- examples/musenalm inspect --collections --indexes
cargo run -p objekte-cli -- examples/musenalm relations --collections
cargo run -p objekte-cli -- examples/musenalm cache refresh --collection contents --data-dir /tmp/objekte-a
cargo run -p objekte-cli -- examples/musenalm cache status --data-dir /tmp/objekte-a
```

Use `--cache disabled` for direct execution, `--fresh-cache` for a new isolated cache
namespace, and `--output ndjson` for one returned row per line. Table output escapes
control characters and abbreviates long cells; JSON output retains full values.
`--help` lists record/reference operations and all options. The CLI chooses an OS
application-data directory when `--data-dir` is omitted; the library always receives
an explicit directory.

`load_repository` only loads metadata. `open_view` enumerates/reconciles the requested
collection. The default `metadata-on-open` policy fingerprints a sorted manifest
once on first collection use per handle. Unchanged manifests reuse SQLite without
reading record files; changed metadata selects the files to read/hash. Unix stamps
include device, inode, and ctime alongside size/mtime. This verifies metadata, not
all content: use `--reconcile hash-on-open` for strict full-content checking,
including edits invisible to metadata. `metadata-on-request` and `hash-on-request`
repeat their respective checks on every request. Explicit `refresh` uses the
selected policy. Watchers and background/UI work remain deferred in [UI.md](UI.md).

`list.subSorts` adds ordered secondary keys after `defaultSort`; `--then-sort
FIELD:asc|desc` can be repeated on the CLI. An explicit primary sort replaces the
configured default order. Data-field sort keys must be declared sortable; ID is
always available as the final tie-breaker (ascending unless explicitly supplied
last). Views report
compound `view_hashes` for filters, the resolved sort order, projection/pagination,
and their combination. These fingerprints do not store query results or create
additional SQL compound indexes. Schema/mapping, typed indexes, source manifests,
and view settings have separate fingerprints.

Cache format 2 rebuilds older disposable databases once. The initial build still
reads all source records; the new unchanged-manifest shortcut applies on later
opens. `--diagnostics` logs the policy, view fingerprints, and phase counters/timers
to stderr; `cache status` exposes persisted manifest/view fingerprints.

The implementation is read-only toward source files. Record writes, rendered
markup, richer queries, query-specific result caches, and a desktop UI are later
stages. SQLite holds source JSON text and separate typed index values; it is safe
to discard. A cache error is reported; direct mode remains available independently.

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
