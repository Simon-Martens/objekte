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

`prepare` builds `objekte-desktop`, validates the fixed 1920 × 1080 output at scale 1 and 60 Hz, starts the loopback VNC pointer server, and launches a tracked native window. The viewer cannot resize this output because WayVNC runs with `-R`. The application accepts a folder, several nonoverlapping folders, or a configured repository. Its sidebar chooses a collection or all collections; Full Text and Structured inputs combine in one query. Result pages, detail and source views, relation links, history, saved queries, columns, theme, batch input, and diagnostics are available in the single window. Help shows keyboard controls and the application version.

Source documents and collection schemas remain read-only. **Save Query** writes the dedicated desktop settings file at `.objekte/.cache/obj-desktop.json` under the opened repository root. **Export Results** and **Export Selected** require a new output file outside scanned collection directories; they never overwrite an existing file. **Write Index** explicitly saves a derived snapshot at the selected default or custom index path. Ordinary Open and search with Snapshot None leave snapshots untouched. The TUI's settings file remains separate. On Wayland, the tested `wl-copy` command carries complete field and record values from background workers to the system clipboard; the launcher checks for it.

The GPUI window owns bounded display pages and sends typed actions to `DesktopController`. Worker threads own repository loading, queries, record reads, batch execution, exports, snapshot work, and preference writes. The controller retains one repository across searches, cancels superseded requests, and rejects completion messages from old generations. The desktop backend reuses the CLI loader/execution protocol, core query and streaming APIs, and TUI record/export helpers. This keeps complete output available while the window holds at most 100 result rows and bounded detail text.

At the integration branch's debug build, graphical opening of `examples/musenalm` took 56.5 seconds for seven collections and 148,486 records. The first 100-row page took 8.1 ms after opening; the process held about 711 MiB RSS. A same-corpus debug CLI run took 55.6 seconds and about 581 MiB peak RSS. The large startup cost is index construction in debug mode; it does not block the window. A release core benchmark built the corpus in 1.125 seconds, so do not compare that release figure directly with the debug GUI timing. See the [desktop parity record](../ai/generated/desktop_application_parity.md) for exact checks and limits.

## How the session works

`scripts/gui/headless_session.py` starts one independent Sway user service on demand. Sway uses the wlroots headless backend, one 1920 × 1080 output at scale 1 and 60 Hz, and the Intel render node on the tested host. It does not need the physical display, GDM seat, or a human VNC viewer. A private `dbus-run-session` bus stays with the compositor. The Sway startup helper records its **actual** Wayland and IPC sockets in `/run/user/$UID/gpui-headless-sway/manifest.json`. Subsequent SSH commands validate the service PID, ownership, sockets, and output mode before using those values. No display variables need to be copied into an SSH shell.

`WLR_BACKENDS=headless` prevents use of the physical DRM, nested Wayland, and X11 backends. `WLR_LIBINPUT_NO_DEVICES=1` avoids physical input devices, and `WLR_HEADLESS_OUTPUTS=1` creates one virtual output. `WLR_RENDER_DRM_DEVICE=/dev/dri/renderD128` selects the tested Intel render node; adapt this path after verifying GPU permissions on another host. Sway uses hardware GLES through Mesa while the GPUI client uses its normal Linux Vulkan path. The app is launched as a separate tracked user unit with the recorded Wayland socket and private bus address.

`grim` captures the selected output directly. `wtype` enters Unicode text. This host's first event from a new `wtype` virtual keyboard was lost, so the `type` command primes it with a harmless Shift event. `wlrctl` pointer clicks were unreliable with the ephemeral virtual pointer; the supported pointer path is WayVNC plus `vncdo` for absolute clicks and wheel events. WayVNC listens only on `127.0.0.1:5907`, and `-R` disables viewer-driven output resizing. The launcher refuses any live output other than 1920 × 1080, scale 1, 60 Hz. Coordinates in this guide are output-local physical pixels.

## Prerequisites and setup

The tested host is Fedora 44 with Rust 1.99.1, Sway 1.11, GPUI Kit 0.9.0 with `gpui-pre` 0.3.11, Mesa 26.2.6 on Intel RPL-S, WayVNC 0.9.1, grim 1.5, `wtype` 0.4, and `wl-copy`. The user needs access to a working GPU render node and a user-owned `/run/user/$UID`. A user systemd manager is used for process lifetime. Lingering is enabled on the tested account so the manager can survive the last SSH logout; that final-logout case was not directly exercised. Host RPM installation requires administrator authentication here, but the Sway, swaybg, and wtype executables can be unpacked under ignored `target/gui-validation/local-root/`:

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
