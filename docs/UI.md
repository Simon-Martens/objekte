# Deferred desktop UI work

There is no persistent desktop UI yet. The current Rust library and CLI provide source-backed views, JSON filters, validation commands, source refresh, and in-memory indexes. The UI will keep a `Repository` handle open, schedule expensive calls away from its event loop, and present their results. The CLI remains a single-command caller.

## Library state the UI can use today

- A lazily built in-memory picker supports repeated queries within one process. It has changed-file overlays, ordered scalar keys, per-record revisions, and deletion tombstones. `refresh(collection)` reconciles external changes by inventory; the picker also has a targeted `refresh_id` method for a future watcher or editor integration.
- The picker keeps its indexes and revisions only in the active handle. `refresh_configuration()` reports changed inputs and affected collections in that handle. `cache status` reports coverage and schema compatibility, but does not prove source freshness beyond the selected reconciliation policy.
- Views and JSON queries expose query plans, metrics, and source-backed results. JSON query dependency metadata includes searched collections, paths, and a filter hash; A view result also has a view hash. These are inputs for future invalidation, not a complete query-result cache.
- `query_json_stream` sends numbered, validated page records through a callback as they become available. Treat them as provisional until the call returns its total and metadata. A later source or scalar-cardinality error invalidates all earlier records for that request. The CLI `--output events` exposes the same record, complete, and error sequence.
- `get_record` reads the original file. It reports malformed JSON immediately; a parseable record may have `pending` schema status in memory mode. Query matches and in-memory view pages are schema-validated when returned. Explicit `validate` scans source records. Validation status and diagnostics are not persisted in the index; there are no validation workers or update notifications.
- Incoming references are discovered from source on request. For complete coverage in memory mode, request `references_to(..., complete = true, ...)`; partial coverage only includes references already observed in this handle.

## UI features to implement

1. **Open and refresh in the background.** Keep repository handles alive while the project is open. Run `warm_bigram_index` or `rebuild_bigram_index`, source reconciliation, queries, and explicit validation away from the UI thread. Show progress and errors without blocking navigation. Bound concurrency and cancel obsolete work.
2. **Watch relevant paths.** Watch collection directories and configuration/schema/UI files. Coalesce bursts, refresh affected collections, and recover from missed events with an inventory pass. A watcher may use targeted file refresh for individual records, then fall back to a full refresh when events are incomplete. Git status is available on demand in the file picker; it is not a source-change watcher.
3. **Show freshness honestly.** Distinguish a returned result, a result awaiting source verification, and a result verified under the selected freshness policy. Memory indexes are process-local. Do not present an older snapshot as current after reconciliation fails.
4. **Patch visible views after changes.** Reexecute affected requests and compare by collection and record ID. Apply value, membership, ordering, page-boundary, and total-count changes while preserving selection and scroll position where possible. Query dependency metadata can help decide what to rerun, but the current library does not maintain live result subscriptions.
5. **Reject obsolete asynchronous results.** Tag requests with view identity, parameters, and a request ID. A slow earlier filter, sort, or prefetch result must not replace a newer one. Cancel where practical and discard late results otherwise.
6. **Prefetch selectively.** Hover, focus, adjacent pages, and idle time can warm likely next records or collections. Delay fleeting requests, deduplicate work, and cap memory and I/O. File-access frequency may improve prefetch or ranking in a persistent session; it must not change structural query membership or deterministic ID ordering.
7. **Present record state and diagnostics.** Show `pending`, `valid`, `invalid`, and `incorrect_json` distinctly. A malformed record can still provide its ID/path and raw bytes through `get_record`. To show schema diagnostics for a parseable `get_record` response, the UI needs an explicit validation path or a library change that validates that response in all modes. Do not infer validity from typed cache keys.
8. **Use phase timings for troubleshooting.** Display useful counters and timings from library responses. On project close, unsubscribe watchers and cancel jobs; closing the process releases its derived index.
9. **Index scalar-cardinality violations.** Record which source IDs have multiple values at a path used as a scalar, and update that information on edits and deletes. Queries currently scan beyond a full page when a later invalid array could cause a scalar-cardinality error. A reliable violation index would let these queries stop after the page while retaining the same error behavior. Define how nested paths and `OR` branches are tracked before using it for early completion.

## Later acceptance checks

- External edits, deletes, renames, configuration changes, event bursts, and missed watcher events reach visible results without requiring a full scan on every interaction.
- A changed record can enter or leave a filtered page, move under a sort, alter a total, and update an incoming-reference view correctly.
- Rapid navigation and cancellation never display an obsolete response as the latest one.
- Background jobs report useful progress and failures; pending schema status is not mistaken for valid or invalid.
- Index rebuilds and invalid source files preserve the authoritative JSON files.

The active cache contract is in [implementation.md](implementation.md), validation behavior in [validation.md](validation.md), and measured query costs in [Performance.md](Performance.md). Saved/grouped views, source-file write APIs, raw grep results, and cleaned-content search are separate product work in [MVP.md](MVP.md).
