# Fedora GPUI development environment specification

Prepare the existing Fedora server for native Rust desktop development with GPUI and GPUI Kit, including a complete GUI verification loop operated by a coding agent over SSH.

The server already has a physical screen and Hyprland installed. The developer may be elsewhere. Build, run, inspect, and control the application on this server using native Wayland. A physical observer and an open remote-desktop viewer must not be required.

This task prepares the environment and a small verification application. It does not implement the Objekte desktop editor.

## 1. Required outcome and scope

Complete these capabilities:

1. Build the chosen GPUI Kit release and its matching GPUI dependencies.
2. Open a real native Wayland application in a selected Hyprland session.
3. Discover and focus its window, enter text, click, scroll, and capture screenshots.
4. Inspect the resulting images through the coding agent's image capability.
5. Repeat this sequence from a fresh SSH session without manually copying environment variables.
6. Leave reproducible setup, verification, cleanup, and operating instructions.

Proceed with ordinary additive dependency installation, local helper creation, and verification. Make routine implementation decisions independently. If administrator authentication is unavailable, finish the unprivileged work and report the exact commands or capability that remain blocked.

Preserve existing Hyprland configuration, working GPU drivers, desktop applications, Rust installations, and unrelated agent configuration. Do not reboot, log the user out, replace the compositor, change display managers, disable screen locking, enable automatic login, change the default system target, disable SELinux, or open firewall ports as part of routine setup.

### 1.1. Initial product interface reference: DBDelve

The user-selected closest comparable application is **DBDelve**, a database client built with Rust and GPUI:

- Project: https://github.com/ShayanAbbas1/dbdelve
- Website: https://dbdelve.github.io/

DBDelve applies a similar browsing, querying, inspection, and editing concept to databases. Its appearance, layout, navigation, and structural functions are the primary reference for how the first Objekte desktop interface should look and operate. Use it as the starting point when making interface decisions.

Before implementing the Objekte desktop interface, inspect DBDelve's current screenshots, demonstrations, and relevant interaction behavior. Adapt its established desktop patterns to Objekte's file-based model. The following mappings describe the intended Objekte direction:

| Reference concept | Objekte adaptation |
| --- | --- |
| Database connection and object navigation | Local repository, collection, and document navigation |
| Workspace organization and open views | Familiar navigation panes and tabs for collections, queries, and documents |
| Query editing and result inspection | Objekte query controls with a structured result view |
| Record and field inspection/editing | JSON document and field inspection/editing |

Follow the reference's overall visual organization, information density, pane relationships, and interaction structure where they fit Objekte. Preserve familiar keyboard navigation, selection, scrolling, and editing behavior. Use GPUI Kit components and Objekte's existing library and query semantics. These mappings do not introduce SQL, database connections, or a database-backed storage requirement.

Responsiveness remains a requirement: opening a collection must not require loading and rendering every document. Use bounded retrieval and virtualized views where needed, and report any material performance limitation imposed by the data model.

This reference guides subsequent product-interface implementation. The current task remains environment preparation and the small verification application described below; it does not require implementing the full layout or DBDelve feature set.

## 2. Locations and repository rules

Use /home/simon/source for persistent source directories. Never put repositories or worktrees in /tmp.

Create a standalone development utility project at:

    /home/simon/source/gpui-development-environment

If it already exists, inspect it and continue without overwriting unrelated work. Initialize version control if this is a newly created standalone project. Keep verification data and generated evidence in ignored directories inside this project.

Suggested structure:

| Path | Purpose |
| --- | --- |
| Cargo.toml and Cargo.lock | Small standalone Rust workspace with pinned resolution |
| rust-toolchain.toml | Reproducible compatible toolchain, if appropriate for the existing manager |
| crates/gpui-environment-validation/ | Verification application |
| scripts/install_fedora_dependencies.sh | Additive, repeatable dependency setup |
| scripts/check_development_environment.sh | Read-only environment checks |
| scripts/run_gui_validation.sh | Build, operate, and verify the application |
| scripts/gui_control.py | Thin GUI-control command adapter, only if existing tools need a wrapper |
| README.md | Commands for subsequent developers and agents |
| reports/environment_report.md | Findings, changes, versions, verification results, limitations |
| artifacts/ | Ignored screenshots, logs, and validation JSON files |

