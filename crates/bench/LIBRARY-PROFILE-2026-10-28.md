# Current library: CLI startup versus repeated views

Measured on 28 October 2026 against Musenalm `contents`: **133,948 files,
145,870,586 source bytes**, default title sort, first 50 rows, no count. This uses
the current library, not the historical engine microbenchmarks. Sources were
unchanged. The existing cache was `/tmp/objekte-test`; `/tmp` is **tmpfs** on this
machine, while the source repository is on Btrfs. OS file caches were already warm
and were not reset. These are local diagnostic samples, not latency guarantees.

## Result

| Operation | Debug | Release |
| --- | ---: | ---: |
| First view on a handle, existing unchanged cache | 8,485 ms | 3,568 ms |
| Further views on the same handle, median | 4.12 ms | **1.41 ms** |
| First view after reopening the repository handle | 8,388 ms | 3,510 ms |
| First view with an empty cache | not measured | 8,894 ms |

The debug session contains two further requests; release sessions contain four.
Each session has one initial load and one reopen. Metadata loading itself took
14.5 ms debug / 2.8 ms release at the start of the existing-cache profile, and
5.0 ms / 1.1 ms on reopening in the same process. Table figures time `open_view`,
excluding initial metadata load, process startup, Cargo, printing, and result
comparison. The driver checks that all repeated/reopened rows are identical.

Separate fresh-process CLI runs before adding the finer counters measured
8.42 s debug and 3.59–3.62 s release, excluding Cargo. Thus this is not a slow
terminal table or expensive default sort. Debug hashing alone costs seconds.

An additional `cargo run` sample with an already compiled binary and table output
took 8.93 s debug / 3.79 s release; Cargo reported 0.14 s / 0.13 s before launching
the executable. Earlier Cargo samples triggered rebuilding after changing the
instrumentation/build target set and were excluded from this steady-build result.
Compilation can add further time, but is separate from the repeated verification.

## Where the existing-cache first request spends time

| Measured phase | Debug | Release |
| --- | ---: | ---: |
| Two directory inventories, including file metadata and ordered maps | 1,349 ms | 774 ms |
| Per-record canonical path and before/after file metadata checks | 1,929 ms | 1,699 ms |
| Reading all source bytes | 793 ms | 747 ms |
| SHA-256 plus digest formatting | 3,630 ms | 80 ms |
| Loading the saved manifest into memory | 295 ms | 109 ms |
| Other reconciliation bookkeeping, allocation, comparison, transaction/cleanup | 486 ms | 157 ms |
| **Entire reconciliation** | **8,480 ms** | **3,566 ms** |
| Opening/initializing SQLite and recording metadata fingerprints | 1.11 ms | 0.68 ms |
| Executing the page query and constructing its rows | 2.67 ms | 0.98 ms |

Reconciliation is the sum containing the first six rows, not an extra cost. The
bookkeeping row is the residual, not separately attributed CPU profiling. All
source records were reused: **zero source parses/validations, record inserts or
updates, index builds, or index-row writes**. All 133,948 files were still read and
hashed. The two inventories bracket the read pass to detect source instability.
`raw()` additionally canonicalizes the collection directory for every record and
checks each file's metadata before and after reading. These repeated filesystem
operations dominate release mode, where hashing the actual bytes is relatively
cheap.

The cold release sample additionally parsed and validated every source record,
stored 133,948 records and typed key rows, extracted 78,872 outgoing reference
occurrences, and created the key table plus ten field indexes. Its parsing and
validation took 1,473 ms; table/index DDL took 662 ms. Other writes and relation
extraction are included in reconciliation, not those two sub-timers.

## Indexes and query plans

The effective field mapping, derived from the UI and schema, is:

| Column number | Field |
| --- | --- |
| 0 | `@id` (primary key) |
| 1 | `edit_state` |
| 2 | `musenalm_id` |
| 3 | `numbering` |
| 4 | `preferred_title` |
| 5 | `year` |

