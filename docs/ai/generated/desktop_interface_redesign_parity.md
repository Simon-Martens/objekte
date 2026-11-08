# Desktop interface redesign: parity and native evidence

Task: `desktop-interface-redesign`.
Baseline: `master` at `4e15721d3853a6c93e9b7b4bb73e18233144effa`.
Visual capture: `integration/desktop-interface-redesign` at `9274f0f023cc7b7064f9bd096c3171a4e8e12431`.
Validation date: 8 November 2026

The baseline and final sets below were captured through the repository's native Sway/WayVNC loop, with a fixed **1920 × 1080, scale 1, 60 Hz** virtual output. The final app window was also resized to 1440 × 900, 1280 × 800, and 1024 × 768 without changing that output. The populated captures use two JSON records in `/tmp/objekte-desktop-acceptance/items`. This fixture is deliberately small for interaction and layout review; the separate Musenalm benchmark covers the larger corpus. The supplied PDF was the visual target, not an implementation screenshot.

## Screens reviewed

All links point to committed PNGs in [the evidence directory](desktop_interface_redesign_evidence/). Baseline captures belong to `4e15721d38`; all `redesigned-*` captures belong to `9274f0f023`. The subsequent export correction at `d5953e1330` has no visual change.

| State | Native evidence | Review |
| --- | --- | --- |
| Previous opening/workspace | [Opening](desktop_interface_redesign_evidence/baseline-open.png), [populated](desktop_interface_redesign_evidence/baseline-populated.png) | Full-width controls and multiple persistent action rows crowded the table. |
| Default opening | [Open dialog](desktop_interface_redesign_evidence/redesigned-open.png) | Folder path and one Open action are the normal path. |
| Advanced opening | [Expanded](desktop_interface_redesign_evidence/redesigned-advanced-current2.png), [scrolled](desktop_interface_redesign_evidence/redesigned-advanced-scrolled-final.png) | Advanced content scrolls inside the modal; footer confirmation remains reachable. |
| Main workspace | [Populated table](desktop_interface_redesign_evidence/redesigned-workspace-loaded2.png), [menu](desktop_interface_redesign_evidence/redesigned-menu-icons.png) | The table dominates the window; menu overlays it and the complete icon set renders. |
| Filter and helper | [Valid free-text filter](desktop_interface_redesign_evidence/redesigned-filter-valid.png), [synchronized builder](desktop_interface_redesign_evidence/redesigned-builder-final.png), [edited comparison](desktop_interface_redesign_evidence/redesigned-builder-edited-final.png) | `score >= 3` appears in both editors; clicking the builder comparison changed the field to `score < 3` without replacing the table surface. |
| Invalid request | [Retained results](desktop_interface_redesign_evidence/redesigned-invalid-closed.png) | `score >=` shows the byte-position error, disables Run, and labels the old two rows as last valid. |
| Record inspection | [Fields](desktop_interface_redesign_evidence/redesigned-fields-final.png), [original source](desktop_interface_redesign_evidence/redesigned-source-final2.png) | One inspector provides tabs, find, and contextual copy/export. |
| Secondary options | [Query options](desktop_interface_redesign_evidence/redesigned-options-final.png) | Sort, offset, limit, and exact count float over the table. |
| Explicit output actions | [Index completed](desktop_interface_redesign_evidence/redesigned-index-done.png), [Export choices](desktop_interface_redesign_evidence/redesigned-export-menu2.png) | A custom snapshot directory and a JSON export were exercised while the current workspace remained available. |
| Theme and narrow window | [Light theme](desktop_interface_redesign_evidence/redesigned-light.png), [1024 × 768 with inspector](desktop_interface_redesign_evidence/redesigned-1024-inspector.png) | Text and controls remain readable. Selecting a record at 1024 px collapses the sidebar to leave room for the search field and inspector. |

The dark palette, hierarchy, and table emphasis follow the PDF direction. The native result is intentionally denser than the draft illustration where real controls and current data require it. At 1024 px the table itself scrolls horizontally; the application window does not.

## Action-to-location map

