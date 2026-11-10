After you are done, integrate it into master.

After, iterate a bit over the UI. Add options you might think we need like cupy past, a menu stuff like that.

I also want a tab system for everything except the sidebar: so tables and queries in tabs, entry inspector in tabs. Tabs should be movable freely. But we still open entries by default in a split view (new tab bar to  the right) even though the user can move tabs around as he or she wishes.

Spend 4-5 hours iterating over night. All changes on an extra branch. Use subagents and worktrees if you have multiple complex tasks that can be done in parallel. Your integrator should review all chnages and look that they look like a typical program and aesthetically pleasing.

## Reopened for implementation

On 2026-11-10 the user clarified that the requested four-hour feature iteration remained unfinished and rejected the initial tab design and testing quality. The first pass had been merged earlier; the task was reopened on the same integration branch for feature work and native visual testing.

## Implementation report

- Task identifier: `desktop-unified-tabs`.
- Completed: 2026-11-10 after the requested four-hour iteration window.
- Original task base: `0ebb0f5e17bf955944cc92ddb0d94abf208cc3b9`; reopened implementation base: `3a1e29692167888074eb4d2152ac59b1f3b322d1`.
- Integration branch: `integration/desktop-unified-tabs` in `/home/simon/source/objekte-integration-desktop-unified-tabs`.
- Implementation: first-pass commits `a2d22bfc02`, `5c4a23e0fc`, `5eca1ba4c8`; reopened implementation and native QA commits from `4420f6b128` through `2ecaff657a` (with final report commit to follow). Contributing subtask branches `task/desktop-unified-tabs/query-controls` (`0402df20f4`, merged at `dc5f51a43c`) and `task/desktop-unified-tabs/inspector-actions` (`c155bb50a5`, merged at `886d2d03c7`). The earlier first-pass subtask branches were `task/desktop-unified-tabs/query-editing` and `task/desktop-unified-tabs/workspace`.

Tables, independent query drafts, and record inspectors occupy two tab groups. Clicking a result opens its record in the right pane by default; ordinary selection reuses the active record tab, while Shift-click or the row menu opens another. Tabs can be reordered or moved between panes by dragging or through their context menus, and the split is resizable. The active tab is kept visible as panes resize or tab strips overflow. Empty and one-sided layouts keep usable drop targets; closing the last tab preserves keyboard focus and a new query can be opened with Ctrl+T. Ctrl+W, Ctrl+Tab, Ctrl+Shift+Tab, Ctrl+PageUp/Down, and Ctrl+1/2 navigate the focused pane and its tabs.

Query tabs retain their own draft, title, and bounded result preview. Activating an inactive query restores its editor and refreshes its results; clicking a preview row activates it. Empty and stale previews have explicit states. The query menu offers whole-search and whole-filter Copy and Paste; clear controls, undoable replacement, and the application menu support routine editing. Record inspectors retain bounded snapshots for inactive tabs; clicking a preview field activates that record. Inspectors offer field path/value copy, complete-record copy, and relation navigation. Context menus provide record ID/path copy and new-tab opening. Menus, icons, focus states, column selection, grid scrollbars, and compact/dark layouts were refined through native visual testing. New query is disabled until data is open.

Validation on the combined integration branch:

- `cargo test --locked --workspace`: 789 passed, 13 intentionally ignored, 0 failed.
- `cargo build --locked --workspace --all-targets`: passed with no Rust compiler warnings.
- `git diff --check`: passed.
- Native GPUI testing in headless Sway through WayVNC on `HEADLESS-1` at 1920 × 1080, 100% scaling, 60 Hz. A three-record fixture covered tab operations; a 120-record fixture covered list scrolling. A floating 1280 × 800 application window tested compact layouts without changing output resolution. Checks covered default split, cross-pane drag and reordering, context menus, overflow and selected-tab reveal, keyboard focus/navigation and empty-workspace recovery, independent query drafts and previews, copy/paste, light and dark themes, vertical scrolling without horizontal drift, and menu placement. Reproducible observations and curated captures: [`docs/ai/generated/desktop_unified_tabs_evidence/README.md`](../generated/desktop_unified_tabs_evidence/README.md).
- No existing performance benchmark exercises tab layout, GPUI painting, or clipboard interactions. Search/index execution paths did not change, so no comparable benchmark applies. The repository-wide spacing formatter still reports pre-existing differences outside this task, beginning in `crates/cli/src/loader.test.rs`; no unrelated files were reformatted.

Remaining limitation: the controller has one live query editor and result list. When query tabs are visible in both panes, the other pane shows its latest bounded, read-only preview. A preview row activates that query, then the refreshed live results can be inspected. No other in-scope failure remains from this pass.

Cleanup status at report commit: integration worktree retained; completed subordinate worktrees are merged, pushed, and free of unpreserved files. Their normal removal follows the report push. Temporary task memory will be deleted after the report is pushed. The user authorized merging and pushing the integration result to `master`.
