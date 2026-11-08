#!/usr/bin/env python3
"""Start, inspect, use, and stop the task-owned headless Sway session."""

import argparse
import fcntl
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
RUNTIME = Path(f"/run/user/{os.getuid()}")
STATE_DIR = RUNTIME / "gpui-headless-sway"
MANIFEST = STATE_DIR / "manifest.json"
UNIT = "gpui-headless-sway.service"


class Problem(Exception):
    pass


def run(argv, *, env=None, timeout=10):
    result = subprocess.run(argv, capture_output=True, text=True, env=env, timeout=timeout)
    if result.returncode:
        raise Problem(f"{argv[0]}: {result.stderr.strip() or result.stdout.strip()}")
    return result.stdout


def tool(name):
    installed = shutil.which(name)
    local = ROOT / "target/gui-validation/local-root/usr/bin" / name
    if installed:
        return installed
    if local.is_file() and os.access(local, os.X_OK):
        return str(local)
    raise Problem(f"{name} missing; run scripts/gui/install_user_dependencies.sh")


def private_state_dir():
    if not RUNTIME.is_dir() or RUNTIME.stat().st_uid != os.getuid():
        raise Problem(f"Missing same-user runtime directory: {RUNTIME}")
    STATE_DIR.mkdir(mode=0o700, exist_ok=True)
    mode = stat.S_IMODE(STATE_DIR.stat().st_mode)
    if STATE_DIR.stat().st_uid != os.getuid() or mode & 0o077:
        raise Problem(f"State directory must be user-owned and private: {STATE_DIR}")


def unit_active():
    result = subprocess.run(["systemctl", "--user", "is-active", UNIT], capture_output=True, text=True)
    return result.stdout.strip() == "active"


def read_manifest():
    if not MANIFEST.is_file():
        raise Problem("Session manifest absent; run start")
    return json.loads(MANIFEST.read_text())


def session_env(manifest):
    env = os.environ.copy()
    for name in ("DISPLAY", "WAYLAND_SOCKET", "HYPRLAND_INSTANCE_SIGNATURE"):
        env.pop(name, None)
    env.update(
        XDG_RUNTIME_DIR=str(RUNTIME),
        WAYLAND_DISPLAY=manifest["wayland_display"],
        SWAYSOCK=manifest["sway_socket"],
        DBUS_SESSION_BUS_ADDRESS=manifest["dbus_address"],
    )
    env["PATH"] = f"{ROOT / 'target/gui-validation/local-root/usr/bin'}:/usr/bin:/bin"
    return env


def validate():
    private_state_dir()
    if not unit_active():
        raise Problem(f"{UNIT} is inactive; run start")
    manifest = read_manifest()
    pid = int(manifest["sway_pid"])
    process = Path(f"/proc/{pid}")
    if not process.is_dir() or process.stat().st_uid != os.getuid():
        raise Problem("Recorded Sway process is absent or owned by another user")
    if process.joinpath("exe").resolve() != Path(tool("sway")).resolve():
        raise Problem("Recorded process is not the selected Sway executable")
    if UNIT not in process.joinpath("cgroup").read_text():
        raise Problem("Recorded Sway process is outside the task-owned user service")
    for key in ("sway_socket", "wayland_socket"):
        path = Path(manifest[key])
        if not path.is_socket() or path.stat().st_uid != os.getuid() or path.parent != RUNTIME:
            raise Problem(f"Recorded {key} is absent, foreign, or outside the runtime directory")
    env = session_env(manifest)
    outputs = json.loads(run([tool("swaymsg"), "-s", manifest["sway_socket"], "-t", "get_outputs", "-r"], env=env))
    active = [item for item in outputs if item.get("active")]
    if len(active) != 1 or active[0]["name"] != manifest["output"]:
        raise Problem("Recorded headless output is absent or ambiguous")
    output = active[0]
    if not output["name"].startswith("HEADLESS-"):
        raise Problem("Selected output is not headless")
    mode = output["current_mode"]
    if (mode["width"], mode["height"], mode["refresh"], output["scale"]) != (1920, 1080, 60000, 1.0):
        raise Problem(f"Required 1920x1080@60Hz scale 1 unavailable: {mode}, scale={output['scale']}")
    return manifest, output, env


