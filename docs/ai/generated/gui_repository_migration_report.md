# GUI repository migration report

Date: 8 November 2026. Task: `gui-repository-migration`. Objekte base: `a5cf0298d99bf15701cf632e6d29a6ed59f9f5b5`. Integration branch: `integration/gui-repository-migration` in `/home/simon/source/objekte-integration-gui-repository-migration`.

**Result: the established native GPUI validation loop now runs from Objekte.** A fresh SSH login rebuilt and launched the Objekte example, found the headless Sway session, entered Unicode text, clicked, scrolled, saved, and produced the expected screenshots and JSON. The original Objekte `master` checkout and the separate environment repository were left in place. This branch was committed locally and intentionally not pushed, as the task directs.

## Source and inventory

The working source was the separate repository's `integration/headless-wayland-environment` worktree at `/home/simon/source/gpui-development-environment-integration-headless-wayland`, based on `302c2bea19c6422616d4f5feb06e1d5616f18ac8`. Its working GUI program and scripts had uncommitted modifications and additions; they were copied from the worktree rather than reconstructed from its commit. Its root README still described an obsolete Hyprland path; the working Sway report, configuration, and scripts supplied the operating procedure. That repository's `.git`, root manifest and lockfile, build outputs, credentials, and caches were not copied. Its modified and untracked files remain intact as a recovery source.

The separate repository contained an environment verification program, not an Objekte product desktop interface. The migration therefore adds an example-only `objekte-desktop` package instead of claiming a product app exists.

| Objekte path | Migrated purpose |
| --- | --- |
| `crates/desktop/examples/environment_validation.rs` | GPUI Kit window and in-process interaction test; GUI Save writes under the selected checkout's ignored `target/gui-validation/` |
| `scripts/gui/headless_session.py`, `session_ready.py`, `config/sway-headless.conf` | Session lifecycle, actual socket discovery, fixed output checks, and transient Sway service |
| `scripts/gui/gui_control.py`, `run_gui_validation.sh`, `requirements.txt` | Native app, WayVNC pointer, text, screenshot, exercise, and fixture checks |
| `scripts/gui/check_development_environment.sh`, `install_fedora_dependencies.sh`, `install_user_dependencies.sh` | Environment diagnosis and reproducible user-local tool setup |
| `docs/development/gui_environment.md` | Maintained setup, daily loop, architecture, diagnosed failures, and performance evidence; linked from `docs/ai/README.md` |

The root Cargo workspace now includes `crates/desktop` but retains CLI, core, and TUI as `default-members`. GUI dependencies stay in `objekte-desktop`; the example does not use Objekte core, so no core dependency was added. The integrated lockfile resolves GPUI Kit `0.9.0` and `gpui-pre` `0.3.11`. It adds 721 packages, removes none, and changes no original package version. Thirty-four existing lock entries have updated dependency lists, mostly version disambiguation and newly unified features. No nested workspace or separate lockfile was introduced.

## Services and display

Before migration, the task-owned Sway unit referenced the separate worktree's `scripts/session_ready.py`, local Sway tools, and `config/sway-headless.conf`; the app unit referenced its `target/debug/gpui-environment-validation`. The prior `systemctl --user show` output is retained in ignored `target/gui-validation/previous-services.txt`. `headless_session.py update` stopped and restarted only the task-owned Sway, app, and WayVNC units with paths derived from the Objekte integration checkout. The live app executable was `target/debug/examples/environment_validation` under that checkout. No active script, service command, or selected local executable references the old repository.

The live Sway output `HEADLESS-1` was **1920 × 1080, scale 1 (100%), 60 Hz**. Sway configuration requests this mode; both startup and subsequent session checks reject a different live mode. WayVNC binds `127.0.0.1:5907` with `-R` to disable viewer-initiated resizing. The earlier environment check observed unchanged mode after an actual RFB connection; an explicit resize request from every viewer was not tested. Click coordinates and screenshots use the fixed physical output. Sway logs from the established setup selected `/dev/dri/renderD128` and Mesa Intel RPL-S hardware GLES. `vulkaninfo` enumerated the Intel GPU; the GPUI app's chosen Vulkan adapter was not independently logged.

On this host, the relevant versions were Fedora 44, Rust 1.99.1, Sway 1.11, Mesa 26.2.6, WayVNC 0.9.1, grim 1.5, GPUI Kit 0.9.0, and `gpui-pre` 0.3.11. No host packages, graphics drivers, or persistent host settings were changed during migration. Existing lingering remained enabled. Working Sway, swaybg, and wtype executables were copied into the new checkout's ignored `target/gui-validation/local-root/` for this live migration; a future checkout must obtain them through `scripts/gui/install_user_dependencies.sh` if they are not installed system-wide. The migrated installer was syntax checked but was not rerun while the live Sway executable was in use.

## Validation

- `cargo build --locked -p objekte-desktop --example environment_validation` passed. The initial build took 48.28 seconds; a warm locked build during `prepare` took 0.16 seconds.
- `cargo test --locked -p objekte-desktop --example environment_validation` passed its GPUI Kit interaction test. `cargo test --locked --workspace` passed after two CLI/TUI tests were made independent of JSON field order. GPUI enables `serde_json/preserve_order` in a workspace-wide build; those tests had assumed the selected row could never be row zero. The tests now assert that the selected row contains `ordinary title`, preserving the intended check.
- `cargo fmt --package objekte-desktop -- --check`, Bash syntax checks for `scripts/gui/*.sh`, and Python byte compilation for `scripts/gui/*.py` passed.
- `prepare` launched a native `xdg_shell` window with the expected app ID and Objekte executable. An agent inspected `initial.png`, `after-type.png`, `after-controls.png`, `after-scroll.png`, and `final.png` in ignored `target/gui-validation/`. The visible text was `Objekte ÄÖÜ äöü ß 123`; the checkbox was checked, the counter reached 1, and scrolling advanced the list from row 01 to about row 14. GUI Save produced `fixture.json` with exactly `{"text":"Objekte ÄÖÜ äöü ß 123","checked":true,"action_count":1}`.
- A new authenticated localhost SSH login followed the guide's `prepare` and `exercise` commands without copied display variables. Both passed at 1920 × 1080; the new app process and task-owned service paths pointed to the Objekte integration checkout.
- Three migrated 1920 × 1080 screenshots took 111.88, 111.17, and 106.26 ms (median 111.17 ms). The prior ten-sample screenshot median was 105 ms at **1600 × 1000**, so this is only a brief operational comparison, not a same-resolution regression claim. The prior 1600 × 1000 medians were 246 ms session startup, 702 ms launch to visible screenshot, and 413 ms click to captured counter change; conditions and caveats are in the maintained guide and the source report.

## Handoff and limits

After merging into `/home/simon/source/objekte`, run the user-local installer there if Sway, swaybg, or wtype are missing, then run `python3 scripts/gui/headless_session.py update` **from that checkout** before deleting the integration worktree. Check `python3 scripts/gui/headless_session.py status`, then run `bash scripts/gui/run_gui_validation.sh prepare` and `exercise` as documented in [the operating guide](../../development/gui_environment.md). The service update is necessary because transient user units contain absolute paths to the checkout that installed them.

The workflow validates an example, not a product desktop application. Final logout survival is inferred from enabled lingering but was not directly tested; an explicit viewer resize request was not sent during migration; GPUI's selected Vulkan adapter was not separately captured. The old repository remains available for recovery. No push or master merge was performed.
