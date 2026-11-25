# Objekte headless Wayland development follow up plan

Date: 8 November 2026

Replace the Hyprland-dependent GPUI development environment with a dedicated, hardware-accelerated, headless Sway session operated over SSH. Prioritize a working application, reliable input, and fast screenshots before optional remote viewing or additional infrastructure.

This is an implementation task for the agent working on the Fedora development computer. It supersedes the Hyprland session discovery, physical-display integration, compositor-specific control, and Hyprland acceptance requirements in the earlier Fedora GPUI environment specification. Reuse completed dependency installation, Rust builds, the verification application, and useful scripts. The native Wayland requirement, GPUI Kit dependency compatibility, Unicode input, visual inspection, and saved-fixture verification remain in force. The DBDelve interface reference remains relevant to later product work; this task does not implement the Objekte editor.

“Remove Hyprland” means remove it from this development workflow. Do not uninstall or terminate the user's desktop.

## 1. Selected architecture

| Function | Implementation |
| --- | --- |
| Display server | One separate Sway process using only the headless backend |
| Virtual display | One output, initially 1600 × 1000, scale 1, nominal 60 Hz where supported |
| Compositor rendering | Hardware acceleration using Sway's working default renderer; select another supported renderer only for a diagnosed problem |
| Application rendering | GPUI's normal Linux Vulkan path, with the actual adapter verified |
| Window discovery and focus | Sway IPC through the recorded socket |
| Screenshots | Direct, on-demand PNG capture with grim |
| Keyboard | wtype for Unicode text, keys, and shortcuts |
| Pointer | wlrctl virtual-pointer movement, clicks, and scrolling |
| Human viewing | Optional WayVNC, started only when requested |
| Operation | Small scripts with explicit session state and bounded readiness checks |

