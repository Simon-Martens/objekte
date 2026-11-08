# Desktop results usability follow-up

The user reported that the GPUI desktop application was unintuitive, lacked icons, and did not scroll its results list. They asked whether the DBDelve presentation reference had been inspected. This direct request authorized the follow-up; no new task document was supplied in `tasks/`.

## Implementation report

- Task identifier: `desktop-results-usability`
- Completion date: 2026-11-08
- Base commit: `a9e8d0392e` (`integration/desktop-application-initial`)
- Integration branch: `integration/desktop-results-usability`
- Integration worktree: `/home/simon/source/objekte-integration-desktop-results-usability`
- Reviewed DBDelve's query and table screenshots. Added GPUI Kit icons and selected states to the main navigation, collection list, opening choices, search, result paging, and record controls. New windows use the dark theme until a saved preference is loaded.
- Replaced the empty grid with a clear Open repository action. Successful opening closes the setup form. The record pane appears when a result is selected, leaving more width for the grid otherwise.
- Fixed the results viewport's height constraints and non-shrinking rows. It now uses a single GPUI scroll handle for the viewport, vertical scrollbar, and selection scrolling. Row cells have a consistent height and clipped text.
- Validation: `cargo check --locked -p objekte-desktop`, `cargo test --locked -p objekte-desktop --lib` (36 passed), `python3 scripts/format-rust-spacing.py --check`, and `git diff --check` passed. The native Fedora 44/Sway/WayVNC run at 1920 × 1080, scale 1, 60 Hz opened Musenalm's 210-record `places` folder and rendered 100-row pages. Six mouse-wheel steps changed the first visible row from 1 to 13. Evidence: [welcome](../generated/desktop_results_usability/empty.png), [results before scroll](../generated/desktop_results_usability/results-top.png), [results after scroll](../generated/desktop_results_usability/results-scrolled.png).
- Performance benchmarks omitted: this changes presentation and scroll behavior, with no search or index implementation changes.
- Remaining limitation: the result column header scrolls with the rows; a pinned header would require synchronized horizontal and vertical table scrolling. The prior 100-row page size is unchanged.
- No subordinate worktrees were created.
