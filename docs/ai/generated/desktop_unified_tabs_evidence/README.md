# Desktop unified tabs: native QA evidence

Captured on 2026-11-10 from the native GPUI application in headless Sway, viewed through WayVNC. The Sway output stayed at **1920 × 1080, scale 1, 60 Hz** throughout. Compact screenshots use a floating 1280 × 800 application window on that same output. The fixture under the integration worktree's ignored `target/gui-validation/` directory contains a three-record collection for tab behavior and a 120-record collection for scrolling. See the [GUI environment guide](../../../development/gui_environment.md) for the launcher and display verification commands.

| Check | Native observation | Capture |
| --- | --- | --- |
| Default record placement | Clicking a result opened its inspector in a right-pane tab while the table stayed on the left. | [Record split](record-default-split.png) |
| Independent queries | Queries in both panes kept separate text and result counts; Ctrl+1 and Ctrl+2 restored each editor and its results. The inactive pane showed a read-only preview. | [Two-query switch](two-query-switch.png) |
| Cross-pane drag at compact size | A record tab moved from right to left and back by dragging its label. The inspector remained usable at 1280 × 800. | [Compact drag](compact-drag.png) |
| Many tabs | With 35 query tabs, the strip scrolled to the selected tab, and the open-tabs menu scrolled to entries below its initial viewport. | [Overflow menu](overflow-menu.png) |
| Empty query preview | An inactive query with zero matches displayed a centered explanation and an exact zero count. | [Empty preview](empty-preview.png) |
| One-sided layout | After closing the final left tab, the occupied right pane expanded and the left pane remained a narrow drag target. Dragging a record back restored the split. | [One-sided layout](one-sided-layout.png) |
| Dark compact split | At 1280 × 800, table and query tabs, a right record tab, row selection, and both grid scrollbars remained legible in dark mode. | [Dark compact split](dark-compact-split.png) |
| Keyboard recovery | After closing every tab with Ctrl+W, Ctrl+T created a new query in the left pane and restored the search editor. | [Keyboard recovery](keyboard-recovery.png) |
| Active tab after resize | Closing the last left tab kept the right record active; reopening the collection narrowed that pane and scrolled its strip to the selected record. | [Layout reveal](layout-reveal-fixed.png) |
| Compact list scrolling | At 1280 × 800 with 120 records and overflowing columns, the wheel advanced the visible rows while the horizontal position and column headers stayed fixed. Both scrollbars remained visible. | [Compact list scroll](compact-list-scroll.png) |
| Compact menus | The row context menu stayed inside the window near the right edge, and the Columns popover opened above its footer button with nested fields available but unchecked by default. | [Row menu](compact-row-menu.png), [Columns popover](compact-columns-popup.png) |

Additional native checks covered tab context menus, duplicate queries, middle-click close, Ctrl+T/W/Tab/Shift+Tab/PageUp/PageDown, focused-pane close, moving tabs through the pane body, row Shift-click and context-menu opening, clicking read-only preview rows to activate their query, query copy/paste through the Wayland clipboard, record field copy, light and dark themes, Help placement and scrolling, and restoration after closing active or background query tabs. New query creation stayed disabled until a repository was opened. Stale previews were cleared when a query's structured filter became invalid. These checks used mouse and keyboard input against the actual window, with screenshots retained in the ignored `target/gui-validation/desktop/` directory.