Headless Sway over SSH is a documented deployment path in the WayVNC FAQ. A headless output does not require software rendering. Verify GPU access separately; neither the existence of a Wayland socket nor a successful screenshot establishes acceleration. [WayVNC FAQ](https://github.com/any1/wayvnc/blob/master/FAQ.md)

Sway and GPUI need not use the same graphics API. A hardware GLES compositor with a Vulkan GPUI client is an acceptable configuration. Do not force Sway's Vulkan renderer merely because the application uses Vulkan.

The intended speed advantage comes from a smaller operating workflow: one stable session, direct input, and screenshots on demand. There is no assumed benchmark result that Sway renders faster than Hyprland.

## 2. Repository and agent organization

Continue the existing utility project at `/home/simon/source/gpui-development-environment`, if present. Inspect its current state and applicable `AGENTS.md` and `docs/ai/` instructions first. Keep the original Objekte checkout on master and preserve all existing worktree and index changes.

Use one integration branch, `integration/headless-wayland-environment`, in a separate worktree under `/home/simon/source`. A suitable worktree name is `/home/simon/source/gpui-development-environment-integration-headless-wayland`. Do not change the branch of an existing checkout, create worktrees under `/tmp`, merge into master, or push. If no utility repository exists, create the standalone project and establish its initial baseline before creating the integration worktree.

One integrator owns all package, permission, service, compositor, and live GUI changes. If subagents are available, delegate verification-application adjustments and documentation review in separate worktrees after the script interface is agreed. Subagents must not operate the shared desktop or modify host configuration concurrently. Merge their changes into the integration branch and leave that single branch for the user to merge. Keep the first graphical proof sequential; agent coordination must not delay it.

Reuse existing files where practical. The intended deliverables are:

- A minimal `config/sway-headless.conf`.
- A session script providing start, status, command execution, and stop.
- The existing GUI control and validation scripts, adapted to Sway.
- A small optional remote-viewing start/stop command.
- Updated dependency checks and operating instructions.
- `reports/headless_wayland_environment_report.md`, with evidence and measurements.

Keep generated screenshots, raw measurements, runtime manifests, and logs in ignored locations. Follow the repository's task-completion and report-placement rules where they already apply.

## 3. Phase 1 — Prove the graphics path immediately

Do this before constructing a complete automation wrapper.

1. Inspect the installed Fedora release, existing utility project, Rust toolchain, Sway/wlroots versions, GPU drivers, render nodes, and previous setup results. Reuse completed work; do not repeat large installations or rebuild compatible dependencies unnecessarily.
2. Resolve missing packages against the enabled Fedora repositories. Candidate runtime tools are `sway`, `grim`, `wtype`, `wlrctl`, `jq`, and the existing Vulkan diagnostics. Add `wayvnc` only when remote viewing or its input fallback is actually needed. Fedora publishes wlrctl; verify availability for the installed release. [Fedora package](https://packages.fedoraproject.org/pkgs/wlrctl/wlrctl/)
3. Check Vulkan device enumeration as the actual SSH user. Identify the GPU render node and inspect its access permissions. Prefer the established render-device permission mechanism; never make `/dev/dri` or input devices world-writable, replace the graphics stack, or run the compositor as root.
4. Launch a temporary standalone Sway instance with an explicit minimal configuration and the headless backend. The upstream starting point uses `WLR_BACKENDS=headless` and `WLR_LIBINPUT_NO_DEVICES=1`; confirm variables against the installed wlroots version. Do not include the DRM, libinput, nested Wayland, or X11 backends. [Documented starting point](https://github.com/any1/wayvnc/blob/master/FAQ.md)
5. Configure one virtual output at the selected resolution and scale. Obtain its actual name through IPC. Keep Xwayland disabled in this test configuration.
6. Launch the existing GPUI verification application, or the smallest existing compatible GPUI Kit example if the verification application has not been built. If it fails, use a native Wayland Vulkan sample to distinguish a presentation/driver failure from an application failure. Vulkan enumeration alone is insufficient. [Zed Linux graphics diagnostics](https://zed.dev/docs/linux)
7. Capture the real output with grim and inspect the image. Verify a mapped native Wayland window and readable text.
8. Confirm the compositor's renderer and GPUI's selected adapter from their respective diagnostics. Check for software implementations such as llvmpipe or lavapipe.

**Phase exit:** a visible native GPUI window in a standalone headless Sway session, a visually inspected screenshot, and identified rendering devices. Do not call acceleration verified if the evidence is unavailable.

Software rendering is allowed as a clearly labelled diagnostic step. It does not meet the requested fast environment outcome. If hardware rendering is blocked, identify the precise driver, device-access, or presentation error. Do not spend the rest of the task polishing a slow fallback or return to Hyprland.

Suggested commit: `Establish hardware accelerated headless Sway validation`.

## 4. Phase 2 — Make the session independent and reusable

Convert the working launch into a small repeatable interface. Do not add a general desktop-management framework.

### Session configuration

Use the utility project's explicit Sway configuration. Do not import the user's desktop configuration or distribution desktop startup fragments: those can start unrelated services or export a test environment into the user's normal session. The test session needs no panel, wallpaper process, notification daemon, physical input devices, or desktop idle/lock helpers.

Use a validated, user-owned `XDG_RUNTIME_DIR`. Store the task's manifest and IPC state in a private subdirectory. Clear inherited `DISPLAY`, `WAYLAND_DISPLAY`, `WAYLAND_SOCKET`, `SWAYSOCK`, and `HYPRLAND_INSTANCE_SIGNATURE` for the compositor child before launching it; do not change the SSH shell or global profile.

Use a private session bus through `dbus-run-session` for the test process tree where required. Preserve its address for later application launches. Never use `systemctl --user import-environment` or global D-Bus activation-environment updates to publish test-session variables.

Capture the actual `WAYLAND_DISPLAY` and `SWAYSOCK` from a small startup helper executed by this Sway instance. Atomically write only the required fields to a private JSON manifest. Validate the compositor PID, ownership, live sockets, and output before reporting readiness. Do not guess `wayland-0`, select the newest socket, or source arbitrary process-environment text as shell code.

### Lifecycle

Keep the compositor alive across application rebuilds and repeated test commands. Starting an already healthy task-owned session should return its identity instead of creating another instance. Use a simple lock to prevent duplicate starts.

Use a dedicated systemd user service, or the host's equivalent established user-service mechanism, for reliable process lifetime across SSH disconnections. Keep it independent of the physical desktop's graphical-session target and supply its environment explicitly. Start it on demand; boot-time automatic startup is outside scope.

Check the user's existing lingering and logout behavior. If user-manager termination prevents survival after the final SSH session closes, configure lingering only where permitted by the host's administration policy and record the change. Otherwise report the exact remaining administrative requirement. Do not claim persistence based only on closing one of several active SSH sessions.

Stop and cleanup must affect only tracked task-owned processes and resources. Validate process identity before signalling a saved PID. Do not use broad commands such as `pkill sway`, remove the user's runtime directory, or restart the normal desktop.

**Phase exit:** a fresh SSH connection can start, inspect, reuse, and stop the session without copying environment variables. Verify disconnection and reconnection behavior explicitly.

Suggested commit: `Add independent headless session lifecycle management`.

## 5. Phase 3 — Complete direct GUI control

Use established utilities and thin argument-array wrappers. Keep all commands bound to the recorded test-session sockets.

| Operation | Required implementation behavior |
| --- | --- |
| Launch | Start the selected executable with the recorded environment, logs, and tracked process identity |
| Find and focus | Match the application by a stable app ID and tracked process/window identity through Sway IPC |
| Screenshot | Capture the selected output or verified application rectangle to an absolute PNG path |
| Type and keys | Use wtype; confirm the intended window is focused before sending input |
| Move and click | Use wlrctl; expose a clearly documented coordinate system |
| Scroll | Send actual pointer-axis input and verify visible content movement |
| Close | Close only the tracked application and confirm its exit |

wtype supports Unicode input and explicit key/modifier actions. wlrctl supports pointer clicks, relative motion, and scrolling. Its documented `pointer move` arguments are displacements, not absolute coordinates; its scroll argument order is vertical then horizontal. [wtype](https://github.com/atx/wtype), [upstream wlrctl manual distributed by Debian](https://sources.debian.org/src/wlrctl/0.2.2-2/wlrctl.1.scd)

For deterministic absolute clicks, establish a known pointer origin on the single bounded output, then apply the required displacement. Verify this behavior on the installed version, including pointer acceleration and repeated tool invocations. Do not treat a remembered coordinate as reliable after another controller moves the pointer. Recalculate window geometry after resize or movement. A fresh origin check is inexpensive for occasional automated clicks.

If direct pointer positioning cannot be made reliable promptly, use one established RFB client with WayVNC for pointer control. Keep Unicode input through wtype and screenshots through grim if those already work. WayVNC's administrative control socket is not a pointer-injection API. Do not write an RFB implementation or install a privileged global-input daemon to solve this fallback. [WayVNC control interface](https://github.com/any1/wayvnc/blob/master/wayvnc.scd)

Run the existing verification application through this complete sequence:

1. Open and visually inspect the initial window.
2. Type `Objekte ÄÖÜ äöü ß 123` into the actual text field.
3. Click the checkbox and counter/status button.
4. Scroll the list and verify that different items are visible.
5. Invoke Save through GUI input.
6. Read the generated JSON fixture and verify all expected values.
7. Inspect the final screenshot.
8. Close, relaunch, and repeat from a fresh SSH connection.

Use bounded readiness checks, compositor events where practical, and short condition polling where simpler. A compositor IPC response does not prove that the next client frame has been drawn. Wait for the visible change or an appropriate application readiness condition before capturing evidence; do not replace this with multi-second sleeps.

**Phase exit:** the full loop works without a human viewer, a physical screen, an unlocked physical desktop, or Hyprland. Inspect actual images; logs and file existence alone are insufficient.

Suggested commit: `Implement direct Wayland input and visual verification`.

## 6. Phase 4 — Measure the operating loop and add optional viewing

First measure with WayVNC stopped. Reuse the running compositor and prebuilt application. Keep Cargo caches and incremental compilation; never run `cargo clean` as routine validation.

The following are initial engineering targets, not observed results or universal hardware guarantees:

| Measurement | Initial target | Conditions |
| --- | --- | --- |
| Session start to ready | Median at most 3 seconds | Installed dependencies; no compilation |
| Prebuilt application launch to usable window | Median at most 2 seconds | Existing compositor; distinguish mapped from visibly ready |
| Direct screenshot to completed local PNG | Median at most 250 ms | Fixed 1600 × 1000 output; include encoding and file completion |
| Click to captured visible response | Median at most 500 ms | Includes input, condition wait, and screenshot; excludes model reasoning |
| Idle behavior | No sustained busy loop | No viewer, no animation, and no repeated screenshots |

Record five session starts and ten application/input/capture operations where applicable. Report the median, maximum, raw samples, resolution, renderer, build mode, and machine load. Measure SSH command and image-transfer overhead separately from host-local operations. Report incremental build time separately from application launch time.

If a target is missed, identify whether the cost lies in compilation, process startup, rendering, waiting, PNG compression, transfer, or remote viewing. Prefer one measured correction. Do not introduce a new rendering backend, custom screenshot pipeline, or extensive benchmark framework without evidence that the existing path is the problem. An isolated target miss needs explanation, not an open-ended optimization project.

For optional human viewing, start WayVNC against the same recorded session and output. Bind to loopback and document SSH tunnelling. On a multi-user host, remember that loopback alone is not per-user access control: use supported authentication or a private Unix socket where practical. Keep credentials out of version control.

Use the installed WayVNC command syntax. Its frame-rate limit and GPU-related options are distinct from GPUI rendering; enabling `--gpu` is not evidence of accelerated application rendering or hardware video encoding. Avoid automatic viewer resize changing automation coordinates. Measure with a viewer separately if viewing performance matters. [WayVNC manual](https://github.com/any1/wayvnc/blob/master/wayvnc.scd)

Suggested commit: `Document headless development performance and remote viewing`.

## 7. Completion and handoff

Deliver one integration branch with:

- Exact installation, start, status, launch, screenshot, input, validation, stop, and optional viewing commands.
- Fedora, Sway, wlroots, GPUI, GPUI Kit, toolchain, driver, and control-tool versions actually used.
- The compositor renderer and application Vulkan adapter, with supporting diagnostics.
- A capability table marked PASS, FAIL, BLOCKED, or NOT APPLICABLE.
- Initial/final screenshots, the saved JSON fixture, and confirmation that the agent inspected the images.
- Results for fresh-SSH startup, SSH reconnection, application restart, and task-owned cleanup.
- Performance measurements and any unexplained delays or software rendering.
- Host configuration changes and a precise rollback for task-owned service/configuration changes.
- Confirmation that the original Objekte checkout remains on master with its existing changes preserved.

Remove Hyprland dependencies, discovery code, and commands from the utility's supported workflow. Preserve unrelated desktop files, existing drivers, shared packages, and useful completed development work. Do not implement multiple compositor backends, add containers, require a remote viewer, or broaden the task into the Objekte application.

A successful result is an accelerated native Wayland application that an agent can build, launch, inspect, operate, and restart quickly through SSH. A compositor process by itself is not completion.