The contents key table is
`v_d1b2a59fbea7e20077af9f91b27e95e865061b270be03ff539ab3b73587882e8`.
Its title-ascending index is on `(f4_s ASC, f4_v ASC, id ASC)`; the state column
implements value/null/missing ordering. Each non-ID field has ascending and
descending variants. Inspection of the actual cached database confirmed these
definitions. Source payloads have a separate `(collection, id)` primary key.

The library's own bundled SQLite reports this default query plan (long table
hash abbreviated):

```text
MATERIALIZE page
SCAN v USING INDEX v_<contents>_4_asc
SCAN v
SEARCH r USING INDEX sqlite_autoindex_records_1 (collection=? AND id=?)
```

`SCAN ... USING INDEX` here walks the ordered index until the `LIMIT 50` is
satisfied. It is not a full collection sort. The materialized page is chosen from
typed keys before fetching the 50 source JSON payloads; no temporary sorting tree
appears in this plan. Numeric descending sorting uses `_3_desc` and takes a median
**1.46 ms** for a subsequent request, including connection and metadata overhead.

There is a real separate limitation for combined filters and sorts:
`year >= 1800`, sorted by `preferred_title`, takes a median **112.0 ms** per
subsequent release request (111.1 ms query portion), with zero source reads. Its
plan searches the year index and then has `USE TEMP B-TREE FOR ORDER BY` before
selecting the page. Separate field indexes do not make every combination free.
This is a candidate for later measured query planning/index selection, already
deferred by the implementation plan. It does not explain the seconds spent on the
unfiltered default view.

## Meaning for a persistent UI

- `HashOnOpen` means first collection use on a **Repository handle**, not first
  use of a persisted database forever. Every CLI invocation creates a new handle,
  so it verifies all source hashes before trusting its cached collection. This
  catches edits made while the program was closed, even if size and mtime match.
- Reusing one handle skips reconciliation once that collection/revision is verified.
  Ordinary repeated views, sorts, and pages can therefore take milliseconds. An
  explicit refresh, `HashOnRequest`, relevant metadata changes, or invalidated
  cache state can make reconciliation necessary again. Recreating the handle on
  every UI request would recreate the CLI's cost.
- Startup verification and first cache construction remain real UI costs. A UI
  cannot claim verified fresh data after restart without doing the required work.
  Reading an unverified cached view first would need an explicit freshness policy
  and status. Watchers have not been implemented; they do not cover offline edits.
- SQLite is currently opened/initialized **per library operation**, including warm
  views. It also rewrites configuration fingerprints. In this tmpfs measurement
  warm opening/initialization costs about 0.42 ms; it is not the seconds-long
  bottleneck. A disk application directory, synchronous commits, and contention
  can change that cost. Retaining a connection and avoiding unchanged metadata
  writes are later improvements; retain operation-scoped locking rather than
  monopolizing the existing exclusive lock for the session.

The first optimization target is redundant path/metadata work in reconciliation,
preserving the existing unstable-source and symlink tests. Next are persistent
connection management and live invalidation. Metadata-first reconciliation that
only hashes changed stamps would be a different, weaker freshness contract: the
existing equal-size/equal-mtime edit test must not silently stop working. No such
policy change or query optimization was made for this investigation.

## Reproduce

CLI, using the repository-first syntax and an optimized build:

```sh
cargo run -p objekte-cli --release -- examples/musenalm view contents --data-dir /tmp/objekte-test --diagnostics
```

Keep a single library handle for several requests, then reopen it:

```sh
cargo run -p objekte-core --release --example profile_view -- examples/musenalm contents /tmp/objekte-test 5
```

Use a fresh empty data directory to measure construction. Omit `--release` to
compare debug. An optional final path supplies a JSON `ViewRequest`, for example:

```json
{"filters":[{"field":"year","operator":"ge","value":1800}]}
```

The generic driver is [profile_view.rs](../core/examples/profile_view.rs), and the
timing scopes are documented in the [test guide](../core/tests/README.md).
Raw local measurements are under `target/view-profile-2026-10-28/` (ignored build
artifacts). Instrumentation and the driver were added; cache/query semantics were
preserved. Core/CLI tests and Clippy pass.
