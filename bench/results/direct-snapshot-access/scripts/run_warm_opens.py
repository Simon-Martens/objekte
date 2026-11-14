#!/usr/bin/env python3
"""Measure source-free warm managed opens with a fixed benchmark command."""

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
    for name in ("program", "root", "store", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--label", required=True)
    parser.add_argument("--trials", type=int, default=7)
    args = parser.parse_args()
    if args.trials < 1:
        parser.error("trials must be positive")
    args.output.mkdir(parents=True, exist_ok=True)
    records = []
    for mode in ("ondemand", "resident"):
        for trial in range(1, args.trials + 1):
            folder = args.output / mode / str(trial)
            command = [
                str(args.program.resolve()), "--root", str(args.root.resolve()),
                "--result-dir", str(folder), "--extended", "--incremental",
                "--profiles", "ascii-off,unicode-off", "--repetitions", "1",
                "--cases", "Q167", "--verified-cache-bytes", "0",
                "--prepared-cache-bytes", "16777216", "--query-workers", "32",
                "--storage", "custom", str(args.store.resolve()),
            ]
            if mode == "resident":
                command.append("--fully-resident")
            run = subprocess.run(command, capture_output=True, text=True)
            folder.mkdir(parents=True, exist_ok=True)
            (folder / "stdout.log").write_text(run.stdout)
            (folder / "stderr.log").write_text(run.stderr)
            if run.returncode:
                raise RuntimeError(f"{mode} trial {trial}: {run.stderr[-2000:]}")
            index = json.loads((folder / "index-results.json").read_text())
            query = [json.loads(line) for line in (folder / "measurements.jsonl").read_text().splitlines()]
            if len(index) != 2 or len(query) != 2:
                raise RuntimeError("expected ASCII and Unicode profiles")
            if any(row["source_reads"] or row["build_parses"] for row in index):
                raise RuntimeError("benchmark opened by rebuilding from sources")
            records.append({
                "mode": mode, "trial": trial, "command": command,
                "open_ns": {row["alphabet"]: row["open_ns"] for row in index},
                "first_query_ns": {row["profile"]["alphabet"]: row["total_ns"] for row in query},
                "resident_bytes": {row["alphabet"]: row["resident_bytes"] for row in index},
                "peak_resident_bytes": {row["alphabet"]: row["peak_resident_bytes"] for row in index},
                "managed_bytes": {row["alphabet"]: row["managed_bytes"] for row in index},
                "digests": json.loads((folder / "result-digests.json").read_text()),
            })
            print(args.label, mode, trial, "done", flush=True)
    first = records[0]["digests"]
    if any(record["digests"] != first for record in records):
        raise RuntimeError("digest mismatch across trials")
    summary = {}
    for mode in ("ondemand", "resident"):
        for alphabet in ("ascii", "unicode"):
            rows = [row for row in records if row["mode"] == mode]
            summary[f"{mode}-{alphabet}"] = {
                field: statistics.median(row[field][alphabet] for row in rows)
                for field in ("open_ns", "first_query_ns", "resident_bytes", "peak_resident_bytes", "managed_bytes")
            }
    output = {
        "label": args.label, "program_sha256": sha256(args.program),
        "root": str(args.root.resolve()), "store": str(args.store.resolve()),
        "trials": args.trials, "source_free": True,
        "summary_medians": summary, "records": records,
    }
    (args.output / "summary.json").write_text(json.dumps(output, indent=2) + "\n")
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