Keep the existing Objekte checkout on master and leave its files and index unchanged. Read its AGENTS.md, Cargo manifests, toolchain configuration, and relevant development instructions without modifying them.

If a repository-specific change becomes necessary, use a separate branch and worktree under /home/simon/source. Do not change the branch of the original checkout. Do not merge into master or push. Prefer completing this task entirely in the standalone utility project.

One agent must own package, service, and compositor changes; avoid simultaneous mutations of host configuration.

## 3. Inspect before changing the system

Record relevant information without dumping the entire process environment or exposing credentials:

- Fedora release and edition; determine whether this is mutable Fedora or an rpm-ostree system.
- CPU architecture, available memory, free disk space, and available build cores.
- Current user, home directory, login sessions, and available administrator access.
- Existing rustc, cargo, rustup, and mise locations and versions; PATH selection; project overrides.
- Objekte's actual rust-version, edition, and toolchain requirements.
- Installed Hyprland version, running instances, session ownership, outputs, scale, and session/seat state.
- GPU devices, existing driver stack, render-node access, Vulkan loader, and usable Vulkan devices.
- Available screenshot, input, and remote-viewing utilities.
- Agent installation and available image-viewing or MCP capabilities.

Useful read-only starting points include:

```bash
cat /etc/os-release
uname -m
git -C /home/simon/source/objekte status --short --branch
hyprctl instances
rustc --version
cargo --version
rustup show
vulkaninfo --summary
```

Discover the repository path instead of assuming the example path exists. Run optional commands only if installed. A missing command is a finding, not a reason to abort unrelated inspection.

Do not mistake the SSH shell's XDG_SESSION_TYPE for the graphical session's backend. An SSH login can report a terminal session while Hyprland is running separately.

If Fedora is immutable, use its established development-container or package-layering workflow. Keep screenshot and input tools able to reach the host compositor. Do not apply a mutable-Fedora installation script blindly or reboot to activate layering without resolving that requirement.

## 4. Establish a compatible Rust and GPUI dependency set

Read the selected release's official installation instructions and manifest before choosing dependencies. Record exact crate versions or Git revisions and the toolchain actually used.

GPUI Kit already brings in a matching GPUI implementation and re-exports its APIs. Start with the normal upstream backend and the documented Kit entry point. Do not combine an arbitrary GPUI Git revision with an unrelated Kit release, enable an alternate rendering backend, or patch GPUI to get basic environment verification working.

The Objekte manifest inspected while preparing this specification declared Rust 1.99 and edition 2024. Re-read the actual checkout; it is authoritative. The selected toolchain must satisfy both Objekte and the chosen Kit dependency graph. If the required toolchain cannot be obtained, report that precise blocker; do not lower the project's requirement silently.

Reuse the existing Rust management arrangement:

- Respect mise if it already manages Rust selection.
- Respect existing rustup toolchains and project overrides.
- Add only a missing compatible toolchain and necessary components.
- Provide rustfmt, Clippy, and rust-analyzer where not already available through the established setup.
- Do not uninstall distribution Rust or alter the user's global default merely to build the verification application.
- Verify the effective compiler in both the project directory and a fresh SSH shell.

Commit Cargo.lock for the standalone application. Use locked builds after initial dependency resolution. Do not update Objekte's lockfile.

## 5. Install Fedora build and runtime prerequisites

Resolve package names against the installed Fedora release and enabled official repositories using DNF package queries and provider lookups. The following is a candidate mapping, not an unconditional installation list.

