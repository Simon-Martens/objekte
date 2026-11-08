#!/usr/bin/env python3
"""Operate the read-only Objekte desktop window in the verified Sway session."""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time

import gui_control as control
import headless_session as session


ROOT = Path(__file__).resolve().parents[2]
ARTIFACTS = ROOT / "target/gui-validation/desktop"
STATE = session.STATE_DIR / "desktop-state.json"
EXE = ROOT / "target/debug/objekte-desktop"
APP_UNIT = "objekte-desktop-app.service"
APP_ID = "dev.objekte.desktop"
TITLE = "Objekte"
DESCRIPTION = "Objekte desktop application"


def read_state():
    return json.loads(STATE.read_text()) if STATE.exists() else {}


def write_state(state):
    session.private_state_dir()
    fd, temporary = tempfile.mkstemp(prefix="desktop-state-", dir=session.STATE_DIR)
    try:
        with os.fdopen(fd, "w") as stream:
            json.dump(state, stream, indent=2)
            stream.write("\n")
        os.replace(temporary, STATE)
    finally:
        Path(temporary).unlink(missing_ok=True)


def tracked_window(manifest, env, state):
    pid = int(state.get("app_pid", 0))
    if not control.owned_unit(APP_UNIT, DESCRIPTION, pid):
        raise control.Problem("Task-owned desktop app is not running")
    if Path(f"/proc/{pid}/exe").resolve() != EXE.resolve():
        raise control.Problem("Tracked desktop PID has a different executable")
    matches = [node for node in control.tree_nodes(manifest, env)
               if node.get("pid") == pid and node.get("id") == state.get("window_id")]
    if len(matches) != 1:
        raise control.Problem("Tracked desktop window is absent or ambiguous")
    node = matches[0]
    if (not node.get("name", "").startswith(TITLE)
            or node.get("app_id") != APP_ID or node.get("shell") != "xdg_shell"
            or not node.get("visible")):
        raise control.Problem("Desktop window identity, shell, or visibility did not match")
    return node


def focus(manifest, env, state):
    node = tracked_window(manifest, env, state)
    control.sway(manifest, env, f"[con_id={node['id']}]", "focus")
    node = tracked_window(manifest, env, state)
    if not node.get("focused"):
        raise control.Problem("Desktop window did not receive focus; input stopped")
    return node


def close(state):
    control.stop_unit(APP_UNIT, DESCRIPTION, state.get("app_pid"))
    state.pop("app_pid", None)
    state.pop("window_id", None)
    write_state(state)


