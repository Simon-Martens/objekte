#!/usr/bin/env python3
"""Alternate fresh-process incremental Musenalm construction trials."""

import argparse
import json
import statistics
import subprocess
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--before-bin", type=Path, required=True)
parser.add_argument("--after-bin", type=Path, required=True)
parser.add_argument("--before-cwd", type=Path, required=True)
parser.add_argument("--after-cwd", type=Path, required=True)
parser.add_argument("--root", type=Path, required=True)
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--pairs", type=int, default=3)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=False)
root = args.root.resolve(strict=True)
runs = []
for pair in range(args.pairs):
    order = ("before", "after") if pair % 2 == 0 else ("after", "before")
    for position, mode in enumerate(order):
        binary = (args.before_bin if mode == "before" else args.after_bin).resolve(strict=True)
        cwd = (args.before_cwd if mode == "before" else args.after_cwd).resolve(strict=True)
        result_root = args.output / f"{pair+1:02d}-{position+1}-{mode}"
        command = [str(binary), "--root", str(root), "--result-dir", str(result_root),
                   "--alphabet", "ascii", "--workers", "32", "--profile",
                   "--check-queries", "--incremental"]
        result = subprocess.run(command, cwd=cwd, capture_output=True, text=True)
        if result.returncode:
            (args.output / "failure.json").write_text(json.dumps({
                "command": command, "cwd": str(cwd), "status": result.returncode,
                "stdout": result.stdout, "stderr": result.stderr,
            }, indent=2))
            raise SystemExit(f"{mode} pair {pair+1} failed: {result.stderr[-1000:]}")
        artifact = Path(json.loads(result.stdout.strip().splitlines()[-1])["result_dir"])
        sample = json.loads((artifact / "sample.json").read_text())
        manifest = json.loads((artifact / "run-manifest.json").read_text())
        runs.append({"pair": pair+1, "position": position+1, "mode": mode,
                     "command": command, "cwd": str(cwd), "sample": sample,
                     "manifest": manifest})
        (args.output / "runs.json").write_text(json.dumps(runs, indent=2))
        print(f"{pair+1} {mode}: {sample['build_ns']/1e6:.3f} ms, "
              f"HWM {sample['after_build_hwm']/1e6:.3f} MB", flush=True)

def stats(values):
    q1, _, q3 = statistics.quantiles(values, n=4, method="inclusive")
    return {"median": statistics.median(values), "q1": q1, "q3": q3, "iqr": q3-q1}

summary = {}
for metric in ("build_ns", "discovery_ns", "after_build_hwm", "after_build_rss",
               "estimated_index_bytes", "build_parses"):
    summary[metric] = {mode: stats([r["sample"][metric] for r in runs if r["mode"] == mode])
                       for mode in ("before", "after")}
    summary[metric]["after_over_before"] = (
        summary[metric]["after"]["median"] / summary[metric]["before"]["median"] - 1)
(args.output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
print(json.dumps(summary, indent=2))
