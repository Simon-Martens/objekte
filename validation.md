# Ingestion and validation

Source files remain authoritative. All caches are disposable library state under the
application data directory supplied by the caller. Source reconciliation can be current
while individual records are still pending or invalid.

1. Enumerate/compare manifests as before; read and hash changed candidates with the
   existing before/after stability checks. Fresh-cache preparation uses CPU threads / 2
   workers, minimum one. Batches are bounded to 256 records / 8 MiB source bytes,
   except that an oversized record runs alone.
2. Parse with Sonic. Store exact source bytes, identity, metadata and digest. A parse
   failure stores `incorrect_json` plus a diagnostic; successful parses start `pending`.
   Invalid UTF-8 is preserved too. Parseable non-object roots are schema-invalid, not
   malformed JSON. The existing nesting limit is a parser limit.
3. Extract usable typed values and declared references, then commit through one writer.
   No worker accesses SQLite. Correctly typed fields of an otherwise invalid record
   remain queryable. Wrong types/out-of-range numbers have index state 3, after normal
   values (0), null (1), and missing (2), in either sort direction. State 3 never matches
   typed filters. Raw values still appear in projected rows. Malformed rows have no
   derived fields or references; their ID/path remain visible in unfiltered views.
   Only safe, nonempty reference IDs produce edges. Shape/reference-ID errors become
   diagnostics; target existence is still a separate relationship check.
4. After commit, a coordinator first selects a page from the partial pending index,
   then joins its raw snapshots. Validation batches contain at most 1,024 records /
   8 MiB source bytes (one oversized record alone). The coordinator releases
   the writer and validates on CPU threads / 4 background workers, minimum one.
   It reparses cached bytes, collects all schema/domain errors, and produces valid or
   invalid results. No source reread is necessary. Error pointers address original
   values, so diagnostics do not duplicate large source objects.
5. Results return to the same shared writer/connection. Small `record_validation`
   rows keep health updates separate from raw JSON payloads. A transaction updates only
   pending rows whose collection, ID, revision, digest and schema hash match the job.
   Edits, deletions and schema changes cannot receive obsolete results. Validation
   does not alter source freshness, record revision, query indexes or references.
6. Publish coalesced collection notifications after commit. A view returns its own
   status snapshot; clients requery visible rows after notifications.

`Options.validation` defaults to `Background`; `Manual` leaves work pending until
`validate_collection(name)`. `wait_for_validation()` waits for queued work, not future
requests, and returns cumulative validator metrics. `validation_metrics()` is a
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
can finish. Pending rows survive shutdown and resume on the next collection use.
Malformed JSON needs no schema validation until its bytes change. Source edits reset
validation; schema changes reset affected parsed records and rederive indexes/edges;
UI-only changes retain diagnostics. No heuristics or query-result caches are introduced.
A background DB failure rolls back its batch and is reported by wait/poll. Pending
work remains recoverable on a new handle. I/O errors, unstable reads, invalid project
configuration and DB failures can still fail operations; content errors cannot.

The CLI supports `--validation background|manual|wait`, `--background-workers N`,
`--ingest-workers N`, and `cache validate --collection NAME`. Background mode can
exit before validation finishes. Wait mode prints completed row statuses. Cache status
reports per-state counts. Direct execution without SQLite validates inline and returns
the same record diagnostics/query semantics without persistent work.

## Current SQLite schema (cache format 4)

The following is the shared schema; per-collection value tables are generated from
schema/UI options. `json` stores exact raw bytes as a BLOB, including malformed JSON;
it is deliberately not constrained with `json_valid`. SQLite is not the JSON parser.

```sql
CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE inputs(path TEXT PRIMARY KEY, digest TEXT NOT NULL);
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
  PRIMARY KEY(collection,id)
);
CREATE TABLE record_validation(
  collection TEXT NOT NULL, id TEXT NOT NULL,
  parse_status TEXT NOT NULL, validation_state TEXT NOT NULL,
  validation_schema_hash TEXT NOT NULL, issues TEXT NOT NULL,
  PRIMARY KEY(collection,id)
) WITHOUT ROWID;
CREATE INDEX validation_pending ON record_validation(collection,id)
  WHERE validation_state='pending';
CREATE TABLE edges(
  source_collection TEXT NOT NULL, source_id TEXT NOT NULL, location TEXT NOT NULL,
  target_collection TEXT NOT NULL, target_id TEXT NOT NULL,
  PRIMARY KEY(source_collection,source_id,location,target_collection,target_id)
);
CREATE INDEX edge_target ON edges(target_collection,target_id,source_collection,source_id);
```

`record_validation` is inserted/reset/deleted atomically with its source row.
`parse_status` is `parsed` or `incorrect_json`; `validation_state` is `pending`,
`valid`, `invalid`, or `incorrect_json`. `validation_schema_hash` identifies the schema
requested for this snapshot. `issues` is a JSON array of `{location, schema_path,
code, message}`. Schema paths are optional for parser/domain errors. The schema validator enables
arbitrary-precision support, matching the Sonic/Serde value representation; numbers
outside the indexed numeric domain produce diagnostics without rounding or panics.

Each `v_<sha256(collection)>` table has `id TEXT PRIMARY KEY` plus `fN_s INTEGER NOT
NULL, fN_v` for each indexed field other than ID. Each field has ascending and descending
indexes `(fN_s ASC, fN_v ASC|DESC, id ASC)`. Field numbers map to the ordered collection
index specification. Compound view hashes describe query definitions; this change does
not introduce multi-field SQLite indexes or query-specific caches.

Format changes rebuild disposable tables on the next cache access. Deployment migrations
are deferred. Redundant per-record digest lookups and per-record typed-write SQL assembly
remain the next ingestion optimizations, independently measurable from this flow.
