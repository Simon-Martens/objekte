#!/usr/bin/env python3
"""Summarize matched direct build samples with the regression driver's definitions."""

from __future__ import annotations

import argparse
import hashlib
import json
import statistics
import subprocess
from pathlib import Path


def binary_sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def summarize(path: Path) -> dict[str, dict]:
    samples = [
        json.loads(line)
        for line in (path / "measurements.jsonl").read_text().splitlines()
    ]
    result = {}
    for alphabet in ("ascii", "unicode"):
        group = [sample for sample in samples if sample["alphabet"] == alphabet]
        if len(group) != 3:
            raise ValueError(f"{path}: expected three {alphabet} samples, found {len(group)}")
        signatures = {sample["index_signature_sha256"] for sample in group}
        schemas = {sample["schema_manifest_digest"] for sample in group}
        if len(signatures) != 1 or len(schemas) != 1:
            raise ValueError(f"{path}: {alphabet} identity changed between samples")
        result[alphabet] = {
            "samples": len(group),
            "records": sorted({sample["records"] for sample in group}),
            "build_parses": sorted({sample["build_parses"] for sample in group}),
            "failed_parses": sorted({sample.get("failed_parses", 0) for sample in group}),
            "median_wall_ns": int(statistics.median(sample["wall_ns"] for sample in group)),
            "min_wall_ns": min(sample["wall_ns"] for sample in group),
            "max_wall_ns": max(sample["wall_ns"] for sample in group),
            "median_peak_resident_bytes": int(
                statistics.median(sample["peak_resident_bytes"] for sample in group)
            ),
            "index_signature_sha256": signatures.pop(),
            "schema_manifest_digest": schemas.pop(),
        }
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--current", type=Path, required=True)
    parser.add_argument("--baseline-revision", required=True)
    parser.add_argument("--current-revision", required=True)
    parser.add_argument("--baseline-binary", type=Path, required=True)
    parser.add_argument("--current-binary", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    baseline = summarize(args.baseline.resolve())
    current = summarize(args.current.resolve())
    failures = []
    for alphabet in ("ascii", "unicode"):
        for field in ("records", "build_parses", "failed_parses", "index_signature_sha256"):
            if baseline[alphabet][field] != current[alphabet][field]:
                failures.append(f"{alphabet}: {field} differs")

    result = {
        "root": str(args.root.resolve()),
        "repetitions": 3,
        "compiler": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "build_flags": {},
        "baseline": {
            "revision": args.baseline_revision,
            "binary": str(args.baseline_binary.resolve()),
            "binary_sha256": binary_sha256(args.baseline_binary.resolve()),
            "summary": baseline,
        },
        "current": {
            "revision": args.current_revision,
            "binary": str(args.current_binary.resolve()),
            "binary_sha256": binary_sha256(args.current_binary.resolve()),
            "summary": current,
        },
        "comparison": {
            "signatures_equal": all(
                baseline[alphabet]["index_signature_sha256"]
                == current[alphabet]["index_signature_sha256"]
                for alphabet in ("ascii", "unicode")
            ),
            "failures": failures,
        },
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    return int(bool(failures))


if __name__ == "__main__":
    raise SystemExit(main())
