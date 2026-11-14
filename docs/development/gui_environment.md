# Native GUI development over SSH

Objekte's native application lives in `crates/desktop/src/main.rs`. The separate `crates/desktop/examples/environment_validation.rs` still verifies GPUI Kit and the native Wayland control loop. The exact tested host, versions, migration provenance, and evidence are recorded in the [migration report](../ai/generated/gui_repository_migration_report.md).

## Production desktop application

From the chosen Objekte checkout, launch the read-only application through the tested headless loop:

```bash
bash scripts/gui/run_desktop.sh prepare
target/gui-validation/venv/bin/python scripts/gui/desktop_control.py status
target/gui-validation/venv/bin/python scripts/gui/desktop_control.py screenshot desktop.png
bash scripts/gui/run_desktop.sh cleanup
```

`prepare` builds `objekte-desktop`, validates the fixed 1920 × 1080 output at scale 1 and 60 Hz, starts the loopback VNC pointer server, and launches a tracked native window. The viewer cannot resize this output because WayVNC runs with `-R`. The application accepts a folder, several nonoverlapping folders, or a configured repository. The sidebar switches between **Collections** and **Files** modes. Its collection list chooses one collection or all collections; its file tree filters by indexed directory. The persistent **Search** field is full-text search; **Filter** reveals a second free-text field for the complete structured query grammar. Both inputs combine with AND when the filter is enabled.

The welcome screen's **Open data…** button opens a compact dialog. Type a folder path or use **Browse…**; the dialog detects a configured repository without scanning its source files. **Add another folder** supports multiple plain folders. **Advanced options** contains mode override, schema and recursion settings, and an optional **Try loading saved index** choice with Temporary, Repository, or Custom target. The default builds in memory without reading or writing index files. After a successful open, the dialog closes and the results table fills the window.

The Index panel shows build or loaded origin, readiness, source reconciliation, current publication revision, last saved revision and destination generation, validation counts, and classified errors. **Open saved index…** selects one target and replaces the active session only after a replacement is ready. A missing saved generation falls back to a memory build. **Save index…** selects one target and saves a complete publication in a background worker while keeping tabs, search, and results visible. A failed save offers **Retry save** as another explicit action. A custom base must be an existing directory. Remembered custom paths only prefill dialogs and do not start disk operations.

Type in **Search** to run after a short debounce, or press Enter or **Run** to run immediately. The funnel icon opens the editable **Filter records** line; the distinct builder icon opens the optional synchronized helper. Invalid filter text keeps the visibly labelled last-valid results and disables query-dependent actions until corrected. Filter completion offers fields and indexed values while the filter has focus; Up/Down selects and Enter accepts a suggestion. Table headers edit the visible `SORT BY` clause, dragging a header edge resizes a column, and the grid loads the next 100 records as the scroll approaches the bottom. The footer shows the exact total automatically after a background count. Click a row or select it with the grid arrow keys and press Enter to preview the entry beside its table without creating a tab. The preview offers **Open in tab** with an icon and label; Shift-click or choose **Open in new tab** from a row's context menu also retains a separate tab. Pinned record tabs have Back, Previous, Next, Open query, metadata, Fields, Links, and a Source icon. The side preview omits the Preview label, result position, and Open query action. Source contains Copy record and Export record. At a narrow window size, selecting a row collapses the collection sidebar to preserve room for search and the table; the header button restores it.

Tables, queries, and explicitly retained records use movable tabs. A selected record's ordinary preview splits its table and has no tab; an explicitly opened record tab appears beside its source query. Drag a tab label between panes or use its right-click menu. **+** creates a query in that pane, and the list icon opens a scrollable menu of all tabs when the strip fills. Ctrl+T opens a query, Ctrl+W closes the selected tab, Ctrl+Tab or Ctrl+PageDown advances within the focused pane, and Ctrl+Shift+Tab or Ctrl+PageUp goes back. Ctrl+1 and Ctrl+2 focus the left and right panes. An inactive query shows a bounded read-only preview; click a preview row to activate that query and refresh its full results. Choose **Files** in the sidebar to browse indexed directories. Selecting a directory filters the current query, including its count, pages, and exports; **All files** clears that scope. The directory choice is retained in query history and saved queries. The query text menu copies or pastes the entire Search or Filter expression; right-click inside a text field for selection editing.

