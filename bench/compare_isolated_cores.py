#!/usr/bin/env python3
"""Compare schema-enabled cores in isolated processes, with ABBA run order."""

from __future__ import annotations

import argparse
import csv
import json
import statistics
import subprocess
import sys
from collections import defaultdict
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
PACKAGE = ROOT / "bench/old-core-compare"


def read_run(path: Path) -> tuple[dict, dict[str, dict]]:
    entries = [json.loads(line) for line in path.read_text().splitlines()]
    if len(entries) != 35 or entries[0]["type"] != "setup":
        raise ValueError(f"expected one setup and 34 cases in {path}")
    cases = {entry["case"]: entry for entry in entries[1:]}
    if len(cases) != 34 or any(entry["type"] != "case" for entry in entries[1:]):
        raise ValueError(f"duplicate or malformed cases in {path}")
    if any(len(entry["samples_ns"]) != 5 or entry["core"] != entries[0]["core"] for entry in entries[1:]):
        raise ValueError(f"expected five timed repetitions per case in {path}")
    return entries[0], cases


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--result-dir", type=Path, required=True)
    args = parser.parse_args()
    result_dir = args.result_dir.resolve()
    result_dir.mkdir(parents=True, exist_ok=True)
    with (result_dir / "build.log").open("w") as log:
        built = subprocess.run(
            ["cargo", "build", "--release", "--manifest-path", str(PACKAGE / "Cargo.toml"), "--bins"],
            cwd=ROOT, stdout=log, stderr=subprocess.STDOUT,
        )
    if built.returncode:
        raise RuntimeError(f"benchmark binaries failed to build; see {result_dir / 'build.log'}")
    runs = []
    for index, core in enumerate(("old", "new", "new", "old"), 1):
        binary = PACKAGE / "target/release" / f"{core}_only"
        output = result_dir / f"run-{index:02}-{core}.jsonl"
        with output.open("w") as file, (result_dir / f"run-{index:02}-{core}.log").open("w") as log:
            completed = subprocess.run([str(binary)], cwd=ROOT, stdout=file, stderr=log)
        if completed.returncode:
            raise RuntimeError(f"{core} benchmark failed: {result_dir / f'run-{index:02}-{core}.log'}")
        setup, cases = read_run(output)
        if setup["core"] != core or setup["schemas"] != 7:
            raise ValueError(f"schema setup invalid in {output}")
        runs.append((core, setup, cases))
        print(f"finished {core} isolated process {index}/4", flush=True)
    expected_paths = runs[0][1]["indexed_paths"]
    if any(setup["indexed_paths"] != expected_paths for _, setup, _ in runs):
        raise ValueError("old and new configured indexed path sets differ")
    for core, setup, _ in runs:
        if setup["alphabet"] != "ascii" or setup["return_mode"] != "logical" or setup["page_limit"] != 50 or setup["sort"] != "record ID ascending":
            raise ValueError(f"{core} comparison request differs from the common setup")
        if core == "old" and setup["query_cache_bytes"] != 0:
            raise ValueError("old verified query cache must be disabled")
        if core == "new" and setup["verified_cache_bytes"] != 0:
            raise ValueError("new verified query cache must be disabled")
    indexed_count = sum(len(paths) for paths in expected_paths.values())
    if indexed_count != 89:
        raise ValueError(f"expected 89 identical indexed paths, found {indexed_count}")
    cases = sorted(runs[0][2])
    if any(sorted(entries) != cases for _, _, entries in runs):
        raise ValueError("case sets differ between processes")
    timing: dict[str, dict[str, list[int]]] = defaultdict(lambda: defaultdict(list))
    baseline: dict[str, tuple[int, list[str]]] = {}
    for core, _, entries in runs:
        for case, entry in entries.items():
            identity = (entry["count"], entry["ids"])
            if case in baseline and baseline[case] != identity:
                raise ValueError(f"{case} count or ordered page differs across cores/runs")
            baseline[case] = identity
            timing[case][core].extend(entry["samples_ns"])
    output_rows = []
    for case in cases:
        old_ns = int(statistics.median(timing[case]["old"]))
        new_ns = int(statistics.median(timing[case]["new"]))
        output_rows.append({"case": case, "matches": baseline[case][0], "old_median_ns": old_ns,
                            "new_median_ns": new_ns, "old_over_new": old_ns / new_ns,
                            "old_samples_ns": timing[case]["old"], "new_samples_ns": timing[case]["new"]})
    with (result_dir / "timings.csv").open("w", newline="") as file:
        writer = csv.DictWriter(file, fieldnames=("case", "matches", "old_median_ns", "new_median_ns", "old_over_new"), lineterminator="\n")
        writer.writeheader()
        writer.writerows({key: row[key] for key in writer.fieldnames} for row in output_rows)
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    ratios = [row["old_over_new"] for row in output_rows]
    summary = {
        "revision": revision, "cases": 34, "parity_failures": 0, "indexed_paths": indexed_count,
        "process_order": [core for core, _, _ in runs], "samples_per_case_per_core": 10,
        "old_faster": sum(ratio < 1 for ratio in ratios), "new_faster": sum(ratio > 1 for ratio in ratios),
        "median_old_over_new": statistics.median(ratios), "timings": output_rows,
    }
    (result_dir / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    lines = [
        "# Isolated archived-core comparison", "",
        f"Revision: `{revision}`. All 34 exact counts and ordered 50-record pages match.",
        "The old and new cores load the same seven Musenalm schemas and exactly the same 89 indexed paths.",
        "Both use ASCII bigrams, exact counts, logical payloads, ID order, and disabled verified query caches.",
        "Each core runs alone in a process twice, in old/new/new/old order, with five timed repetitions per process.",
        "Index loading and warmup occur before query timing. The old core uses Memory repository mode; the new core uses 32 query workers.",
        f"Median old/new ratio across cases: {statistics.median(ratios):.2f}×. Old faster: {summary['old_faster']}; new faster: {summary['new_faster']}.",
        "", "| Case | Matches | Old ms | New ms | Old / new |", "|---|---:|---:|---:|---:|",
    ]
    for row in output_rows:
        lines.append(f"| {row['case']} | {row['matches']} | {row['old_median_ns']/1e6:.3f} | {row['new_median_ns']/1e6:.3f} | {row['old_over_new']:.2f}× |")
    lines += ["", "Raw process outputs and exact sample durations are retained in this directory.", ""]
    (result_dir / "REPORT.md").write_text("\n".join(lines))
    print(f"34 parity cases; median old/new {statistics.median(ratios):.2f}×")
    return 0


if __name__ == "__main__":
    sys.exit(main())