| Capability | Fedora package candidates |
| --- | --- |
| Native compiler and build utilities | gcc, gcc-c++, clang, make, cmake, ninja-build, pkgconf-pkg-config |
| Bindgen or native compilation support when required | clang-devel, llvm-devel, libclang providers resolved for this Fedora release |
| Source and automation utilities | git, curl, jq, python3 |
| Fonts and shaping dependencies | fontconfig-devel; freetype-devel and related providers when required by the selected dependency graph |
| Wayland development | wayland-devel, wayland-protocols-devel, libxkbcommon-devel |
| X11 libraries still required by enabled upstream build features | libX11-devel, libxcb-devel, libxkbcommon-x11-devel |
| TLS and compression | openssl-devel, libzstd-devel |
| Vulkan loader, diagnostics, and development headers | vulkan-loader, vulkan-tools, vulkan-headers, vulkan-loader-devel |
| Optional graphics diagnostics | vulkan-validation-layers |
| Mesa Vulkan driver where appropriate | mesa-vulkan-drivers |
| GPUI Kit Linux dependency where required | webkit2gtk4.1-devel |
| Optional speech feature | alsa-lib-devel |
| Screenshot and text-input tools | grim, wtype, wl-clipboard |
| Optional remote viewing and alternative input | wayvnc, ydotool; resolve any further selected utility through Fedora packages |

Install only missing prerequisites justified by the selected build or control backend. Do not install every dependency needed to build the complete Zed editor. Do not enable all Kit features just to exercise prerequisites.

The Kit Linux installation instructions include WebKitGTK development files. Check the selected release and feature graph before omitting them. Their presence as a build dependency does not change the requirement for a native GPUI application.

For NVIDIA or another already functioning driver stack, preserve that stack. Do not replace it with Mesa because Mesa appears in the candidate list. A Vulkan loader alone is not a usable renderer.

The installation script must:

- Detect Fedora and fail clearly on an incompatible host.
- Separate package discovery from installation.
- Require privilege only for host operations that need it.
- Be safe to run again without duplicating configuration.
- Record installed package names and versions.
- Avoid a full distribution upgrade, third-party repositories, driver replacement, and removal of unrelated packages.

Do not execute an upstream bootstrap script as root without reviewing its contents and applicability.

## 6. Resolve the Hyprland session from SSH

Implement session discovery as a reusable helper, not a copied environment dump.

For the selected session, resolve and validate:

- The Unix user and session ownership.
- XDG_RUNTIME_DIR and its ownership.
- WAYLAND_DISPLAY and its live socket.
- HYPRLAND_INSTANCE_SIGNATURE and the corresponding live IPC socket.
- The current session bus when required by application integration.
- The selected output, coordinate space, and scale.

Use Hyprland instance metadata, the existing session manager, or narrowly selected process environment fields where needed. Do not assume wayland-0, a fixed instance signature, or a particular PID. Do not source raw process environment output as shell code.

Prefer the existing same-user session. If multiple valid instances exist, allow an explicit instance parameter and remember the selected identity for the operation. Never choose an arbitrary session belonging to another user.

Use the installed version's supported hyprctl syntax. Current online documentation and an older installed Hyprland may use different configuration or dispatcher syntax.

If an existing session is unlocked and usable, attach to it without restarting it. If no session is running, inspect the existing launch arrangement and prepare a supported separate testing-session launch method. A normal SSH shell may lack the active seat or device access needed to launch a hardware-backed compositor. Do not assume that running Hyprland in the background solves this.

If starting a usable session needs an interactive login, seat activation, or unavailable privileges, identify the exact remaining action. Complete package installation, application compilation, and helper preparation meanwhile.

Do not bypass an existing session lock. Do not disable its idle or locking policy. A headless output inside a locked session is not a separate unlocked session.

## 7. Establish rendering and GUI control

First prove that Vulkan sees an appropriate adapter. Then run the validation application on native Wayland using the verified graphical environment. Remove DISPLAY only for that application's launch if needed to prevent an X11 fallback; do not change global environment files.

Validate native Wayland through compositor window metadata, such as the application's xwayland flag where available, and/or explicit backend diagnostics. An environment variable by itself is insufficient evidence.

Prefer the existing hardware renderer. Software rendering may be configured as a documented diagnostic fallback, but report it explicitly. Do not present software-rendered timing as hardware GPU performance.

For a stable virtual output, Hyprland documents:

```bash
hyprctl output create headless GUI-TEST
```

