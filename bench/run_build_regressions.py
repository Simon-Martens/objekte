#!/usr/bin/env python3
"""Measure fresh Musenalm in-memory builds without legacy snapshot modes."""

from __future__ import annotations

import argparse
import json
import os
import statistics
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--result-dir", type=Path, required=True)
    parser.add_argument("--baseline", type=Path, help="prior gate.json from this script")
    parser.add_argument("--build-workers", default="auto")
    parser.add_argument("--binary", type=Path)
    args = parser.parse_args()
    if args.repetitions < 1:
        parser.error("repetitions must be positive")
    if args.build_workers != "auto" and (
        not args.build_workers.isdecimal() or int(args.build_workers) < 1
    ):
        parser.error("build workers must be auto or a positive integer")
    result_dir = args.result_dir.resolve()
    if result_dir.exists() and any(result_dir.iterdir()):
        parser.error(f"result directory is not empty: {result_dir}")
    result_dir.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    binary = (args.binary or target / "release/examples/bench_build_musenalm").resolve()
    if not binary.is_file():
        with (result_dir / "build.log").open("w") as log:
            built = subprocess.run(
                ["cargo", "build", "--release", "-p", "objekte-search-core",
                 "--example", "bench_build_musenalm"],
                cwd=ROOT, stdout=log, stderr=subprocess.STDOUT,
            )
        if built.returncode:
            print(f"benchmark compilation failed: {result_dir / 'build.log'}", file=sys.stderr)
            return built.returncode
    revision = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
    ).strip()
    samples: dict[str, list[dict]] = {"ascii": [], "unicode": []}
    with (result_dir / "measurements.jsonl").open("w") as output:
        for repetition in range(args.repetitions):
            for alphabet in samples:
                command = [
                    str(binary), "--alphabet", alphabet,
                    "--build-workers", args.build_workers,
                    "--schema-manifest-path", str(result_dir / "schema-manifest.json"),
                ]
                completed = subprocess.run(command, cwd=ROOT, text=True, capture_output=True)
                if completed.returncode:
                    raise RuntimeError(f"{command}: {completed.stderr}")
                sample = json.loads(completed.stdout)
                if sample["records"] != 148486 or sample["build_parses"] != 148486:
                    raise ValueError(f"incomplete {alphabet} build")
                if sample.get("failed_parses", 0):
                    raise ValueError(f"failed parses in {alphabet} build")
                sample["repetition"] = repetition
                sample["command"] = command
                samples[alphabet].append(sample)
                output.write(json.dumps(sample) + "\n")
                output.flush()
                print(f"{alphabet} #{repetition + 1}: {sample['wall_ns'] / 1e9:.3f}s", flush=True)
    summary = {}
    for alphabet, group in samples.items():
        for field in ("schema_manifest_digest", "index_signature_sha256"):
            if len({sample[field] for sample in group}) != 1:
                raise ValueError(f"{alphabet} {field} changed between builds")
        summary[alphabet] = {
            "samples": len(group),
            "median_wall_ns": int(statistics.median(s["wall_ns"] for s in group)),
            "median_peak_resident_bytes": int(
                statistics.median(s["peak_resident_bytes"] for s in group)
            ),
            "min_wall_ns": min(s["wall_ns"] for s in group),
            "max_wall_ns": max(s["wall_ns"] for s in group),
            "index_signature_sha256": group[0]["index_signature_sha256"],
        }
    failures = []
    if args.baseline:
        baseline = json.loads(args.baseline.read_text())
        for alphabet, current in summary.items():
            old = baseline["summary"][alphabet]
            if current["index_signature_sha256"] != old["index_signature_sha256"]:
                failures.append(f"{alphabet}: index signature changed")
            if current["median_wall_ns"] > max(
                old["median_wall_ns"] * 1.5, old["median_wall_ns"] + 2_000_000_000
            ):
                failures.append(f"{alphabet}: build wall time exceeded baseline")
            if current["median_peak_resident_bytes"] > max(
                old["median_peak_resident_bytes"] * 1.3,
                old["median_peak_resident_bytes"] + 64 * 1024 * 1024,
            ):
                failures.append(f"{alphabet}: peak RSS exceeded baseline")
    report = {
        "revision": revision,
        "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "binary": str(binary),
        "baseline": str(args.baseline) if args.baseline else None,
        "summary": summary,
        "failures": failures,
    }
    (result_dir / "gate.json").write_text(json.dumps(report, indent=2) + "\n")
    lines = [
        "# Musenalm fresh in-memory builds", "",
        f"Revision: `{revision}`. Repetitions: {args.repetitions}.",
        "", "| Alphabet | Median s | Range s | Peak RSS MiB |",
        "| --- | ---: | ---: | ---: |",
    ]
    for alphabet, row in summary.items():
        lines.append(
            f"| {alphabet} | {row['median_wall_ns']/1e9:.3f} | "
            f"{row['min_wall_ns']/1e9:.3f}–{row['max_wall_ns']/1e9:.3f} | "
            f"{row['median_peak_resident_bytes']/1048576:.1f} |"
        )
    lines += ["", f"Failures: {len(failures)}.", *[f"- {failure}" for failure in failures]]
    (result_dir / "REPORT.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))
    return int(bool(failures))


if __name__ == "__main__":
    sys.exit(main())
