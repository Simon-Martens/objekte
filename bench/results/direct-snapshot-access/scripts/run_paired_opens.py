#!/usr/bin/env python3
"""Seven alternating pairs of source-free warm managed opens."""

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
    for name in ("baseline-program", "candidate-program", "baseline-store",
                 "candidate-store", "root", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--trials", type=int, default=7)
    args = parser.parse_args()
    if args.trials < 1 or args.output.exists():
        parser.error("trials must be positive and output must not exist")
    output = args.output.resolve()
    output.mkdir(parents=True)
    root = args.root.resolve()
    programs = {label: getattr(args, label + "_program").resolve() for label in ("baseline", "candidate")}
    stores = {label: getattr(args, label + "_store").resolve() for label in programs}
    records = []
    for mode in ("ondemand", "resident"):
        for trial in range(1, args.trials + 1):
            labels = ("baseline", "candidate") if trial % 2 else ("candidate", "baseline")
            for label in labels:
                result = output / mode / str(trial) / label
                command = [
                    str(programs[label]), "--root", str(root),
                    "--result-dir", str(result), "--extended", "--incremental",
                    "--profiles", "ascii-off,unicode-off", "--repetitions", "1",
                    "--cases", "Q167", "--verified-cache-bytes", "0",
                    "--prepared-cache-bytes", "16777216", "--query-workers", "32",
                    "--storage", "custom", str(stores[label]),
                ]
                if mode == "resident":
                    command.append("--fully-resident")
                run = subprocess.run(command, capture_output=True, text=True)
                result.mkdir(parents=True, exist_ok=True)
                (result / "stdout.log").write_text(run.stdout)
                (result / "stderr.log").write_text(run.stderr)
                if run.returncode:
                    raise RuntimeError(f"{label} {mode} trial {trial}: {run.stderr[-2000:]}")
                index = json.loads((result / "index-results.json").read_text())
                query = [json.loads(line) for line in (result / "measurements.jsonl").read_text().splitlines()]
                if len(index) != 2 or len(query) != 2 or any(
                    row["source_reads"] or row["build_parses"] for row in index
                ):
                    raise RuntimeError(f"{label} {mode} trial {trial} did not source-free open")
                records.append({
                    "label": label, "mode": mode, "trial": trial, "command": command,
                    "open_ns": {row["alphabet"]: row["open_ns"] for row in index},
                    "first_query_ns": {row["profile"]["alphabet"]: row["total_ns"] for row in query},
                    "resident_bytes": {row["alphabet"]: row["resident_bytes"] for row in index},
                    "peak_resident_bytes": {row["alphabet"]: row["peak_resident_bytes"] for row in index},
                    "managed_bytes": {row["alphabet"]: row["managed_bytes"] for row in index},
                    "digests": json.loads((result / "result-digests.json").read_text()),
                })
            print(mode, trial, "pair complete", flush=True)
    first = records[0]["digests"]
    if any(record["digests"] != first for record in records):
        raise RuntimeError("result digest mismatch across roles or trials")
    medians = {}
    ratios = {}
    for mode in ("ondemand", "resident"):
        for alphabet in ("ascii", "unicode"):
            for label in programs:
                group = [row for row in records if row["mode"] == mode and row["label"] == label]
                medians[f"{label}-{mode}-{alphabet}"] = {
                    field: statistics.median(row[field][alphabet] for row in group)
                    for field in ("open_ns", "first_query_ns", "resident_bytes", "peak_resident_bytes", "managed_bytes")
                }
            pairs = []
            for trial in range(1, args.trials + 1):
                before = next(row for row in records if row["mode"] == mode and row["trial"] == trial and row["label"] == "baseline")
                after = next(row for row in records if row["mode"] == mode and row["trial"] == trial and row["label"] == "candidate")
                pairs.append(after["open_ns"][alphabet] / before["open_ns"][alphabet])
            ratios[f"{mode}-{alphabet}"] = statistics.median(pairs)
    summary = {"program_sha256": {label: sha256(program) for label, program in programs.items()},
               "root": str(root), "stores": {label: str(store) for label, store in stores.items()},
               "trials": args.trials, "medians": medians,
               "median_paired_open_ratio": ratios, "records": records}
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps({"medians": medians, "paired_open_ratio": ratios}, indent=2))


if __name__ == "__main__":
    main()
