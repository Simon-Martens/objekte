#!/usr/bin/env python3
"""Control only the validation window in the task-owned headless Sway session."""

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time

import headless_session as session

ROOT = Path(__file__).resolve().parents[2]
ARTIFACTS = ROOT / "target/gui-validation"
STATE = session.STATE_DIR / "gui-state.json"
EXE = ROOT / "target/debug/examples/environment_validation"
VNCDO = ARTIFACTS / "venv/bin/vncdo"
APP_UNIT = "gpui-validation-app.service"
VNC_UNIT = "gpui-wayvnc.service"
APP_ID = "dev.objekte.gpui-environment-validation"
TITLE = "GPUI Environment Validation"
PORT = 5907


class Problem(Exception):
    pass


def run(argv, *, env=None, timeout=10):
    result = subprocess.run(argv, capture_output=True, text=True, env=env, timeout=timeout)
    if result.returncode:
        raise Problem(f"{argv[0]}: {result.stderr.strip() or result.stdout.strip()}")
    return result.stdout


def read_state():
    return json.loads(STATE.read_text()) if STATE.exists() else {}


def write_state(state):
    session.private_state_dir()
    fd, temporary = tempfile.mkstemp(prefix="gui-state-", dir=session.STATE_DIR)
    try:
        with os.fdopen(fd, "w") as stream:
            json.dump(state, stream, indent=2)
            stream.write("\n")
        os.replace(temporary, STATE)
    finally:
        Path(temporary).unlink(missing_ok=True)


def unit_info(unit):
    result = subprocess.run(
        ["systemctl", "--user", "show", unit, "-p", "ActiveState", "-p", "MainPID", "-p", "Description"],
        capture_output=True, text=True,
    )
    if result.returncode:
        return {}
    return dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line)


def owned_unit(unit, description, expected_pid):
    info = unit_info(unit)
    return (info.get("ActiveState") == "active"
            and info.get("Description") == description
            and int(info.get("MainPID", 0)) == expected_pid)


def sway(manifest, env, *args):
    return run([session.tool("swaymsg"), "-s", manifest["sway_socket"], *args], env=env)


def tree_nodes(manifest, env):
    root = json.loads(sway(manifest, env, "-t", "get_tree", "-r"))
    stack = [root]
    while stack:
        node = stack.pop()
        yield node
        stack.extend(node.get("nodes", []) + node.get("floating_nodes", []))


def tracked_window(manifest, env, state):
    pid = int(state.get("app_pid", 0))
    if not owned_unit(APP_UNIT, "GPUI validation application", pid):
        raise Problem("Task-owned validation app is not running")
    if Path(f"/proc/{pid}/exe").resolve() != EXE.resolve():
        raise Problem("Tracked app PID has a different executable")
    matches = [node for node in tree_nodes(manifest, env)
               if node.get("pid") == pid and node.get("id") == state.get("window_id")]
    if len(matches) != 1:
        raise Problem("Tracked validation window is absent or ambiguous")
    node = matches[0]
    if node.get("name") != TITLE or node.get("app_id") != APP_ID or node.get("shell") != "xdg_shell":
        raise Problem("Window identity or native Wayland shell did not match")
    if not node.get("visible"):
        raise Problem("Validation window is not visible")
    return node


def focus(manifest, env, state):
    node = tracked_window(manifest, env, state)
    sway(manifest, env, f"[con_id={node['id']}]", "focus")
    node = tracked_window(manifest, env, state)
    if not node.get("focused"):
        raise Problem("Validation window did not receive focus; input stopped")
    return node


def vnc(*commands):
    if not VNCDO.is_file():
        raise Problem("vncdo missing; install scripts/gui/requirements.txt in target/gui-validation/venv")
    run([str(VNCDO), "-s", f"127.0.0.1::{PORT}", "-t", "10", *map(str, commands)], timeout=15)


def ensure_pointer(manifest, env, state):
    focus(manifest, env, state)
    if not owned_unit(VNC_UNIT, "GPUI validation pointer server", int(state.get("vnc_pid", 0))):
        raise Problem("Task-owned pointer server is not running; run start-vnc")


