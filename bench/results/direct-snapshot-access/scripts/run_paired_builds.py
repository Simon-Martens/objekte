#!/usr/bin/env python3
"""Seven alternating pairs of fresh incremental ASCII construction trials."""

import argparse
import hashlib
import json
import statistics
import subprocess
from pathlib import Path


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser()
    for name in ("baseline-program", "candidate-program", "root", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--trials", type=int, default=7)
    args = parser.parse_args()
    if args.trials < 1 or args.output.exists():
        parser.error("trials must be positive and output must not exist")
    output = args.output.resolve()
    output.mkdir(parents=True)
    root = args.root.resolve()
    programs = {label: getattr(args, label + "_program").resolve() for label in ("baseline", "candidate")}
    records = []
    for trial in range(1, args.trials + 1):
        labels = ("baseline", "candidate") if trial % 2 else ("candidate", "baseline")
        for label in labels:
            result = output / str(trial) / label
            command = [str(programs[label]), "--root", str(root), "--result-dir", str(result),
                       "--alphabet", "ascii", "--workers", "32", "--incremental"]
            run = subprocess.run(command, capture_output=True, text=True)
            result.mkdir(parents=True, exist_ok=True)
            (result / "stdout.log").write_text(run.stdout)
            (result / "stderr.log").write_text(run.stderr)
            if run.returncode:
                raise RuntimeError(f"{label} trial {trial}: {run.stderr[-2000:]}")
            samples = list(result.glob("trial-*/sample.json"))
            if len(samples) != 1:
                raise RuntimeError(f"{label} trial {trial}: expected one sample")
            sample = json.loads(samples[0].read_text())
            records.append({"label": label, "trial": trial, "command": command,
                            "sample": {field: sample[field] for field in (
                                "build_ns", "completion_ns", "first_batch_ns",
                                "after_build_rss", "after_build_hwm")}})
        print(trial, "pair complete", flush=True)
    medians = {}
    ratios = {}
    for label in programs:
        group = [row["sample"] for row in records if row["label"] == label]
        medians[label] = {field: statistics.median(row[field] for row in group)
                          for field in group[0]}
    for field in medians["baseline"]:
        pairs = []
        for trial in range(1, args.trials + 1):
            before = next(row for row in records if row["label"] == "baseline" and row["trial"] == trial)["sample"]
            after = next(row for row in records if row["label"] == "candidate" and row["trial"] == trial)["sample"]
            pairs.append(after[field] / max(before[field], 1))
        ratios[field] = statistics.median(pairs)
    summary = {"program_sha256": {label: sha256(program) for label, program in programs.items()},
               "root": str(root), "trials": args.trials, "medians": medians,
               "median_paired_ratios": ratios, "records": records}
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps({"medians": medians, "paired_ratios": ratios}, indent=2))


if __name__ == "__main__":
    main()
