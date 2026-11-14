#!/usr/bin/env python3
"""Measure controlled index-page eviction before paired managed opens."""

import argparse
import json
import os
import statistics
import subprocess
from pathlib import Path


def index_files(store):
    return [path for path in store.rglob("*") if path.is_file() and path.suffix in (".idx", ".bin")]


def resident_bytes(files):
    if not files:
        return 0
    run = subprocess.run(["fincore", "-J", "-b", *(str(path) for path in files)],
                         capture_output=True, text=True, check=True)
    return sum(int(row["res"]) for row in json.loads(run.stdout)["fincore"])


def evict(files):
    for path in files:
        with path.open("rb") as source:
            os.posix_fadvise(source.fileno(), 0, 0, os.POSIX_FADV_DONTNEED)


def main():
    parser = argparse.ArgumentParser()
    for name in ("baseline-program", "candidate-program", "baseline-store",
                 "candidate-store", "root", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--trials", type=int, default=3)
    args = parser.parse_args()
    if args.trials < 1 or args.output.exists() or not hasattr(os, "posix_fadvise"):
        parser.error("positive trials, absent output, and POSIX_FADV_DONTNEED are required")
    output = args.output.resolve()
    output.mkdir(parents=True)
    programs = {label: getattr(args, label + "_program").resolve() for label in ("baseline", "candidate")}
    stores = {label: getattr(args, label + "_store").resolve() for label in programs}
    root = args.root.resolve()
    records = []
    for trial in range(1, args.trials + 1):
        labels = ("baseline", "candidate") if trial % 2 else ("candidate", "baseline")
        for label in labels:
            files = index_files(stores[label])
            if not files:
                raise RuntimeError(f"no managed index files in {stores[label]}")
            before = resident_bytes(files)
            evict(files)
            after_evict = resident_bytes(files)
            result = output / str(trial) / label
            command = [str(programs[label]), "--root", str(root),
                       "--result-dir", str(result), "--extended", "--incremental",
                       "--profiles", "ascii-off,unicode-off", "--repetitions", "1",
                       "--cases", "Q167", "--verified-cache-bytes", "0",
                       "--prepared-cache-bytes", "16777216", "--query-workers", "32",
                       "--storage", "custom", str(stores[label])]
            time_file = output / f"{trial}-{label}.time"
            run = subprocess.run(["/usr/bin/time", "-f",
                                  "major_faults=%F minor_faults=%R filesystem_inputs=%I filesystem_outputs=%O",
                                  "-o", str(time_file), *command], capture_output=True, text=True)
            result.mkdir(parents=True, exist_ok=True)
            (result / "stdout.log").write_text(run.stdout)
            (result / "stderr.log").write_text(run.stderr)
            if run.returncode:
                raise RuntimeError(f"{label} trial {trial}: {run.stderr[-2000:]}")
            index = json.loads((result / "index-results.json").read_text())
            query = [json.loads(line) for line in (result / "measurements.jsonl").read_text().splitlines()]
            if len(index) != 2 or len(query) != 2 or any(row["source_reads"] or row["build_parses"] for row in index):
                raise RuntimeError(f"{label} trial {trial} did not source-free open")
            records.append({
                "label": label, "trial": trial, "command": command,
                "index_file_bytes": sum(path.stat().st_size for path in files),
                "cached_before": before, "cached_after_fadvise": after_evict,
                "cached_after_query": resident_bytes(files),
                "time": time_file.read_text().strip(),
                "open_ns": {row["alphabet"]: row["open_ns"] for row in index},
                "first_query_ns": {row["profile"]["alphabet"]: row["total_ns"] for row in query},
                "digests": json.loads((result / "result-digests.json").read_text()),
            })
        print(trial, "pair complete", flush=True)
    first = records[0]["digests"]
    if any(record["digests"] != first for record in records):
        raise RuntimeError("cold result digest mismatch")
    medians = {}
    for label in programs:
        for alphabet in ("ascii", "unicode"):
            rows = [row for row in records if row["label"] == label]
            medians[f"{label}-{alphabet}"] = {
                "open_ns": statistics.median(row["open_ns"][alphabet] for row in rows),
                "first_query_ns": statistics.median(row["first_query_ns"][alphabet] for row in rows),
                "cached_after_fadvise": statistics.median(row["cached_after_fadvise"] for row in rows),
            }
    (output / "summary.json").write_text(json.dumps({"trials": args.trials, "medians": medians,
                                                      "records": records}, indent=2) + "\n")
    print(json.dumps(medians, indent=2))


if __name__ == "__main__":
    main()
