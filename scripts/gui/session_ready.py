#!/usr/bin/python3
"""Record the sockets exported by this Sway process after its output is ready."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time


def main():
    state_dir = Path(os.environ["GPUI_STATE_DIR"])
    runtime = Path(os.environ["XDG_RUNTIME_DIR"])
    sway_socket = Path(os.environ["SWAYSOCK"])
    wayland = os.environ["WAYLAND_DISPLAY"]
    swaymsg = os.environ["GPUI_SWAYMSG"]
    pid = int(sway_socket.name.split(".")[-2])
    wayland_socket = Path(wayland) if wayland.startswith("/") else runtime / wayland
    if runtime.stat().st_uid != os.getuid() or state_dir.stat().st_uid != os.getuid():
        raise RuntimeError("runtime or state directory belongs to another user")
    if not sway_socket.is_socket() or not wayland_socket.is_socket():
        raise RuntimeError("Sway did not export live sockets")
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        result = subprocess.run(
            [swaymsg, "-s", str(sway_socket), "-t", "get_outputs", "-r"],
            capture_output=True, text=True, timeout=2,
        )
        if result.returncode == 0:
            outputs = [item for item in json.loads(result.stdout) if item.get("active")]
            if len(outputs) == 1 and outputs[0]["name"].startswith("HEADLESS-"):
                output = outputs[0]
                mode = output["current_mode"]
                if (mode["width"], mode["height"], mode["refresh"], output["scale"]) != (1920, 1080, 60000, 1.0):
                    raise RuntimeError(f"Required 1920x1080@60Hz scale 1 unavailable: {mode}, scale={output['scale']}")
                data = {
                    "sway_pid": pid,
                    "sway_socket": str(sway_socket),
                    "wayland_display": wayland,
                    "wayland_socket": str(wayland_socket),
                    "dbus_address": os.environ["DBUS_SESSION_BUS_ADDRESS"],
                    "output": output["name"],
                    "width": output["current_mode"]["width"],
                    "height": output["current_mode"]["height"],
                    "refresh_mhz": output["current_mode"]["refresh"],
                    "scale": output["scale"],
                }
                fd, temporary = tempfile.mkstemp(prefix="manifest-", dir=state_dir)
                try:
                    with os.fdopen(fd, "w") as stream:
                        json.dump(data, stream, indent=2)
                        stream.write("\n")
                    os.replace(temporary, state_dir / "manifest.json")
                finally:
                    Path(temporary).unlink(missing_ok=True)
                return
        time.sleep(0.1)
    raise RuntimeError("headless output did not become ready in 10 seconds")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"session_ready: {error}", file=sys.stderr)
        sys.exit(1)
