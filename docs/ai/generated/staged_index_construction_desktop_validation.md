# Staged index construction: desktop validation

2026-11-12, desktop task branch. The native checks used the repository-local headless Sway session at 1920 × 1080, scale 1, 60 Hz. Generated source fixtures were kept under ignored `target/gui-validation/`; the four PNGs linked below are committed evidence.

## Checks

| Check | Result |
| --- | --- |
| `cargo test --locked -p objekte-desktop --lib` | 177 passed, 1 ignored after the desktop catalogue and status changes. |
| `cargo build --locked -p objekte-desktop --bin objekte-desktop` | Passed; the native process launched under Sway. |
| Plain-folder Open, 3 JSON files | SearchReady returned 3 rows with stable `@file.id` and paths. The Files sidebar showed the same 3 file rows and one nested folder. [Screenshot](staged_index_construction_desktop_evidence/staged-opened.png). |
| Boolean type filter | `kind:array = true` returned only the array-valued `c.json` record. [Screenshot](staged_index_construction_desktop_evidence/staged-array-filter-3.png). |
| Index management | Displayed independent field-index state (`Not requested` for the unconfigured folder). [Screenshot](staged_index_construction_desktop_evidence/staged-index-extended.png). |
| Large file catalogue | A generated 30,000-file JSON folder opened and its Files sidebar listed 30,000 virtualized rows without a window failure. [Screenshot](staged_index_construction_desktop_evidence/staged-large-json-progress.png). |

The active desktop Open path starts one core staged task. It uses that task's catalogue for listing and does not start a separate `SourceScan`. Search drafts entered during a replacement Open are held until SearchReady. Catalogue row preparation and collection grouping run on the discovery worker, outside the GPUI render thread. Capability-only status is keyed separately from the data publication used for query and tab freshness.

One existing desktop cache-reuse test failed immediately after the core partial-publication checkpoint: its second identical FTS/filter/sort query missed the verified-result cache. Core commit `691cd2daf4ea` restored the blocking `Index::build` segmented layout; the previously failing test and the complete desktop suite then passed. No test expectation was weakened.

## Remaining native evidence

The 30,000-file base reached SearchReady before the first screenshot (about half a second on this fixture), so these captures do not show a separate catalogue-only frame. The explicit live inference action, cancellation/resume interaction, smaller window, and light theme will be checked after the corresponding core API and desktop control are wired. A plain-folder `.jsonl` probe listed zero files because the default plain-folder source choice is JSON; the large probe used `.json` files instead.
