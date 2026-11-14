#!/usr/bin/env python3
"""Pair instrumented managed opens to compare gross allocation work."""

import argparse
import hashlib
import json
import statistics
import subprocess
from pathlib import Path


def sha256(path):
    hash_state = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            hash_state.update(chunk)
    return hash_state.hexdigest()


def main():
    parser = argparse.ArgumentParser()
    for name in ("baseline-program", "candidate-program", "baseline-store",
                 "candidate-store", "root", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--trials", type=int, default=7)
    args = parser.parse_args()
    if args.trials < 1:
        parser.error("trials must be positive")
    programs = {label: getattr(args, label.replace("-", "_") + "_program").resolve()
                for label in ("baseline", "candidate")}
    stores = {label: getattr(args, label.replace("-", "_") + "_store").resolve()
              for label in ("baseline", "candidate")}
    root = args.root.resolve()
    samples = []
    for mode in ("ondemand", "resident"):
        for alphabet in ("ascii", "unicode"):
            for trial in range(args.trials):
                labels = ("baseline", "candidate") if trial % 2 == 0 else ("candidate", "baseline")
                for label in labels:
                    command = [str(programs[label]), "--root", str(root),
                               "--store", str(stores[label]), "--alphabet", alphabet]
                    if mode == "resident":
                        command.append("--resident")
                    run = subprocess.run(command, capture_output=True, text=True)
                    if run.returncode:
                        raise RuntimeError(f"{label} {mode} {alphabet} trial {trial + 1}: {run.stderr}")
                    row = json.loads(run.stdout)
                    if row["source_reads"] or row["build_parses"]:
                        raise RuntimeError("instrumented open rebuilt from sources")
                    samples.append({"label": label, "mode": mode, "alphabet": alphabet,
                                    "trial": trial + 1, "command": command, "sample": row})
                print(mode, alphabet, trial + 1, "done", flush=True)
    medians = {}
    for label in programs:
        for mode in ("ondemand", "resident"):
            for alphabet in ("ascii", "unicode"):
                group = [row["sample"] for row in samples if row["label"] == label
                         and row["mode"] == mode and row["alphabet"] == alphabet]
                medians[f"{label}-{mode}-{alphabet}"] = {
                    field: statistics.median(row[field] for row in group)
                    for field in ("open_ns", "allocation_calls", "gross_allocated_bytes")
                }
    output = {"program_sha256": {label: sha256(path) for label, path in programs.items()},
              "trials": args.trials, "medians": medians, "samples": samples}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(output, indent=2) + "\n")
    print(json.dumps(medians, indent=2))


if __name__ == "__main__":
    main()
