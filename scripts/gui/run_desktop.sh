#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}"
control=("$project_root/target/gui-validation/venv/bin/python" "$project_root/scripts/gui/desktop_control.py")

case "${1:-}" in
    prepare)
        cd "$project_root"
        cargo build --locked -p objekte-desktop --bin objekte-desktop
        if [[ ! -x target/gui-validation/venv/bin/vncdo ]]; then
            uv venv target/gui-validation/venv --python /usr/bin/python3
            uv pip install --python target/gui-validation/venv/bin/python -r scripts/gui/requirements.txt
        fi
        python3 scripts/gui/headless_session.py start
        "${control[@]}" start-vnc
        "${control[@]}" close
        "${control[@]}" launch
        "${control[@]}" screenshot initial.png
        ;;
    cleanup)
        "${control[@]}" cleanup
        ;;
    *)
        echo 'Usage: run_desktop.sh prepare|cleanup' >&2
        exit 2
        ;;
esac