**Columns** sits in the table footer and opens above its button. Top-level fields are shown by default; nested fields can be enabled in this menu. A header context menu offers **Wrap text**. The first grid column shows green or red status dots, and an empty string appears as `-`. The header hamburger opens the application menu with Save Query, Columns, Export, Index, batch queries, diagnostics, theme, and help. Saved queries and collapsible history are visible in the sidebar. Saving uses an automatic title that can be renamed inline. The default theme is dark; a saved theme choice takes precedence when a repository opens. Menus float over the table without reducing its height. The earlier redesign's action map and native screenshots remain in the [desktop interface redesign parity record](../ai/generated/desktop_interface_redesign_parity.md).

Filter field suggestions include the reserved `@record`, `@file`, `@collection`, and `@repository` metadata fields in every collection scope. The condition builder assigns their scalar types automatically. Numeric value controls accept byte sizes such as `23MB`, `2KB`, `02GiB`, and `1.5KiB`, as does the filter text (`@file.size >= 23MB`). Decimal units use powers of 1000; binary units such as KiB, MiB, and GiB use powers of 1024. Unit names are case-sensitive, and values must resolve to a nonnegative whole number of bytes. Metadata columns display values captured with the result page, including when the JSON payload is unavailable; file sizes and byte ranges use bytes, timestamp columns use Unix epoch milliseconds, and a missing creation timestamp displays `null`.

The File column, `@file.path`, record tabs and exports show source paths
relative to the opened repository root. The owning session retains that root
for source access; two open repositories may display the same relative path
without sharing an absolute source location.

The production `desktop_control.py status` and `screenshot` commands were also run from a fresh `ssh localhost` login on the tested host. They rediscovered the live Sway and application service without copied display variables; the status still reported 1920 × 1080, scale 1, and 60 Hz. The full native acceptance captures and results are indexed in the [desktop parity record](../ai/generated/desktop_application_parity.md).

Source documents and collection schemas remain read-only. **Save Query** writes the dedicated desktop settings file at `.objekte/.cache/obj-desktop.json` under the opened repository root. **Export Results** and **Export Selected** require a new output file outside scanned collection directories; they never overwrite an existing file. The derived index is saved only through **Save index…**. Ordinary close, repository switching, and source reconciliation do not save it. The **Index** panel shows the active origin, explicit save outcome, and any storage error. An ordinary memory build does not read or write saved index files. The CLI terminal UI's settings file remains separate. On Wayland, `wl-copy` carries complete field and record values to the system clipboard, and `wl-paste` supplies query text for the Paste menu. The launcher checks for both commands.

The GPUI window owns a virtualized result list and a bounded cache of projected 100-row pages, plus bounded detail text. It sends typed actions to `DesktopController`. Worker threads own repository loading, queries, record reads, posting-value completion, batch execution, exports, managed index work, and preference writes. The controller retains one repository across searches, cancels superseded grid requests, and rejects completion messages from old generations. Record tabs keep separate query contexts and operation generations. The desktop backend uses the core explicit load and flush APIs, CLI definition preparation, and core query and streaming APIs. Its record formatting, relation rows, and JSON export code live in the desktop crate.

Fresh index construction exposes a searchable file catalogue before every batch finishes. A canceled open cancels its initial build task and leaves the previous repository visible. Query, batch, and export work use cooperative cancellation.

At the integration branch's debug build, graphical opening of `examples/musenalm` took 56.5 seconds for seven collections and 148,486 records. The first 100-row page took 8.1 ms after opening; the process held about 711 MiB RSS. A same-corpus debug CLI run took 55.6 seconds and about 581 MiB peak RSS. The large startup cost is index construction in debug mode; it does not block the window. A release core benchmark built the corpus in 1.125 seconds, so do not compare that release figure directly with the debug GUI timing. See the [desktop parity record](../ai/generated/desktop_application_parity.md) for exact checks and limits.

## How the session works

