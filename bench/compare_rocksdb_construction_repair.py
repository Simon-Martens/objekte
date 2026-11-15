#!/usr/bin/env python3
"""Validate raw RocksDB repair artifacts and write the gate comparison."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


def load(path: Path) -> dict:
    with path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--native-control", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--validation", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--base-revision", required=True)
    args = parser.parse_args()

    baseline = load(args.baseline)
    native = load(args.native_control)
    candidate = load(args.candidate)
    validation = load(args.validation)

    expected_schema = baseline["schema_manifest_digest"]
    if native["schema_digest"] != expected_schema:
        raise SystemExit("native control schema manifest does not match baseline")
    if candidate["schema_digest"] != expected_schema:
        raise SystemExit("candidate schema manifest does not match baseline")
    if native["configuration_digest"] != candidate["configuration_digest"]:
        raise SystemExit("native control and candidate configuration differ")
    if validation.get("errors"):
        raise SystemExit("candidate validation has errors")

    baseline_wall = int(baseline["wall_ns"])
    baseline_resident = int(baseline["resident_bytes"])
    baseline_peak = int(baseline["peak_resident_bytes"])
    candidate_wall = int(candidate["elapsed_ns"])
    candidate_resident = int(candidate["resident_bytes"])
    candidate_peak = int(candidate["peak_resident_bytes"])
    limits = {
        "wall_ns": baseline_wall * 115 // 100,
        "resident_bytes": baseline_resident * 140 // 100,
        "peak_resident_bytes": baseline_peak * 160 // 100,
    }
    gates = {
        "construction_time": candidate_wall <= limits["wall_ns"],
        "resident_memory": candidate_resident <= limits["resident_bytes"],
        "peak_memory": candidate_peak <= limits["peak_resident_bytes"],
        "internal_validation": not validation.get("errors"),
    }
    output = {
        "task": "rocksdb-construction-repair",
        "base_revision": args.base_revision,
        "raw_inputs": {
            "baseline": {"path": str(args.baseline), "sha256": digest(args.baseline)},
            "native_control": {
                "path": str(args.native_control),
                "sha256": digest(args.native_control),
            },
            "candidate": {"path": str(args.candidate), "sha256": digest(args.candidate)},
            "validation": {
                "path": str(args.validation),
                "sha256": digest(args.validation),
            },
        },
        "schema_digest": expected_schema,
        "raw_values": {
            "baseline": {
                "wall_ns": baseline_wall,
                "resident_bytes": baseline_resident,
                "peak_resident_bytes": baseline_peak,
            },
            "native_control": {
                "wall_ns": int(native["elapsed_ns"]),
                "resident_bytes": int(native["resident_bytes"]),
                "peak_resident_bytes": int(native["peak_resident_bytes"]),
            },
            "candidate": {
                "wall_ns": candidate_wall,
                "resident_bytes": candidate_resident,
                "peak_resident_bytes": candidate_peak,
            },
        },
        "limits_floor": limits,
        "ratios": {
            "candidate_to_baseline_time": candidate_wall / baseline_wall,
            "candidate_to_baseline_resident": candidate_resident / baseline_resident,
            "candidate_to_baseline_peak": candidate_peak / baseline_peak,
            "native_control_to_baseline_time": int(native["elapsed_ns"]) / baseline_wall,
        },
        "gates": gates,
        "outcome": "performance_failure"
        if not all(gates.values())
        else "pass_construction_screen",
        "correctness_scope": "full ASCII internal invariants; native exact facts on fixtures only",
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(output, indent=2) + "\n", encoding="utf-8")
    return 0 if gates["internal_validation"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
