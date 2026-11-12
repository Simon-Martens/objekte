# Staged index construction: desktop validation

2026-11-12, desktop task branch. The native checks used the repository-local headless Sway session at 1920 × 1080, scale 1, 60 Hz. Generated source fixtures were kept under ignored `target/gui-validation/`; the linked PNGs are committed evidence.

## Checks

| Check | Result |
| --- | --- |
| `cargo test --locked -p objekte-desktop --lib` | 180 passed, 1 ignored after incremental catalogue and live inference changes. |
| `cargo build --locked -p objekte-desktop --bin objekte-desktop` | Passed; the native process launched under Sway. |
| Plain-folder Open, 3 JSON files | SearchReady returned 3 rows with stable `@file.id` and paths. The Files sidebar showed the same 3 file rows and one nested folder. [Screenshot](staged_index_construction_desktop_evidence/staged-opened.png). |
| Boolean type filter | `kind:array = true` returned only the array-valued `c.json` record. [Screenshot](staged_index_construction_desktop_evidence/staged-array-filter-3.png). |
| Index management | Displayed independent field-index state (`Not requested` for the unconfigured folder). [Screenshot](staged_index_construction_desktop_evidence/staged-index-extended.png). |
| Large file catalogue | A generated 30,000-file JSON folder opened and its Files sidebar listed 30,000 virtualized rows without a window failure. [Screenshot](staged_index_construction_desktop_evidence/staged-large-json-progress.png). |
| Explicit live inference | A 24-record plain folder opened with field indexes `Not requested`, then **Infer field indexes** reached `Complete` without clearing its 24 results. [Before](staged_index_construction_desktop_evidence/index-before-inference.png) · [After](staged_index_construction_desktop_evidence/index-after-inference.png). |
| On-demand capability details | **Index → Details…** showed five observed field paths, four admitted, one dropped, coverage, and capability revision 4 after inference. [Screenshot](staged_index_construction_desktop_evidence/index-details.png). |
| Theme and narrow layout | At an 1100 × 700 native window size, the light theme retained the Files sidebar, search, table and count. Selecting a row and pressing Enter opened its record preview in the same window. [Window](staged_index_construction_desktop_evidence/small-light-window.png) · [Preview](staged_index_construction_desktop_evidence/small-keyboard-preview.png). |

The active desktop Open path starts one core staged task. It uses that task's catalogue for listing and does not start a separate `SourceScan`. The watcher opens the workspace after the first published discovery entry, refreshes file rows as the catalogue grows, and leaves the current query and tabs in place on later catalogue updates. Search drafts entered during a replacement Open are held until SearchReady. Catalogue row preparation and collection grouping run on the discovery worker, outside the GPUI render thread. Capability-only status is keyed separately from the data publication used for query and tab freshness.

Focused backend tests cover plain-folder staged opening, explicit write followed by cached load, configured cache reuse versus memory build, file identity and collection grouping, and queued queries during replacement. A state test confirms that a late catalogue update preserves the current result selection and search text.

One existing desktop cache-reuse test failed immediately after the core partial-publication checkpoint: its second identical FTS/filter/sort query missed the verified-result cache. Core commit `691cd2daf4ea` restored the blocking `Index::build` segmented layout; the previously failing test and the complete desktop suite then passed. No test expectation was weakened.

Two desktop batch parity tests initially compared exact physical work counters with the CLI. The CLI now opens through staged construction while that desktop test fixture uses a blocking build, so those plan counters legitimately differ. The parity assertion now compares protocol fields, hits, counts and errors, leaving physical counters to the respective execution diagnostics; all seven batch tests pass. This changes no query-result expectation.

## Remaining native evidence

The 30,000-file base reached SearchReady before the first screenshot (about half a second on this fixture), so the native captures do not isolate the early catalogue-only frame. Core's focused callback test confirms discovery publication after the first accepted file, and the desktop watcher now consumes those callbacks. The Cancel/Resume/Retry controls are wired to the core extended task, but this short native fixture completed inference before a pointer cancellation could be captured. Core lifecycle tests exercise interruption and resumption. A plain-folder `.jsonl` probe listed zero files because the default plain-folder source choice is JSON; the large probe used `.json` files instead.