`scripts/gui/headless_session.py` starts one independent Sway user service on demand. Sway uses the wlroots headless backend, one 1920 × 1080 output at scale 1 and 60 Hz, and the Intel render node on the tested host. It does not need the physical display, GDM seat, or a human VNC viewer. A private `dbus-run-session` bus stays with the compositor. The Sway startup helper records its **actual** Wayland and IPC sockets in `/run/user/$UID/gpui-headless-sway/manifest.json`. Subsequent SSH commands validate the service PID, ownership, sockets, and output mode before using those values. No display variables need to be copied into an SSH shell.

`WLR_BACKENDS=headless` prevents use of the physical DRM, nested Wayland, and X11 backends. `WLR_LIBINPUT_NO_DEVICES=1` avoids physical input devices, and `WLR_HEADLESS_OUTPUTS=1` creates one virtual output. `WLR_RENDER_DRM_DEVICE=/dev/dri/renderD128` selects the tested Intel render node; adapt this path after verifying GPU permissions on another host. Sway uses hardware GLES through Mesa while the GPUI client uses its normal Linux Vulkan path. The app is launched as a separate tracked user unit with the recorded Wayland socket and private bus address.

`grim` captures the selected output directly. `wtype` enters Unicode text. This host's first event from a new `wtype` virtual keyboard was lost, so the `type` command primes it with a harmless Shift event. `wlrctl` pointer clicks were unreliable with the ephemeral virtual pointer; the supported pointer path is WayVNC plus `vncdo` for absolute clicks and wheel events. WayVNC listens only on `127.0.0.1:5907`, and `-R` disables viewer-driven output resizing. The launcher refuses any live output other than 1920 × 1080, scale 1, 60 Hz. Coordinates in this guide are output-local physical pixels.

## Prerequisites and setup

The tested host is Fedora 44 with Rust 1.99.1, Sway 1.11, GPUI Kit 0.9.0 with `gpui-pre` 0.3.11, Mesa 26.2.6 on Intel RPL-S, WayVNC 0.9.1, grim 1.5, `wtype` 0.4, and `wl-copy`/`wl-paste` from `wl-clipboard`. The user needs access to a working GPU render node and a user-owned `/run/user/$UID`. A user systemd manager is used for process lifetime. Lingering is enabled on the tested account so the manager can survive the last SSH logout; that final-logout case was not directly exercised. Host RPM installation requires administrator authentication here, but the Sway, swaybg, and wtype executables can be unpacked under ignored `target/gui-validation/local-root/`:

```bash
cd /home/simon/source/objekte-integration-gui-repository-migration
bash scripts/gui/install_fedora_dependencies.sh check
bash scripts/gui/install_user_dependencies.sh   # only if Sway/wtype are absent from PATH and target/
bash scripts/gui/check_development_environment.sh
```

The user-local installer downloads Fedora RPMs; it does not install host packages or change the RPM database. `prepare` creates an ignored Python environment for the pinned RFB client if needed. All generated screenshots, logs, fixtures, and local executables stay under ignored `target/gui-validation/`. Cargo uses its usual persistent `target/` and global download cache. Keep incremental compilation enabled; do not use `cargo clean` for ordinary UI edits.

## Daily validation loop

From the selected Objekte checkout:

```bash
bash scripts/gui/run_gui_validation.sh prepare
# Inspect target/gui-validation/initial.png with an image-capable tool.
bash scripts/gui/run_gui_validation.sh exercise 400 132 34 173 76 214 500 450 180 214
cat target/gui-validation/fixture.json
bash scripts/gui/run_gui_validation.sh cleanup
```

`prepare` performs `cargo build --locked -p objekte-desktop --example environment_validation`, starts or reuses Sway, starts the loopback pointer server, launches the example, checks its app ID/PID and `xdg_shell` identity, and captures the initial PNG. `exercise` enters `Objekte ÄÖÜ äöü ß 123`, checks the checkbox, increments the counter, scrolls the list, saves through GUI input, and verifies the exact JSON fixture. Inspect `after-type.png`, `after-controls.png`, `after-scroll.png`, and `final.png` in `target/gui-validation/` before recording visual success. The expected fixture is `{"text":"Objekte ÄÖÜ äöü ß 123","checked":true,"action_count":1}`. The list should move from row 01 to about row 14.

For individual operations:

