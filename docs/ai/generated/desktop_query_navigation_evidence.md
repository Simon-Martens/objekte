# Desktop query navigation: native evidence

These PNGs are direct captures of the Objekte Wayland window in the maintained headless Sway session. The compositor output was fixed at 1920 × 1080, scale 1, 60 Hz. Window-size captures show the application floating at the named dimensions within that output; the PNG dimensions remain 1920 × 1080. All record data in these captures comes from ignored local test fixtures under `target/gui-validation/desktop/`.

| Capture | What it shows |
| --- | --- |
| [Dark, 1440 × 900](desktop_query_navigation_evidence/dark-1440-final.png), [dark, 1280 × 800](desktop_query_navigation_evidence/dark-1280-final.png) | Search, sidebar, table, and inspector at both requested window sizes. |
| [Light, 1440 × 900](desktop_query_navigation_evidence/light-1440-final.png), [light, 1280 × 800](desktop_query_navigation_evidence/light-1280-final.png), [light, 900 × 650](desktop_query_navigation_evidence/light-small-900.png) | Light theme and narrow-window behavior. |
| [Application menu](desktop_query_navigation_evidence/menu-dark-1440.png), [Columns menu](desktop_query_navigation_evidence/columns-light-clean.png) | Compact hamburger menu and live checkbox chooser. |
| [Filter and FTS emphasis](desktop_query_navigation_evidence/inspector-span-light.png) | Verified FTS span in a record field with the captured filter context. |
| [Builder suggestions](desktop_query_navigation_evidence/builder-values-light.png), [invalid filter](desktop_query_navigation_evidence/invalid-filter-light.png) | Indexed values and visible last-valid-results state. |
| [Expanded complex field](desktop_query_navigation_evidence/author-complex-expanded2.png), [Source view](desktop_query_navigation_evidence/author-source.png), [Links view](desktop_query_navigation_evidence/long-author-links.png) | Structured fields, metadata, Source record actions, and paged backlinks. |
| [Three tabs](desktop_query_navigation_evidence/three-tabs.png), [relation navigation](desktop_query_navigation_evidence/backlink-followed.png) | Independent tabs and link navigation. |
| [Wrapped column](desktop_query_navigation_evidence/wrap-checked-current.png), [first batch](desktop_query_navigation_evidence/fixture-open-final.png) | Column display choice and a 100-row initial delivery. |
| [1,201 rows exhausted](desktop_query_navigation_evidence/long-exhausted.png), [distant Open query](desktop_query_navigation_evidence/distant-open-query.png) | Automatic batches and restored anchor beyond the first page. |
| [Empty system font configuration](desktop_query_navigation_evidence/fontconfig-empty.png) | App launched with an empty Fontconfig file; `FONTCONFIG_FILE=… fc-list` reported zero installed fonts before launch. Bundled typography remained visible. |

The corresponding validation numbers and limitations are in the [completed task report](../done/objekte_desktop_query_navigation_specification.md#implementation-report). Screenshots show UI state, while the tests and measurements in that report establish behavior not visible in a still image.
