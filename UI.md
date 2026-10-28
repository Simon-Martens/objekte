# Deferred UI work

This is a backlog for the later persistent UI, not a requirement for the current
CLI implementation. Current work focuses on fast repository opening, source
comparison, cache updates, queries, and phase timings in individual commands.
The features below are planned; this document does not make them implemented.

Repository semantics, dependency tracking, reconciliation, and caches remain
library responsibilities. The UI supplies user intent and presents results. Its
need for an event loop must not require the CLI to keep a session running.

## Features

1. **Optimistic delivery of cached results.** Return a compatible cached view
   immediately, with an explicit freshness state, while verification runs. Check
   repository identity and applicable schema/mapping/cache compatibility before
   reusing it. Publish updated results after the library commits a consistent
   cache revision. Distinguish cached results awaiting verification from results
   verified under the selected reconciliation policy. If verification fails,
   report the problem without presenting the previous snapshot as current.

2. **Patch visible results after reconciliation.** Identify rows by collection
   and record ID. Changes can affect values, ordering, filter membership, page
   boundaries, labels, and totals. Reexecute affected active views through the
   library and compare their results; the UI applies inserts, removals, moves,
   and value updates while preserving selection and scroll position where
   possible. Updating the fields of already visible rows alone is insufficient.
   Finer dependency-based invalidation can follow the library's query work.

3. **Filesystem watching for external changes.** Watch requested collections and
   relevant configuration/schema/UI files. Coalesce event bursts and mark affected
   paths dirty; the library reconciles additions, changes, removals, and renames.
   Events are hints, not the sole source of truth. Startup reconciliation still
   covers changes while the app was closed; dropped events or watcher failures
   trigger reconciliation. Define periodic recovery according to the chosen
   freshness policy. These subscriptions are unnecessary for a single CLI command.

   The library now persists configuration digests and their collection consumers.
   Use `refresh_configuration()` on startup and for configuration events. Its report
   identifies changed input files and candidate collections; `cache status` exposes
   applied versus expected schema hashes and `configuration_current`. Schedule
   collection refreshes for affected active views. Also check persisted compatibility
   on startup: a previous process may have observed new configuration without
   updating a collection. An empty change report does not prove all caches are current.
   Record files still need their separate collection manifest reconciliation.

4. **Background jobs.** Run verification, cache construction, queries, and other
   expensive work away from the UI thread. Background reconciliation is the normal
   freshness mechanism while a project is open: the UI schedules library refreshes
   for initial verification, watcher events, and periodic recovery checks. Users
   need not manually refresh to observe external edits, and ordinary view
   interactions need not trigger a full source scan. Keep the compatible cached
   snapshot usable with its freshness state while work runs, then patch affected
   views after the library commits the updated cache revision, as described above.
   The library owns change detection and cache synchronization; the UI owns job
   scheduling and presentation. Provide progress, cancellation, and
   completion/failure notifications through library APIs. Prioritize work needed
   by the visible view over speculative work. Bound concurrency and batch source
   changes; commit consistent updates before notifying consumers. Avoid holding
   an exclusive repository/cache lock throughout an entire UI session.

5. **Prefetch on hover, focus, or idle time.** The UI can request a likely next
   record, related view, or neighboring page before navigation, or warm explicitly
   chosen data while the user is idle. Use the same library operations and cache
   rules as ordinary requests. Delay fleeting hover requests, deduplicate work,
   limit speculative I/O/memory, and cancel obsolete work. A prefetch remains
   optional and must not block an explicit request or scan every collection.
   This does not introduce heuristic index creation into the current CLI plan.

6. **Reuse repository state and database connections.** Keep repository handles
   alive for an open project and retain useful parsed metadata, prepared queries,
   and connections where appropriate. Reuse must remain compatible with cache
   deletion, another process updating the cache, configuration changes, and source
   invalidation. Load collections lazily. Operation-scoped locks and transactions
   allow other clients to use the repository. Tuning this session lifetime is
   deferred until the UI work; it is not the current command performance target.

7. **Reject obsolete asynchronous results.** Tag requests with the view identity
   and parameters, request ID, and relevant cache revision. An earlier slow query
   or prefetch must not replace a newer sort, filter, page, or repository selection.
   Cancel obsolete work when possible and discard its results when necessary.

8. **Freshness, progress, and timing presentation.** Show whether a view is cached,
   being checked, current under the selected policy, or affected by an error.
   Retain library phase timers and structured logs, shared with the CLI. The UI
   consumes these events for progress and troubleshooting; it does not implement
   a second source scanner or cache updater. On project close, unsubscribe and
   cancel background jobs; interruption must leave recoverable cache state.

## Later acceptance checks

- Cached rows can appear before verification finishes, with accurate freshness.
- Reconciliation patches changed values, removals, ordering, filter membership,
  page boundaries, and totals correctly.
- Changes made while the app is closed, event bursts, and missed events recover
  through reconciliation.
- External edits while the UI remains open reach visible views through background
  reconciliation without manual refresh or a full scan on every interaction.
- Rapid sort/filter/navigation changes cannot display obsolete responses.
- Hover and idle prefetch stay within configured budgets and yield to user work.
- Cancellation, cache deletion, invalid files, and interrupted jobs preserve
  authoritative source files and do not falsely report a verified view.

The initial library/CLI contract is in [implementation.md](implementation.md).
The measured startup and query costs are in
[the library profile](crates/bench/LIBRARY-PROFILE-2026-10-28.md).

## Validation status available from the library

The cache now distinguishes pending, valid, invalid and incorrect JSON per record.
Render pending separately from known-invalid records. Views supply `states[id]`;
record details include all JSON Pointer diagnostics and original values. Incorrect
JSON rows still have ID/path and `get_record` supplies exact raw bytes for an editor.
A malformed file replaces any older cached data; do not display the old row as current.

Use `poll_validation_updates()` to receive coalesced collection changes after commits
and requery visible rows. Workers and writer serialization live in the library. UI
file watchers/background freshness checks still call `refresh`; validation workers
are not file watchers and cannot observe edits until reconciliation sees them.
`wait_for_validation()` is available for explicit completion flows and tests. Do not
block the UI on it. Background failures surface through wait/poll APIs; pending work
survives and can resume on a new repository handle. Ordinary schema errors are data,
not background-job failures. Validity does not change query membership by itself;
invalid typed fields have their own index state and raw values remain displayable.
