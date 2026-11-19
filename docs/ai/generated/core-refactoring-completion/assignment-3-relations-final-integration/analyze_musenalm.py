#!/usr/bin/env python3
"""Summarize the paired Musenalm query and construction measurements."""

from __future__ import annotations

import argparse
import json
from collections import defaultdict
from pathlib import Path
from statistics import median


def load_jsonl(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()]


def case_medians(path: Path) -> dict[str, int]:
    samples: dict[str, list[int]] = defaultdict(list)
    for row in load_jsonl(path):
        samples[row["case_id"]].append(row["total_ns"])
    return {case_id: round(median(values)) for case_id, values in samples.items()}


def summarize_build(path: Path) -> dict:
    rows = load_jsonl(path)
    output = {}
    for alphabet in ("ascii", "unicode"):
        selected = [row for row in rows if row["alphabet"] == alphabet]
        wall = [row["wall_ns"] for row in selected]
        peak = [row["peak_resident_bytes"] for row in selected]
        output[alphabet] = {
            "samples": len(selected),
            "wall_ns": wall,
            "wall_median_ns": round(median(wall)),
            "wall_min_ns": min(wall),
            "wall_max_ns": max(wall),
            "peak_resident_bytes": peak,
            "peak_resident_median_bytes": round(median(peak)),
            "peak_resident_min_bytes": min(peak),
            "peak_resident_max_bytes": max(peak),
            "records": sorted({row["records"] for row in selected}),
            "build_parses": sorted({row["build_parses"] for row in selected}),
            "failed_parses": sorted({row["failed_parses"] for row in selected}),
            "resolved_build_workers": sorted(
                {row["resolved_build_workers"] for row in selected}
            ),
            "available_parallelism": sorted(
                {row["available_parallelism"] for row in selected}
            ),
            "index_signature_sha256": sorted(
                {row["index_signature_sha256"] for row in selected}
            ),
            "schema_manifest_digest": sorted(
                {row["schema_manifest_digest"] for row in selected}
            ),
        }
    return output


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("evidence", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()

    root = args.evidence
    query_comparison = json.loads(
        (root / "comparison/query-comparison.json").read_text(encoding="utf-8")
    )
    build_comparison = json.loads(
        (root / "comparison/build-comparison.json").read_text(encoding="utf-8")
    )
    query = query_comparison["comparison"]
    assert query["baseline_samples"] == query["current_samples"] == query["matched_samples"] == 450
    assert query["ordered_result_cases"] == query["ordered_result_matches"] == 150
    assert query["queries_equal"] and query["schemas_equal"] and query["workload_bytes_equal"]
    assert not query["compatibility_failures"]
    assert build_comparison["comparison"]["signatures_equal"]
    assert not build_comparison["comparison"]["failures"]

    common = root / "common-root"
    baseline = case_medians(common / "baseline/queries/measurements.jsonl")
    current = case_medians(common / "candidate/queries/measurements.jsonl")
    assert baseline.keys() == current.keys() and len(baseline) == 150
    paired = [
        {
            "case_id": case_id,
            "baseline_median_ns": baseline[case_id],
            "candidate_median_ns": current[case_id],
            "difference_ns": current[case_id] - baseline[case_id],
            "difference_percent": (
                (current[case_id] - baseline[case_id]) * 100 / baseline[case_id]
                if baseline[case_id]
                else 0.0
            ),
        }
        for case_id in sorted(baseline)
    ]
    by_difference = sorted(paired, key=lambda row: row["difference_ns"], reverse=True)
    result = {
        "query": {
            "case_count": len(paired),
            "slower_cases": sum(row["difference_ns"] > 0 for row in paired),
            "faster_cases": sum(row["difference_ns"] < 0 for row in paired),
            "equal_cases": sum(row["difference_ns"] == 0 for row in paired),
            "median_case_difference_ns": round(
                median(row["difference_ns"] for row in paired)
            ),
            "largest_slowdowns": by_difference[:10],
            "largest_improvements": list(reversed(by_difference[-10:])),
            "paired_medians": paired,
        },
        "construction": {
            "baseline": summarize_build(common / "baseline/build/measurements.jsonl"),
            "candidate": summarize_build(common / "candidate/build/measurements.jsonl"),
        },
    }
    for alphabet in ("ascii", "unicode"):
        before = result["construction"]["baseline"][alphabet]
        after = result["construction"]["candidate"][alphabet]
        before["wall_difference_ns"] = after["wall_median_ns"] - before["wall_median_ns"]
        before["wall_difference_percent"] = (
            before["wall_difference_ns"] * 100 / before["wall_median_ns"]
        )
        before["peak_resident_difference_bytes"] = (
            after["peak_resident_median_bytes"] - before["peak_resident_median_bytes"]
        )
        before["peak_resident_difference_percent"] = (
            before["peak_resident_difference_bytes"]
            * 100
            / before["peak_resident_median_bytes"]
        )

    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
