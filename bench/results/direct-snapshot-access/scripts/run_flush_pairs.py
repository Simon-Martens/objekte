#!/usr/bin/env python3
"""Seven alternating pairs of fresh managed Musenalm flushes."""

import argparse
import hashlib
import json
import shutil
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
    parser.add_argument("--alphabet", choices=("ascii", "unicode"), default="ascii")
    parser.add_argument("--trials", type=int, default=7)
    args = parser.parse_args()
    if args.trials < 1 or args.output.exists():
        parser.error("positive trials and absent output are required")
    output = args.output.resolve()
    output.mkdir(parents=True)
    root = args.root.resolve()
    programs = {label: getattr(args, label + "_program").resolve()
                for label in ("baseline", "candidate")}
    records = []
    for trial in range(1, args.trials + 1):
        labels = ("baseline", "candidate") if trial % 2 else ("candidate", "baseline")
        for label in labels:
            store = output / "stores" / str(trial) / label
            command = [str(programs[label]), "--root", str(root), "--output", str(store),
                       "--alphabet", args.alphabet]
            run = subprocess.run(command, capture_output=True, text=True)
            log = output / f"{trial}-{label}"
            log.with_suffix(".stdout").write_text(run.stdout)
            log.with_suffix(".stderr").write_text(run.stderr)
            if run.returncode:
                raise RuntimeError(f"{label} trial {trial}: {run.stderr[-2000:]}")
            result = json.loads(run.stdout)
            if result["alphabet"] != args.alphabet or result["source_reads"] != 148486:
                raise RuntimeError("unexpected fixture or source-read count")
            size = sum(path.stat().st_size for path in store.rglob("*") if path.is_file())
            records.append({"trial": trial, "label": label, "command": command,
                            "stored_bytes": size, "result": result})
            shutil.rmtree(store)
        print(trial, "pair complete", flush=True)
    medians = {label: {field: statistics.median(
        row["stored_bytes"] if field == "stored_bytes" else row["result"][field]
        for row in records if row["label"] == label)
        for field in ("flush_ns", "unchanged_flush_ns", "stored_bytes")}
        for label in programs}
    result = {"alphabet": args.alphabet, "trials": args.trials, "root": str(root),
              "program_sha256": {label: sha256(program) for label, program in programs.items()},
              "medians": medians, "records": records}
    (output / "summary.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(medians, indent=2))


if __name__ == "__main__":
    main()
