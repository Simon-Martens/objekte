# Tolerant ingestion and background validation — 2026-10-28

Compared this implementation against `f4c3188` on the same machine. Both release
builds use `-C target-cpu=native`, four ingestion workers and a 64 MiB SQLite page
cache. Background validation uses four workers. These explicit counts make the
comparison independent of the new CPU-derived defaults.

Each sample uses a new SQLite directory on the workspace filesystem, with the OS
source-file cache warm. Three samples per collection/build, alternating build order.
Commands request the first 50 rows and total count. Complete row equality is checked
between builds and warm runs. A separate full Musenalm acceptance test compares all
148,486 records, sort/filter cases and relationship occurrences, and checks completed
validation for every record.

| Collection | Records | Previous first view | New first view | New validation completion, separate CLI | Previous warm CLI | New warm CLI |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| contents | 133,948 | 5.418 s | 5.710 s | 3.342 s | 370 ms | 369 ms |
| entries | 4,928 | 2.539 s | 2.362 s | 0.734 s | 26 ms | 27 ms |

Values are medians of complete subprocess wall time. New first views use manual
validation to isolate ingestion; the default background policy schedules validation
after the view snapshot is built. The separate validation command includes another
process startup and manifest check, but reads **zero source files**. Every pending
record is validated and published. Warm views also read zero source files.

The new flow separates validation from availability and retains broken records.
It adds work: cached bytes are reparsed and per-record results must be persisted.
For these runs, first-view performance is broadly unchanged; total time including
validation is higher than the earlier synchronous path. It would be misleading to
count deferred validation as eliminated work. The next ingestion tasks remain
manifest-based elimination of redundant digest lookups and collection-specific
prepared writes.

| New phase | contents | entries |
| --- | ---: | ---: |
| Reconciliation/build wall time inside first view | 5.496 s | 2.195 s |
| Background cached snapshot reads | 0.929 s | 0.102 s |
| Parallel decode + validation wall time | 0.404 s | 0.421 s |
| Conditional result writes/commits | 1.625 s | 0.183 s |
| Decode CPU time, summed across workers | 0.602 s | 0.597 s |
| Schema/domain validation CPU time, summed across workers | 0.813 s | 0.813 s |

Timings are medians independently and are not additive. Writer time includes lock
waits and commits. Decode/validation CPU timers overlap inside parallel wall time.
Cold-build maximum RSS was about 138.4 MiB for contents and 134.4 MiB for entries,
versus 136.6 and 133.3 MiB before. The 64 MiB page-cache budget is not a process RSS
limit. Background validation RSS is recorded separately in the raw measurements.

Validation state lives in compact `record_validation` rows, separate from full raw
payloads. Pending pages are materialized from the partial pending index before
joining payloads, avoiding repeated scans over completed records. Validation batches
are limited to 1,024 records and normally 8 MiB of raw bytes; oversized records run
alone. All results use the same writer, with revision/digest/schema guards. WAL and
FULL synchronization stay enabled.

Reproduce the new phases, using an empty application directory for the first command:

```sh
target/release/objekte examples/musenalm view contents \
  --data-dir /tmp/objekte-validation --ingest-workers 4 --sqlite-cache-mib 64 \
  --background-workers 4 --validation manual --output json --count --diagnostics

target/release/objekte examples/musenalm cache validate --collection contents \
  --data-dir /tmp/objekte-validation --ingest-workers 4 --sqlite-cache-mib 64 \
  --background-workers 4 --diagnostics
```

Repeat the view to measure warm execution. Use `--validation wait` for a single CLI
invocation that prints completed statuses, or the default `background` policy for a
running application. Raw measurements and the local comparison runner are under
ignored `target/validation-flow/`; source collections are unchanged.
