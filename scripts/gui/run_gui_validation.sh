#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}"
control=("$project_root/target/gui-validation/venv/bin/python" "$project_root/scripts/gui/gui_control.py")
session=(python3 "$project_root/scripts/gui/headless_session.py")

case "${1:-}" in
    prepare)
        cd "$project_root"
        cargo build --locked -p objekte-desktop --example environment_validation
        if [[ ! -x target/gui-validation/venv/bin/vncdo ]]; then
            uv venv target/gui-validation/venv --python /usr/bin/python3
            uv pip install --python target/gui-validation/venv/bin/python -r scripts/gui/requirements.txt
        fi
        "${session[@]}" start
        "${control[@]}" status
        "${control[@]}" start-vnc
        "${control[@]}" close
        "${control[@]}" launch
        "${control[@]}" screenshot initial.png
        echo 'Inspect target/gui-validation/initial.png through an image-capable agent before exercise.'
        ;;
    exercise)
        if (( $# != 11 )); then
            echo 'Usage: run_gui_validation.sh exercise INPUT_X INPUT_Y CHECK_X CHECK_Y ACTION_X ACTION_Y SCROLL_X SCROLL_Y SAVE_X SAVE_Y' >&2
            exit 2
        fi
        # Require GUI Save to create fresh evidence for this run.
        python3 -c 'import sys; from pathlib import Path; Path(sys.argv[1]).unlink(missing_ok=True)' \
            "$project_root/target/gui-validation/fixture.json"
        "${control[@]}" type "$2" "$3" 'Objekte ÄÖÜ äöü ß 123'
        "${control[@]}" screenshot after-type.png
        "${control[@]}" click "$4" "$5"
        "${control[@]}" click "$6" "$7"
        "${control[@]}" screenshot after-controls.png
        "${control[@]}" scroll "$8" "$9" down 5
        "${control[@]}" screenshot after-scroll.png
        "${control[@]}" click "${10}" "${11}"
        "${control[@]}" screenshot final.png
        python3 - "$project_root/target/gui-validation/fixture.json" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
actual = json.loads(path.read_text())
expected = {"text": "Objekte ÄÖÜ äöü ß 123", "checked": True, "action_count": 1}
if actual != expected:
    raise SystemExit(f"Fixture mismatch: {actual!r}")
print(f"Fixture PASS: {path}")
PY
        echo 'Inspect after-type.png, after-controls.png, after-scroll.png and final.png before claiming visual PASS.'
        ;;
    cleanup)
        "${session[@]}" stop
        ;;
    *)
        echo 'Usage: run_gui_validation.sh prepare|exercise COORDINATES|cleanup' >&2
        exit 2
        ;;
esac