def status():
    manifest, output, _ = validate()
    linger = run(["loginctl", "show-user", str(os.getuid()), "-p", "Linger"]).strip()
    print(json.dumps({
        "service": UNIT,
        "sway_pid": manifest["sway_pid"],
        "sway_socket": manifest["sway_socket"],
        "wayland_display": manifest["wayland_display"],
        "output": output["name"],
        "size": [output["current_mode"]["width"], output["current_mode"]["height"]],
        "scale": output["scale"],
        "refresh_hz": output["current_mode"]["refresh"] / 1000,
        "linger": linger.split("=", 1)[-1],
    }, indent=2))


def start():
    private_state_dir()
    with (STATE_DIR / "start.lock").open("a+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if unit_active():
            status()
            return
        MANIFEST.unlink(missing_ok=True)
        sway = tool("sway")
        swaymsg = tool("swaymsg")
        argv = [
            "systemd-run", "--user", "--unit=gpui-headless-sway", "--collect",
            "--description=GPUI headless Sway validation",
            "-p", f"WorkingDirectory={ROOT}",
            "/usr/bin/dbus-run-session", "--", "/usr/bin/env",
            "-u", "DISPLAY", "-u", "WAYLAND_DISPLAY", "-u", "WAYLAND_SOCKET",
            "-u", "SWAYSOCK", "-u", "HYPRLAND_INSTANCE_SIGNATURE",
            f"XDG_RUNTIME_DIR={RUNTIME}",
            "WLR_BACKENDS=headless", "WLR_LIBINPUT_NO_DEVICES=1", "WLR_HEADLESS_OUTPUTS=1",
            "WLR_RENDER_DRM_DEVICE=/dev/dri/renderD128",
            f"GPUI_STATE_DIR={STATE_DIR}",
            f"GPUI_SESSION_READY_HELPER={ROOT / 'scripts/gui/session_ready.py'}",
            f"GPUI_SWAYMSG={swaymsg}",
            f"PATH={ROOT / 'target/gui-validation/local-root/usr/bin'}:/usr/bin:/bin",
            sway, "-c", str(ROOT / "scripts/gui/config/sway-headless.conf"),
        ]
        run(argv)
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if MANIFEST.is_file():
                try:
                    status()
                    return
                except (Problem, json.JSONDecodeError):
                    pass
            if not unit_active():
                break
            time.sleep(0.1)
        subprocess.run(["systemctl", "--user", "stop", UNIT], check=False)
        raise Problem(f"Session did not become ready; inspect journalctl --user -u {UNIT}")


def stop():
    private_state_dir()
    if unit_active():
        # The fixed transient unit is owned by this helper. Refuse a name collision.
        description = run(["systemctl", "--user", "show", UNIT, "-p", "Description"]).strip()
        if description != "Description=GPUI headless Sway validation":
            raise Problem(f"Refusing to stop unexpected service {UNIT}")
        subprocess.run([sys.executable, str(ROOT / "scripts/gui/gui_control.py"), "cleanup"], check=False)
        desktop_control = ROOT / "scripts/gui/desktop_control.py"
        if desktop_control.is_file():
            subprocess.run([sys.executable, str(desktop_control), "cleanup"], check=False)
        run(["systemctl", "--user", "stop", UNIT])
    MANIFEST.unlink(missing_ok=True)
    print("Stopped task-owned headless Sway session; artifacts retained.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="action", required=True)
    for name in ("start", "status", "stop", "update"):
        commands.add_parser(name)
    command = commands.add_parser("exec")
    command.add_argument("argv", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.action == "start":
        start()
    elif args.action == "status":
        status()
    elif args.action == "stop":
        stop()
    elif args.action == "update":
        stop()
        start()
    else:
        manifest, _, env = validate()
        argv = args.argv[1:] if args.argv[:1] == ["--"] else args.argv
        if not argv:
            raise Problem("exec requires a command after --")
        raise SystemExit(subprocess.run(argv, env=env, check=False).returncode)


if __name__ == "__main__":
    try:
        main()
    except (Problem, subprocess.TimeoutExpired, json.JSONDecodeError, KeyError) as error:
        print(f"BLOCKED: {error}", file=sys.stderr)
        sys.exit(2)
