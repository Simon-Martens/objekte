#!/usr/bin/env python3
"""Measure paired one/automatic fresh builds on generated source layouts."""

from __future__ import annotations

import argparse
import csv
import json
import os
import resource
import statistics
import subprocess
from pathlib import Path


HERE = Path(__file__).resolve().parent
REPO = HERE.parents[3]
BINARY = REPO / "target/release/examples/bench_build_musenalm"


def verify_fixture(root: Path, manifest: dict) -> None:
    import hashlib

    schema = root / ".objekte/collections/fixture.schema.json"
    if hashlib.sha256(schema.read_bytes()).hexdigest() != manifest["schema_sha256"]:
        raise ValueError("fixture schema changed")
    for name, digest in manifest["source_sha256"].items():
        if hashlib.sha256((root / name).read_bytes()).hexdigest() != digest:
            raise ValueError(f"fixture source changed: {name}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--large-root", type=Path, required=True)
    parser.add_argument("--mixed-root", type=Path, required=True)
    parser.add_argument("--expected-sha", required=True)
    args = parser.parse_args()
    sha = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip()
    if not sha.startswith(args.expected_sha):
        raise ValueError(f"wrong measured revision: {sha}")
    roots = {"large": args.large_root, "mixed": args.mixed_root}
    manifests = {name: json.loads((HERE / f"fixture-{name}.json").read_text()) for name in roots}
    for name, root in roots.items():
        verify_fixture(root, manifests[name])
    samples = []
    with (HERE / "layout-samples.jsonl").open("w") as output:
        for repetition in range(3):
            for name, root in roots.items():
                for alphabet in ("ascii", "unicode"):
                    for workers in (("1", "auto") if repetition % 2 == 0 else ("auto", "1")):
                        command = [str(BINARY), "--root", str(root), "--snapshot-mode", "disabled",
                                   "--alphabet", alphabet, "--build-workers", workers]
                        before = resource.getrusage(resource.RUSAGE_CHILDREN)
                        completed = subprocess.run(command, cwd=REPO, text=True, capture_output=True)
                        after = resource.getrusage(resource.RUSAGE_CHILDREN)
                        if completed.returncode:
                            raise RuntimeError(f"{command}: {completed.stderr}")
                        sample = json.loads(completed.stdout)
                        if sample["records"] != manifests[name]["records"] or sample["build_parses"] != manifests[name]["records"]:
                            raise ValueError("incomplete fixture build")
                        if sample["failed_parses"] or sample["snapshot_bytes"] is not None:
                            raise ValueError("fixture build was not fresh and complete")
                        sample.update(layout=name, requested_build_workers=workers, repetition=repetition,
                                      command=command, measured_sha=sha,
                                      process_user_seconds=after.ru_utime-before.ru_utime,
                                      process_system_seconds=after.ru_stime-before.ru_stime)
                        samples.append(sample)
                        output.write(json.dumps(sample) + "\n")
                        output.flush()
                        print(f"{name} {alphabet} {workers} #{repetition + 1}: {sample['wall_ns']/1e6:.1f} ms", flush=True)
    rows = []
    for name in roots:
        for alphabet in ("ascii", "unicode"):
            compared = [sample for sample in samples if sample["layout"] == name and sample["alphabet"].lower() == alphabet]
            if len({sample["schema_manifest_digest"] for sample in compared}) != 1:
                raise ValueError("schema identity changed within fixture")
            if len({sample["distinct_gram_keys"] for sample in compared}) != 1:
                raise ValueError("index gram count differs by worker count")
            if len({sample["index_signature_sha256"] for sample in compared}) != 1:
                raise ValueError("semantic index signature differs by worker count")
            for workers in ("1", "auto"):
                group = [sample for sample in compared if sample["requested_build_workers"] == workers]
                rows.append({
                    "layout": name, "alphabet": alphabet, "workers": workers,
                    "resolved_workers": group[0]["resolved_build_workers"],
                    "median_wall_ms": statistics.median(sample["wall_ns"] for sample in group) / 1e6,
                    "min_wall_ms": min(sample["wall_ns"] for sample in group) / 1e6,
                    "max_wall_ms": max(sample["wall_ns"] for sample in group) / 1e6,
                    "median_cpu_seconds": statistics.median(sample["process_user_seconds"] + sample["process_system_seconds"] for sample in group),
                    "median_peak_rss_bytes": statistics.median(sample["peak_resident_bytes"] for sample in group),
                    "records": manifests[name]["records"], "files": manifests[name]["files"],
                    "source_bytes": manifests[name]["source_bytes"],
                    "distinct_gram_keys": group[0]["distinct_gram_keys"],
                })
    with (HERE / "layout-results.csv").open("w", newline="") as output:
        writer = csv.DictWriter(output, fieldnames=rows[0].keys())
        writer.writeheader()
        writer.writerows(rows)
    (HERE / "layout-results.json").write_text(json.dumps({"sha": sha, "rows": rows}, indent=2) + "\n")
    print("\n| Layout | Alphabet | Workers | Median ms | Min–max ms | CPU s | Peak RSS MiB |")
    print("|---|---|---:|---:|---:|---:|---:|")
    for row in rows:
        print(f"| {row['layout']} | {row['alphabet']} | {row['resolved_workers']} | "
              f"{row['median_wall_ms']:.1f} | {row['min_wall_ms']:.1f}–{row['max_wall_ms']:.1f} | "
              f"{row['median_cpu_seconds']:.2f} | {row['median_peak_rss_bytes']/1048576:.1f} |")


if __name__ == "__main__":
    main()
