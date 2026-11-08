# Desktop index lifecycle integration contracts

Task: `desktop-index-lifecycle`
Base: pushed core integration commit `a39434ca00162c63beddf3d541e1135ace579ff9`
Integration: `integration/desktop-index-lifecycle`

The desktop task is based on the core integration branch because the prerequisite has not been merged into `master`. Merge `integration/index-lifecycle` before `integration/desktop-index-lifecycle`.

## Core capability map

| Desktop operation | Implemented core contract |
| --- | --- |
| Active source and status | `IndexedRepository::index()` and `Index::status()` report instance, revision, saved revision, reconciliation phase, file and record counts. |
| Cache-ready open | `Index::open_repository()` or `Index::load_cache()` can return a searchable handle before its initial reconciliation finishes. |
| Check for changes | `Index::start_reconcile()` coalesces concurrent scans; its task exposes progress, cancellation, completion and a bounded issue report. |
| Rebuild and compact | `Index::start_rebuild()` and `start_compaction()` retain the published view until a successful replacement publication. A different simultaneous maintenance type is rejected. |
| Save | `Index::flush_cache_with_stats(path)` saves a captured revision to a concrete destination and protects source/configuration files. |
| Periodic checking | `Index::start_periodic_reconciliation(interval)` and `stop_periodic_reconciliation()` own one coalescing scheduler per handle. |
| Automatic cache persistence | Repository convenience mode has a selected destination and a coalescing core writer. The desktop adapter needs a narrow public runtime toggle for the existing core writer. |
| Pinned execution | `Index::view()` supplies an immutable `IndexView` with instance/revision identity for a result context, its continuations and count. |
| Shutdown | `Index::shutdown()` stops periodic and persistence workers and joins or cancels active maintenance; it must run off the UI thread. |

The core currently has no event subscription. The desktop controller will sample cheap status and task progress at a coalesced UI rate, and use task terminal outcomes for prompt, typed completion. It will not derive operation state from presentation strings.

## Ownership

- Backend adapter: session configuration, explicit cache paths, core calls and narrow runtime automatic-save API.
- Desktop controls: opening drafts, index dialog, typed task/status updates and scoped preferences.
- Navigation: pinned-view query and record APIs, revision keys, bounded projection and output ownership helpers.
- Supervisor: `controller/runtime.rs`, window wiring, integration, validation, report and cleanup.

The original checkout remains on `master`. Its untracked desktop task document stays authoritative until the final report is committed on this integration branch.