def pointer_ready(manifest, env, state):
    focus(manifest, env, state)
    pointer = control.read_state().get("vnc_pid")
    if not pointer or not control.owned_unit(control.VNC_UNIT, "GPUI validation pointer server", int(pointer)):
        raise control.Problem("Pointer server is not running; run start-vnc")


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
            close(state)
        if args.command in ("stop-vnc", "cleanup"):
            print(control.run([sys.executable, str(ROOT / "scripts/gui/gui_control.py"), "stop-vnc"]).strip())
        return

    manifest, output, env = session.validate()
    if args.command == "status":
        pid = int(state.get("app_pid", 0))
        print(json.dumps({
            "sway_pid": manifest["sway_pid"],
            "output": output["name"],
            "size": [output["current_mode"]["width"], output["current_mode"]["height"]],
            "refresh_hz": output["current_mode"]["refresh"] / 1000,
            "scale": output["scale"],
            "app_pid": pid if control.owned_unit(APP_UNIT, DESCRIPTION, pid) else None,
            "vnc_pid": control.read_state().get("vnc_pid"),
        }, indent=2))
    elif args.command == "launch":
        if not EXE.is_file():
            raise control.Problem(f"Executable missing: {EXE}; run cargo build --locked -p objekte-desktop --bin objekte-desktop")
        if state.get("app_pid") and control.owned_unit(APP_UNIT, DESCRIPTION, state["app_pid"]):
            raise control.Problem("Desktop app is already running; close it first")
        control.run([sys.executable, str(ROOT / "scripts/gui/gui_control.py"), "close"])
        values = [f"XDG_RUNTIME_DIR={session.RUNTIME}", f"WAYLAND_DISPLAY={manifest['wayland_display']}",
                  f"DBUS_SESSION_BUS_ADDRESS={manifest['dbus_address']}"]
        pid = control.start_unit("objekte-desktop-app", DESCRIPTION, [EXE], values)
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            matches = [node for node in control.tree_nodes(manifest, env) if node.get("pid") == pid
                       and node.get("app_id") == APP_ID and node.get("name", "").startswith(TITLE)
                       and node.get("shell") == "xdg_shell"]
            if len(matches) == 1 and matches[0].get("visible"):
                state.update(app_pid=pid, window_id=matches[0]["id"])
                write_state(state)
                focus(manifest, env, state)
                print(f"Launched native Objekte window pid={pid}, con_id={matches[0]['id']}")
                return
            if not control.owned_unit(APP_UNIT, DESCRIPTION, pid):
                raise control.Problem("Desktop app exited before its window appeared")
            time.sleep(0.1)
        control.stop_unit(APP_UNIT, DESCRIPTION, pid)
        raise control.Problem("Desktop window did not appear within 20 seconds")
    elif args.command == "start-vnc":
        print(control.run([sys.executable, str(ROOT / "scripts/gui/gui_control.py"), "start-vnc"]).strip())
    elif args.command == "focus":
        focus(manifest, env, state)
        print("Desktop window focused.")
    elif args.command == "screenshot":
        focus(manifest, env, state)
        if not re.fullmatch(r"[A-Za-z0-9_.-]+\.png", args.name):
            raise control.Problem("Screenshot name must be a simple PNG filename")
        ARTIFACTS.mkdir(parents=True, exist_ok=True)
        target = ARTIFACTS / args.name
        control.run(["grim", "-o", output["name"], str(target)], env=env)
        from PIL import Image
        with Image.open(target) as picture:
            if picture.size != (output["current_mode"]["width"], output["current_mode"]["height"]):
                raise control.Problem("Screenshot size does not match selected output")
            extrema = picture.convert("RGB").getextrema()
            if all(lo == hi for lo, hi in extrema):
                raise control.Problem("Screenshot is uniform; visual result not verified")
        print(target.resolve())
    elif args.command in ("move", "click", "scroll", "type", "key"):
        pointer_ready(manifest, env, state)
        if args.command in ("move", "click", "scroll", "type"):
            x, y = control.point(args, output)
        if args.command == "move":
            control.vnc("move", x, y)
        elif args.command == "click":
            control.vnc("move", x, y, "click", 1)
        elif args.command == "scroll":
            if not 1 <= args.steps <= 20:
                raise control.Problem("Scroll steps must be between 1 and 20")
            wheel = 4 if args.direction == "up" else 5
            control.vnc("move", x, y, *[part for _ in range(args.steps) for part in ("click", wheel)])
        elif args.command == "type":
            control.vnc("move", x, y, "click", 1, "type", "X")
            control.run([session.tool("wtype"), "-k", "Shift_L", "-k", "BackSpace", args.text], env=env)
        else:
            if not re.fullmatch(r"[A-Za-z0-9_+-]+", args.keysym):
                raise control.Problem("Use a simple wtype keysym")
            control.run([session.tool("wtype"), "-k", "Shift_L", "-k", args.keysym], env=env)
        print("Input sent to focused Objekte window.")


if __name__ == "__main__":
    try:
        main()
    except (control.Problem, session.Problem, subprocess.TimeoutExpired, json.JSONDecodeError,
            OSError, ValueError) as error:
        print(f"BLOCKED: {error}", file=sys.stderr)
        sys.exit(2)
