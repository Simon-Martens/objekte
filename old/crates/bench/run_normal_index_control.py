#!/usr/bin/env python3
"""Measure normal release CLI rebuild and fresh-process snapshot reuse."""

import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import time


ROOT = Path(__file__).resolve().parents[2]


def execute(command, output_dir, label):
    started = time.monotonic()
    result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    elapsed_ms = (time.monotonic() - started) * 1000
    (output_dir / f"{label}.stdout.log").write_text(result.stdout)
    (output_dir / f"{label}.stderr.log").write_text(result.stderr)
    if result.returncode:
        raise RuntimeError(f"{label} exited {result.returncode}; see stderr log")
    return {"command": command, "exit_status": result.returncode,
            "wall_ms": elapsed_ms, "stdout": result.stdout}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--repo", type=Path, default=Path("examples/musenalm"))
    parser.add_argument("--collection", default="contents")
    parser.add_argument("--filter", default="year = 1800")
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    args.output_dir.mkdir(parents=True, exist_ok=True)
    cache_dir = args.repo / ".objekte/.cache"
    with tempfile.TemporaryDirectory(prefix="objekte-normal-index-control-") as backup_name:
        backup = Path(backup_name)
        originals = sorted(path for path in cache_dir.glob("*") if path.is_file())
        for path in originals:
            shutil.copy2(path, backup / path.name)
        try:
            records = []
            for run in range(1, args.runs + 1):
                build = execute([str(args.binary), str(args.repo), "cache", "reindex",
                                 "--collection", args.collection], args.output_dir, f"build-{run}")
                restore = execute([str(args.binary), str(args.repo), "query", args.collection,
                                   args.filter, "--count", "--limit", "3", "--output", "json",
                                   "--diagnostics"],
                                  args.output_dir, f"restore-{run}")
                result = json.loads(restore.pop("stdout"))
                build.pop("stdout")
                if not isinstance(result, dict):
                    raise ValueError(f"restore-{run}: expected a JSON object")
                records.append({"round": run, "build": build, "restore": restore,
                                "result": result})
                (args.output_dir / "results.json").write_text(json.dumps(records, indent=2) + "\n")
                print(f"validated normal build/restore round {run}", flush=True)
        finally:
            for path in originals:
                shutil.copy2(backup / path.name, path)


if __name__ == "__main__":
    main()
