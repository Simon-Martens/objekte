#!/usr/bin/env python3
"""Repeat the small cache-64 query differences in alternating order."""

import argparse
import json
import os
from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parents[4]
CASE_IDS = (2, 3, 25, 37, 39, 40, 41, 42, 44, 45, 46)


def run(binary: Path, workload: Path, destination: Path) -> list[dict]:
    command = [
        "taskset", "-c", "0-15", str(binary), "--repo", str(ROOT / "examples/musenalm"),
        "--workload", str(workload), "--runs", "7", "--bigram-encoding", "ascii",
        "--query-cache-mib", "64",
    ]
    result = subprocess.run(command, cwd=ROOT,
                            env=dict(os.environ, RAYON_NUM_THREADS="16"),
                            capture_output=True, text=True)
    destination.with_suffix(".stdout.ndjson").write_text(result.stdout)
    destination.with_suffix(".stderr.log").write_text(result.stderr)
    destination.with_suffix(".command.json").write_text(json.dumps({
        "command": command, "exit_status": result.returncode,
        "rayon_threads": 16, "affinity": "0-15",
    }, indent=2) + "\n")
    if result.returncode:
        raise RuntimeError(f"{destination}: exited {result.returncode}")
    rows = [json.loads(line) for line in result.stdout.splitlines()]
    if len(rows) != len(CASE_IDS) + 1 or any(len(row["samples"]) != 7 for row in rows[1:]):
        raise RuntimeError(f"{destination}: incomplete workload")
    return rows


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--master-bin", type=Path, required=True)
    parser.add_argument("--current-bin", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=True)
    full = json.loads((ROOT / "crates/bench/workloads/musenalm-mix-100.json").read_text())
    subset = [{**full[case_id - 1], "id": index}
              for index, case_id in enumerate(CASE_IDS, 1)]
    workload = output / "workload.json"
    workload.write_text(json.dumps(subset, indent=2) + "\n")
    binaries = {
        "master": args.master_bin.resolve(strict=True),
        "current": args.current_bin.resolve(strict=True),
    }
    for pair in range(1, 4):
        order = ("current", "master") if pair % 2 else ("master", "current")
        rows = {}
        for label in order:
            print(f"cache64 suspect pair {pair}/3 {label}", flush=True)
            rows[label] = run(binaries[label], workload, output / f"pair{pair}-{label}")
        for master, current in zip(rows["master"][1:], rows["current"][1:]):
            keys = ("kind", "filter", "matches", "first_id")
            if any(master[key] != current[key] for key in keys):
                raise RuntimeError(f"pair {pair}: signature mismatch for {master['filter']}")


if __name__ == "__main__":
    main()
