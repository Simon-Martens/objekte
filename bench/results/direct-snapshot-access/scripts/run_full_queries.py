#!/usr/bin/env python3
"""Paired 220-case ASCII/Unicode query campaign across three storage modes."""

import argparse
import csv
import gzip
import hashlib
import json
import math
import os
import statistics
import subprocess
import time
from pathlib import Path


def digest(path):
    hash_state = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            hash_state.update(chunk)
    return hash_state.hexdigest()


def invoke(command, log):
    started = time.monotonic()
    run = subprocess.run(command, capture_output=True, text=True)
    log.write_text(run.stdout + "\nSTDERR\n" + run.stderr)
    if run.returncode:
        raise RuntimeError(f"{command[0]} failed ({run.returncode}): {log}")
    return time.monotonic() - started


def rows(path):
    with path.open(newline="") as source:
        values = list(csv.DictReader(source))
    result = {(row["case_id"], row["profile"]): row for row in values}
    if len(result) != len(values):
        raise RuntimeError(f"duplicate query rows in {path}")
    return result


def main():
    parser = argparse.ArgumentParser()
    for name in ("baseline-program", "candidate-program", "root", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--repetitions", type=int, default=30)
    args = parser.parse_args()
    if args.repetitions < 1 or args.output.exists():
        parser.error("repetitions must be positive and output must not exist")
    output = args.output.resolve()
    output.mkdir(parents=True)
    programs = {
        "baseline": args.baseline_program.resolve(),
        "candidate": args.candidate_program.resolve(),
    }
    root = args.root.resolve()
    common = [
        "--root", str(root), "--extended", "--incremental",
        "--profiles", "ascii-off,unicode-off", "--verified-cache-bytes", "0",
        "--prepared-cache-bytes", "16777216", "--query-workers", "32",
    ]
    runs = []
    for label, program in programs.items():
        store = output / f"{label}-store"
        store.mkdir()
        result = output / f"prime-{label}"
        command = [str(program), *common, "--repetitions", "1", "--cases", "Q001",
                   "--result-dir", str(result), "--storage", "custom", str(store)]
        elapsed = invoke(command, output / f"prime-{label}.log")
        runs.append({"label": label, "mode": "prime", "command": command, "seconds": elapsed})
        print(f"primed {label}: {elapsed:.1f}s", flush=True)
    all_rows = []
    summary = {}
    for mode in ("memory", "resident", "ondemand"):
        paired = {}
        for label, program in programs.items():
            result = output / f"{mode}-{label}"
            command = [str(program), *common, "--repetitions", str(args.repetitions),
                       "--result-dir", str(result)]
            if mode == "memory":
                command.extend(["--storage", "memory"])
            else:
                command.extend(["--storage", "custom", str(output / f"{label}-store")])
                if mode == "resident":
                    command.append("--fully-resident")
            elapsed = invoke(command, output / f"{mode}-{label}.log")
            opened = json.loads((result / "index-results.json").read_text())
            if mode != "memory" and any(row["source_reads"] or row["build_parses"] for row in opened):
                raise RuntimeError(f"{mode} {label} reopened from sources")
            runs.append({"label": label, "mode": mode, "command": command, "seconds": elapsed,
                         "binary_sha256": digest(program), "index_results": opened})
            paired[label] = rows(result / "query-results.csv")
            source = result / "measurements.jsonl"
            with source.open("rb") as raw, gzip.open(output / f"{mode}-{label}.jsonl.gz", "wb") as compressed:
                for chunk in iter(lambda: raw.read(1024 * 1024), b""):
                    compressed.write(chunk)
            print(f"{mode} {label}: {elapsed:.1f}s", flush=True)
        baseline, candidate = paired["baseline"], paired["candidate"]
        if baseline.keys() != candidate.keys() or len(baseline) != 440:
            raise RuntimeError(f"query case set differs in {mode}")
        if json.loads((output / f"{mode}-baseline" / "result-digests.json").read_text()) != json.loads(
            (output / f"{mode}-candidate" / "result-digests.json").read_text()
        ):
            raise RuntimeError(f"result digests differ in {mode}")
        mode_rows = []
        for case in sorted(baseline):
            old, new = baseline[case], candidate[case]
            if any(old[field] != new[field] for field in ("status", "total_matches", "returned_records")) \
                    or old["status"] != "ok" or int(old["verified_result_hits"]) != 0 \
                    or int(new["verified_result_hits"]) != 0:
                raise RuntimeError(f"query result or verified-cache mismatch: {mode} {case}")
            row = {"mode": mode, "case_id": case[0], "profile": case[1]}
            for field in ("median_ns", "p95_ns"):
                before, after = int(old[field]), int(new[field])
                row[f"baseline_{field}"] = before
                row[f"candidate_{field}"] = after
                row[f"ratio_{field}"] = after / max(before, 1)
            mode_rows.append(row)
        all_rows.extend(mode_rows)
        summary[mode] = {field: math.exp(statistics.mean(math.log(row[f"ratio_{field}"]) for row in mode_rows))
                         for field in ("median_ns", "p95_ns")}
        summary[mode]["pairs"] = len(mode_rows)
    with (output / "per-case.csv").open("w", newline="") as destination:
        writer = csv.DictWriter(destination, fieldnames=list(all_rows[0]), lineterminator="\n")
        writer.writeheader()
        writer.writerows(all_rows)
    governor = Path("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
    report = {
        "program_sha256": {name: digest(program) for name, program in programs.items()},
        "root": str(root), "repetitions": args.repetitions,
        "governor": governor.read_text().strip() if governor.exists() else None,
        "affinity": sorted(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None,
        "runs": runs, "geometric_mean_ratios": summary,
    }
    (output / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
