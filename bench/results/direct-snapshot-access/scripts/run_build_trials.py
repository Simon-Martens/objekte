#!/usr/bin/env python3
"""Seven fresh incremental ASCII construction trials on the fixed fixture."""

import argparse
import hashlib
import json
import statistics
import subprocess
from pathlib import Path


def main():
    parser = argparse.ArgumentParser()
    for name in ("program", "root", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--label", required=True)
    parser.add_argument("--trials", type=int, default=7)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    records = []
    for trial in range(1, args.trials + 1):
        folder = args.output / str(trial)
        command = [
            str(args.program.resolve()), "--root", str(args.root.resolve()),
            "--result-dir", str(folder), "--alphabet", "ascii",
            "--workers", "32", "--incremental",
        ]
        run = subprocess.run(command, capture_output=True, text=True)
        folder.mkdir(parents=True, exist_ok=True)
        (folder / "stdout.log").write_text(run.stdout)
        (folder / "stderr.log").write_text(run.stderr)
        if run.returncode:
            raise RuntimeError(f"trial {trial}: {run.stderr[-2000:]}")
        samples = list(folder.glob("trial-*/sample.json"))
        if len(samples) != 1:
            raise RuntimeError(f"trial {trial}: expected one sample, got {len(samples)}")
        sample = json.loads(samples[0].read_text())
        records.append({
            "trial": trial, "command": command, "sample_path": str(samples[0]),
            "build_ns": sample["build_ns"],
            "completion_ns": sample["completion_ns"],
            "first_batch_ns": sample["first_batch_ns"],
            "after_build_rss": sample["after_build_rss"],
            "after_build_hwm": sample["after_build_hwm"],
        })
        print(args.label, trial, "done", flush=True)
    digest = hashlib.sha256(args.program.read_bytes()).hexdigest()
    output = {
        "label": args.label, "program_sha256": digest,
        "root": str(args.root.resolve()), "trials": args.trials,
        "medians": {
            field: statistics.median(row[field] for row in records)
            for field in ("build_ns", "completion_ns", "first_batch_ns", "after_build_rss", "after_build_hwm")
        },
        "records": records,
    }
    (args.output / "summary.json").write_text(json.dumps(output, indent=2) + "\n")
    print(json.dumps(output["medians"], indent=2))


if __name__ == "__main__":
    main()
