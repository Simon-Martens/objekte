#!/usr/bin/env python3
"""Benchmark fresh Musenalm index builds in four configurations."""

from __future__ import annotations

import argparse
import json
import statistics
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
BASELINE = ROOT / "bench/baselines/musenalm-build.json"
def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--result-dir", type=Path)
    parser.add_argument("--baseline", type=Path, default=BASELINE)
    parser.add_argument("--establish-baseline", action="store_true")
    args = parser.parse_args()
    if args.repetitions < 3:
        parser.error("at least three fresh-process repetitions are required")
    revision = subprocess.check_output(["git", "rev-parse", "--short=10", "HEAD"], cwd=ROOT, text=True).strip()
    result_dir = (args.result_dir or ROOT / "bench/results/musenalm" / f"build-{revision}").resolve()
    result_dir.mkdir(parents=True, exist_ok=True)
    binary = ROOT / "target/release/examples/bench_build_musenalm"
    with (result_dir / "build.log").open("w") as log:
        completed = subprocess.run(["cargo", "build", "--release", "--example", "bench_build_musenalm"], cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
    if completed.returncode:
        print(f"build benchmark compilation failed: {result_dir / 'build.log'}", file=sys.stderr)
        return completed.returncode
    configurations = tuple((alphabet, snapshot) for alphabet in ("ascii", "unicode") for snapshot in ("disabled", "rebuild"))
    samples: dict[str, list[dict]] = {f"{alphabet}-{snapshot}": [] for alphabet, snapshot in configurations}
    with (result_dir / "measurements.jsonl").open("w") as output:
        for repetition in range(args.repetitions):
            for alphabet, snapshot in configurations:
                command = [str(binary), "--alphabet", alphabet, "--snapshot-mode", snapshot]
                completed = subprocess.run(command, cwd=ROOT, text=True, capture_output=True)
                if completed.returncode:
                    raise RuntimeError(f"{command}: {completed.stderr}")
                sample = json.loads(completed.stdout)
                sample["repetition"] = repetition
                if sample["records"] != 148486 or sample["build_parses"] != 148486:
                    raise ValueError(f"incomplete build: {sample}")
                key = f"{alphabet}-{snapshot}"
                samples[key].append(sample)
                output.write(json.dumps(sample) + "\n")
                output.flush()
                print(f"{key} #{repetition + 1}: {sample['wall_ns'] / 1e9:.3f}s", flush=True)
    summary = {}
    for name, group in samples.items():
        summary[name] = {
            "median_wall_ns": int(statistics.median(sample["wall_ns"] for sample in group)),
            "median_build_ns": int(statistics.median(sample["build_ns"] for sample in group)),
            "median_snapshot_write_ns": int(statistics.median(sample["snapshot_write_ns"] for sample in group)),
            "median_peak_resident_bytes": int(statistics.median(sample["peak_resident_bytes"] for sample in group)),
            "snapshot_bytes": group[0]["snapshot_bytes"],
            "samples": len(group),
        }
    environment = {
        "cpu_model": next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")), "unknown"),
        "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "revision": revision,
    }
    failures = []
    baseline_path = args.baseline.resolve()
    if args.establish_baseline:
        baseline_path.parent.mkdir(parents=True, exist_ok=True)
        if baseline_path.exists():
            raise FileExistsError(f"baseline exists: {baseline_path}")
        baseline_path.write_text(json.dumps({"environment": environment, "summary": summary}, indent=2) + "\n")
    else:
        baseline = json.loads(baseline_path.read_text())
        if baseline["environment"]["cpu_model"] == environment["cpu_model"]:
            for name, current in summary.items():
                previous = baseline["summary"][name]
                if current["median_wall_ns"] > max(previous["median_wall_ns"] * 1.5, previous["median_wall_ns"] + 2_000_000_000):
                    failures.append(f"{name}: build {current['median_wall_ns'] / 1e9:.3f}s exceeds baseline {previous['median_wall_ns'] / 1e9:.3f}s")
                if current["median_peak_resident_bytes"] > max(previous["median_peak_resident_bytes"] * 1.3, previous["median_peak_resident_bytes"] + 64 * 1024 * 1024):
                    failures.append(f"{name}: peak RSS exceeds baseline")
                if current["snapshot_bytes"] and current["snapshot_bytes"] > previous["snapshot_bytes"] * 1.1:
                    failures.append(f"{name}: snapshot size exceeds baseline")
    report = {"environment": environment, "summary": summary, "baseline": str(baseline_path), "failures": failures}
    (result_dir / "gate.json").write_text(json.dumps(report, indent=2) + "\n")
    for failure in failures:
        print(failure, file=sys.stderr)
    return int(bool(failures))


if __name__ == "__main__":
    sys.exit(main())