Use it only when needed and supported by the installed version. Configure a known output size and scale through version-appropriate commands, preserve the physical outputs, and place only the validation window there. A new output shares session focus and input; it does not isolate concurrent users. Remove only outputs created by this setup when cleaning up.

Select one working automation path:

1. Prefer compositor/Wayland control plus grim and a supported virtual keyboard/pointer utility, if these cover the required operations.
2. Alternatively, use WayVNC with an actual RFB-capable controller for screenshots and input. WayVNC's administrative control socket is not itself a general mouse/keyboard automation client.
3. Use ydotool as a fallback when needed. Give its daemon only the required device access and restrict its control socket. Do not run the coding agent as root, make /dev/uinput world-writable, or grant broad input-device access merely for convenience.

Provide these operations through existing tools or a thin wrapper:

| Operation | Required behavior |
| --- | --- |
| status | Report selected session, output, application PID/window, and control capabilities |
| launch | Start the known verification executable with explicit environment and logs |
| focus | Activate only the verification window |
| screenshot | Capture the current application region or dedicated output and return an absolute PNG path |
| move and click | Use documented coordinates and support real press/release events |
| type and key | Enter text and shortcuts into the verified focused application |
| scroll | Deliver actual wheel/scroll input and verify a visible response |
| close | Stop only the tracked verification application |
| cleanup | Remove only processes, outputs, and files owned by this validation run |

Account for output offsets and fractional scaling. Re-read geometry after moving or resizing the window. Before input, verify focus still belongs to the intended application; stop rather than type into an unrelated terminal.

Use bounded readiness waits with meaningful failure messages. Do not rely on long fixed sleeps. Never claim an application screenshot passed when the image is black, stale, empty, or shows the wrong window.

Keep wrappers small: call established utilities with argument arrays, validate arguments, and avoid interpolating arbitrary text into shell commands. Do not build a new desktop automation framework.

Do not make gpui-agent or patched GPUI a prerequisite. Its documented platform and revision support must be verified separately if evaluated later.

## 8. Build the verification application and prove the agent loop

Create one small application named gpui-environment-validation. It should exercise both the GPUI platform and actual GPUI Kit components, with:

- A recognizable title and normal text rendering.
- An editable text input.
- A checkbox.
- A button that visibly changes a counter or status.
- A short scrollable area.
- A Save action that writes a tiny JSON fixture only inside artifacts/.

Use the selected release's documented initialization, component state, fonts, assets, and root-view conventions. Do not invent APIs from other GPUI versions. Do not add an Objekte product interface, repository loading, schema-driven editing, authentication, or packaging.

Run this acceptance sequence through the chosen GUI tools:

1. Build with Cargo and launch the application in the selected Wayland session.
2. Verify the window identity and native Wayland backend.
3. Capture and visually inspect the initial image.
4. Focus the input and type the exact text: Objekte ÄÖÜ äöü ß 123.
5. Click the checkbox and action button; verify their visible states change.
6. Scroll the list and verify different content becomes visible.
7. Activate Save through real GUI input.
8. Verify that the JSON fixture contains the typed value, checkbox state, and expected action result.
9. Capture and inspect the final image.
10. Close the application and repeat launch/control from a fresh SSH session.

Checking only logs, file existence, or model state does not replace inspecting the rendered UI. Likewise, an attractive screenshot does not prove saving worked.

Show the PNG to the coding agent through its available image tool or an MCP tool returning image content. If the agent cannot view images, report visual inspection as blocked; do not claim it performed a visual check.

Add only the small tests needed for this verification program. If the selected Kit release offers supported headless interaction or rendering tests, demonstrate one appropriate test without making those tests a substitute for the native Wayland loop.

## 9. Optional remote viewing

If WayVNC is available, prepare a documented start/stop command for the selected output and bind it to loopback. Provide an SSH-tunnel example for the developer's machine using the actual host and chosen port once known.

Keep the viewer optional. GUI automation must work with no human VNC client connected.

Do not expose VNC on public interfaces, create broad firewall rules, or enable a persistent desktop-control service without a demonstrated need. Preserve any existing WayVNC configuration. If remote viewing is untested, label that capability separately; it need not block the core SSH-operated validation.

## 10. Build efficiency and reproducibility

