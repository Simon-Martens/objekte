#!/usr/bin/env python3
"""Run the 150-sample iteration gate or the full 3,000-sample query gate."""

from __future__ import annotations

import argparse
import csv
import json
import platform
import subprocess
import sys
from collections import defaultdict
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
BASELINE = ROOT / "bench/results/musenalm/final-8dd0c2aa6d"
JQ_COUNTS = ROOT / "bench/baselines/musenalm-jq-counts.json"
PROFILES = ("ascii-off", "ascii-warm", "unicode-off", "unicode-warm")


def rows(path: Path) -> dict[tuple[str, str], dict[str, str]]:
    with path.open(newline="") as file:
        values = list(csv.DictReader(file))
    result = {(row["case_id"], row["profile"]): row for row in values}
    if len(result) != len(values):
        raise ValueError(f"duplicate query rows in {path}")
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("quick", "full"))
    parser.add_argument("--result-dir", type=Path)
    parser.add_argument("--baseline", type=Path, default=BASELINE)
    parser.add_argument("--verify-only", action="store_true", help="check an existing completed run")
    args = parser.parse_args()
    revision = subprocess.check_output(["git", "rev-parse", "--short=10", "HEAD"], cwd=ROOT, text=True).strip()
    result_dir = (args.result_dir or ROOT / "bench/results/musenalm" / f"{args.mode}-{revision}").resolve()
    result_dir.mkdir(parents=True, exist_ok=True)
    profiles = PROFILES if args.mode == "full" else PROFILES[:1]
    repetitions = 5 if args.mode == "full" else 1
    command = [
        "cargo", "run", "--release", "--example", "bench_musenalm", "--",
        "--profiles", ",".join(profiles), "--repetitions", str(repetitions),
        "--result-dir", str(result_dir),
    ]
    if not args.verify_only:
        with (result_dir / "console.log").open("w") as console:
            completed = subprocess.run(command, cwd=ROOT, stdout=console, stderr=subprocess.STDOUT)
        if completed.returncode:
            print(f"benchmark failed: see {result_dir / 'console.log'}", file=sys.stderr)
            return completed.returncode
    baseline = args.baseline.resolve()
    old_manifest = json.loads((baseline / "run-manifest.json").read_text())
    new_manifest = json.loads((result_dir / "run-manifest.json").read_text())
    old_rows = rows(baseline / "query-results.csv")
    new_rows = rows(result_dir / "query-results.csv")
    jq_counts = json.loads(JQ_COUNTS.read_text())["counts"]
    expected_keys = {(f"Q{index:03}", profile) for index in range(1, 151) for profile in profiles}
    failures = []
    if set(new_rows) != expected_keys:
        failures.append(f"query row keys differ: missing={sorted(expected_keys - set(new_rows))}, extra={sorted(set(new_rows) - expected_keys)}")
    if set(jq_counts) != {f"Q{index:03}" for index in range(1, 151)}:
        failures.append("independent jq count baseline does not cover all 150 cases")
    if new_manifest["repetitions"] != repetitions or new_manifest["failures"] != 0:
        failures.append("benchmark repetition or failure count invalid")
    for key in ("corpus_fingerprint", "schema_fingerprint", "workload_fingerprint"):
        if old_manifest[key] != new_manifest[key]:
            failures.append(f"{key} differs from baseline")
    samples = [json.loads(line) for line in (result_dir / "measurements.jsonl").read_text().splitlines()]
    if len(samples) != 150 * len(profiles) * repetitions:
        failures.append(f"sample count {len(samples)} differs from {150 * len(profiles) * repetitions}")
    groups: dict[tuple[str, str], list[dict]] = defaultdict(list)
    for sample in samples:
        profile = sample["profile"]
        name = profile["alphabet"] + ("-warm" if profile["warm"] else "-off")
        groups[(sample["case_id"], name)].append(sample)
        if sample["status"] != "ok":
            failures.append(f"{sample['case_id']} {name}: {sample['status']} {sample['error']}")
    for key in expected_keys:
        if len(groups[key]) != repetitions:
            failures.append(f"{key}: expected {repetitions} samples, found {len(groups[key])}")
    for key in expected_keys & set(new_rows):
        old = old_rows[key]
        new = new_rows[key]
        for field in ("status", "total_matches", "returned_records"):
            if old[field] != new[field]:
                failures.append(f"{key}: {field} baseline={old[field]} actual={new[field]}")
        if int(new["total_matches"]) != jq_counts[key[0]]:
            failures.append(f"{key}: count {new['total_matches']} differs from jq {jq_counts[key[0]]}")
    cpu_model = next(
        (line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")),
        "unknown",
    )
    same_machine = old_manifest["environment"]["cpu_model"] == cpu_model and platform.machine() == old_manifest["environment"]["architecture"]
    relative = 1.5 if args.mode == "full" else 1.8
    absolute_ns = 2_000_000 if args.mode == "full" else 5_000_000
    slowdowns = []
    for key in sorted(expected_keys & set(new_rows)):
        old_ns = int(old_rows[key]["median_ns"])
        new_ns = int(new_rows[key]["median_ns"])
        slowdowns.append((new_ns / max(old_ns, 1), key, old_ns, new_ns))
        if same_machine and new_ns > max(old_ns * relative, old_ns + absolute_ns):
            failures.append(f"{key}: {new_ns / 1e6:.3f} ms exceeds {old_ns / 1e6:.3f} ms baseline")
    old_total = sum(int(old_rows[key]["median_ns"]) for key in expected_keys)
    new_total = sum(int(new_rows[key]["median_ns"]) for key in expected_keys & set(new_rows))
    aggregate_limit = 1.25 if args.mode == "full" else 1.35
    if same_machine and new_total > old_total * aggregate_limit:
        failures.append(f"aggregate latency ratio {new_total / old_total:.3f} exceeds {aggregate_limit}")
    result = {
        "mode": args.mode, "expected_samples": 150 * len(profiles) * repetitions,
        "jq_count_baseline": str(JQ_COUNTS),
        "actual_samples": len(samples), "baseline": str(baseline),
        "result_dir": str(result_dir), "same_machine": same_machine,
        "aggregate_latency_ratio": new_total / old_total,
        "thresholds": {"per_case_relative": relative, "per_case_absolute_ns": absolute_ns, "aggregate_relative": aggregate_limit},
        "worst_slowdowns": [
            {"ratio": ratio, "case": key[0], "profile": key[1], "baseline_ns": old_ns, "actual_ns": new_ns}
            for ratio, key, old_ns, new_ns in sorted(slowdowns, reverse=True)[:20]
        ],
        "failures": failures,
        "command": command,
    }
    (result_dir / "gate.json").write_text(json.dumps(result, indent=2) + "\n")
    print(f"{args.mode}: {len(samples)} samples, aggregate ratio {new_total / old_total:.3f}, failures {len(failures)}")
    print(result_dir)
    for failure in failures[:20]:
        print(failure, file=sys.stderr)
    return int(bool(failures))


if __name__ == "__main__":
    sys.exit(main())