| Existing capability | Current desktop location/behavior | Main implementation |
| --- | --- | --- |
| Open one folder or configured repository | Welcome **Open data…**, header **Open…**, or Ctrl+O; asynchronous configuration detection; optional Browse | `ui/open_dialog.rs`, `controller/open_detection.rs` |
| Open several folders, schema, recursion, inference, alphabet | Open dialog **Add another folder** and **Advanced options** | `ui/open_dialog.rs`, `controller/adapt.rs` |
| Four snapshot policies and custom location | Open **Advanced options**; default remains in-memory with no snapshot write | `ui/open_dialog.rs`, `controller/adapt.rs` |
| Index-only work | Header **Menu → Index…**; current workspace remains open | `ui/panels.rs`, `controller/runtime.rs` |
| Collection scope | Left collection navigation; collapses at narrow inspector width | `ui/render.rs`, `ui/mod.rs` |
| Full-text search, case and regex | Persistent **Search** row, Aa and .* toggles | `ui/query_bar.rs`, `controller/runtime.rs` |
| Structured language | Optional **Filter** line; exact text is authoritative and combines with search using AND | `ui/query_bar.rs`, `controller/adapt.rs` |
| Filter helper | **Build filter** popover, synchronized with supported scalar conditions; unsupported expressions stay intact in text | `ui/filter_builder.rs`, `ui/query_bar.rs` |
| Filter errors and stale results | Inline error/byte position, disabled Run/export, **Showing last valid results** label; validation uses no scan | `controller/filter_validation.rs`, `controller/runtime.rs`, `ui/query_bar.rs` |
| Sort and query ranges | Global header sorting; **Options** for sort, offset, limit, exact count; footer for pages/counts | `ui/render.rs`, `ui/query_bar.rs`, `controller/adapt.rs` |
| Result columns | Stationary header, bounded rows, horizontal table scroll and column resize; footer **Columns** | `ui/render.rs`, `controller/projection.rs`, `ui/panels.rs` |
| Fields, source, relations, find and copy | Resizable inspector tabs and contextual actions; metadata collapses beneath | `ui/panels.rs`, `controller/runtime.rs` |
| History, saved queries and save | Sidebar shortcuts and **Menu** searchable lists/naming action | `ui/render.rs`, `ui/panels.rs`, `ui/preferences.rs` |
| Result, selected-record and NDJSON exports | Inspector **Export…** or **Menu → Export…** scope/format/path, one confirmation | `ui/panels.rs`, `controller/runtime.rs`, `backend/export.rs` |
| Batch queries | **Menu → Run query batch…** with bounded outcomes | `ui/panels.rs`, `controller/runtime.rs` |
| Diagnostics, theme, help/version | Status-bar Diagnostics and header **Menu** | `ui/render.rs`, `ui/panels.rs` |

The CLI and TUI remain available and share the same core and source-safety paths. Source documents and schemas are read-only; explicit exports and snapshot writes retain their no-overwrite and source-directory restrictions. Tests for request mapping, modal/detection state, filter serialization and validation, exports, backend behavior, CLI, and TUI ran in the workspace suite. The native review exercised the default and advanced open dialogs, folder detection, table/inspector/source, filter validation and builder edit, menu/options, theme, narrow window, index-only operation, and results JSON export. The index action wrote `search-core-ascii.snapshot` to a custom directory outside the source; the export action wrote a two-hit JSON document outside the source. Both left the current session visible. It did not manually exercise every file picker, relation, batch, or export format in the native session; those paths are covered by targeted and existing automated tests where available.

## Performance and validation

Release core benchmark on `examples/musenalm` (148,486 files), `bench_musenalm --extended --cases Q035,Q045 --profiles ascii-off --repetitions 5`, same host and command for baseline and integration:

| Metric | Baseline `4e15721d38` | Integration `d5953e1330` | Interpretation |
| --- | ---: | ---: | --- |
| Q035 median | 35.094 µs | 33.963 µs | No observed regression in this five-run sample. |
| Q045 median | 184.267 µs | 171.845 µs | No observed regression in this five-run sample. |
| Build | 1.252 s | 1.130 s | One build per branch; not a stable throughput estimate. |
| Snapshot write | 0.326 s | 0.249 s | One write per branch; affected by host I/O. |

These results do not establish a speedup from a visual redesign. The first single-run integration sample was slower than its prior record; matching five-run baseline/integration measurements resolved that apparent discrepancy. The code limits rendered rows to the current page, derives field choices from schemas/available columns, and validates filters without walking source records.

Warm local GUI control was measured separately from index building and image capture, on both revisions with the same 200-file JSON folder, debug build, dark theme, fixed virtual output, and 1920 × 1080 app window except during resize. Each median is eight commands from the local shell; resize alternated 1440 × 900 and 1280 × 800 app windows while leaving the virtual output fixed. Capture medians are separate `desktop_control.py screenshot` calls after each input. No SSH viewer or remote screenshot transport was involved.

| Action | Baseline control dispatch | Integration control dispatch | Baseline capture | Integration capture |
| --- | ---: | ---: | ---: | ---: |
| Scroll | 307.1 ms | 308.1 ms | 121.1 ms | 119.7 ms |
| Selection/inspector | 310.9 ms | 312.9 ms | 117.8 ms | 116.3 ms |
| Resize | 27.5 ms | 28.4 ms | 120.5 ms | 126.5 ms |
| Type one character | 344.4 ms | 343.8 ms | 122.0 ms | 122.5 ms |

The nearly identical results show the local control/capture loop's cost, not GPUI frame latency; every input command starts a helper process and pointer/keyboard bridge. The captures verified that each action took effect. A pure application frame benchmark is not available, so no UI speedup is claimed from these figures.

`cargo test --locked --workspace`, `cargo clippy --locked --workspace --all-targets`, and `python3 scripts/format-rust-spacing.py --check` passed on the combined branch. Clippy emits pre-existing warnings in core/CLI tests but none in the desktop crate. The native launcher verified output size, scale, and refresh before each capture.
