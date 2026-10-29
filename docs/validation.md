# Ingestion and validation

Source files remain authoritative. All caches are disposable library state under the
application data directory supplied by the caller. Source reconciliation can be current
while individual records are still pending or invalid.

## Completion status

- [x] Ingest stable source snapshots in bounded batches while retaining malformed and schema-invalid records.
- [x] Validate cached records in the background with durable pending state and stale-result guards.
- [x] Report record status, diagnostics, metrics, and collection notifications through the library and CLI.
- [x] Keep validation state separate from source freshness and recover pending work after restart.
- [ ] Remove redundant per-record digest lookups during ingestion.

## Implemented flow

1. Enumerate/compare manifests as before; read and hash changed candidates with the
   existing before/after stability checks. Fresh-cache preparation uses CPU threads / 2
   workers, minimum one. Batches are bounded to 256 records / 8 MiB source bytes,
   except that an oversized record runs alone.
2. Parse with Sonic. Store exact source bytes, identity, metadata and digest. A parse
   failure stores `incorrect_json` plus a diagnostic; successful parses start `pending`.
   Invalid UTF-8 is preserved too. Parseable non-object roots are schema-invalid, not
   malformed JSON. The existing nesting limit is a parser limit.
3. Extract declared references and commit through one writer. SQLite derives keys
   for any existing JSON expression indexes as the raw records are written. Fresh
   builds create only the requested empty indexes, then reuse one record's keys
   across writes. Later index builds use a transient 128 MiB key budget and extract
   only needed fields, validating the entire document. All work remains in the same
   writer transaction; no extra field columns are written.
   No worker accesses SQLite. Correctly typed fields of an otherwise invalid record
   remain queryable. Wrong types/out-of-range numbers have index state 3, after normal
   values (0), null (1), and missing (2), in either sort direction. State 3 never matches
   typed filters. Raw values still appear in projected rows. Malformed rows have no
   derived fields or references; their ID/path remain visible in unfiltered views.
   Only safe, nonempty reference IDs produce edges. Shape/reference-ID errors become
   diagnostics; target existence is still a separate relationship check.
4. On an explicit validation run or before a JSON query, a coordinator selects cached
   pending snapshots. Validation batches contain at most 1,024 records /
   8 MiB source bytes (one oversized record alone). The coordinator releases
   the writer and validates on CPU threads / 4 background workers, minimum one.
   It reparses cached bytes, collects all schema/domain errors, and produces valid or
   invalid results. No source reread is necessary. Error pointers address original
   values, so diagnostics do not duplicate large source objects.
5. Results return to the same shared writer/connection. Status and issues live on the
   record row. A transaction updates only
   pending rows whose collection, ID, revision, digest and schema hash match the job.
   Edits, deletions and schema changes cannot receive obsolete results. Validation
   does not alter source freshness, record revision, indexed field values or references.
6. Publish coalesced collection notifications after commit. A view returns its own
   status snapshot; clients requery visible rows after notifications.

Views and record reads leave work pending. `query_json` waits for pending work in
all collections in its scope before evaluating its filter. Explicit
`validate_collection(name)` and `validate_all()` also wait for completion.
`wait_for_validation()` waits for queued work, not future requests, and returns
cumulative validator metrics. `validation_metrics()` is a
nonblocking snapshot; `poll_validation_updates()` drains collection notifications.
Validator snapshot reads, parallel preparation wall time, decode/validation CPU time,
and writer time are separate from foreground metrics. CPU times are summed across workers, not wall time.

Worker pools are created lazily, with 16 MiB virtual stacks to accommodate Sonic's
bounded nesting in debug builds. Callers using small custom thread stacks must also
allow sufficient stack for foreground parsing/projection. Collection batches are
scheduled round-robin so a large collection does not monopolize validation. One shared writer per cache path serializes handles
within a process; an operation-scoped file lock also coordinates processes. Workers
only return results. Separate processes may duplicate CPU validation work, but the
conditional update allows only one publication. SQLite uses WAL, FULL synchronization
and a 64 MiB page-cache budget by default. Counts and budget are caller-configurable.

Closing a handle stops further batches without draining a collection; an active batch
can finish. Pending rows survive shutdown until the next explicit validation run.
Malformed JSON needs no schema validation until its bytes change. Source edits reset
validation; schema changes reset affected parsed records and rederive indexes/edges;
UI-only changes retain diagnostics. No heuristics or query-result caches are introduced.
A background DB failure rolls back its batch and is reported by wait/poll. Pending
work remains recoverable on a new handle. I/O errors, unstable reads, invalid project
configuration and DB failures can still fail operations; content errors cannot.

The CLI supports `--background-workers N`, `--ingest-workers N`, and
`validate [COLLECTION]`. Omitting the collection validates
all registered collections; specifying it validates only that collection. Both
reconcile source changes and wait. The existing
`cache validate --collection NAME` form remains available. CLI JSON results and
diagnostics display all metric durations as floating-point milliseconds (`*_ms`).
Ordinary read commands do not start validation. Cache status
reports per-state counts. Explicit validation without SQLite runs inline and creates
no persistent work.

