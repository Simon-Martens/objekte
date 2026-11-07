#!/usr/bin/env python3
"""Alternate one-worker and automatic fresh Musenalm builds."""

import json
import resource
import statistics
import subprocess
from pathlib import Path


HERE = Path(__file__).resolve().parent
REPO = HERE.parents[3]
BINARY = REPO / "target/release/examples/bench_build_musenalm"
SHA = "d15b689d1ac21709195b79fc382c045d8dec9867"


def main() -> None:
    actual = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip()
    if actual != SHA:
        raise ValueError(f"wrong measured revision: {actual}")
    samples = []
    with (HERE / "paired-musenalm-samples.jsonl").open("w") as output:
        for repetition in range(3):
            for alphabet in ("ascii", "unicode"):
                order = ("1", "auto") if repetition % 2 == 0 else ("auto", "1")
                for workers in order:
                    command = [str(BINARY), "--alphabet", alphabet,
                               "--snapshot-mode", "disabled", "--build-workers", workers]
                    before = resource.getrusage(resource.RUSAGE_CHILDREN)
                    completed = subprocess.run(command, cwd=REPO, text=True, capture_output=True)
                    after = resource.getrusage(resource.RUSAGE_CHILDREN)
                    if completed.returncode:
                        raise RuntimeError(f"{command}: {completed.stderr}")
                    sample = json.loads(completed.stdout)
                    if sample["records"] != 148486 or sample["build_parses"] != 148486 or sample["failed_parses"]:
                        raise ValueError("incomplete fresh build")
                    sample.update(repetition=repetition, command=command, measured_sha=SHA,
                                  requested_build_workers=workers,
                                  process_user_seconds=after.ru_utime-before.ru_utime,
                                  process_system_seconds=after.ru_stime-before.ru_stime)
                    samples.append(sample)
                    output.write(json.dumps(sample) + "\n")
                    output.flush()
                    print(f"{alphabet} {workers} #{repetition + 1}: {sample['wall_ns']/1e6:.1f} ms", flush=True)
    summary = {}
    for alphabet in ("ascii", "unicode"):
        group = [sample for sample in samples if sample["alphabet"].lower() == alphabet]
        if len({sample["index_signature_sha256"] for sample in group}) != 1:
            raise ValueError(f"semantic index mismatch: {alphabet}")
        for workers in ("1", "auto"):
            selected = [sample for sample in group if sample["requested_build_workers"] == workers]
            summary[f"{alphabet}-{workers}"] = {
                "median_wall_ms": statistics.median(sample["wall_ns"] for sample in selected) / 1e6,
                "min_wall_ms": min(sample["wall_ns"] for sample in selected) / 1e6,
                "max_wall_ms": max(sample["wall_ns"] for sample in selected) / 1e6,
                "median_peak_rss_bytes": statistics.median(sample["peak_resident_bytes"] for sample in selected),
                "resolved_build_workers": selected[0]["resolved_build_workers"],
                "index_signature_sha256": selected[0]["index_signature_sha256"],
            }
    (HERE / "paired-musenalm-summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print("\n| Alphabet | Workers | Median ms | Min–max ms | Peak RSS MiB |")
    print("|---|---:|---:|---:|---:|")
    for key, row in summary.items():
        print(f"| {key.split('-')[0]} | {row['resolved_build_workers']} | {row['median_wall_ms']:.1f} | "
              f"{row['min_wall_ms']:.1f}–{row['max_wall_ms']:.1f} | {row['median_peak_rss_bytes']/1048576:.1f} |")


if __name__ == "__main__":
    main()
