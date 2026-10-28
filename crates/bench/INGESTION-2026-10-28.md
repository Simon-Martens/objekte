# First-cache ingestion and Turso, 28 October 2026

The active library uses bundled SQLite through rusqlite 0.41. Turso is an optional
benchmark dependency, pinned to 0.8.3. This investigation measures the actual
library pipeline, including source checks, Sonic parsing, validation, relationships,
typed view rows, indexes, commit, and the first default page.

## Current engine versions

As checked on 28 October 2026:

- Latest stable embedded Rust Turso crate: **0.8.3**, released 12 October.
  [Published crate versions](https://docs.rs/crate/turso/latest).
- Latest prerelease: **0.9.0-pre.2**, released 28 October. It supersedes
  0.9.0-pre.1 from 15 October.
  [Release](https://github.com/tursodatabase/turso/releases/tag/v0.9.0-pre.2).

These refer to the Rust database engine, not the separate libSQL client or cloud
service. No engine dependency was changed during this investigation.

Turso 0.8 focuses on concurrent writers and group commit. Its own throughput
benchmark uses 100-row transactions over disjoint keys and multiple connections;
SQLite is faster in its single-connection case. Our library currently loads a
collection through one connection and one transaction. Therefore the published
concurrency gains do not establish a speedup for our build pipeline.
[Official release analysis](https://turso.tech/blog/turso-0.8.0).

The [older engine experiment](RESULTS.md#embedded-engine-comparison-sqlite-turso-surrealdb)
already used Turso 0.8.3. It found faster ingestion for 200,000 small synthetic
records, but slower ingestion and index construction for Musenalm `entries`.
That experiment used a different storage layout and durability settings and omitted
source validation and relationships. It is not a benchmark of the current cache,
and neither it nor this investigation measures 0.9.0-pre.2.

## Measurement and small implemented improvement

Baseline: commit `11cbe54` with additional phase timers. Candidate: the same code
and timers with the existing per-record digest lookup changed to `prepare_cached`.
Previously SQLite compiled that SELECT for every record; it now reuses the statement.
Record comparisons, revision rules, transactions, durability, and cache format
remain identical.

Both binaries are release builds with `-C target-cpu=native` and Sonic. Runs use
fresh temporary application directories on the workspace's Btrfs filesystem,
WAL and `synchronous=FULL`. Source filesystem caches are warm and uncontrolled.
Compilation, integrity checks, and comparison hashing are outside the timed CLI
execution. Each invocation returns the default 50-row page. These are local
measurements, not promises about another disk or machine.

Three alternating runs per implementation give the following `contents` medians:

| Stage | Baseline | Reused statement |
| --- | ---: | ---: |
| Entire CLI, including first cache | 10.318 s | 9.105 s |
| Source inventory | 0.364 s | 0.368 s |
| Candidate checks and file opening | 0.910 s | 0.833 s |
| Source reading | 0.431 s | 0.408 s |
| Source hashing | 0.089 s | 0.087 s |
| Full parsing and validation | 1.793 s | 1.649 s |
| Record digest lookup and row write | 2.117 s | 1.329 s |
| Relationship extraction for writes | 0.104 s | 0.091 s |
| Relationship comparison and writes | 1.233 s | 1.158 s |
| Typed view-row construction and writes | 0.520 s | 0.459 s |
| Table/index DDL | 1.028 s | 1.026 s |
| Publishing collection state and commit | 0.995 s | 1.008 s |

`contents` has 133,948 files. Total latency fell about 12%; record-write time fell
about 37%. Individual timings vary, and changes in unrelated phases should not be
attributed directly to the prepared statement. Baseline whole-CLI runs ranged from
10.094–10.412 s; candidate runs ranged from 9.028–9.224 s. The timings do not account
for every instruction and are medians taken independently, so they do not sum
exactly to total latency.

For the 4,928 larger `entries` files, three-run whole-CLI medians were 4.569 s
before and 4.514 s after. Their ranges overlap (4.518–4.718 s versus 4.441–4.628 s),
so this does not establish a meaningful overall speedup. Baseline parsing and
validation took 1.65 s, record writes 0.71 s, relationship writes 0.72 s, and commit
0.69 s. Table/index DDL took only 0.023 s. This collection has far fewer repeated
record lookups; the prepared-statement change addresses much less of its workload.

Secondary view indexes are already built after rows are loaded, and the whole
collection already uses one transaction. Primary-key indexes and the reverse-edge
index are maintained while rows are inserted: their cost is included in write
stages. Thus `index_nanos` is **not** the cost of every index operation, but index
construction alone does not explain the first-build delay.

The new counters remain in the library and CLI diagnostics. Their exact scopes are
listed in the [test/profiling documentation](../core/tests/README.md#profile-one-view-across-requests-and-sessions).

## Next experiments, in order

1. **Use the already-loaded manifest during bulk ingestion.** It already tells us
   whether a record exists and its previous digest. Avoid the redundant digest
   SELECT entirely in that path. For genuinely new records, insert outgoing edges
   directly instead of querying and comparing a necessarily empty old set. Keep
   the incremental comparison path for updates and partial-cache recovery.
2. **Prepare collection-specific writes once.** Compute the typed table name,
   parameter layout, and statement once per collection rather than re-hashing its
   name and constructing SQL for every row. Keep bound parameters and the same
   schema-derived layout.
3. **Measure an explicit page-cache budget.** Compare defaults with bounded
   settings such as 16 and 64 MiB, recording peak memory, writes, commit time, and
   total time on a disk-backed cache. SQLite's normal default is about 2 MiB;
   `PRAGMA cache_size` changes a connection's suggested page-cache limit. A larger
   budget may reduce repeated page eviction, but needs measurement and a library
   option exposed by the CLI. This investigation did not change that setting.
   [SQLite page-cache documentation](https://www.sqlite.org/pragma.html#pragma_cache_size).
4. **Reduce full-record conversion and pipeline CPU work.** Large `entries`
   spend substantial time parsing and validating. Sonic currently converts its
   document into the compatible public value representation. Measure this separately;
   then consider bounded parallel reading/parsing feeding one transactional writer.
   Preserve stable-file checks, exact numbers, deterministic errors, bounded memory,
   and atomic publication of a complete collection.
5. **Compare Turso using the real cache layout before migrating.** Use the same
   source pipeline, records, reference occurrences, scalar indexes, and durability.
   Test 0.8.3 and 0.9.0-pre.2 separately. Include fresh ingestion, targeted updates,
   filtered/sorted pages, rollback, interruption recovery, and memory use. A concurrent
   loader would additionally need a staging/publication design so partial collections
   never appear current.

## Verification and reproduction

For both collections, the baseline and candidate passed SQLite `integrity_check`.
Every persisted row in `records`, `edges`, and the typed view table was hashed in
stable key order and matched exactly. All measured default pages, view hashes,
and query plans also matched. Comparison work is excluded from timing.

All 95 routine core/CLI tests pass, as do release Clippy with warnings denied and
formatting checks. The complete Musenalm acceptance passes in 80.14 s, covering
all 148,486 records and 173,427 reference occurrences, direct/SQLite query parity,
secondary sorts, and metadata/strict reopens. This acceptance timing is a separate
correctness run that returns full collections, not the 50-row benchmark above.

Build and profile the current library with a new application directory:

```sh
cargo build -p objekte-cli --release
cache_dir=$(mktemp -d target/ingestion.XXXXXX)
target/release/objekte examples/musenalm view contents --data-dir "$cache_dir" --diagnostics
cargo test -p objekte-core -p objekte-cli --release
cargo clippy -p objekte-core -p objekte-cli --release --all-targets -- -D warnings
mkdir -p target/ingest-profile
OBJEKTE_ACCEPTANCE_REPORT=target/ingest-profile/musenalm-acceptance.json cargo test -p objekte-core --release --test musenalm -- --ignored --nocapture
```

The example uses `contents`; use a different fresh directory for `entries` or other
collections. Reusing a directory measures verification of an existing cache. The
local comparison script, frozen binaries, raw results, and verification logs are
under ignored `target/ingest-profile/`; they are not application data or source files.