def point(args, output):
    mode = output["current_mode"]
    if not (0 <= args.x < mode["width"] and 0 <= args.y < mode["height"]):
        raise Problem("Coordinates are outside the headless output in physical pixels")
    return args.x, args.y


def stop_unit(unit, description, pid):
    if not pid:
        return
    info = unit_info(unit)
    if info.get("ActiveState") != "active":
        return
    if not owned_unit(unit, description, int(pid)):
        raise Problem(f"Refusing to stop unverified unit {unit}")
    run(["systemctl", "--user", "stop", unit])


def start_unit(name, description, argv, env_values):
    command = ["systemd-run", "--user", f"--unit={name}", "--collect",
               f"--description={description}", "-p", f"WorkingDirectory={ROOT}",
               "/usr/bin/env", "-u", "DISPLAY", *env_values, *map(str, argv)]
    run(command)
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        info = unit_info(f"{name}.service")
        if info.get("ActiveState") == "active" and info.get("Description") == description:
            pid = int(info.get("MainPID", 0))
            if pid:
                return pid
        time.sleep(0.05)
    raise Problem(f"{name} service did not start")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("status", "launch", "focus", "start-vnc", "stop-vnc", "close", "cleanup"):
        sub.add_parser(name)
    shot = sub.add_parser("screenshot")
    shot.add_argument("name", nargs="?", default="screenshot.png")
    for name in ("move", "click", "scroll"):
        command = sub.add_parser(name)
        command.add_argument("x", type=int)
        command.add_argument("y", type=int)
        if name == "scroll":
            command.add_argument("direction", choices=("up", "down"))
            command.add_argument("steps", type=int, nargs="?", default=3)
    typed = sub.add_parser("type")
    typed.add_argument("x", type=int)
    typed.add_argument("y", type=int)
    typed.add_argument("text")
    sub.add_parser("key").add_argument("keysym")
    args = parser.parse_args()
    state = read_state()

    if args.command in ("close", "cleanup", "stop-vnc"):
        if args.command in ("close", "cleanup"):
            stop_unit(APP_UNIT, "GPUI validation application", state.get("app_pid"))
            state.pop("app_pid", None)
            state.pop("window_id", None)
        if args.command in ("stop-vnc", "cleanup"):
            stop_unit(VNC_UNIT, "GPUI validation pointer server", state.get("vnc_pid"))
            state.pop("vnc_pid", None)
        write_state(state)
        print(f"Task-owned {args.command} completed; artifacts retained.")
        return

    manifest, output, env = session.validate()
    if args.command == "status":
        print(json.dumps({
            "sway_pid": manifest["sway_pid"], "wayland_display": manifest["wayland_display"],
            "output": output["name"], "size": [output["current_mode"]["width"], output["current_mode"]["height"]],
            "refresh_hz": output["current_mode"]["refresh"] / 1000, "scale": output["scale"],
            "app_pid": state.get("app_pid"), "vnc_pid": state.get("vnc_pid"),
            "wtype": session.tool("wtype"), "wayvnc": shutil.which("wayvnc"), "vncdo": VNCDO.is_file(),
        }, indent=2))
    elif args.command == "launch":
        if not EXE.is_file():
            raise Problem(f"Executable missing: {EXE}; run cargo build --locked")
        if state.get("app_pid") and owned_unit(APP_UNIT, "GPUI validation application", state["app_pid"]):
            raise Problem("Validation app is already running; close it first")
        values = [f"XDG_RUNTIME_DIR={session.RUNTIME}", f"WAYLAND_DISPLAY={manifest['wayland_display']}",
                  f"DBUS_SESSION_BUS_ADDRESS={manifest['dbus_address']}"]
        pid = start_unit("gpui-validation-app", "GPUI validation application", [EXE], values)
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            matches = [node for node in tree_nodes(manifest, env) if node.get("pid") == pid
                       and node.get("app_id") == APP_ID and node.get("name") == TITLE
                       and node.get("shell") == "xdg_shell"]
            if len(matches) == 1 and matches[0].get("visible"):
                state.update(app_pid=pid, window_id=matches[0]["id"])
                write_state(state)
                focus(manifest, env, state)
                print(f"Launched native Wayland window pid={pid}, con_id={matches[0]['id']}")
                return
            if not owned_unit(APP_UNIT, "GPUI validation application", pid):
                raise Problem("Validation app exited before its window appeared")
            time.sleep(0.1)
        stop_unit(APP_UNIT, "GPUI validation application", pid)
        raise Problem("Validation window did not appear within 20 seconds")
    elif args.command == "focus":
        focus(manifest, env, state)
        print("Validation window focused.")
    elif args.command == "screenshot":
        focus(manifest, env, state)
        if not re.fullmatch(r"[A-Za-z0-9_.-]+\.png", args.name):
            raise Problem("Screenshot name must be a simple PNG filename")
        ARTIFACTS.mkdir(exist_ok=True)
        target = ARTIFACTS / args.name
        run(["grim", "-o", output["name"], str(target)], env=env)
        from PIL import Image
        with Image.open(target) as picture:
            if picture.size != (output["current_mode"]["width"], output["current_mode"]["height"]):
                raise Problem("Screenshot size does not match selected output")
            extrema = picture.convert("RGB").getextrema()
            if all(lo == hi for lo, hi in extrema):
                raise Problem("Screenshot is uniform; visual result not verified")
        print(target.resolve())
    elif args.command == "start-vnc":
        if state.get("vnc_pid") and owned_unit(VNC_UNIT, "GPUI validation pointer server", state["vnc_pid"]):
            print(f"Pointer server already listening on 127.0.0.1:{PORT}")
            return
        executable = shutil.which("wayvnc")
        if not executable:
            raise Problem("WayVNC is missing")
        with socket.socket() as probe:
            if probe.connect_ex(("127.0.0.1", PORT)) == 0:
                raise Problem(f"Loopback port {PORT} already occupied")
        values = [f"XDG_RUNTIME_DIR={session.RUNTIME}", f"WAYLAND_DISPLAY={manifest['wayland_display']}"]
        pid = start_unit("gpui-wayvnc", "GPUI validation pointer server",
                         [executable, "-o", output["name"], "-R", "127.0.0.1", str(PORT)], values)
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            with socket.socket() as probe:
                if probe.connect_ex(("127.0.0.1", PORT)) == 0:
                    state["vnc_pid"] = pid
                    write_state(state)
                    # Establish the virtual pointer before the first real click.
                    vnc("move", 0, 0)
                    print(f"WayVNC pointer server listening on 127.0.0.1:{PORT}; pid={pid}")
                    return
            if not owned_unit(VNC_UNIT, "GPUI validation pointer server", pid):
                raise Problem("WayVNC exited early")
            time.sleep(0.1)
        stop_unit(VNC_UNIT, "GPUI validation pointer server", pid)
        raise Problem("WayVNC did not listen within 10 seconds")
    elif args.command in ("move", "click", "scroll", "type", "key"):
        ensure_pointer(manifest, env, state)
        if args.command in ("move", "click", "scroll", "type"):
            x, y = point(args, output)
        if args.command == "move":
            vnc("move", x, y)
        elif args.command == "click":
            vnc("move", x, y, "click", 1)
        elif args.command == "scroll":
            if not 1 <= args.steps <= 20:
                raise Problem("Scroll steps must be between 1 and 20")
            wheel = 4 if args.direction == "up" else 5
            vnc("move", x, y, *[part for _ in range(args.steps) for part in ("click", wheel)])
        elif args.command == "type":
            # This host drops the first key event from each new wtype device.
            # RFB establishes input focus; Shift primes wtype, and BackSpace
            # removes the temporary ASCII seed before Unicode text is sent.
            vnc("move", x, y, "click", 1, "type", "X")
            run([session.tool("wtype"), "-k", "Shift_L", "-k", "BackSpace", args.text], env=env)
        else:
            if not re.fullmatch(r"[A-Za-z0-9_+-]+", args.keysym):
                raise Problem("Use a simple wtype keysym")
            run([session.tool("wtype"), "-k", "Shift_L", "-k", args.keysym], env=env)
        print("Input sent to focused validation window.")


if __name__ == "__main__":
    try:
        main()
    except (Problem, session.Problem, subprocess.TimeoutExpired, json.JSONDecodeError, OSError, ValueError) as error:
        print(f"BLOCKED: {error}", file=sys.stderr)
        sys.exit(2)
