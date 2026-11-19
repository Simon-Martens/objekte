#!/usr/bin/env python3
"""Summarize the focused and per-series-commit Q136 reruns."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from statistics import median


STAGES = {
    "b": "0f1d5e5ffa566a911f926ffe564e49c0c69ded77",
    "a1": "b4172a1749750ee1c87ef74a56f5ee7cec6c40ee",
    "a2": "9ae617b5df3f0a082aa923195a28019d5c36815d",
    "a3-1": "9c62fedec51ce846612fa1b58cb7ea1a5bb0fd26",
    "a3-2": "0b36bc256fee1ada7c7efa0fd0a931f5af4478a1",
    "c": "acac94f486ee0f9d35eb5ba834d26cdc01eb1c5e",
    "repair": "fd25d8678af5161cb92647d7dbddefa7f1aff391",
}


def load_rows(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()]


def result_dir(root: Path, side: str, batch: int) -> Path:
    return root / side / f"batch-{batch}" / "queries"


def summarize_samples(paths: list[Path]) -> dict:
    rows = [row for path in paths for row in load_rows(path / "measurements.jsonl")]
    values = [row["total_ns"] for row in rows]
    return {
        "samples": len(values),
        "median_ns": round(median(values)),
        "min_ns": min(values),
        "max_ns": max(values),
        "values_ns": values,
        "statuses": sorted({row["status"] for row in rows}),
        "counters": [row["counters"] for row in rows],
    }


def focused(q136: Path) -> dict:
    baseline_batches = []
    candidate_batches = []
    for batch in (1, 2, 3):
        baseline = summarize_samples([result_dir(q136, "baseline", batch)])
        candidate = summarize_samples([result_dir(q136, "candidate", batch)])
        baseline_batches.append(baseline["median_ns"])
        candidate_batches.append(candidate["median_ns"])
        comparison = json.loads(
            (q136 / f"comparison/batch-{batch}.json").read_text(encoding="utf-8")
        )["comparison"]
        assert comparison["matched_samples"] == 3
        assert comparison["ordered_result_matches"] == 1
        assert not comparison["mismatch_counts"]
        assert not comparison["compatibility_failures"]
    baseline = summarize_samples(
        [result_dir(q136, "baseline", batch) for batch in (1, 2, 3)]
    )
    candidate = summarize_samples(
        [result_dir(q136, "candidate", batch) for batch in (1, 2, 3)]
    )
    difference = candidate["median_ns"] - baseline["median_ns"]
    return {
        "baseline": baseline,
        "candidate": candidate,
        "baseline_batch_medians_ns": baseline_batches,
        "candidate_batch_medians_ns": candidate_batches,
        "difference_ns": difference,
        "difference_percent": difference * 100 / baseline["median_ns"],
        "ordered_result_matches": 3,
        "paired_samples": 9,
        "work_counter_mismatches": 0,
    }


def series(q136: Path) -> dict:
    stage_results = {}
    for stage, revision in STAGES.items():
        root = q136 / "series-repeated" / stage
        batch_medians = []
        revisions = set()
        for batch in (1, 2, 3):
            result = root / f"batch-{batch}/queries"
            summary = summarize_samples([result])
            batch_medians.append(summary["median_ns"])
            manifest = json.loads((result / "run-manifest.json").read_text(encoding="utf-8"))
            revisions.add((manifest["code_revision"], manifest["binary_build_revision"]))
        assert revisions == {(revision, revision)}
        summary = summarize_samples(
            [root / f"batch-{batch}/queries" for batch in (1, 2, 3)]
        )
        summary["batch_medians_ns"] = batch_medians
        stage_results[stage] = summary

    order = ("b", "a1", "a2", "a3-1", "a3-2", "c", "repair")
    steps = []
    for before, after in zip(order, order[1:]):
        difference = stage_results[after]["median_ns"] - stage_results[before]["median_ns"]
        steps.append(
            {
                "from": before,
                "to": after,
                "difference_ns": difference,
                "difference_percent": difference * 100 / stage_results[before]["median_ns"],
            }
        )
    return {
        "stages": stage_results,
        "adjacent_steps": steps,
        "largest_step": max(steps, key=lambda row: row["difference_ns"]),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("q136", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    result = {
        "focused_alternating": focused(args.q136),
        "series_narrowing": series(args.q136),
    }
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
