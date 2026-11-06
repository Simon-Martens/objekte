# Bounded parallel ingestion and SQLite page-cache tuning

Follow-up to [the initial ingestion profile](INGESTION-2026-10-28.md), measured
on 28 October 2026. This changes the general Rust library; collection names and
fields are still derived from repository configuration, schemas, and UI files.

## Implemented behavior

- `Options::ingest_workers`: default available parallelism capped at four, configurable
  from 1 to 64. An empty collection cache reads, hashes, decodes, and validates records
  in a private worker pool. One worker retains the serial path. Existing/partial
  collection caches continue through the incremental reconciliation path.
- Batches contain at most 256 records and normally at most 8 MiB of source bytes.
  One oversized record runs alone. Raw text, decoded trees, and allocator overhead
  mean this is not a strict total-memory bound. Batches are prepared and then written;
  this version does not overlap SQLite writing with preparation of the next batch.
- Only one thread writes to SQLite. Results and errors are consumed in manifest order,
  all writes remain in the existing collection transaction, and the final directory
  stability check still precedes commit. No partially built generation is published.
- `Options::sqlite_cache_mib`: defaults to `Some(64)`. `None` retains SQLite's default.
  The setting is applied on every connection and never changes cache identity or
  fingerprints. WAL and `synchronous=FULL` remain enabled. Cache spilling and automatic
  checkpoint settings are unchanged.
- Valid records use the validator's boolean check before constructing detailed error
  diagnostics only on failure. Numeric-domain checks remain required. This small
  change alone did not materially improve overall ingestion in the exploratory run.
- `json_decode_nanos` and `validation_nanos` split the existing `parse_nanos` timer.
  During parallel preparation these are summed worker durations, not wall time.
  `parallel_prepare_nanos` measures the wall time of pool setup and batch preparation;
  `sync_nanos` remains the elapsed reconciliation time.

The manifest-based lookup elimination and collection-specific prepared writes are
still the next tasks. Their implementation has not been folded into this comparison.

## Controlled comparison

Three fresh processes and fresh disk caches per collection and implementation,
with alternating order. Release/native-CPU builds, source filesystem caches warm
and uncontrolled, Btrfs application directories, default 50-row views. Source JSON
is unchanged. Baseline is commit `3a263b0` plus the split timers and cache-size option,
with serial ingestion and SQLite's own page-cache default. Candidate uses the new
validator path, four preparation workers, and a 64 MiB page-cache budget. Compilation,
row hashing, and integrity checks are outside the timed CLI process.

| Collection | Before, median | After, median | Reduction | Peak RSS before / after, median |
| --- | ---: | ---: | ---: | ---: |
| `contents`, 133,948 records | 9.291 s | 6.657 s | 28% | 51.2 / 136.4 MiB |
| `entries`, 4,928 records | 4.439 s | 2.975 s | 33% | 22.7 / 133.5 MiB |

`contents` ranges: 9.277–9.341 s before, 6.577–6.791 s after. `entries` ranges:
4.347–4.467 s before, 2.824–2.988 s after. Peak RSS is measured by `/usr/bin/time`.
These are local results, not expected universal ratios for other repositories or disks.
The larger memory footprint is a real tradeoff; the page-cache budget is only one
part of process memory.

## What the previous approximately nine seconds contained

Baseline `contents` stage medians from the same comparison:

| Stage | Time |
| --- | ---: |
| Directory inventory and manifest fingerprint | 0.395 s |
| Candidate opening/stability checks | 0.869 s |
| File reading | 0.415 s |
| Content hashing | 0.089 s |
| JSON decoding and compatible value conversion | 0.726 s |
| Schema and numeric-domain validation | 1.001 s |
| Record lookup and writing source text/metadata | 1.364 s |
| Reference extraction and persisted-edge comparison/writes | 1.271 s |
| Typed view-row construction and insertion | 0.470 s |
| Table/index DDL | 0.983 s |
| Collection-state publication and commit | 1.024 s |

The rest includes opening/closing the database, preparing paths and loop state,
freeing parsed documents, and process/output overhead. Independently computed
medians need not sum exactly. Primary-key and reverse-reference-index maintenance
is included in row-writing time, not table/index DDL.

After the change, the entire parallel preparation phase takes about **0.961 s**
for `contents` and **0.499 s** for `entries`. Validation still happens; its worker
time overlaps reading and parsing of other records. Do not add the worker timers
to the preparation timer or to total elapsed time. The new `contents` build still
spends about 1.156 s writing records, 0.557 s writing edges, 0.546 s writing typed
rows, 1.264 s on table/index DDL, and 1.150 s publishing/committing.

