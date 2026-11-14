#!/usr/bin/env python3
"""Collect feature-gated fresh-binding timings from the Musenalm harness."""

import argparse
import hashlib
import json
import os
import statistics
import subprocess
from pathlib import Path


def summary(values):
    values = sorted(values)
    if not values:
        return None
    return {"calls": len(values), "total_ms": sum(values) / 1_000_000,
            "mean_us": statistics.mean(values) / 1_000,
            "median_us": statistics.median(values) / 1_000,
            "p95_us": values[(95 * len(values) - 1) // 100] / 1_000,
            "max_us": values[-1] / 1_000}


def main():
    parser = argparse.ArgumentParser()
    for name in ("program", "root", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--repetitions", type=int, default=5)
    args = parser.parse_args()
    if args.repetitions < 1 or args.output.exists():
        parser.error("positive repetitions and absent output required")
    output = args.output.resolve()
    output.mkdir(parents=True)
    program = args.program.resolve()
    root = args.root.resolve()
    runs = {}
    for label, extended in (("standard-150", False), ("extended-220", True)):
        result = output / label
        command = [str(program), "--root", str(root), "--incremental",
                   "--profiles", "ascii-off,unicode-off", "--verified-cache-bytes", "0",
                   "--query-workers", "32", "--repetitions", str(args.repetitions),
                   "--result-dir", str(result), "--storage", "memory"]
        if extended:
            command.append("--extended")
        process = subprocess.run(command, capture_output=True, text=True,
                                 env=dict(os.environ, OBJEKTE_BIND_PHASES="1"))
        (output / f"{label}.stdout").write_text(process.stdout)
        (output / f"{label}.stderr").write_text(process.stderr)
        if process.returncode:
            raise RuntimeError(f"{label}: {process.stderr[-2000:]}")
        samples = []
        for line in process.stderr.splitlines():
            if not line.startswith("query-binding "):
                continue
            fields = dict(part.split("=", 1) for part in line.split()[1:])
            samples.append({"ns": int(fields["ns"]), "regex": fields["regex"] == "true"})
        runs[label] = {"command": command, "all": summary([row["ns"] for row in samples]),
                       "regex": summary([row["ns"] for row in samples if row["regex"]]),
                       "ordinary": summary([row["ns"] for row in samples if not row["regex"]]),
                       "samples": samples}
        print(label, runs[label]["all"], flush=True)
    result = {"program_sha256": hashlib.sha256(program.read_bytes()).hexdigest(),
              "root": str(root), "repetitions": args.repetitions, "runs": runs}
    (output / "summary.json").write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