## Current SQLite schema (cache format 7)

All collections share one `records` table keyed by `(collection,id)`. Status and
diagnostics are stored on the same row as the raw JSON.
Partial JSON expression indexes are generated from schema/UI options. `json` stores exact raw bytes as a BLOB, including malformed JSON;
it is deliberately not constrained with `json_valid`. SQLite is not the JSON parser.

```sql
CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE migrations(
  version INTEGER PRIMARY KEY, name TEXT NOT NULL,
  applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE inputs(path TEXT PRIMARY KEY, digest TEXT NOT NULL);
CREATE TABLE input_collections(
  path TEXT NOT NULL, collection TEXT NOT NULL, PRIMARY KEY(path,collection)
) WITHOUT ROWID;
CREATE TABLE collections(
  name TEXT PRIMARY KEY, state TEXT NOT NULL,
  schema_hash TEXT NOT NULL DEFAULT '', mapping_hash TEXT NOT NULL DEFAULT '',
  index_hash TEXT NOT NULL DEFAULT '', revision INTEGER NOT NULL DEFAULT 0,
  manifest_hash TEXT NOT NULL DEFAULT '', record_count INTEGER NOT NULL DEFAULT 0,
  view_hash TEXT NOT NULL DEFAULT ''
);
CREATE TABLE records(
  collection TEXT NOT NULL, id TEXT NOT NULL, path TEXT NOT NULL,
  size INTEGER NOT NULL, modified TEXT NOT NULL, identity TEXT NOT NULL,
  digest TEXT NOT NULL, json BLOB NOT NULL, revision INTEGER NOT NULL,
  parse_status TEXT NOT NULL, validation_state TEXT NOT NULL,
  validation_schema_hash TEXT NOT NULL, issues TEXT NOT NULL,
  PRIMARY KEY(collection,id)
);
CREATE INDEX validation_pending ON records(collection,id)
  WHERE validation_state='pending';
CREATE TABLE edges(
  source_collection TEXT NOT NULL, source_id TEXT NOT NULL, location TEXT NOT NULL,
  target_collection TEXT NOT NULL, target_id TEXT NOT NULL,
  PRIMARY KEY(source_collection,source_id,location,target_collection,target_id)
);
CREATE INDEX edge_target ON edges(target_collection,target_id,source_collection,source_id);
```

Status is inserted/reset/deleted atomically with its source row.
`parse_status` is `parsed` or `incorrect_json`; `validation_state` is `pending`,
`valid`, `invalid`, or `incorrect_json`. `validation_schema_hash` identifies the schema
requested for this snapshot. `issues` is a JSON array of `{location, schema_path,
code, message}`. Schema paths are optional for parser/domain errors. The schema validator enables
arbitrary-precision support, matching the Sonic/Serde value representation; numbers
outside the indexed numeric domain produce diagnostics without rounding or panics.

`inputs` is the last successfully observed configuration manifest, not a validation
checkpoint. Its digests are read and compared before they are replaced.
`input_collections` associates inputs with direct consumers so modified/shared files
and removed configuration can be attributed across restarts. Both tables update in
one transaction. Removed registrations remove obsolete manifest entries, while
collection caches remain disposable and can be reused if registered again.

`collections.schema_hash` advances only when that collection's reconciliation
commits. `records.validation_schema_hash` identifies the schema for that
record's pending or completed validation. An observed schema edit therefore cannot
make old validation results appear applicable to the new schema: cache status
reports incompatible configuration until reconciliation resets affected records to
pending. Malformed JSON retains its parse-error status. Configuration refresh alone
does not run validation or read source records. The ownership table is part of
the current configuration manifest schema.

There are no extracted-value tables or stored field columns. Required view indexes
use deterministic `objekte_view_state(json,id,spec)` and
`objekte_view_value(json,id,spec)` expressions on that collection's raw table.
General query indexes use `objekte_json_key(json,id,path)` with the exact
`objekte_json_v1` collation. The record ID enables transaction-local reuse during
index construction; the functions' result remains determined by the JSON and spec.
SQLite stores and transactionally maintains the derived keys. Field permissions
come from configured filter/default-sort/sortable fields; the requested operation
determines which physical indexes to build. A covering index is created for compound
filters/sorts. Unfiltered counts and ID orders use the existing primary key.
Index-definition version 3 accompanies cache format 7. Statistics for the collection
primary key and new indexes are collected together. Library connections register the
Rust functions; external integrity checks use `register_cache_sqlite_functions`.

Startup migrations create the shared tables from numbered SQL files and write a
history row in the same transaction as each schema change. Format 6 caches are
discarded and rebuilt from source files. Unknown cache formats are rejected. Redundant per-record digest lookups remain a later ingestion optimization. The
per-record typed-table writes have been removed.