Use a persistent local Cargo target directory and preserve incremental compilation. Reuse downloaded dependencies between runs. Do not run cargo clean routinely or move the repository onto a network filesystem.

Record the first observed build and a subsequent incremental build with their conditions. Do not call a build cold if its dependency cache was already populated.

Do not force release builds for every UI edit. Leave global compiler flags, target-cpu, linker selection, sccache, and optimization settings alone unless a measured problem requires a project-local adjustment.

Pin enough information to reproduce the working configuration without freezing the whole operating system: toolchain, Cargo.lock, selected features, installed packages, application revision, and chosen control backend.

## 11. Deliverables and completion criteria

Finish with the utility project, working scripts, README, verification application, and environment report. Keep generated evidence out of commits; include source and configuration templates where appropriate. If version control was initialized for this new project, commit the completed work locally. Do not push or merge.

The report must contain:

- Fedora, Rust, GPUI Kit, GPUI, Hyprland, and graphics-driver versions.
- The selected Vulkan adapter and whether rendering used hardware or software.
- Every installed package and every host configuration/service change.
- The graphical-session selection method and its behavior after a new SSH login.
- Exact commands for check, build, launch, screenshot, input, close, and cleanup.
- Paths to representative initial/final screenshots and the saved JSON fixture.
- A command/result table with PASS, FAIL, BLOCKED, or NOT APPLICABLE for each capability.
- Build measurements with cache conditions.
- Remaining limitations and a precise rollback procedure for task-owned changes.
- Confirmation that Objekte's original checkout remains on master with its original worktree changes preserved.

Do not automatically uninstall shared packages during rollback. Record which packages were newly installed, and remove only task-owned services, outputs, configuration fragments, and helper files when appropriate.

Core completion requires all of the following:

| Check | Acceptance condition |
| --- | --- |
| Compilation | The selected GPUI Kit application builds with the chosen compatible toolchain |
| Native display | A real window runs through Wayland in Hyprland |
| Rendering | Text, controls, and assets are visible; the renderer is identified |
| Pointer control | The agent successfully clicks a real control |
| Keyboard control | The exact Unicode test string is entered correctly |
| Scrolling | Actual scroll input produces a visible change |
| Persistence | GUI Save writes the expected JSON fixture |
| Image inspection | The agent views and assesses actual captured images |
| SSH reuse | A fresh SSH session can rediscover and operate the application |
| Cleanup | Only task-owned application resources are stopped or removed |
| Preservation | Existing desktop configuration and Objekte master remain intact |

Installation alone is not completion. Report partial readiness accurately if the graphical session or privileges prevent the full loop.

## Reference material

Resolve instructions against the chosen versions at execution time. These sources inform setup; the local machine and pinned dependency graph determine the exact commands.

- GPUI Kit installation: https://gpui-kit.com/docs/installation/
- DBDelve, the initial Objekte interface reference: https://github.com/ShayanAbbas1/dbdelve
- DBDelve website and demonstrations: https://dbdelve.github.io/
- GPUI Kit application initialization: https://gpui-kit.com/docs/getting-started/
- GPUI Kit testing: https://gpui-kit.com/docs/test/
- GPUI Kit Linux dependency script: https://github.com/longbridge/gpui-kit/blob/main/script/install-linux.sh
- Zed Linux dependency mapping, for reference rather than wholesale installation: https://github.com/zed-industries/zed/blob/main/script/linux
- Zed Vulkan diagnostics: https://zed.dev/docs/linux
- Fedora package catalogue: https://packages.fedoraproject.org/
- Hyprland control and virtual outputs: https://wiki.hypr.land/configuring/core/advanced-configuration/using-hyprctl/
- Hyprland IPC: https://wiki.hypr.land/ipc/
- Hyprland remote display guidance: https://wiki.hypr.land/configuring/extra/virtual-gpu/
- Hyprland screenshot tools: https://wiki.hypr.land/useful-utilities/screenshots-and-recording/
- WayVNC: https://github.com/any1/wayvnc
- wtype: https://github.com/atx/wtype
- ydotool: https://github.com/ReimuNotMoe/ydotool

