# Desktop capability parity

Task: `desktop-application-initial`. Baseline: Objekte `master` at `1da55f0c6d9dce7d2cb73430c2d67d974577fcce` (8 November 2026). This inventory is the acceptance checklist for the native GPUI application. The CLI and TUI implementations at that commit define the semantics; the task specification `objekte_initial_desktop_application_implementation.md` supplies additional presentation requirements. Results will be filled with test and GUI evidence as each phase is integrated. **Pending means unverified, not implemented.**

The baseline has two terminal frontends: `crates/cli/src/{arguments,query_input,execution,output}.rs` and `crates/tui/src/tui/`. The latter's actions are in `app.rs` and `mod.rs`; settings, detail, links, and exports are in `settings.rs`, `record.rs`, `relations.rs`, and `export.rs`. `docs/cli/obj.md` describes the public CLI behavior. Some TUI code is also mirrored under `crates/cli/src/tui/`; parity is judged against behavior, not the duplicate source location.

## Opening and data operations

| ID | Baseline capability | Desktop workflow | Verification |
| --- | --- | --- | --- |
| O01 | Default current folder and one explicit `--path`; single `files` collection | Open folder or enter a path | Pending |
| O02 | Repeated `--path`, distinct names, nonoverlap, useful common root, selected-only discovery | Add multiple folders | Pending |
| O03 | `--repo` configured collections, schema metadata, relation targets, path validation | Open configured repository | Pending |
| O04 | `--schema` for one folder and incompatible-option errors | Schema option with validation | Pending |
| O05 | `--no-recurse` for one-folder discovery; `--repo` conflict | Recursion option with validation | Pending |
| O06 | In-memory default; no implicit snapshot write | Open with snapshot operation off | Pending |
| O07 | `--load-index` valid restore or fatal failure, default/custom path | Load Index option | Pending |
| O08 | `--write-index` fresh build/save; default/custom path | Write Index action | Pending |
| O09 | Load+write recoverable rebuild/save, matching directory rule | Combined snapshot options | Pending |
| O10 | Index-only invocation without a query | Write/Load Index with no search | Pending |
| O11 | `--auto-index` inference and bounded ordered postings | Automatic indexing option | Pending |
| O12 | `--index-alphabet ascii/unicode`, independent of query case | Alphabet selector | Pending |
| O13 | Loading before results, startup progress/diagnostics, visible errors | Nonblocking loading/status area | Pending |
| O14 | One/all collection scope; single-folder semantics | Collection sidebar | Pending |

## Search, results, and output

| ID | Baseline capability | Desktop workflow | Verification |
| --- | --- | --- | --- |
| Q01 | Separate full-text and structured filter, combined with AND | Full Text and Structured inputs | Pending |
| Q02 | Full accepted structured syntax and parser errors retained | Structured input and inline error | Pending |
| Q03 | Literal FTS and CLI-style complete JSON-string literal decoding | Full Text/direct query controls | Pending |
| Q04 | Regex/literal and case-sensitive/insensitive FTS; sensitive literal default | Regex and Case controls | Pending |
| Q05 | 180 ms debounce, explicit Enter execution, cancel, stale-result rejection | Live and Run controls | Pending |
| Q06 | TUI sort picker and inline `SORT BY` with conflict semantics | Sort selector plus query parser | Pending |
| Q07 | Global result ordering through core, not visible-page sorting | Result grid | Pending |
| Q08 | `--offset`, finite/unlimited `--limit`, zero-limit | Range controls | Pending |
| Q09 | Exact `--count`, all-results exact total, count pending state | Count control and status | Pending |
| Q10 | Bounded result pages, Home/End/Page/arrow and mouse selection | Virtualized result grid | Pending |
| Q11 | Configured/default columns, show/hide, horizontal navigation | Column menu/grid | Pending |
| Q12 | Match fields/evidence, preview visibility, selected record | Match indication and preview toggle | Pending |
| Q13 | Selected-record TUI JSON export with metadata and payload | Export Selected | Pending |
| Q14 | All-match TUI JSON export over requested query range, no overwrite | Export Results (JSON) | Pending |
| Q15 | CLI-compatible NDJSON, event order, full payload/metadata, fallback representation | Export Results (NDJSON) | Pending |
| Q16 | File or pasted physical-line batch, BOM/line endings/blanks, line numbers, per-line continuation | Batch input and outcomes | Pending |
| Q17 | Query/run/output errors and cancellation; no false success | Status and per-query error list | Pending |
| Q18 | Operational diagnostics, inference/index counters and timing | Diagnostics panel | Pending |

## Inspection and interaction

| ID | Baseline capability | Desktop workflow | Verification |
| --- | --- | --- | --- |
| I01 | Selected record preview, structured detail, raw source | Preview/detail panel and raw toggle | Pending |
| I02 | Object fold and bounded array expansion | Detail tree controls | Pending |
| I03 | Metadata and source diagnostic fields | Detail metadata/diagnostic sections | Pending |
| I04 | JSON null versus logical unavailable/fallback/raw records | Distinct detail labels | Pending |
| I05 | Previous/next result and section navigation | Detail navigation | Pending |
| I06 | Find in record, next/previous match | Detail Find controls | Pending |
| I07 | Outgoing and backlink relation traversal, continuation, Back | Relation rows and Back | Pending |
| I08 | Copy selected value/relation and complete record | Copy Value/Copy Record | Pending |
| I09 | Query history (bounded, deduplicated, newest last) | History menu | Pending |
| I10 | Named saved queries with replace/restore semantics | Save Query/Saved Queries | Pending |
| I11 | Restore text, filter, collection, case, regex from history/saved | Restore action | Pending |
| I12 | Persisted theme and hidden columns without overwriting TUI settings | Desktop preferences | Pending |
| I13 | Action picker, shortcut help, mouse/keyboard navigation | Actions/help/focus traversal | Pending |
| I14 | Discoverable help/version and desktop instead of `--tui` terminal launch | About/help | Pending |

## Safety and responsiveness

| ID | Baseline or task requirement | Desktop workflow | Verification |
| --- | --- | --- | --- |
| S01 | Source JSON and schemas remain unchanged | Read-only open, query, inspect | Pending |
| S02 | Generated indexes, exports, and settings have explicit destinations | Write Index/Export Results/Save Query | Pending |
| S03 | Exports cannot overwrite source records or add scanned records | Output path validation | Pending |
| S04 | Repository retained across query/scope changes; superseded work canceled | Worker controller | Pending |
| S05 | All-results mode streams output without retaining all bodies in the UI | Paged grid and export worker | Pending |
| S06 | Display stays responsive during load, scan, export, and cancellation | Native GUI interaction | Pending |
| S07 | 1920 × 1080 at scale 1/60 Hz; RFB viewer does not resize output | Existing headless launcher | Pending |

## Presentation reference

The [DBDelve project](https://github.com/ShayanAbbas1/dbdelve), its [query screenshot](https://github.com/ShayanAbbas1/dbdelve/blob/main/assets/screenshots/query-dark.png), [record screenshot](https://github.com/ShayanAbbas1/dbdelve/blob/main/assets/screenshots/table-dark.png), and the README's scrolling/launch demonstrations were inspected on 8 November 2026. The desktop layout will adapt the compact sidebar, query area, result grid, and right record panel. DBDelve's SQL, connection management, query tabs, and record editing are outside this task.