## Exploratory option sweep

These are single runs, useful for choosing candidates but not stable speed ratios.
Serial page-cache sweep, before the validator and worker changes:

| SQLite budget | `contents` | `entries` | Peak RSS, contents / entries |
| --- | ---: | ---: | ---: |
| SQLite default, about 2 MiB | 9.189 s | 4.409 s | 50.7 / 22.2 MiB |
| 16 MiB | 9.061 s | 4.434 s | 82.1 / 36.7 MiB |
| 64 MiB | 9.134 s | 4.376 s | 133.7 / 86.4 MiB |
| 256 MiB | 8.386 s | 4.033 s | 339.4 / 240.1 MiB |

More cache reduced edge-write cost, but some of that saving moved into DDL and
commit work. Larger is not automatically faster. In the worker sweep, using the
new validator path and SQLite's default page cache, 1/2/4/8 workers gave `contents`
9.031/7.798/7.203/6.660 s and `entries` 4.405/3.750/3.327/3.119 s. Four workers with
256 MiB gave 6.379/2.778 s, at 342.2/287.3 MiB peak RSS. Four workers and 64 MiB
are the shipped default compromise; both settings are independently configurable.

## Validation and SQLite semantics

Currently a new/changed record is read and hashed, parsed by Sonic, schema-checked,
and checked for supported numeric values before its cached record/edges/scalars
are written. Schema changes revalidate unchanged cached bytes. Unchanged compatible
records reuse their prior validation. An invalid record fails the collection update;
its transaction rolls back and the collection is not advertised as current.

SQLite itself does not validate our `json TEXT NOT NULL` column. It can store
malformed text, since this table has no `CHECK(json_valid(json))` constraint. JSON
functions generally reject malformed JSON when they consume it; `json_valid` instead
reports validity. Our normal ingestion performs parsing in Rust, and typed view
queries do not ask SQLite to parse the source text.
[SQLite JSON documentation](https://www.sqlite.org/json1.html#interface_overview).

Deferred schema validation is feasible as a separate cache contract: keep
`pending`/`valid`/`invalid` status, the validated content digest and schema hash,
and diagnostics. Apply a background result only if its digest and schema hash still
match; clear the status when either changes. A single `validated` boolean cannot
distinguish unexamined and invalid data. Syntactically valid but schema-invalid
records must be distinguished from malformed JSON. Raw storage can precede parsing,
but extracting reliable typed keys and relationships still requires parsing and
appropriate field checks. View/error-row semantics need to be specified before
unvalidated records are exposed. This change retains validation before insertion.

WAL is already active, and still permits only one writer per database at a time.
Parallel preparation is useful without multiple database writers. Switching to a
concurrent-write engine would be a different experiment with its own publication,
rollback, and recovery design.
[SQLite WAL concurrency](https://www.sqlite.org/wal.html#concurrency).
The page-cache option uses negative `PRAGMA cache_size` values in KiB, a per-connection
suggested page-cache limit, not total RSS.
[SQLite page-cache documentation](https://www.sqlite.org/pragma.html#pragma_cache_size).

## Verification and reproduction

102 routine core/CLI tests pass, release Clippy passes with warnings denied, and
formatting checks pass. New tests cover budgets on reopened connections, unchanged
cache reuse after option changes, invalid CLI/library values, ordered parallel
batches, a 9 MiB oversized record, identical reference occurrences, targeted updates,
deterministic errors after earlier batches, rollback, and dirty-cache recovery.
The existing killed-builder, source-stability, numeric, and schema-change tests pass.
Full Musenalm acceptance passes in 68.72 s: all 148,486 records and 173,427 reference
occurrences, direct/SQLite query parity, and metadata/strict reopens. This separate
acceptance run returns full collections; its time is not part of the 50-row benchmark.

The disk comparison also checks every persisted row in `records`, `edges`, and the
typed view tables by stable ordered hashes against the earlier baseline. All hashes,
default view rows, fingerprints, and query plans match. SQLite `integrity_check`
passes. Verification hashing runs outside the measured CLI process.

```sh
cargo build -p objekte-cli --release
cache_dir=$(mktemp -d target/ingestion.XXXXXX)
target/release/objekte examples/musenalm view contents --data-dir "$cache_dir" \
  --ingest-workers 4 --sqlite-cache-mib 64 --diagnostics
```

Use a new directory for each fresh-cache timing. `--ingest-workers 1` forces serial
preparation; `--sqlite-cache-mib default` uses SQLite's default cache budget. The
raw sweeps, alternating comparisons, frozen binaries, scripts, and verification
reports remain under ignored `target/ingest-tuning/`.