```bash
python3 scripts/gui/headless_session.py start
python3 scripts/gui/headless_session.py status
target/gui-validation/venv/bin/python scripts/gui/gui_control.py status
target/gui-validation/venv/bin/python scripts/gui/gui_control.py launch
target/gui-validation/venv/bin/python scripts/gui/gui_control.py focus
target/gui-validation/venv/bin/python scripts/gui/gui_control.py screenshot manual.png
target/gui-validation/venv/bin/python scripts/gui/gui_control.py type 400 132 'Objekte ÄÖÜ äöü ß 123'
target/gui-validation/venv/bin/python scripts/gui/gui_control.py click 34 173
target/gui-validation/venv/bin/python scripts/gui/gui_control.py scroll 500 450 down 5
target/gui-validation/venv/bin/python scripts/gui/gui_control.py close
python3 scripts/gui/headless_session.py stop
```

`headless_session.py exec -- COMMAND ARGS...` also runs a short command with the validated session environment. Input commands verify that the tracked window still has focus. `status` prints the selected output, geometry, app PID, and pointer server PID. The validation executable is `target/debug/examples/environment_validation`; its screenshot and fixture paths must be under this checkout. To run the in-process GPUI Kit interaction test separately, use `cargo test --locked -p objekte-desktop --example environment_validation`. The test does not replace the native GUI loop.

From a fresh SSH login, change to the chosen checkout and run the same `prepare` and `exercise` commands. The manifest discovers the live sockets. `prepare` deliberately restarts only the task-owned validation app; the Sway session remains available across builds. After merging this branch and switching to `/home/simon/source/objekte`, run `bash scripts/gui/install_user_dependencies.sh` there if Sway, swaybg, or wtype are unavailable on `PATH`: the ignored local executables do not move with Git. Then run `python3 scripts/gui/headless_session.py update` **from that checkout** before removing the integration worktree. `update` stops the old task-owned app, pointer server, and Sway unit, then starts a service with paths derived from the current checkout. Verify with `status`, then run `prepare` again. If an update fails, the separate environment repository remains a recovery source; its `scripts/headless_session.py start` can restore the prior session after the failed unit is stopped.

For optional human viewing, keep the loopback server running and tunnel it: `ssh -L 5907:127.0.0.1:5907 simon@work`, then point a VNC viewer at `localhost:5907`. The viewer is not required for agent control. A first immediate RFB capture was gray while its initial frame arrived; a capture 0.2 seconds later showed the application. WayVNC's `-R` prevents client resize requests from changing the output. The mode was unchanged after an actual RFB connection; an explicit resize request from every viewer type was not tested. Loopback does not prevent another local user from connecting, so use host-appropriate access control if this becomes a shared machine service.

## Diagnosed failures and limits

- Direct Hyprland launch from this SSH login failed to acquire the physical `libseat` seat. Headless Sway avoids that seat; this observation does not rule out other Hyprland configurations.
- A Gamescope parent exposed `wl_compositor` v5 while the installed Hyprland requested v6. The nested attempt failed on that version mismatch.
- GNOME Shell's headless display rendered on the Intel GPU, but direct WayVNC and `grim` control failed because GNOME did not expose the required virtual-pointer and screencopy protocols in that probe. GNOME Remote Desktop was not integrated into this workflow.
- A missing local `swaybg` executable caused a harmless background-start log error. The local installer now supplies it.
- Direct `wlrctl` move/click invocations reported success without a reliable control change. WayVNC plus `vncdo` supplied working absolute pointer input. The first VNC connection is primed before real clicks; `wtype` is primed before Unicode text.
- `sudo -n` requires a password on this host. User-local RPM extraction supplied Sway tools without modifying host packages. A different host may need its normal administrator package process.

The prior 1600 × 1000 measurement campaign found medians of 246 ms for session start (five samples), 702 ms for app launch to a visible screenshot (ten), 105 ms for screenshot (ten), and 413 ms for click to a captured visible counter change (ten). Those are **not** 1920 × 1080 timings. The change to Sway simplified session startup and control; no direct Sway-versus-Hyprland performance comparison was measured. A later 1080p incremental locked build took 0.16 seconds with caches populated. The migration report records any new comparison and remaining limits.
