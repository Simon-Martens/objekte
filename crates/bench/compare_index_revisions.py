#!/usr/bin/env python3
"""Compare two builds of the identical profile_indexes driver, in fresh caches."""
import argparse
import json
import statistics
import subprocess
import tempfile
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    parser.add_argument("repository", type=Path)
    parser.add_argument("workload", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--samples", type=int, default=3)
    parser.add_argument("--repeats", type=int, default=3)
    args = parser.parse_args()
    if min(args.samples, args.repeats) < 1:
        parser.error("samples/repeats must be positive")
    args.output.mkdir(parents=True, exist_ok=True)
    samples = {"before": [], "after": []}
    expected = {}
    for sample in range(args.samples):
        order = ["before", "after"] if sample % 2 == 0 else ["after", "before"]
        for label in order:
            with tempfile.TemporaryDirectory(prefix="objekte-index-comparison-") as cache:
                command = [str(getattr(args, label).resolve()), str(args.repository.resolve()), cache,
                           str(args.workload.resolve()), str(args.repeats)]
                output = subprocess.check_output(command, text=True)
            rows = [json.loads(line) for line in output.splitlines()]
            for row in rows:
                name = row["case"]
                assert row["digest"] == expected.setdefault(name, row["digest"]), (label, sample, name)
            samples[label].extend(rows)
            (args.output / f"{label}-{sample}.jsonl").write_text(output)
            print(f"{label} sample {sample + 1}: {len(rows)} matching results", flush=True)
    report = {"samples": args.samples, "warm_repeats_per_sample": args.repeats, "cases": {}}
    for name in expected:
        report["cases"][name] = {}
        for label, rows in samples.items():
            report["cases"][name][label] = {
                phase + "_ms": statistics.median(row["elapsed_ms"] for row in rows
                                                  if row["case"] == name and row["phase"] == phase)
                for phase in ["first", "warm", "reopen"]
            }
    (args.output / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
