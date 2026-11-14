#!/usr/bin/env python3
"""Paired disposable first-update-after-reopen locality measurements."""

import argparse
import hashlib
import json
import os
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


def fixture(root, count):
    data = root / "data"
    data.mkdir(parents=True)
    for number in range(count):
        (data / f"{number:06}.jsonl").write_text(
            json.dumps({"id": f"item-{number:06}", "name": f"name-{number:06}"}) + "\n"
        )


def parse_rows(text):
    lines = text.splitlines()
    if not lines or lines[0].split("\t")[:2] != ["phase", "ms"]:
        raise RuntimeError("mutation benchmark header missing")
    rows = []
    for line in lines[1:]:
        parts = line.split("\t")
        if len(parts) != 8:
            raise RuntimeError(f"malformed mutation row: {line}")
        rows.append({
            "phase": parts[0], "ms": float(parts[1]), "rss_kb": int(parts[2]),
            "hwm_kb": int(parts[3]), "revision": int(parts[4]),
            "files": int(parts[5]), "records": int(parts[6]), "detail": parts[7],
        })
    return rows


def one(rows, phase, sample=None):
    found = [row for row in rows if row["phase"] == phase
             and (sample is None or f"sample={sample};" in row["detail"])]
    if len(found) != 1:
        raise RuntimeError(f"expected one {phase} sample {sample}, found {len(found)}")
    return found[0]


def main():
    parser = argparse.ArgumentParser()
    for name in ("baseline-program", "candidate-program", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--files", type=int, default=16384)
    parser.add_argument("--trials", type=int, default=3)
    args = parser.parse_args()
    if args.files < 1 or args.trials < 1 or args.output.exists():
        parser.error("positive file/trial count and absent output are required")
    output = args.output.resolve()
    output.mkdir(parents=True)
    programs = {label: getattr(args, label + "_program").resolve()
                for label in ("baseline", "candidate")}
    records = []
    for trial in range(1, args.trials + 1):
        labels = ("baseline", "candidate") if trial % 2 else ("candidate", "baseline")
        for label in labels:
            root = output / "fixtures" / str(trial) / label
            fixture(root, args.files)
            command = [str(programs[label]), "locality", str(root)]
            env = dict(os.environ, OBJEKTE_BENCH_MUTATE_COPY="1",
                       OBJEKTE_BENCH_RESTORE_LOCALITY="1")
            run = subprocess.run(command, env=env, capture_output=True, text=True)
            log = output / f"{trial}-{label}"
            log.with_suffix(".stdout").write_text(run.stdout)
            log.with_suffix(".stderr").write_text(run.stderr)
            if run.returncode:
                raise RuntimeError(f"{label} trial {trial}: {run.stderr[-2000:]}")
            rows = parse_rows(run.stdout)
            first = one(rows, "verified_original_update", 0)
            count = one(rows, "count")
            if f"count={args.files}" not in count["detail"] or first["files"] != args.files:
                raise RuntimeError("mutation changed file count")
            records.append({"trial": trial, "label": label, "command": command, "rows": rows,
                            "build_ms": one(rows, "build")["ms"],
                            "initial_flush_ms": one(rows, "flush")["ms"],
                            "restore_ms": one(rows, "restore")["ms"],
                            "first_update_and_flush_ms": first["ms"],
                            "first_update_detail": first["detail"],
                            "reopened_rss_kb": one(rows, "restore")["rss_kb"]})
            shutil.rmtree(root)
        print(trial, "pair complete", flush=True)
    medians = {label: {field: statistics.median(row[field] for row in records
                                               if row["label"] == label)
                       for field in ("build_ms", "initial_flush_ms", "restore_ms",
                                     "first_update_and_flush_ms", "reopened_rss_kb")}
               for label in programs}
    result = {"files": args.files, "trials": args.trials,
              "program_sha256": {label: sha256(program) for label, program in programs.items()},
              "medians": medians, "records": records}
    (output / "summary.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(medians, indent=2))


if __name__ == "__main__":
    main()
